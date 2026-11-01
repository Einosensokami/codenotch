//! Zed hosted AI billing. Browser cookies and editor credentials are separate
//! sources: never combine accounts or send an editor token to the billing API.
use crate::usage::{LimitWindow, UsageSnapshot};
use crate::AppState;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const COOKIE_TARGET: &str = "Codenotch/Zed/session";
static REFRESH: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
fn credential(target: &str, write: Option<&str>, remove: bool) -> Result<(String, String), String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::*;
    let mut target: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if remove {
            if let Err(error) = CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0) {
                if error.code() != windows::Win32::Foundation::ERROR_NOT_FOUND.to_hresult() {
                    return Err("Cannot remove Zed session".into());
                }
            }
            return Ok(Default::default());
        }
        if let Some(secret) = write {
            let mut bytes = secret.as_bytes().to_vec();
            let entry = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                CredentialBlobSize: bytes.len() as u32,
                CredentialBlob: bytes.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..Default::default()
            };
            let result = CredWriteW(&entry, 0).map_err(|_| "Cannot save Zed session".to_string());
            bytes.fill(0);
            result?;
            return Ok(Default::default());
        }
        let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0, &mut ptr)
            .map_err(|_| "Zed session unavailable".to_string())?;
        if ptr.is_null() {
            return Err("Zed session unavailable".into());
        }
        let entry = &*ptr;
        let user = if entry.UserName.is_null() {
            Ok(String::new())
        } else {
            entry
                .UserName
                .to_string()
                .map_err(|_| "Invalid Zed account".to_string())
        };
        let secret = if entry.CredentialBlob.is_null() || entry.CredentialBlobSize == 0 {
            Err("Zed session unavailable".to_string())
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(entry.CredentialBlob, entry.CredentialBlobSize as usize)
                    .to_vec(),
            )
            .map_err(|_| "Invalid Zed session".to_string())
        };
        CredFree(ptr.cast());
        Ok((user?, secret?))
    }
}
#[cfg(not(windows))]
fn credential(_: &str, _: Option<&str>, _: bool) -> Result<(String, String), String> {
    Err("Zed credential storage requires Windows".into())
}

fn session_cookie(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.bytes().any(|c| c.is_ascii_control()) {
        return Err("Invalid Zed session cookie".into());
    }
    let value = input
        .split(';')
        .find_map(|part| part.trim().strip_prefix("zed.session="))
        .ok_or("Paste a Cookie header containing zed.session=…")?;
    if value.is_empty() || value.len() > 2400 || value.contains(char::is_whitespace) {
        return Err("Invalid Zed session cookie".into());
    }
    Ok(format!("zed.session={value}"))
}

#[tauri::command]
pub fn set_zed_cookie(app: AppHandle, cookie: String) -> Result<(), String> {
    let cookie = session_cookie(&cookie)?;
    credential(COOKIE_TARGET, Some(&cookie), false)?;
    clear(&app);
    request_refresh();
    Ok(())
}
#[tauri::command]
pub fn remove_zed_cookie(app: AppHandle) -> Result<(), String> {
    credential(COOKIE_TARGET, None, true)?;
    clear(&app);
    request_refresh();
    Ok(())
}
fn clear(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let snap = load_persisted();
    *app.state::<AppState>().zed.lock().unwrap() = snap.clone();
    let _ = app.emit("zed", &snap);
}
pub fn load_persisted() -> UsageSnapshot {
    UsageSnapshot {
        status: "absent".into(),
        ..Default::default()
    }
}
pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}
fn quota(id: &str, label: &str, value: &Value, reset: Option<u64>) -> Option<LimitWindow> {
    let used = number(value.get("used")?)?;
    let raw = value.get("limit")?;
    let limit = raw.get("limited").unwrap_or(raw);
    let limit = number(limit);
    Some(LimitWindow {
        id: id.into(),
        label: if raw == "unlimited" {
            format!("{label} · Unlimited")
        } else {
            label.into()
        },
        resets_at: reset,
        used: limit
            .filter(|n| *n > 0.0)
            .map(|cap| (used / cap).clamp(0.0, 1.0))
            .unwrap_or(0.0),
        count: if limit.is_some_and(|n| n > 0.0) {
            None
        } else {
            Some(used as i64)
        },
        ..Default::default()
    })
}

