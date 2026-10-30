//! Independent prepaid key budgets; never aggregate them into an account balance.
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

static REFRESH: AtomicBool = AtomicBool::new(false);
static ACCOUNTS: Mutex<Vec<Account>> = Mutex::new(Vec::new());
static EDIT: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Default, Serialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub status: String,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub usage: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
    pub reset: Option<String>,
    pub fetched_at: u64,
    pub note: String,
    #[serde(skip)]
    retry_at: u64,
}

fn path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("openrouter-profiles.json")
}
fn profiles() -> Result<Vec<Profile>, String> {
    match std::fs::read_to_string(path()) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|_| "Cannot read OpenRouter profiles".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err("Cannot read OpenRouter profiles".into()),
    }
}
fn save(profiles: &[Profile]) -> Result<(), String> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "Cannot create configuration directory")?;
    }
    let text = serde_json::to_string_pretty(profiles).map_err(|_| "Cannot encode profiles")?;
    std::fs::write(p, text).map_err(|_| "Cannot save OpenRouter profiles".into())
}

#[cfg(windows)]
fn vault(id: &str, write: Option<&str>, remove: bool) -> Result<String, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::*;
    let mut target: Vec<u16> = format!("Codenotch/OpenRouter/{id}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        if remove {
            CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0)
                .map_err(|_| "Cannot remove OpenRouter credential".to_string())?;
            return Ok(String::new());
        }
        if let Some(key) = write {
            let mut bytes = key.as_bytes().to_vec();
            let cred = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                CredentialBlobSize: bytes.len() as u32,
                CredentialBlob: bytes.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                ..Default::default()
            };
            let result =
                CredWriteW(&cred, 0).map_err(|_| "Cannot save OpenRouter credential".to_string());
            bytes.fill(0);
            result?;
            return Ok(String::new());
        }
        let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, 0, &mut ptr)
            .map_err(|_| "OpenRouter credential unavailable".to_string())?;
        if ptr.is_null() {
            return Err("OpenRouter credential unavailable".into());
        }
        let cred = &*ptr;
        let result = if cred.CredentialBlobSize == 0 || cred.CredentialBlob.is_null() {
            Err("OpenRouter credential unavailable".into())
        } else {
            String::from_utf8(
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
                    .to_vec(),
            )
            .map_err(|_| "OpenRouter credential unavailable".into())
        };
        CredFree(ptr.cast());
        result
    }
}
#[cfg(not(windows))]
fn vault(_: &str, _: Option<&str>, _: bool) -> Result<String, String> {
    Err("OpenRouter key storage is supported on Windows only".into())
}

pub fn snapshot() -> crate::usage::UsageSnapshot {
    let accounts = get_openrouter();
    crate::usage::UsageSnapshot {
        status: if accounts.is_empty() { "absent" } else { "ok" }.into(),
        windows: accounts
            .iter()
            .filter(|a| {
                a.status == "ok"
                    && a.fetched_at > 0
                    && crate::now_ms().saturating_sub(a.fetched_at) <= 900_000
            })
            .filter_map(|a| {
                let limit = a.limit?;
                let remaining = a.remaining?;
                Some(crate::usage::LimitWindow {
                    id: a.id.clone(),
                    label: a.name.clone(),
                    used: if limit > 0.0 {
                        (1.0 - remaining / limit).clamp(0.0, 1.0)
                    } else {
                        1.0
                    },
                    ..Default::default()
                })
            })
            .collect(),
        ..Default::default()
    }
}

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub fn get_openrouter() -> Vec<Account> {
    ACCOUNTS.lock().unwrap().clone()
}

