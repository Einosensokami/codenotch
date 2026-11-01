const {readFileSync}=require('node:fs');
const {join}=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const html=readFileSync(join(__dirname,'codenotch/ui/notch.html'),'utf8');
const ctx=vm.createContext({});
for(const name of ['HEADLINE','WEEKLY']) {
  const begin=`// BEGIN TESTABLE ${name} SELECTOR`,end=`// END TESTABLE ${name} SELECTOR`;
  vm.runInContext(html.slice(html.indexOf(begin)+begin.length,html.indexOf(end)),ctx);
}
test('Zed prioritizes AI spend and never selects unrelated or weekly limits',()=>{
  const windows=[{id:'edit_predictions',used:0.9},{id:'spend',used:0.2}];
  assert.equal(ctx.headlineOf({windows},'zed').id,'spend');
  assert.equal(ctx.headlineOf({windows:[{id:'unknown',used:1}]},'zed'),null);
  assert.equal(ctx.weeklyOf({windows},'zed'),null);
});
test('editor-only Zed accounts can show their reported quota',()=>{
  assert.equal(ctx.headlineOf({windows:[{id:'edit_predictions'}]},'zed').id,'edit_predictions');
  assert.equal(ctx.headlineOf({windows:[]},'zed'),null);
});
test('Zed exact counts are not labelled approximate requests today',()=>{
  const start=html.indexOf("if(p.base==='zed'){");
  const branch=html.slice(start,html.indexOf('continue;',start));
  assert.ok(branch.includes('esc(String(w.count))'));
  assert.ok(!branch.includes('today'));
});
