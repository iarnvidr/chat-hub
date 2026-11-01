import {fixtureAdapter, normalizeList, normalizeSearch} from './data.js';
import {branchChoices,orderedMessages,messageText,hasUnsupportedContent,renderMarkdown,speakerPresentation,decorateArchiveTranscript} from './render.js';
import {updateStatus} from './status.js';
import {createConversationTabs, findCapturedChatRecord} from './conversation-tabs.js';
import {installTabShortcuts} from './tab-shortcuts.js';
import {chatgptConversationId} from './continuation.js';
import {groupSearchConversations, appendSearchHighlight} from './search.js';
import {branchForAnchor} from './entity-model.js';
import {createRowActions} from './row-actions.js';
import {appendPinMarker} from './pin-marker.js';
const $=id=>document.getElementById(id),native=typeof window.__TAURI_INTERNALS__?.invoke==='function';const invoke=(...args)=>window.__TAURI_INTERNALS__.invoke(...args);window.ChatHub={invoke};const fixture=!native&&['localhost','127.0.0.1'].includes(location.hostname)&&new URLSearchParams(location.search).get('fixture')==='demo';
const adapter=fixture?fixtureAdapter():{
  async listConversations(query,cursor){return invoke('list_conversations',{query,cursor,source:$('history-source').value,projectKey:$('project-filter').value||null,tag:$('tag-filter').value||null,archiveState:$('show-archived').getAttribute('aria-pressed')==='true'?'archived':'active'});},
  async getConversation(id,leafNodeId){return invoke('get_conversation',leafNodeId!==undefined?{id,leafNodeId}:{id});},
  async importChatgptExport(){return invoke('import_chatgpt_export');}
};
$('fixture-banner').hidden=!fixture;let selected=null,conversation=null,items=[],timer,queryToken=0,readerToken=0,cursor=null,revision=null,queryValue='',cursorRetry=false,lastConversationButton=null,liveDraftSequence=0;const errText=e=>String(e?.message||e);const safeError=code=>({invalid_request:'The request was invalid.',invalid_cursor:'The page cursor is invalid.',stale_cursor:'The library changed; restart the search.',library_changed:'The library changed; restart the search.',unauthorized:'Sign in to the desktop service.',not_found:'The requested conversation was not found.',service_unavailable:'The desktop service is unavailable.',busy:'The desktop service is busy.',response_too_large:'This conversation is too large to display (8 MiB response limit). No content has been truncated.'}[code]||'The desktop service returned an error.');function showError(s){$('list-state').textContent=s;}
function providerLabel(provider) {
  return {'chatgpt':'ChatGPT', 'paseo-chatgpt':'ChatGPT · imported', 'paseo-codex_local':'Codex local · imported', 'paseo-codex_cloud':'Codex cloud · imported'}[provider] || provider || 'Unknown provider';
}
const projectNames = new Map();
const rowActions = createRowActions({invoke, onChanged: () => window.dispatchEvent(new Event('chat-hub:history-updated'))});
function projectLabel(project) {
  return project.display_name || project.title || projectNames.get(project.project_key) || 'Project name pending';
}
function drawList() {
  const box = $('conversation-list');
  rowActions.close(false);
  box.replaceChildren();
  $('load-more').hidden = !cursor;
  const displayed = queryValue ? groupSearchConversations(items) : items;
  if (queryValue && displayed.length) showError(`${displayed.length} chat${displayed.length === 1 ? '' : 's'}${cursor ? ' · more results available' : ''}`);
  for (const c of displayed) {
    const button = document.createElement('button');
    button.className = 'conversation-item' + (c.id === selected ? ' selected' : '');
    button.type = 'button';
    button.setAttribute('aria-label',c.title || 'Untitled conversation');
    const title = document.createElement('strong'), metadata = document.createElement('span');
    if (queryValue) appendSearchHighlight(title, c.title ?? 'Untitled conversation', queryValue);
    else title.textContent = c.title ?? 'Untitled conversation';
    appendPinMarker(title,c.pinned === true);
    metadata.textContent = providerLabel(c.provider);
    metadata.hidden = $('history-source').value === 'chatgpt';
    if (!c.provider?.startsWith('paseo-') && Number.isFinite(c.message_count)) metadata.textContent += ` · ${c.message_count} messages`;
    button.append(title, metadata);
    if (c.project) {
      const project = document.createElement('span'); project.className = 'project-label';
      project.textContent = projectLabel(c.project);
      button.append(project);
    }
    if (c.search_hit) {
      const snippet = document.createElement('span');
      snippet.className = 'search-snippet'; appendSearchHighlight(snippet, c.snippet || '', queryValue); button.append(snippet);
      if (c.matched_message_count > 1) {
        const count = document.createElement('span'); count.className = 'search-match-count';
        count.textContent = `${c.matched_message_count} matching messages`; button.append(count);
      }
    }
    button.addEventListener('click', () => openConversation(c.id, c.search_leaf_node_id, c.search_node_id));
    box.append(rowActions.wrap(c,button));
  }
  if (!items.length && !$('list-state').textContent.includes('failed')) showError('No conversations found.');
}
async function loadList(query=$('search').value,reset=true){const token=++queryToken;if(reset){queryValue=query.trim();cursor=null;revision=null;items=[];cursorRetry=false;}showError('Loading conversations…');try{const response=await adapter.listConversations(queryValue,cursor);const page=queryValue?normalizeSearch(response):normalizeList(response);if(token!==queryToken)return;if(!page||!Array.isArray(page.items))throw Error('Invalid conversation list response');if(revision!==null&&page.library_revision!==revision){cursor=null;revision=null;items=[];showError('Library changed; refreshed results');if(!reset)return loadList(queryValue,true);return;}revision=page.library_revision;cursor=page.next_cursor;const seen=new Set(items.map(x=>queryValue?JSON.stringify([x.id,x.search_node_id]):x.id));items.push(...page.items.filter(x=>!seen.has(queryValue?JSON.stringify([x.id,x.search_node_id]):x.id)));showError('');drawList();}catch(e){if(token!==queryToken)return;const s=errText(e);if(!reset&&!cursorRetry&&/invalid_cursor|stale_cursor|library_changed/i.test(s)){cursorRetry=true;cursor=null;revision=null;items=[];showError('Library changed; restarting from the first page.');return loadList(queryValue,false);}showError(`Could not load conversations: ${safeError(s)}`);}}
const conversationTabs = createConversationTabs({
  container: $('conversation-tabs'),
  onSelect: tab => {
    if (tab.mode === 'live') window.dispatchEvent(new CustomEvent('chat-hub:show-live', {detail: {
      conversationId: tab.sourceConversationId ?? null, title: tab.title, localId: tab.localId,
      leafNodeId: tab.leafNodeId, accountScope: tab.accountScope,
    }}));
    else openConversation(tab.id, tab.leafNodeId, tab.anchorNodeId);
  },
  onEmpty: () => {
    ++readerToken; selected = null; conversation = null;
    $('reader').hidden = true; $('reader-empty').hidden = false;
    $('messages').replaceChildren(); drawList();
    window.dispatchEvent(new Event('chat-hub:show-local'));
  },
});
installTabShortcuts({
  target: window,
  tabs: conversationTabs,
  onNewTab: () => window.dispatchEvent(new Event('chat-hub:live-open')),
});
window.addEventListener('chat-hub:live-open', () => {
  ++readerToken;
  conversationTabs.open({id: `draft:chatgpt:${Date.now()}-${++liveDraftSequence}`,
    title: 'New conversation', mode: 'live', sourceConversationId: null});
});
window.addEventListener('chat-hub:read-saved', event => {
  if (event.detail?.localId) openConversation(event.detail.localId, event.detail.leafNodeId);
});
$('continue-chatgpt').addEventListener('click', () => {
  const sourceConversationId = chatgptConversationId(conversation?.conversation);
  if (!sourceConversationId || !selected) return;
  const title = conversation.conversation.title || 'Conversation';
  const leafNodeId = $('branch').value || conversation.selected_node_id;
  ++readerToken;
  conversationTabs.update(selected, {mode: 'live', sourceConversationId, localId: selected, title, leafNodeId,
    accountScope: conversation.conversation.account_scope});
  window.dispatchEvent(new CustomEvent('chat-hub:show-live', {detail: {
    conversationId: sourceConversationId, title, localId: selected, leafNodeId,
    accountScope: conversation.conversation.account_scope, freshNavigation: true,
  }}));
});
function reconcileLiveTab(tab) {
  if (!tab?.sourceConversationId) return false;
  const record = findCapturedChatRecord(items, tab.sourceConversationId, tab.accountScope);
  if (!record) return false;
  const prior = items.find(item => item.id === tab.localId);
  const changes = {mode: 'live', sourceConversationId: tab.sourceConversationId,
    localId: record.id, accountScope: record.account_scope,
    title: record.title || tab.title || 'ChatGPT conversation'};
  if (prior?.provider === 'paseo-chatgpt') {
    changes.leafNodeId = null;
    changes.anchorNodeId = null;
    changes.branchReset = true;
  }
  if (tab.id === record.id) { conversationTabs.update(tab.id, changes); return true; }
  return conversationTabs.rekey(tab.id, record.id, changes);
}
function providerConversationChanged(event) {
  const sourceConversationId = event.detail?.conversationId;
  if (typeof sourceConversationId !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(sourceConversationId)) return;
  let tab = conversationTabs.selected();
  if (tab?.mode !== 'live') tab = null;
  const accountScope = typeof event.detail?.accountScope === 'string' &&
    /^chatgpt:observed:[0-9a-f]{64}$/.test(event.detail.accountScope)
    ? event.detail.accountScope : null;
  if (!tab) {
    const canonical = `live:chatgpt:${sourceConversationId}`;
    const id = conversationTabs.get(canonical) ? `${canonical}:pending:${++liveDraftSequence}` : canonical;
    conversationTabs.open({id, title: 'ChatGPT conversation', mode: 'live', sourceConversationId, accountScope, localId: null, leafNodeId: null});
    tab = conversationTabs.get(id);
  } else if (tab.sourceConversationId === sourceConversationId &&
      !/^chatgpt:observed:[0-9a-f]{64}$/.test(tab.accountScope || '')) {
    // Continuing an imported copy binds this live tab to the current session,
    // without changing the saved record's ownership or creating a duplicate tab.
    conversationTabs.update(tab.id, {accountScope});
    tab = conversationTabs.get(tab.id);
  } else if (tab.sourceConversationId === sourceConversationId &&
      (tab.accountScope !== accountScope && (tab.accountScope !== null || accountScope !== null))) {
    // A matching URL cannot prove that the provider account stayed the same.
    // Keep the old tab scoped to its original account and start a separate live placeholder.
    const canonical = `live:chatgpt:${sourceConversationId}`;
    const id = conversationTabs.get(canonical) ? `${canonical}:pending:${++liveDraftSequence}` : canonical;
    const prior = items.find(item => item.id === tab.localId);
    const cachedTranscript = prior?.provider === 'paseo-chatgpt';
    conversationTabs.open({id, title: tab.title || 'ChatGPT conversation', mode: 'live', sourceConversationId, accountScope,
      localId: cachedTranscript ? tab.localId : null,
      leafNodeId: cachedTranscript ? tab.leafNodeId : null,
      anchorNodeId: cachedTranscript ? tab.anchorNodeId : null});
    tab = conversationTabs.get(id);
  } else if (tab.sourceConversationId !== sourceConversationId) {
    const changes = {sourceConversationId, title: 'ChatGPT conversation', localId: null,
      accountScope, leafNodeId: null, anchorNodeId: null};
    if (tab.id.startsWith('draft:chatgpt:')) {
      const canonical = `live:chatgpt:${sourceConversationId}`;
      if (conversationTabs.rekey(tab.id, canonical, changes)) tab = conversationTabs.get(canonical);
      else {
        const id = `${canonical}:pending:${++liveDraftSequence}`;
        conversationTabs.open({id, ...changes, mode: 'live'}); tab = conversationTabs.get(id);
      }
    } else {
      const canonical = `live:chatgpt:${sourceConversationId}`;
      const id = conversationTabs.get(canonical) ? `${canonical}:pending:${++liveDraftSequence}` : canonical;
      conversationTabs.open({id, ...changes, mode: 'live'}); tab = conversationTabs.get(id);
    }
  }
  reconcileLiveTab(tab);
}
window.addEventListener('chat-hub:provider-conversation', providerConversationChanged);
async function rereadSelected() {
  const tab = conversationTabs.selected();
  if (tab?.mode === 'live') {
    const id = tab.localId;
    if (!id) return;
    try {
      const detail = await adapter.getConversation(id, tab.leafNodeId);
      if (detail?.conversation?.id !== id || conversationTabs.selected()?.id !== tab.id) return;
      const title = detail.conversation.title || tab.title;
      conversationTabs.update(tab.id, {title, leafNodeId: tab.branchReset ? null : (tab.leafNodeId ?? detail.selection?.selected_node_id)});
      if (conversation?.id === id) conversation = {...detail.conversation, ...detail,
        selected_node_id: detail.selection?.selected_node_id};
    } catch { /* A background refresh must not take focus away from the live ChatGPT view. */ }
    return;
  }
  if (!selected) return;
  openConversation(selected, $('branch').value || conversation?.selected_node_id,
    undefined, {focus: false, selectTab: false});
}
window.addEventListener('chat-hub:history-updated', async () => {
  await loadProjects();
  await loadList($('search').value, true);
  const tab = conversationTabs.selected();
  if (tab?.mode === 'live') reconcileLiveTab(tab);
  await rereadSelected();
});
for (const [id, source] of [['source-chat', 'chatgpt'], ['source-codex', 'codex']]) {
  $(id).addEventListener('click', () => {
    if ($('history-source').value === source) return;
    $('history-source').value = source;
    $('source-chat').setAttribute('aria-pressed', String(source === 'chatgpt'));
    $('source-codex').setAttribute('aria-pressed', String(source === 'codex'));
    $('project-filter').value = '';
    loadProjects();
    loadList($('search').value, true);
  });
}
$('history-source').addEventListener('change', () => loadList($('search').value, true));
$('project-filter').addEventListener('change', () => loadList($('search').value, true));
$('show-archived').addEventListener('click',()=>{
  const archived=$('show-archived').getAttribute('aria-pressed')!=='true';
  $('show-archived').setAttribute('aria-pressed',String(archived));
  $('chat-list-label').textContent=archived?'Archived chats':'Chats';
  loadList($('search').value,true);
});
let projectsRequest = 0;
async function loadProjects() {
  if (!native) return;
  const token = ++projectsRequest, source = $('history-source').value;
  try {
    const response = await invoke('list_projects', {source});
    if (token !== projectsRequest || source !== $('history-source').value || !Array.isArray(response?.projects)) return;
    const select = $('project-filter'), selectedProject = select.value;
    projectNames.clear();
    select.replaceChildren();
    const all = document.createElement('option'); all.value = ''; all.textContent = 'All projects'; select.append(all);
    const none = document.createElement('option'); none.value = '__none__'; none.textContent = 'No project'; select.append(none);
    for (const [index, project] of response.projects.entries()) {
      const option = document.createElement('option'); option.value = project.project_key;
      const label = project.display_name || project.title || `Unnamed project ${index + 1}`;
      projectNames.set(project.project_key, label);
      option.textContent = `${label} (${project.conversation_count})`;
      option.selected = option.value === selectedProject; select.append(option);
    }
    if (selectedProject === '__none__' || response.projects.some(project => project.project_key === selectedProject)) select.value = selectedProject;
    $('edit-project-name').disabled = !select.value || select.value === '__none__';
    drawList();
    if (conversation?.conversation?.project) $('reader-project').textContent = projectLabel(conversation.conversation.project);
  } catch { /* Saved chats remain usable while project metadata is unavailable. */ }
}
loadProjects();
window.addEventListener('chat-hub:open-conversation', event => {
  if (typeof event.detail?.id === 'string') openConversation(event.detail.id, event.detail.leafNodeId, event.detail.anchorNodeId);
});
$('show-message-graph').addEventListener('click', () => {
  if (conversation?.conversation) window.dispatchEvent(new CustomEvent('chat-hub:show-message-graph', {detail: conversation}));
});
$('show-chat-connections').addEventListener('click', () => {
  if (conversation?.conversation) window.dispatchEvent(new CustomEvent('chat-hub:show-chat-connections', {detail: conversation.conversation}));
});
$('set-chat-project').addEventListener('click', () => {
  if (conversation?.conversation) window.dispatchEvent(new CustomEvent('chat-hub:edit-chat-project', {detail: conversation.conversation}));
});
function openConversation(id, leafNodeId, anchorNodeId, options={}) {
  const focus = options.focus !== false;
  if (focus) window.dispatchEvent(new Event('chat-hub:show-local'));
  const title = items.find(item => item.id === id)?.title || 'Conversation';
  if (options.selectTab !== false) conversationTabs.open({id, title, leafNodeId, anchorNodeId, mode: 'local'});
  const token = ++readerToken;
  lastConversationButton = [...document.querySelectorAll('.conversation-item')].find(b => b.classList.contains('selected')) || document.activeElement;
  selected = id;
  window.dispatchEvent(new CustomEvent('chat-hub:conversation-loading',{detail:{id}}));
  document.body.classList.add('mobile-reader');
  $('reader-empty').hidden = true;
  $('reader').hidden = false;
  $('title').textContent = 'Loading…';
  $('account-attribution').textContent = '';
  $('messages').replaceChildren();
  $('continue-chatgpt').hidden = true;
  $('set-chat-project').hidden = true;
  $('reader-project').hidden = true;
  adapter.getConversation(id, leafNodeId).then(c => {
    if(token!==readerToken)return;
    if (c?.conversation?.id !== id) throw Error('not_found');
    if(anchorNodeId&&!leafNodeId){const leaf=branchForAnchor(c,anchorNodeId);if(leaf&&leaf!==c.selection?.selected_node_id){conversationTabs.update(options.tabId||id,{leafNodeId:leaf,anchorNodeId});openConversation(id,leaf,anchorNodeId,{...options,selectTab:false});return;}}
    conversation = {...c.conversation, ...c, selected_node_id: c.selection?.selected_node_id};
    $('continue-chatgpt').hidden = !chatgptConversationId(c.conversation);
    $('set-chat-project').hidden = !chatgptConversationId(c.conversation);
    $('title').textContent = c.conversation?.title ?? 'Untitled conversation';
    const project = c.conversation?.project;
    $('reader-project').hidden = !project;
    $('reader-project').textContent = project ? projectLabel(project) : '';
    conversationTabs.update(options.tabId || id, {title: $('title').textContent});
    $('provider').textContent = providerLabel(c.conversation?.provider);
    const scope = c.conversation?.account_scope;
    $('account-attribution').textContent = typeof scope !== 'string' || scope.startsWith('provisional:')
      ? 'Account ownership unverified'
      : scope.startsWith('chatgpt:observed:') ? 'Account attributed from provider request' : 'Account scope recorded';
    const select = $('branch');
    const cachedTranscript = c.conversation?.provider?.startsWith('paseo-');
    select.closest('label').hidden = !!cachedTranscript;
    if (cachedTranscript) $('account-attribution').textContent = 'Saved copy · read offline or continue this chat in ChatGPT';
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
    window.dispatchEvent(new CustomEvent('chat-hub:conversation-loaded',{detail:c.conversation}));
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
$('tag-filter').addEventListener('change',()=>loadList($('search').value,true));
window.addEventListener('chat-hub:tags-updated',()=>loadList($('search').value,true));
function drawMessages(leaf){const root=$('messages');root.replaceChildren();try{for(const m of orderedMessages(conversation,leaf)){const article=document.createElement('article'),role=document.createElement('div'),body=document.createElement('div'),speaker=speakerPresentation(m);article.className='message archive-message';article.dataset.speaker=speaker.key;role.className='role archive-message__meta';role.textContent=speaker.label;body.className='markdown archive-message__body';const text=messageText(m);if(text)renderMarkdown(text,body);if(m.role==='archive')decorateArchiveTranscript(body);if(hasUnsupportedContent(m)){const marker=document.createElement('div');marker.className='unsupported-content';marker.textContent='[Unsupported content]';body.append(marker);}article.id=messageDomId(conversation.id,m.node_id||'unknown');article.dataset.nodeId=m.node_id||'';article.append(role,body);root.append(article);}}catch(e){root.textContent=safeError(errText(e));}}
$('back-to-library').addEventListener('click',()=>{document.body.classList.remove('mobile-reader');const target=lastConversationButton?.isConnected?lastConversationButton:$('search');target.focus();});$('search').addEventListener('input',()=>{clearTimeout(timer);timer=setTimeout(()=>loadList($('search').value,true),150);});$('import').addEventListener('click',async()=>{const b=$('import');b.disabled=true;b.textContent='Importing…';try{const r=await adapter.importChatgptExport();if(r?.cancelled){$('app-status').textContent='Import cancelled.';return;}if(!r||!r.result||typeof r.result!=='object')throw Error('Invalid import receipt');const x=r?.result||{},errs=Array.isArray(x.errors)?x.errors:[];$('app-status').textContent=`Import finished: ${x.imported??0} imported, ${x.updated??0} updated, ${x.unchanged??0} unchanged, ${x.failed??0} failed${x.errors_truncated?` (error list truncated; showing ${errs.length})`:''}${errs.length?`. ${errs.map(e=>e.message||String(e)).join('; ')}`:'.'}`;await loadList($('search').value,true);}catch(e){showError(`Import failed: ${errText(e)}`);}finally{b.disabled=false;b.textContent='Import export';}});$('load-more')?.addEventListener('click',()=>{if(cursor)loadList(queryValue,false);});const status=()=>updateStatus({invoke:native?invoke:null,setVersion:v=>$('version').textContent=v,setMessage:v=>$('message').textContent=v,setHealth:v=>$('health').textContent=v,setRetryVisible:v=>{$('retry-health').hidden=!v;}});$('retry-health').addEventListener('click',status);
let saveBusy=false;function setSaveBusy(v){saveBusy=v;$('backup-library').disabled=v;$('export-library-json').disabled=v;}const saveErrors={cancelled:'Save cancelled.',invalid_request:'The save request was invalid.',unauthorized:'Sign in to the desktop service before saving.',busy:'The desktop service is busy. Try again later.',archive_busy:'Another archive save is already in progress.',service_unavailable:'The desktop service is unavailable. Try again later.',not_found:'The library was not found.',permission_denied:'The destination could not be written.',io_error:'The destination could not be written.',archive_published:'The backup file was published, but its durability could not be confirmed. Check the saved file before trying again.'};async function saveLibrary(button,command,kind){if(!native||saveBusy)return;setSaveBusy(true);button.textContent=kind==='backup'?'Saving backup…':'Exporting JSON…';$('app-status').textContent=kind==='backup'?'Choose where to save the library backup.':'Choose where to export the library as JSON.';try{const r=await invoke(command);if(!r||typeof r.cancelled!=='boolean'||typeof r.published!=='boolean'||typeof r.durability_confirmed!=='boolean'||r.kind!==kind)throw Error('invalid_request');if(r.cancelled){$('app-status').textContent='Save cancelled.';return;}if(r.published&&!r.durability_confirmed){$('app-status').textContent='The file was published, but its durability could not be confirmed. Check the saved file before trying again.';return;}if(r.published&&r.durability_confirmed){$('app-status').textContent=kind==='backup'?'Library backup saved.':'Library JSON export saved.';return;}$('app-status').textContent='The save did not complete.';}catch(e){const code=typeof e==='string'?e:e?.code;const safe=String(code||'').toLowerCase().replace(/[^a-z0-9_]/g,'');$('app-status').textContent=saveErrors[safe]||'The save could not be completed.';}finally{setSaveBusy(false);button.textContent=kind==='backup'?'Save library backup':'Export library as JSON';}}$('backup-library').addEventListener('click',()=>saveLibrary($('backup-library'),'backup_library','backup'));$('export-library-json').addEventListener('click',()=>saveLibrary($('export-library-json'),'export_library_json','json'));
let updateBusy=false;function setUpdateBusy(v){updateBusy=v;$('check').disabled=v;$('install').disabled=v;}
$('check').addEventListener('click',async()=>{if(!native)return;$('install').hidden=true;setUpdateBusy(true);$('message').textContent='Checking for updates…';try{const r=await invoke('check_for_updates');$('message').textContent=r?.message||'Update check complete.';$('install').hidden=r?.status!=='available';}catch(e){$('message').textContent=`Update check failed: ${errText(e)}`;}finally{setUpdateBusy(false);}});
$('install').addEventListener('click',async()=>{if(!native)return;setUpdateBusy(true);$('message').textContent='Installing update…';try{const r=await invoke('install_update');$('message').textContent=r?.message||'Update installed. Restart the application to finish.';}catch(e){$('message').textContent=`Update failed: ${errText(e)}`;}finally{setUpdateBusy(false);}});loadList('',true);status();
