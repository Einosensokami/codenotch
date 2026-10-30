const {readFileSync}=require('node:fs');
const {join}=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const html=readFileSync(join(__dirname,'codenotch','ui','notch.html'),'utf8');
const settings=readFileSync(join(__dirname,'codenotch','ui','settings.html'),'utf8');
function block(start,end){return html.slice(html.indexOf(start)+start.length,html.indexOf(end));}
const ctx=vm.createContext({stateSnap:{lang_resolved:'zh-Hant'},Date,Number,
  esc:s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;').replaceAll('"','&quot;'),tone:()=> '#fff'});
vm.runInContext(block('// OPENROUTER_BUDGET_START','// OPENROUTER_BUDGET_END'),ctx);
vm.runInContext(block('// BEGIN TESTABLE HEADLINE SELECTOR','// END TESTABLE HEADLINE SELECTOR'),ctx);
vm.runInContext(block('// BEGIN TESTABLE WEEKLY SELECTOR','// END TESTABLE WEEKLY SELECTOR'),ctx);
function account(overrides={}){return {id:'a',name:'Personal',status:'ok',limit:10,remaining:3.2,usage:6.8,daily:0.4,monthly:2,reset:null,fetched_at:Date.now(),note:'',...overrides};}
test('multiple keys produce independent budget cells, not an aggregated balance',()=>{
  const cells=ctx.openrouterCells([account(),account({id:'b',remaining:8})]);
  assert.equal(cells.length,2); assert.equal(cells[0].id,'openrouter@a');assert.equal(cells[1].id,'openrouter@b');
  assert.ok(Math.abs(cells[0].snap.windows[0].used-0.68)<1e-10);
  assert.ok(Math.abs(cells[1].snap.windows[0].used-0.2)<1e-10);
  assert.equal(ctx.headlineOf(cells[0].snap,'openrouter').id,'budget');
  assert.equal(ctx.weeklyOf(cells[0].snap,'openrouter'),null);
});
test('unlimited, zero-cap, exhausted and missing budgets stay distinct',()=>{
  assert.equal(ctx.openrouterCells([account({limit:null,remaining:null})])[0].snap.windows.length,0);
  assert.equal(ctx.openrouterCells([account({limit:0,remaining:0})])[0].snap.windows[0].used,1);
  assert.match(ctx.openrouterCard(account({remaining:0})),/已耗盡/);
  assert.match(ctx.openrouterCard(account({limit:null,remaining:null})),/未設 Key 上限/);
  assert.match(ctx.openrouterCard(account({status:'loading',fetched_at:0,limit:null,remaining:null})),/等待首次讀取/);
});
test('prepaid budgets do not invent a reset, periodic budgets use current spend',()=>{
  const prepaid=ctx.openrouterCard(account());
  assert.doesNotMatch(prepaid,/Key 預算重置/);
  const periodic=ctx.openrouterCard(account({usage:102,remaining:8,reset:'monthly'}));
  assert.match(periodic,/預算已用 \$2\.00 \/ \$10\.00/);
  assert.match(periodic,/Key 累計花費 \$102\.00/);
  assert.match(periodic,/Key 預算重置: monthly/);
});
test('failures show last known budgets rather than current exhausted readings',()=>{
  const failed=ctx.openrouterCard(account({status:'needsAuth',remaining:0}));
  assert.match(failed,/Key 無效/); assert.doesNotMatch(failed,/已耗盡/);
  const stale=ctx.openrouterCard(account({fetched_at:Date.now()-1000000}));
  assert.match(stale,/非即時資料/);
});
test('user-provided labels and server strings are escaped in cards',()=>{
  const card=ctx.openrouterCard(account({note:'<img src=x onerror=alert(1)>',reset:'<script>'}));
  assert.doesNotMatch(card,/<img|<script>/);assert.match(card,/&lt;script&gt;/);
});
test('settings and notch inline scripts parse',()=>{
  for(const doc of [html,settings]) for(const [,source] of doc.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g))new vm.Script(source);
  assert.match(settings,/type="password"/);
  assert.match(settings,/add_openrouter_key/);assert.match(settings,/remove_openrouter_key/);
});