fn parse(root: &Value, web: bool) -> Result<UsageSnapshot, String> {
    let usage = if web {
        root.get("current_usage")
    } else {
        root.pointer("/plan/usage")
    }
    .filter(|v| v.is_object())
    .ok_or("Zed usage response format changed")?;
    let mut snap = UsageSnapshot {
        status: "ok".into(),
        fetched_at: crate::now_ms(),
        ..Default::default()
    };
    let reset = root
        .pointer("/plan/subscription_period/ended_at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|d| u64::try_from(d.timestamp_millis()).ok());
    if web {
        let spend = usage
            .get("token_spend")
            .ok_or("Zed token spend is not reported")?;
        let used = spend
            .get("spend_in_cents")
            .and_then(number)
            .ok_or("Zed token spend is not reported")?;
        let cap = spend.get("limit_in_cents").and_then(number);
        snap.note = match cap {
            Some(cap) => format!(
                "AI spend ${:.2} / ${:.2} · Remaining ${:.2}",
                used / 100.0,
                cap / 100.0,
                (cap - used).max(0.0) / 100.0
            ),
            None => format!(
                "AI spend ${:.2} · Spending limit not reported",
                used / 100.0
            ),
        };
        if let Some(cap) = cap {
            snap.windows.push(LimitWindow {
                id: "spend".into(),
                label: "AI spending limit".into(),
                used: if cap > 0.0 {
                    (used / cap).clamp(0.0, 1.0)
                } else {
                    1.0
                },
                ..Default::default()
            });
        }
    } else {
        snap.note = "Editor quota only. Connect a Zed browser session in Settings → Accounts to see AI spend.".into();
    }
    for (id, label) in [
        ("model_requests", "Model requests"),
        ("edit_predictions", "Edit predictions"),
    ] {
        if let Some(window) = usage.get(id).and_then(|v| quota(id, label, v, reset)) {
            snap.windows.push(window);
        }
    }
    if !web && snap.windows.is_empty() {
        return Err("Zed quota is not reported".into());
    }
    Ok(snap)
}

fn read_once() -> UsageSnapshot {
    let cookie = credential(COOKIE_TARGET, None, false)
        .ok()
        .map(|(_, secret)| secret);
    let editor = if cookie.is_none() {
        credential("zed:url=https://zed.dev", None, false).ok()
    } else {
        None
    };
    let web = cookie.is_some();
    // Never forward a session to a redirect destination.
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut request = agent
        .get(if web {
            "https://cloud.zed.dev/frontend/billing/usage"
        } else {
            "https://cloud.zed.dev/client/users/me"
        })
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15));
    if let Some(cookie) = cookie {
        let Ok(cookie) = session_cookie(&cookie) else {
            return failure("needsAuth", "Reconnect the Zed browser session");
        };
        request = request.set("Cookie", &cookie);
    } else if let Some((user, token)) = editor {
        if user.parse::<u64>().is_err() || token.bytes().any(|b| b.is_ascii_control()) {
            return failure("needsAuth", "Sign in again in Zed");
        }
        request = request.set("Authorization", &format!("{user} {token}"));
    } else {
        let installed = dirs::config_dir().is_some_and(|d| d.join("Zed").is_dir())
            || dirs::data_local_dir().is_some_and(|d| d.join("Zed").is_dir());
        return if installed {
            failure(
                "needsAuth",
                "Connect a Zed browser session in Settings → Accounts to see AI spend",
            )
        } else {
            load_persisted()
        };
    }
    match request.call() {
        Ok(response) => match response.into_json::<Value>() {
            Ok(root) => parse(&root, web).unwrap_or_else(|e| failure("error", &e)),
            Err(_) => failure("error", "Invalid Zed usage response"),
        },
        Err(ureq::Error::Status(401 | 403, _)) => failure(
            "needsAuth",
            if web {
                "Zed browser session expired; reconnect it in Settings → Accounts"
            } else {
                "Sign in again in Zed"
            },
        ),
        Err(ureq::Error::Status(429, response)) => {
            let delay = retry_delay(response.header("Retry-After"), crate::now_ms());
            let mut snap = failure("backoff", "Zed is rate limited; retrying later");
            snap.backoff_until = crate::now_ms().saturating_add(delay.saturating_mul(1000));
            snap
        }
        Err(_) => failure(
            "error",
            "Could not read Zed usage; check your connection and try again",
        ),
    }
}
fn failure(status: &str, note: &str) -> UsageSnapshot {
    UsageSnapshot {
        status: status.into(),
        note: note.into(),
        ..Default::default()
    }
}
fn retry_delay(header: Option<&str>, now: u64) -> u64 {
    header
        .and_then(|s| {
            s.parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(s)
                    .ok()
                    .and_then(|d| u64::try_from(d.timestamp_millis()).ok())
                    .map(|deadline| deadline.saturating_sub(now).saturating_add(999) / 1000)
            })
        })
        .unwrap_or(300)
        .max(300)
}
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last_attempt = 0;
        let mut last_generation = 0;
        loop {
            let generation = GENERATION.load(Ordering::SeqCst);
            let previous = app.state::<AppState>().zed.lock().unwrap().clone();
            let now = crate::now_ms();
            let ready = now >= previous.backoff_until
                && (generation != last_generation || now.saturating_sub(last_attempt) >= 60_000);
            if ready {
                last_attempt = now;
                last_generation = generation;
                let snap = read_once();
                let state = app.state::<AppState>();
                let mut current = state.zed.lock().unwrap();
                if generation == GENERATION.load(Ordering::SeqCst) {
                    *current = snap.clone();
                    let _ = app.emit("zed", &snap);
                }
                drop(current);
                crate::tray::refresh_menu(&app);
            }
            for _ in 0..300 {
                if REFRESH.swap(false, Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn billing_uses_reported_cents_and_never_invents_a_limit() {
        let root =
            json!({"current_usage":{"token_spend":{"spend_in_cents":1234,"limit_in_cents":2000}}});
        let snap = parse(&root, true).unwrap();
        assert!((snap.windows[0].used - 0.617).abs() < 0.0001);
        assert!(snap.note.contains("$12.34 / $20.00"));
        let no_limit = json!({"current_usage":{"token_spend":{"spend_in_cents":1234}}});
        assert!(parse(&no_limit, true).unwrap().windows.is_empty());
        assert!(parse(&json!({"current_usage":{"token_spend":{}}}), true).is_err());
    }
    #[test]
    fn cookie_is_restricted_to_zed_session() {
        assert_eq!(
            session_cookie("other=x; zed.session=abc==; tracking=y").unwrap(),
            "zed.session=abc=="
        );
        for invalid in ["abc", "zed.session=", "zed.session=x\r\nInjected: y"] {
            assert!(session_cookie(invalid).is_err());
        }
    }
    #[test]
    fn editor_quota_keeps_unlimited_as_a_count() {
        let snap = parse(
            &json!({"plan":{"usage":{"edit_predictions":{"used":42,"limit":"unlimited"}}}}),
            false,
        )
        .unwrap();
        assert_eq!(snap.windows[0].count, Some(42));
        assert!(snap.note.contains("Editor quota only"));
    }
    #[test]
    fn rate_limit_honors_seconds_and_http_date() {
        assert_eq!(retry_delay(Some("900"), 0), 900);
        assert_eq!(retry_delay(Some("Thu, 01 Jan 1970 01:00:00 GMT"), 0), 3600);
        assert_eq!(retry_delay(Some("garbage"), 0), 300);
    }
    #[test]
    fn zero_budget_is_exhausted_and_missing_data_is_an_error() {
        let root = serde_json::json!({"current_usage":{"token_spend":{"spend_in_cents":0,"limit_in_cents":0}}});
        assert_eq!(parse(&root, true).unwrap().windows[0].used, 1.0);
        for root in [
            serde_json::json!({}),
            serde_json::json!({"current_usage":null}),
            serde_json::json!({"current_usage":{"token_spend":{"spend_in_cents":-1}}}),
        ] {
            assert!(parse(&root, true).is_err());
        }
    }
}
