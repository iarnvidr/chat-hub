const test=require('node:test');
const assert=require('node:assert/strict');
const {readFileSync}=require('node:fs');
const {resolve}=require('node:path');
const modulePromise=import('data:text/javascript;base64,'+readFileSync(resolve(__dirname,'../desktop-ui/pending-conversations.js')).toString('base64'));
const ID='00000000-0000-0000-0000-000000000001';
const A='chatgpt:observed:'+'a'.repeat(64),B='chatgpt:observed:'+'b'.repeat(64);
function memory(){let value=null;return {getItem(){return value;},setItem(_key,text){value=text;}};}

test('unknown navigation scope retains known ownership across restart',async()=>{
  const {createPendingConversationStore}=await modulePromise,storage=memory();
  const store=createPendingConversationStore(storage,()=>123);
  store.attachProviderConversation('tab',ID,A);
  store.attachProviderConversation('tab',ID,null);
  const reopened=createPendingConversationStore(storage);
  assert.equal(reopened.get('tab').account_scope,A);
  assert.equal(reopened.reconcileCaptured([{provider:'chatgpt',source_id:ID,account_scope:A}]),1);
});
test('exact saved duplicates reconcile but another account never does',async()=>{
  const {createPendingConversationStore}=await modulePromise,store=createPendingConversationStore(memory());
  store.attachProviderConversation('tab',ID,A);
  assert.equal(store.reconcileCaptured([{provider:'chatgpt',source_id:ID,account_scope:B}]),0);
  const saved={provider:'chatgpt',source_id:ID,account_scope:A};
  assert.equal(store.reconcileCaptured([saved,{...saved}]),1);
});
test('explicit scope conflicts cannot reassign pending ownership',async()=>{
  const {createPendingConversationStore}=await modulePromise,store=createPendingConversationStore(memory());
  store.attachProviderConversation('tab',ID,A);
  assert.equal(store.attachProviderConversation('tab',ID,B),null);
  assert.equal(store.setAccountScope('tab',B),false);
  assert.equal(store.get('tab').account_scope,A);
});
test('storage errors are visible while session metadata remains usable',async()=>{
  const {createPendingConversationStore}=await modulePromise;
  const store=createPendingConversationStore({getItem(){return null;},setItem(){throw new Error('secret path');}});
  store.attachProviderConversation('tab',ID,A);
  assert.equal(store.list().length,1);
  assert.equal(store.persistenceStatus().metadata_persisted,false);
  assert.equal(store.persistenceStatus().code,'pending_storage_unavailable');
  assert.ok(!JSON.stringify(store.persistenceStatus()).includes('secret'));
});
test('corrupt stored data is not silently overwritten',async()=>{
  const {createPendingConversationStore}=await modulePromise;
  for(const input of ['{}','{broken']){
    let writes=0;
    const store=createPendingConversationStore({getItem(){return input;},setItem(){writes++;}});
    store.attachProviderConversation('tab',ID,A);
    assert.equal(store.persistenceStatus().code,'pending_storage_invalid');
    assert.equal(writes,0);
  }
});
test('invalid IDs and unknown account scope cannot claim persistence',async()=>{
  const {createPendingConversationStore}=await modulePromise,store=createPendingConversationStore(memory());
  assert.equal(store.attachProviderConversation('',ID,A),null);
  assert.equal(store.attachProviderConversation('x'.repeat(161),ID,A),null);
  store.attachProviderConversation('tab',ID,null);
  assert.equal(store.reconcileCaptured([{provider:'chatgpt',source_id:ID,account_scope:A}]),0);
  assert.equal(store.persistenceStatus().unknown_scope_count,1);
});
