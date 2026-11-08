const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const source=fs.readFileSync(path.resolve(__dirname,'../desktop-ui/history-sync.js'),'utf8');
const KEY='chat-hub.chatgpt-history-cooldown.v1';
function environment(saved=null,invoke=async()=>({state:'idle'})){
  let now=1_000_000,stored=saved&&JSON.stringify(saved);
  const calls=[],timers=new Map();let next=0;
  const element={textContent:'',addEventListener(){},disabled:false};
  const context={document:{getElementById(){return element;}},Date:{now:()=>now},
    Event:class{constructor(type){this.type=type;}},
    setTimeout(fn){timers.set(++next,fn);return next;},clearTimeout(id){timers.delete(id);},
    window:{__TAURI_INTERNALS__:{invoke(){}},ChatHub:{invoke:async(name)=>{calls.push(name);return invoke(name);}},
      addEventListener(){},dispatchEvent(){},localStorage:{getItem(){return stored;},setItem(key,value){assert.equal(key,KEY);stored=value;},removeItem(){stored=null;}}}};
  vm.createContext(context);
  // Exercise the production functions without starting the app's startup job.
  vm.runInContext(source.replace(/restoreCooldown\(\);\nif \(hasNative\) \{[\s\S]*$/,'')+`
    globalThis.testApi={setCooldown,restoreCooldown,scheduleAutomaticRetry,syncPins,start,poll,
      state:()=>({cooldownUntil,backoffUntil,automaticRetriesRemaining,automaticRetryPaused})};`,context);
  return {api:context.testApi,calls,timers,setNow(value){now=value;},saved:()=>stored&&JSON.parse(stored)};
}
test('polling a shrinking Retry-After does not reapply exponential backoff',()=>{
  const env=environment();env.api.setCooldown(45);
  const deadline=env.api.state().cooldownUntil;
  env.setNow(1_035_000);env.api.setCooldown(10);
  assert.equal(env.api.state().cooldownUntil,deadline);
  env.setNow(1_040_000);env.api.setCooldown(60);
  assert.equal(env.api.state().cooldownUntil,1_100_000);
});
test('retry budget and expiry survive a restart',()=>{
  const env=environment({provider:'chatgpt',until:1_010_000,backoff_until:1_005_000,automatic_retries_remaining:1});
  env.api.restoreCooldown();
  assert.equal(env.api.state().automaticRetriesRemaining,1);
  assert.equal(env.api.state().cooldownUntil,1_010_000);
  assert.equal(env.saved().automatic_retries_remaining,1);
});
test('exhausted or invalid persisted budgets stay paused after expiry',()=>{
  for(const value of [0,-1,99,'secret']){
    const env=environment({provider:'chatgpt',until:1,automatic_retries_remaining:value});
    env.api.restoreCooldown();
    assert.equal(env.api.state().automaticRetryPaused,true);
    assert.equal(env.api.state().automaticRetriesRemaining,0);
  }
});
test('pins never bypass a shared history cooldown, including post-history refresh',async()=>{
  const env=environment();env.api.setCooldown(60);
  assert.equal(await env.api.syncPins({accountScope:'chatgpt:observed:'+'a'.repeat(64),afterHistory:true}),false);
  assert.equal(env.calls.length,0);
});
test('failed pin requests import the native shared provider deadline',async()=>{
  const env=environment(null,async name=>{
    if(name==='sync_chatgpt_pins')throw new Error('history_rate_limited');
    return {state:'complete',retry_after:120};
  });
  assert.equal(await env.api.syncPins({accountScope:'chatgpt:observed:'+'a'.repeat(64)}),false);
  assert.equal(env.api.state().cooldownUntil,1_120_000);
});
test('manual resume cannot skip Retry-After and can reset an exhausted budget after it',async()=>{
  const env=environment({provider:'chatgpt',until:1_010_000,automatic_retries_remaining:0});
  env.api.restoreCooldown();await env.api.start({automatic:false});
  assert.equal(env.calls.length,0);
  env.setNow(1_011_000);await env.api.start({automatic:false});
  assert.ok(env.calls.includes('sync_chatgpt_history'));
  assert.equal(env.api.state().automaticRetriesRemaining,3);
  assert.equal(env.api.state().automaticRetryPaused,false);
});

test('an old completed receipt does not reset the persisted retry budget',async()=>{
  const env=environment({provider:'chatgpt',until:1,automatic_retries_remaining:1},async()=>({state:'complete'}));
  env.api.restoreCooldown();await env.api.poll();
  assert.equal(env.api.state().automaticRetriesRemaining,1);
});
