const STORAGE_KEY='chat-hub.pending-chatgpt-conversations.v1';
const conversationId=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const accountScope=/^chatgpt:observed:[0-9a-f]{64}$/;
const MAX_PENDING=1_000;

function validRow(value) {
  return value && typeof value==='object' && !Array.isArray(value) &&
    typeof value.pending_id==='string' && value.pending_id.length>0 && value.pending_id.length<=160 &&
    conversationId.test(value.source_id||'') &&
    (value.account_scope===null || accountScope.test(value.account_scope)) &&
    Number.isSafeInteger(value.created_at_ms) && value.created_at_ms>=0;
}

export function createPendingConversationStore(storage=globalThis.localStorage,now=()=>Date.now()) {
  let rows=[];
  try {
    const parsed=JSON.parse(storage?.getItem(STORAGE_KEY)||'[]');
    if(Array.isArray(parsed))rows=parsed.filter(validRow).slice(-MAX_PENDING).map(row=>({...row}));
  } catch { rows=[]; }
  function persist(){
    try {
      // Unsaved blank drafts are session UI state only. Persist a row once it has a provider ID.
      const durable=rows.filter(row=>conversationId.test(row.source_id||'')).slice(-MAX_PENDING);
      storage?.setItem(STORAGE_KEY,JSON.stringify(durable));
    } catch { /* Sidebar pending state remains available for this app session. */ }
  }
  function list(){return rows.map(row=>({...row}));}
  function get(pendingId){const row=rows.find(item=>item.pending_id===pendingId);return row?{...row}:null;}
  function createDraft(pendingId,createdAt=now()){
    if(typeof pendingId!=='string'||!pendingId||pendingId.length>160)return null;
    let row=rows.find(item=>item.pending_id===pendingId);
    if(!row){row={pending_id:pendingId,source_id:null,account_scope:null,created_at_ms:createdAt};rows.push(row);}
    return {...row};
  }
  function attachProviderConversation(pendingId,sourceId,scope){
    if(typeof pendingId!=='string'||!conversationId.test(sourceId||''))return null;
    let row=rows.find(item=>item.pending_id===pendingId);
    if(!row){row={pending_id:pendingId,source_id:null,account_scope:null,created_at_ms:now()};rows.push(row);}
    row.source_id=sourceId;
    row.account_scope=accountScope.test(scope||'')?scope:null;
    persist();
    return {...row};
  }
  function setAccountScope(pendingId,scope){
    const row=rows.find(item=>item.pending_id===pendingId);
    if(!row||!conversationId.test(row.source_id||'')||!accountScope.test(scope||''))return false;
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
    rows=rows.filter(row=>{
      if(!row.source_id||!row.account_scope)return true;
      const matches=source.filter(item=>item?.provider==='chatgpt'&&item.source_id===row.source_id&&item.account_scope===row.account_scope);
      return matches.length!==1;
    });
    if(rows.length!==before)persist();
    return before-rows.length;
  }
  return {list,get,createDraft,attachProviderConversation,setAccountScope,removeUnstartedDraft,reconcileCaptured};
}

export function pendingConversationItem(row) {
  if(!row||typeof row.pending_id!=='string')return null;
  return {
    id:`pending:${row.pending_id}`,
    pending_id:row.pending_id,
    source_id:row.source_id||null,
    account_scope:row.account_scope||null,
    provider:'chatgpt-pending',
    title:row.source_id?'ChatGPT conversation':'New ChatGPT conversation',
    updated_at_ms:row.created_at_ms,
    pending:true,
  };
}