#[tauri::command]
pub fn add_openrouter_key(name: String, key: String) -> Result<(), String> {
    let _edit = EDIT.lock().unwrap();
    let name = name.trim();
    let key = key.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Use a name of 1–80 characters".into());
    }
    if !key.starts_with("sk-or-") || key.len() > 1024 || key.chars().any(char::is_whitespace) {
        return Err("Enter a valid OpenRouter API key".into());
    }
    let mut list = profiles()?;
    if list.len() >= 20 {
        return Err("At most 20 OpenRouter keys can be tracked".into());
    }
    for p in &list {
        if vault(&p.id, None, false).is_ok_and(|saved| saved == key) {
            return Err("This OpenRouter key is already tracked".into());
        }
    }
    let mut random = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random)
        .map_err(|_| "Cannot generate credential identifier")?;
    let id = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    vault(&id, Some(key), false)?;
    list.push(Profile {
        id: id.clone(),
        name: name.into(),
    });
    if let Err(e) = save(&list) {
        let _ = vault(&id, None, true);
        return Err(e);
    }
    ACCOUNTS.lock().unwrap().push(Account {
        id,
        name: name.into(),
        status: "loading".into(),
        ..Default::default()
    });
    request_refresh();
    Ok(())
}

#[tauri::command]
pub fn remove_openrouter_key(id: String) -> Result<(), String> {
    let _edit = EDIT.lock().unwrap();
    let mut list = profiles()?;
    if !list.iter().any(|p| p.id == id) {
        return Err("OpenRouter key not found".into());
    }
    // Persist the removal before deleting the secret; never expose a secret through configuration.
    let original = list.clone();
    list.retain(|p| p.id != id);
    save(&list)?;
    // A missing vault entry is already removed (e.g. deleted in Credential Manager).
    if vault(&id, None, false).is_ok() {
        if let Err(error) = vault(&id, None, true) {
            save(&original)?;
            return Err(error);
        }
    }
    ACCOUNTS.lock().unwrap().retain(|p| p.id != id);
    request_refresh();
    Ok(())
}

fn number(v: &serde_json::Value, key: &str) -> Option<f64> {
    v.get(key)?.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}
