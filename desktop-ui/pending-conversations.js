const STORAGE_KEY='chat-hub.pending-chatgpt-conversations.v1';
const conversationId=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const accountScope=/^chatgpt:observed:[0-9a-f]{64}$/;
const MAX_PENDING=1_000;
const validPendingId=value=>typeof value==='string'&&value.length>0&&value.length<=160;

function validRow(value) {
  return value && typeof value==='object' && !Array.isArray(value) &&
    validPendingId(value.pending_id) && conversationId.test(value.source_id||'') &&
    (value.account_scope===null || accountScope.test(value.account_scope)) &&
    Number.isSafeInteger(value.created_at_ms) && value.created_at_ms>=0;
}

export function createPendingConversationStore(storage,now=()=>Date.now()) {
  let rows=[],persistenceCode=null;
  // Access to the localStorage property itself can throw in restricted webviews.
  try {
    if(storage===undefined)storage=globalThis.localStorage;
    if(!storage || typeof storage.getItem!=='function' || typeof storage.setItem!=='function') {
      persistenceCode='pending_storage_unavailable';
    } else {
      const text=storage.getItem(STORAGE_KEY)||'[]';
      let parsed;
      try { parsed=JSON.parse(text); } catch { persistenceCode='pending_storage_invalid'; }
      if(!Array.isArray(parsed)||parsed.some(row=>!validRow(row))) {
        persistenceCode='pending_storage_invalid';
      } else if(parsed.length>MAX_PENDING) {
        persistenceCode='pending_capacity_exceeded';
      } else rows=parsed.map(row=>({...row}));
    }
  } catch { persistenceCode='pending_storage_unavailable'; }
  function persistenceStatus(){
    return {code:persistenceCode,metadata_persisted:persistenceCode===null,
      pending_count:rows.filter(row=>row.source_id).length,
      unknown_scope_count:rows.filter(row=>row.source_id&&!row.account_scope).length};
  }
  function notify(){
    if(typeof window!=='undefined'&&typeof window.dispatchEvent==='function'&&typeof CustomEvent==='function') {
      window.dispatchEvent(new CustomEvent('chat-hub:pending-persistence',{detail:persistenceStatus()}));
    }
  }
  function persist(){
    // Do not overwrite a corrupt/over-capacity record with an empty replacement.
    // It may be the only remaining record of conversations awaiting recovery.
    if(['pending_storage_invalid','pending_capacity_exceeded'].includes(persistenceCode)){notify();return false;}
    try {
      const durable=rows.filter(row=>conversationId.test(row.source_id||''));
      if(durable.length>MAX_PENDING){persistenceCode='pending_capacity_exceeded';notify();return false;}
      if(!storage || typeof storage.setItem!=='function')throw new Error('unavailable');
      storage.setItem(STORAGE_KEY,JSON.stringify(durable));
      persistenceCode=null;
    } catch { persistenceCode='pending_storage_unavailable'; }
    notify();
    return persistenceCode===null;
  }
  function list(){return rows.map(row=>({...row}));}
  function get(pendingId){const row=rows.find(item=>item.pending_id===pendingId);return row?{...row}:null;}
  function createDraft(pendingId,createdAt=now()){
    if(!validPendingId(pendingId))return null;
    let row=rows.find(item=>item.pending_id===pendingId);
    if(!row){
      if(rows.length>=MAX_PENDING){persistenceCode='pending_capacity_exceeded';notify();return null;}
      row={pending_id:pendingId,source_id:null,account_scope:null,
        created_at_ms:Number.isSafeInteger(createdAt)&&createdAt>=0?createdAt:0};
      rows.push(row);
    }
    return {...row};
  }
  function attachProviderConversation(pendingId,sourceId,scope){
    if(!validPendingId(pendingId)||!conversationId.test(sourceId||''))return null;
    let row=rows.find(item=>item.pending_id===pendingId);
    if(!row){if(!createDraft(pendingId))return null;row=rows.find(item=>item.pending_id===pendingId);}
    const incomingScope=accountScope.test(scope||'')?scope:null;
    // Navigation events often arrive before the account probe. An unknown scope
    // is not evidence of an account change and must not erase known ownership.
    if(row.source_id===sourceId&&row.account_scope&&incomingScope&&row.account_scope!==incomingScope)return null;
    const retainedScope=row.source_id===sourceId?row.account_scope:null;
    row.source_id=sourceId;
    row.account_scope=incomingScope||retainedScope;
    persist();
    return {...row};
  }
  function setAccountScope(pendingId,scope){
    const row=rows.find(item=>item.pending_id===pendingId);
    if(!row||!conversationId.test(row.source_id||'')||!accountScope.test(scope||''))return false;
    if(row.account_scope&&row.account_scope!==scope)return false;
    row.account_scope=scope;persist();return true;
  }
  function removeUnstartedDraft(pendingId){
    const index=rows.findIndex(row=>row.pending_id===pendingId);
    if(index<0||rows[index].source_id)return false;
    rows.splice(index,1);persist();return true;
  }
  function reconcileCaptured(items){
    const source=Array.isArray(items)?items:[];
    const before=rows.length;
    rows=rows.filter(row=>!row.source_id||!row.account_scope||!source.some(item=>
      item?.provider==='chatgpt'&&item.source_id===row.source_id&&item.account_scope===row.account_scope));
    // More than one saved copy with the exact same provider/account/source still
    // proves persistence. Display deduplication is a separate concern (#37).
    if(rows.length!==before)persist();
    return before-rows.length;
  }
  notify();
  return {list,get,createDraft,attachProviderConversation,setAccountScope,removeUnstartedDraft,reconcileCaptured,persistenceStatus};
}

export function pendingConversationItem(row) {
  if(!row||typeof row.pending_id!=='string')return null;
  return {
    id:`pending:${row.pending_id}`,pending_id:row.pending_id,
    source_id:row.source_id||null,account_scope:row.account_scope||null,
    provider:'chatgpt-pending',title:row.source_id?'ChatGPT conversation':'New ChatGPT conversation',
    updated_at_ms:row.created_at_ms,pending:true,
  };
}
