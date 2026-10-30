const {readFileSync}=require('node:fs');
const {join}=require('node:path');
const vm=require('node:vm');
const assert=require('node:assert/strict');
const {test}=require('node:test');
const root=join(__dirname,'codenotch','ui');
const helper=readFileSync(join(root,'sidebar-order.js'),'utf8');
const notch=readFileSync(join(root,'notch.html'),'utf8');
const settings=readFileSync(join(root,'settings.html'),'utf8');
const plain=value=>JSON.parse(JSON.stringify(value));
function context(){const ctx=vm.createContext({});vm.runInContext(helper,ctx);return ctx;}

test('moves each key independently across providers, preserving missing accounts',()=>{
  const ctx=context();
  const items=[{id:'claude'},{id:'openrouter@a'},{id:'openrouter@b'},{id:'codex'}];
  let order=['claude','offline','openrouter@a','openrouter@b','codex'];
  order=ctx.moveSidebarItem(order,items,'openrouter@b',-1);
  order=ctx.moveSidebarItem(order,items,'openrouter@b',-1);
  assert.deepEqual(plain(ctx.orderSidebarItems(items,order)).map(p=>p.id),['openrouter@b','claude','openrouter@a','codex']);
  assert.equal(order[1],'offline');
  assert.deepEqual(plain(ctx.moveSidebarItem(order,items,'openrouter@b',-1)),plain(order));
  assert.deepEqual(plain(ctx.moveSidebarItem(order,items,'unknown',1)),plain(order));
  assert.equal(ctx.orderSidebarItems([...items,{id:'new'}],order).at(-1).id,'new');
  assert.equal(ctx.orderSidebarItems([...items,{id:'offline'}],order)[1].id,'offline');
});

test('default order is stable and moving from default creates an explicit order',()=>{
  const ctx=context(), items=[{id:'a'},{id:'b'},{id:'c'}];
  assert.deepEqual(plain(ctx.orderSidebarItems(items,[])),items);
  assert.deepEqual(plain(ctx.moveSidebarItem([],items,'c',-1)),['a','c','b']);
  assert.deepEqual(items,[{id:'a'},{id:'b'},{id:'c'}]);
});

test('real notch provider path applies saved order after visibility filtering and restart',()=>{
  function restarted(order,slots){
    const ctx=context();
    Object.assign(ctx,{usage:{windows:[]},openrouterAccounts:[{id:'a',name:'A'},{id:'b',name:'B'}]});
    for(const name of ['codexSnap','glmSnap','opencodeSnap','commandcodeSnap','cursorSnap','grokSnap','copilotSnap','agSnap'])ctx[name]={status:'absent'};
    ctx.codexSnap={status:'ok'};
    vm.runInContext(notch.slice(notch.indexOf('function openrouterCells('),notch.indexOf('function openrouterCard(')),ctx);
    vm.runInContext(notch.slice(notch.indexOf('let notchSlots=null;'),notch.indexOf('// Antigravity\'s "Notch reads"')),ctx);
    ctx.order=JSON.parse(JSON.stringify(order));ctx.slots=slots;
    vm.runInContext('sidebarOrder=order;notchSlots=slots;',ctx);
    return ctx.providers().map(p=>p.id);
  }
  const saved=['codex','openrouter@b','claude','openrouter@a'];
  assert.deepEqual(plain(restarted(saved,[])),saved);
  assert.deepEqual(plain(restarted(saved,[{provider:'openrouter'}])),['openrouter@b','openrouter@a']);
  assert.deepEqual(plain(restarted(saved,[{provider:'unavailable'}])),saved);
});

test('Claude account ids match the notch across multiple profiles',()=>{
  const ctx=context();
  ctx.usage={windows:[{id:'session',group:'Personal'},{id:'weekly',group:'Personal'},{id:'session@work',group:'Work'}]};
  vm.runInContext(notch.slice(notch.indexOf('function claudeCells('),notch.indexOf('function providers(')),ctx);
  assert.deepEqual(plain(ctx.claudeSidebarItems(ctx.usage.windows)).map(p=>p.id),plain(ctx.claudeCells()).map(p=>p.id));
  assert.equal(ctx.claudeSidebarItems([])[0].id,'claude');
});

test('settings click saves the new order and recovers controls after a failed save',async()=>{
  const ctx=context(), listeners={}, nodes={};
  ctx.document={getElementById:id=>nodes[id]??=( {addEventListener:(event,fn)=>listeners[event]=fn} ),querySelectorAll:()=>[]};
  ctx.window={__TAURI__:{event:{listen(){}}}};
  ctx.notchOn=()=>[{provider:'claude'},{provider:'codex'},{provider:'openrouter'}];
  ctx.providerList=()=>[{id:'claude'},{id:'openrouter'},{id:'codex',label:'Codex',status:'ok'}];
  ctx.orText=en=>en;ctx.esc=s=>String(s).replaceAll('<','&lt;').replaceAll('"','&quot;');ctx.errText=String;
  ctx.toast=()=>{};let failure='';ctx.strip=s=>failure=s;
  const section=settings.slice(settings.indexOf('let orAccounts=[];'),settings.indexOf('function orText('));
  vm.runInContext(section,ctx);
  vm.runInContext('sidebarLoaded=true;orAccounts=[{id:"a",name:"<Key>"}];renderSidebarOrder();',ctx);
  assert.match(nodes['sidebar-order'].innerHTML,/&lt;Key>/);
  let saved;
  ctx.invoke=async(cmd,args)=>{assert.equal(cmd,'set_sidebar_order');saved=plain(args.order);return args.order;};
  const event={target:{closest:()=>({dataset:{orderId:'codex',direction:'-1'}})}};
  await listeners.click(event);
  assert.deepEqual(saved,['claude','codex','openrouter@a']);
  ctx.invoke=async()=>{throw Error('disk full');};
  await listeners.click(event);
  assert.match(failure,/disk full/);
  assert.equal(vm.runInContext('sidebarBusy',ctx),false);
  assert.deepEqual(plain(vm.runInContext('sidebarOrder',ctx)),saved);
});