fn parse(v: &serde_json::Value, old: &Account, now: u64) -> Result<Account, &'static str> {
    let data = v
        .get("data")
        .filter(|d| d.is_object())
        .ok_or("Invalid OpenRouter response")?;
    let usage = number(data, "usage").ok_or("OpenRouter usage missing")?;
    let limit = match data.get("limit") {
        Some(serde_json::Value::Null) => None,
        Some(_) => Some(number(data, "limit").ok_or("Invalid OpenRouter limit")?),
        None => return Err("OpenRouter limit missing"),
    };
    // Use the server's remaining value: lifetime usage is not a periodic budget's spend.
    let remaining = match limit {
        Some(_) => {
            Some(number(data, "limit_remaining").ok_or("OpenRouter remaining budget missing")?)
        }
        None => None,
    };
    Ok(Account {
        id: old.id.clone(),
        name: old.name.clone(),
        status: "ok".into(),
        limit,
        remaining,
        usage: Some(usage),
        daily: number(data, "usage_daily"),
        monthly: number(data, "usage_monthly"),
        reset: data
            .get("limit_reset")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        fetched_at: now,
        ..Default::default()
    })
}
fn failure(old: &Account, status: &str, note: &str, retry_at: u64) -> Account {
    Account {
        status: status.into(),
        note: note.into(),
        retry_at,
        ..old.clone()
    }
}
fn fetch(old: &Account) -> Account {
    let now = crate::now_ms();
    if now < old.retry_at {
        return old.clone();
    }
    let Ok(key) = vault(&old.id, None, false) else {
        return failure(
            old,
            "needsAuth",
            "Credential unavailable; remove and add this key again",
            now + 300_000,
        );
    };
    // Do not forward Authorization to a redirected endpoint, or include response bodies in errors.
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(15))
        .build();
    match agent
        .get("https://openrouter.ai/api/v1/key")
        .set("Authorization", &format!("Bearer {key}"))
        .call()
    {
        Ok(response) => match response
            .into_json::<serde_json::Value>()
            .ok()
            .and_then(|v| parse(&v, old, now).ok())
        {
            Some(account) => Account {
                retry_at: now + 300_000,
                ..account
            },
            None => failure(
                old,
                "error",
                "Invalid OpenRouter budget response",
                now + 300_000,
            ),
        },
        Err(ureq::Error::Status(401 | 403, _)) => failure(
            old,
            "needsAuth",
            "Key invalid or access denied; remove it if no longer needed",
            now + 300_000,
        ),
        Err(ureq::Error::Status(429, response)) => {
            let seconds = response
                .header("Retry-After")
                .and_then(|s| s.parse::<u64>().ok())
                .map(|s| s.saturating_mul(1000).saturating_add(now))
                .or_else(|| {
                    response
                        .header("Retry-After")
                        .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
                        .map(|d| d.timestamp_millis().max(0) as u64)
                })
                .unwrap_or(now + 300_000);
            failure(
                old,
                "backoff",
                "Rate limited; showing last known budget",
                seconds.max(now + 300_000),
            )
        }
        Err(_) => failure(
            old,
            "error",
            "Query failed; showing last known budget",
            now + 300_000,
        ),
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        {
            let _edit = EDIT.lock().unwrap();
            if let Ok(list) = profiles() {
                *ACCOUNTS.lock().unwrap() = list
                    .into_iter()
                    .map(|p| Account {
                        id: p.id,
                        name: p.name,
                        status: "loading".into(),
                        ..Default::default()
                    })
                    .collect();
            }
        }
        loop {
            let current = ACCOUNTS.lock().unwrap().clone();
            for old in current {
                let updated = fetch(&old);
                let mut accounts = ACCOUNTS.lock().unwrap();
                if let Some(entry) = accounts.iter_mut().find(|a| a.id == updated.id) {
                    *entry = updated;
                }
            }
            let _ = app.emit("openrouter", get_openrouter());
            for _ in 0..15 {
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
    fn prepaid_and_periodic_budgets_use_server_remaining() {
        let old = Account {
            id: "a".into(),
            name: "Development".into(),
            ..Default::default()
        };
        let a = parse(
            &json!({"data":{"limit":10,"limit_remaining":3.2,"usage":6.8,"usage_daily":0.4}}),
            &old,
            100,
        )
        .unwrap();
        assert_eq!(a.remaining, Some(3.2));
        assert_eq!(a.reset, None);
        assert_eq!(a.fetched_at, 100);
        let a = parse(
            &json!({"data":{"limit":10,"limit_remaining":8,"usage":102,"limit_reset":"monthly"}}),
            &old,
            200,
        )
        .unwrap();
        assert_eq!(a.remaining, Some(8.0));
        assert_eq!(a.usage, Some(102.0));
        assert_eq!(a.reset.as_deref(), Some("monthly"));
    }
    #[test]
    fn zero_unlimited_and_missing_are_distinct() {
        let old = Account::default();
        let a = parse(
            &json!({"data":{"limit":0,"limit_remaining":0,"usage":0}}),
            &old,
            1,
        )
        .unwrap();
        assert_eq!(a.limit, Some(0.0));
        assert_eq!(a.remaining, Some(0.0));
        let a = parse(&json!({"data":{"limit":null,"usage":4}}), &old, 1).unwrap();
        assert_eq!(a.remaining, None);
        assert!(parse(&json!({"data":{"limit":10,"usage":4}}), &old, 1).is_err());
        assert!(parse(&json!({"data":{}}), &old, 1).is_err());
    }
    #[test]
    fn failures_keep_last_reading_and_never_become_zero() {
        let a = Account {
            remaining: Some(3.0),
            fetched_at: 42,
            ..Default::default()
        };
        let failed = failure(&a, "needsAuth", "Key invalid", 100);
        assert_eq!(failed.remaining, Some(3.0));
        assert_eq!(failed.fetched_at, 42);
        assert_eq!(failed.status, "needsAuth");
        assert!(!serde_json::to_string(&failed).unwrap().contains("retry_at"));
    }
}
