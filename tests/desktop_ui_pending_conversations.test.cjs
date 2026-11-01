const test=require('node:test'),assert=require('node:assert/strict');

class Storage {
  constructor(){this.values=new Map();}
  getItem(key){return this.values.get(key)||null;}
  setItem(key,value){this.values.set(key,String(value));}
}
const UUID='123e4567-e89b-12d3-a456-426614174000';
const SCOPE='chatgpt:observed:'+'a'.repeat(64),OTHER='chatgpt:observed:'+'b'.repeat(64);

test('blank drafts are session-only; provider-ID pending chats persist minimal metadata across restart',async()=>{
  const {createPendingConversationStore}=await import('../desktop-ui/pending-conversations.js');
  const storage=new Storage(),store=createPendingConversationStore(storage,()=>1234);
  store.createDraft('pending-one');
  assert.equal(storage.getItem('chat-hub.pending-chatgpt-conversations.v1'),null);
  store.attachProviderConversation('pending-one',UUID,SCOPE);
  const serialized=storage.getItem('chat-hub.pending-chatgpt-conversations.v1');
  assert.ok(serialized);assert.doesNotMatch(serialized,/prompt|message|token|content/i);
  const restarted=createPendingConversationStore(storage);
  assert.deepEqual(restarted.list(),[{pending_id:'pending-one',source_id:UUID,account_scope:SCOPE,created_at_ms:1234}]);
});

test('capture reconciliation requires exactly one provider and account-scope match',async()=>{
  const {createPendingConversationStore}=await import('../desktop-ui/pending-conversations.js');
  const store=createPendingConversationStore(new Storage(),()=>5);
  store.attachProviderConversation('pending-one',UUID,SCOPE);
  assert.equal(store.reconcileCaptured([{provider:'chatgpt',source_id:UUID,account_scope:OTHER}]),0);
  assert.equal(store.list().length,1);
  assert.equal(store.reconcileCaptured([
    {provider:'chatgpt',source_id:UUID,account_scope:SCOPE},
    {provider:'chatgpt',source_id:UUID,account_scope:SCOPE},
  ]),0);
  assert.equal(store.list().length,1);
  assert.equal(store.reconcileCaptured([{provider:'chatgpt',source_id:UUID,account_scope:SCOPE}]),1);
  assert.deepEqual(store.list(),[]);
});

test('non-UUID provider identifiers are never persisted as pending chat targets',async()=>{
  const {createPendingConversationStore}=await import('../desktop-ui/pending-conversations.js');
  const storage=new Storage(),store=createPendingConversationStore(storage);
  assert.equal(store.attachProviderConversation('pending-one','not-a-chat-id',SCOPE),null);
  assert.deepEqual(store.list(),[]);assert.equal(storage.getItem('chat-hub.pending-chatgpt-conversations.v1'),null);
});
