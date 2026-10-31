import {fixtureAdapter, normalizeList, normalizeSearch} from './data.js';
import {branchChoices,orderedMessages,messageText,hasUnsupportedContent,renderMarkdown} from './render.js';
import {updateStatus} from './status.js';
import {createConversationTabs} from './conversation-tabs.js';
const $=id=>document.getElementById(id),native=typeof window.__TAURI_INTERNALS__?.invoke==='function';const invoke=(...args)=>window.__TAURI_INTERNALS__.invoke(...args);window.ChatHub={invoke};const fixture=!native&&['localhost','127.0.0.1'].includes(location.hostname)&&new URLSearchParams(location.search).get('fixture')==='demo';const adapter=fixture?fixtureAdapter():{async listConversations(query,cursor){return invoke('list_conversations',{query,cursor,source:$('history-source').value});},async getConversation(id,leafNodeId){return invoke('get_conversation',leafNodeId!==undefined?{id,leafNodeId}:{id});},async importChatgptExport(){return invoke('import_chatgpt_export');}};$('fixture-banner').hidden=!fixture;let selected=null,conversation=null,items=[],timer,queryToken=0,readerToken=0,cursor=null,revision=null,queryValue='',cursorRetry=false,lastConversationButton=null;const errText=e=>String(e?.message||e);const safeError=code=>({invalid_request:'The request was invalid.',invalid_cursor:'The page cursor is invalid.',stale_cursor:'The library changed; restart the search.',library_changed:'The library changed; restart the search.',unauthorized:'Sign in to the desktop service.',not_found:'The requested conversation was not found.',service_unavailable:'The desktop service is unavailable.',busy:'The desktop service is busy.',response_too_large:'This conversation is too large to display (8 MiB response limit). No content has been truncated.'}[code]||'The desktop service returned an error.');function showError(s){$('list-state').textContent=s;}
function providerLabel(provider) {
  return {'paseo-chatgpt':'ChatGPT · saved in Paseo', 'paseo-codex_local':'Codex local · saved in Paseo', 'paseo-codex_cloud':'Codex cloud · saved in Paseo'}[provider] || provider || 'Unknown provider';
}
function drawList() {
  const box = $('conversation-list');
  box.replaceChildren();
  $('load-more').hidden = !cursor;
  for (const c of items) {
    const button = document.createElement('button');
    button.className = 'conversation-item' + (c.id === selected ? ' selected' : '');
    button.type = 'button';
    button.title = c.title || 'Untitled conversation';
    const title = document.createElement('strong'), metadata = document.createElement('span');
    title.textContent = c.title ?? 'Untitled conversation';
    metadata.textContent = providerLabel(c.provider);
    if (!c.provider?.startsWith('paseo-') && Number.isFinite(c.message_count)) metadata.textContent += ` · ${c.message_count} messages`;
    button.append(title, metadata);
    if (c.search_hit) {
      const snippet = document.createElement('span');
      snippet.className = 'search-snippet'; snippet.textContent = c.snippet || ''; button.append(snippet);
    }
    button.addEventListener('click', () => openConversation(c.id, c.search_leaf_node_id, c.search_node_id));
    box.append(button);
  }
  if (!items.length && !$('list-state').textContent.includes('failed')) showError('No conversations found.');
}
async function loadList(query=$('search').value,reset=true){const token=++queryToken;if(reset){queryValue=query.trim();cursor=null;revision=null;items=[];cursorRetry=false;}showError('Loading conversations…');try{const response=await adapter.listConversations(queryValue,cursor);const page=queryValue?normalizeSearch(response):normalizeList(response);if(token!==queryToken)return;if(!page||!Array.isArray(page.items))throw Error('Invalid conversation list response');if(revision!==null&&page.library_revision!==revision){cursor=null;revision=null;items=[];showError('Library changed; refreshed results');if(!reset)return loadList(queryValue,true);return;}revision=page.library_revision;cursor=page.next_cursor;const seen=new Set(items.map(x=>queryValue?JSON.stringify([x.id,x.search_node_id]):x.id));items.push(...page.items.filter(x=>!seen.has(queryValue?JSON.stringify([x.id,x.search_node_id]):x.id)));showError('');drawList();}catch(e){if(token!==queryToken)return;const s=errText(e);if(!reset&&!cursorRetry&&/invalid_cursor|stale_cursor|library_changed/i.test(s)){cursorRetry=true;cursor=null;revision=null;items=[];showError('Library changed; restarting from the first page.');return loadList(queryValue,false);}showError(`Could not load conversations: ${safeError(s)}`);}}
const conversationTabs = createConversationTabs({
  container: $('conversation-tabs'),
  onSelect: tab => {
    if (tab.mode === 'live') window.dispatchEvent(new Event('chat-hub:show-live'));
    else openConversation(tab.id, tab.leafNodeId, tab.anchorNodeId);
  },
  onEmpty: () => {
    ++readerToken; selected = null; conversation = null;
    $('reader').hidden = true; $('reader-empty').hidden = false;
    $('messages').replaceChildren(); drawList();
    window.dispatchEvent(new Event('chat-hub:show-local'));
  },
});
window.addEventListener('chat-hub:live-open', () => {
  conversationTabs.open({id: 'draft:chatgpt', title: 'New conversation', mode: 'live'});
});
window.addEventListener('chat-hub:history-updated', () => loadList($('search').value, true));
$('history-source').addEventListener('change', () => loadList($('search').value, true));
function openConversation(id, leafNodeId, anchorNodeId) {
  window.dispatchEvent(new Event('chat-hub:show-local'));
  const title = items.find(item => item.id === id)?.title || 'Conversation';
  conversationTabs.open({id, title, leafNodeId, anchorNodeId});
  const token = ++readerToken;
  lastConversationButton = [...document.querySelectorAll('.conversation-item')].find(b => b.classList.contains('selected')) || document.activeElement;
  selected = id;
  document.body.classList.add('mobile-reader');
  $('reader-empty').hidden = true;
  $('reader').hidden = false;
  $('title').textContent = 'Loading…';
  $('account-attribution').textContent = '';
  $('messages').replaceChildren();
  adapter.getConversation(id, leafNodeId).then(c => {
    if(token!==readerToken)return;
    if (c?.conversation?.id !== id) throw Error('not_found');
    conversation = {...c.conversation, ...c, selected_node_id: c.selection?.selected_node_id};
    $('title').textContent = c.conversation?.title ?? 'Untitled conversation';
    conversationTabs.update(id, {title: $('title').textContent});
    $('provider').textContent = providerLabel(c.conversation?.provider);
    const scope = c.conversation?.account_scope;
    $('account-attribution').textContent = typeof scope !== 'string' || scope.startsWith('provisional:')
      ? 'Account ownership unverified'
      : scope.startsWith('chatgpt:observed:') ? 'Account attributed from provider request' : 'Account scope recorded';
    const select = $('branch');
    const cachedTranscript = c.conversation?.provider?.startsWith('paseo-');
    select.closest('label').hidden = !!cachedTranscript;
    if (cachedTranscript) $('account-attribution').textContent = 'Saved Paseo transcript. Alternate branches were not stored in this cache.';
    select.replaceChildren();
    for (const n of branchChoices(c)) {
      const option = document.createElement('option');
      option.value = n.id;
      option.textContent = n.id === c.selection?.selected_node_id ? 'Current selected branch' : `Branch · ${n.id}`;
      option.selected = n.id === c.selection?.selected_node_id;
      select.append(option);
    }
    select.onchange = () => {
      conversationTabs.update(id, {leafNodeId: select.value});
      drawMessages(select.value);
    };
    drawMessages(c.selection?.selected_node_id);
    drawList();
    if (anchorNodeId) document.getElementById(messageDomId(id,anchorNodeId))?.scrollIntoView?.();
  }).catch(e => {
    if (token !== readerToken) return;
    $('title').textContent = 'Unable to open conversation';
    $('messages').replaceChildren();
    const code = typeof e === 'string' ? e : errText(e);
    $('messages').textContent = safeError(code.includes('response_too_large') ? 'response_too_large' : code);
  });
}
function messageDomId(conversationId,nodeId){return `message-${encodeURIComponent(conversationId)}-${encodeURIComponent(nodeId)}`;}
function drawMessages(leaf){const root=$('messages');root.replaceChildren();try{for(const m of orderedMessages(conversation,leaf)){const article=document.createElement('article'),role=document.createElement('div'),body=document.createElement('div');article.className='message';role.className='role';role.textContent=m.role==='archive'?'Saved transcript':m.role||'Message';body.className='markdown';const text=messageText(m);if(text)renderMarkdown(text,body);if(hasUnsupportedContent(m)){const marker=document.createElement('div');marker.className='unsupported-content';marker.textContent='[Unsupported content]';body.append(marker);}article.id=messageDomId(conversation.id,m.node_id||'unknown');article.dataset.nodeId=m.node_id||'';article.append(role,body);root.append(article);}}catch(e){root.textContent=safeError(errText(e));}}
$('back-to-library').addEventListener('click',()=>{document.body.classList.remove('mobile-reader');const target=lastConversationButton?.isConnected?lastConversationButton:$('search');target.focus();});$('search').addEventListener('input',()=>{clearTimeout(timer);timer=setTimeout(()=>loadList($('search').value,true),150);});$('import').addEventListener('click',async()=>{const b=$('import');b.disabled=true;b.textContent='Importing…';try{const r=await adapter.importChatgptExport();if(r?.cancelled){$('app-status').textContent='Import cancelled.';return;}if(!r||!r.result||typeof r.result!=='object')throw Error('Invalid import receipt');const x=r?.result||{},errs=Array.isArray(x.errors)?x.errors:[];$('app-status').textContent=`Import finished: ${x.imported??0} imported, ${x.updated??0} updated, ${x.unchanged??0} unchanged, ${x.failed??0} failed${x.errors_truncated?` (error list truncated; showing ${errs.length})`:''}${errs.length?`. ${errs.map(e=>e.message||String(e)).join('; ')}`:'.'}`;await loadList($('search').value,true);}catch(e){showError(`Import failed: ${errText(e)}`);}finally{b.disabled=false;b.textContent='Import export';}});$('load-more')?.addEventListener('click',()=>{if(cursor)loadList(queryValue,false);});const status=()=>updateStatus({invoke:native?invoke:null,setVersion:v=>$('version').textContent=v,setMessage:v=>$('message').textContent=v,setHealth:v=>$('health').textContent=v,setRetryVisible:v=>{$('retry-health').hidden=!v;}});$('retry-health').addEventListener('click',status);
let saveBusy=false;function setSaveBusy(v){saveBusy=v;$('backup-library').disabled=v;$('export-library-json').disabled=v;}const saveErrors={cancelled:'Save cancelled.',invalid_request:'The save request was invalid.',unauthorized:'Sign in to the desktop service before saving.',busy:'The desktop service is busy. Try again later.',archive_busy:'Another archive save is already in progress.',service_unavailable:'The desktop service is unavailable. Try again later.',not_found:'The library was not found.',permission_denied:'The destination could not be written.',io_error:'The destination could not be written.',archive_published:'The backup file was published, but its durability could not be confirmed. Check the saved file before trying again.'};async function saveLibrary(button,command,kind){if(!native||saveBusy)return;setSaveBusy(true);button.textContent=kind==='backup'?'Saving backup…':'Exporting JSON…';$('app-status').textContent=kind==='backup'?'Choose where to save the library backup.':'Choose where to export the library as JSON.';try{const r=await invoke(command);if(!r||typeof r.cancelled!=='boolean'||typeof r.published!=='boolean'||typeof r.durability_confirmed!=='boolean'||r.kind!==kind)throw Error('invalid_request');if(r.cancelled){$('app-status').textContent='Save cancelled.';return;}if(r.published&&!r.durability_confirmed){$('app-status').textContent='The file was published, but its durability could not be confirmed. Check the saved file before trying again.';return;}if(r.published&&r.durability_confirmed){$('app-status').textContent=kind==='backup'?'Library backup saved.':'Library JSON export saved.';return;}$('app-status').textContent='The save did not complete.';}catch(e){const code=typeof e==='string'?e:e?.code;const safe=String(code||'').toLowerCase().replace(/[^a-z0-9_]/g,'');$('app-status').textContent=saveErrors[safe]||'The save could not be completed.';}finally{setSaveBusy(false);button.textContent=kind==='backup'?'Save library backup':'Export library as JSON';}}$('backup-library').addEventListener('click',()=>saveLibrary($('backup-library'),'backup_library','backup'));$('export-library-json').addEventListener('click',()=>saveLibrary($('export-library-json'),'export_library_json','json'));
let updateBusy=false;function setUpdateBusy(v){updateBusy=v;$('check').disabled=v;$('install').disabled=v;}
$('check').addEventListener('click',async()=>{if(!native)return;$('install').hidden=true;setUpdateBusy(true);$('message').textContent='Checking for updates…';try{const r=await invoke('check_for_updates');$('message').textContent=r?.message||'Update check complete.';$('install').hidden=r?.status!=='available';}catch(e){$('message').textContent=`Update check failed: ${errText(e)}`;}finally{setUpdateBusy(false);}});
$('install').addEventListener('click',async()=>{if(!native)return;setUpdateBusy(true);$('message').textContent='Installing update…';try{const r=await invoke('install_update');$('message').textContent=r?.message||'Update installed. Restart the application to finish.';}catch(e){$('message').textContent=`Update failed: ${errText(e)}`;}finally{setUpdateBusy(false);}});loadList('',true);status();
