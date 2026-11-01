const {test}=require('bun:test');
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const {webcrypto}=require('node:crypto');
const source=fs.readFileSync(require.resolve('../src-tauri/src/pin_snapshot.js'),'utf8');
const id='123e4567-e89b-42d3-a456-426614174000';
async function run(payload,{scopeAccount='account-a',response}={}){
  const claims={'https://api.openai.com/auth':{chatgpt_account_id:'account-a',chatgpt_user_id:'user-a'}};
  const token=`header.${btoa(JSON.stringify(claims)).replace(/=/g,'').replace(/\+/g,'-').replace(/\//g,'_')}.signature`;
  const hash=new Uint8Array(await webcrypto.subtle.digest('SHA-256',new TextEncoder().encode(JSON.stringify(['chatgpt-request-identity-v1',scopeAccount,'user-a']))));
  const scope='chatgpt:observed:'+Array.from(hash,b=>b.toString(16).padStart(2,'0')).join('');
  const calls=[];
  const context={window:{},location:{origin:'https://chatgpt.com'},URL,AbortController,TextEncoder,TextDecoder,Response,atob,crypto:webcrypto,setTimeout,clearTimeout,
    fetch:async(input,options={})=>{const url=new URL(input);calls.push({path:url.pathname,method:options.method||'GET',query:url.search});return url.pathname==='/api/auth/session'?new Response(JSON.stringify({accessToken:token,user:{}}),{headers:{'content-type':'application/json'}}):response||new Response(JSON.stringify(payload),{headers:{'content-type':'application/json'}});}};
  vm.runInNewContext(`(${source})`,context)('snapshot-test','nonce-test',scope);
  for(let i=0;i<200&&context.window['snapshot-test'].state==='running';i++)await new Promise(resolve=>setTimeout(resolve,1));
  return{state:JSON.parse(JSON.stringify(context.window['snapshot-test'])),calls,token,scope};
}
test('pin snapshot reads a complete list without any provider writes or credential output',async()=>{
  const result=await run({items:[{type:'conversation',id}],total:1,has_more:false});
  assert.equal(result.state.state,'complete');assert.equal(result.state.complete,true);assert.equal(result.state.account_scope,result.scope);assert.deepEqual(result.state.pinned_ids,[id]);
  assert.deepEqual(result.calls.map(call=>[call.path,call.method]),[['/api/auth/session','GET'],['/backend-api/pins','GET']]);
  assert.equal(result.calls[1].query,'?item_type=conversation');assert.doesNotMatch(JSON.stringify(result.state),new RegExp(result.token));
  const empty=await run({items:[],total:0,has_more:false});assert.equal(empty.state.complete,true);assert.deepEqual(empty.state.pinned_ids,[]);
});
test('partial, ambiguous and unknown pin payloads cannot publish an authoritative empty list',async()=>{
  for(const payload of [{items:[],total:3},{items:[],has_more:true},{items:[],total:'3'},{items:[],next_cursor:'more'},{items:[],pins:[]},{items:[{type:'unexpected',id}]},{items:[{type:'conversation',id},{type:'conversation',id}]}]){
    const {state}=await run(payload);assert.equal(state.state,'failed',JSON.stringify(payload));assert.notEqual(state.complete,true);assert.equal(state.pinned_ids,undefined);
  }
});
test('pin snapshot checks the observed account before reading pins and retains server Retry-After',async()=>{
  const wrong=await run({items:[]},{scopeAccount:'other-account'});assert.equal(wrong.state.error_code,'account_changed');assert.equal(wrong.calls.length,1);
  const limited=await run(null,{response:new Response(null,{status:429,headers:{'retry-after':'7200'}})});assert.equal(limited.state.error_code,'rate_limited');assert.equal(limited.state.retry_after,7200);assert.equal(limited.state.pinned_ids,undefined);
});
test('HTTP failure diagnostics expose the status without publishing the response body',async()=>{
  const failure=await run(null,{response:new Response('private provider response',{status:422})});
  assert.equal(failure.state.state,'failed');assert.equal(failure.state.error_code,'request_failed');assert.equal(failure.state.http_status,422);assert.equal(failure.state.pinned_ids,undefined);assert.ok(!JSON.stringify(failure.state).includes('private provider response'));
});
