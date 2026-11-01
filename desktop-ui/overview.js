import {prepareConnections, prepareEntityConnections, messageGraph} from './graph-model.js';
import {createGraphView} from './graph-view.js';
import {entityGraph,entityKinds} from './entity-model.js';
import {normalizeSearch} from './data.js';
import {groupSearchConversations} from './search.js';
import {createMapSearch} from './map-search.js';
import {normalizeOverviewMode,overviewModeForAction} from './overview-mode.js';
import {orderedMessages} from './render.js';
import {createHoverPreview} from './graph-interactions.js';
import {createTranscriptPager} from './saved-transcript.js';

const $ = id => document.getElementById(id);
const panel = $('library-overview');
let request = 0, rebuilding, refreshTimer, graph, messageDetail, entityDetail, focusId,selectionRequest=0;
const DISPLAY_MODE_KEY='chat-hub.overview-display-mode';
let displayMode=(()=>{try{return normalizeOverviewMode(localStorage.getItem(DISPLAY_MODE_KEY));}catch{return 'graph';}})();
const native = () => typeof window.__TAURI_INTERNALS__?.invoke === 'function';
const invoke = (command, args = {}) => window.ChatHub.invoke(command, args);
const filters = () => ({source: $('history-source').value, projectKey: $('project-filter').value || null,tag:$('tag-filter').value||null});
const element = (tag, text, cls) => {
  const node = document.createElement(tag); if (text !== undefined) node.textContent = text;
  if (cls) node.className = cls; return node;
};
const hoverPreview=element('aside',undefined,'graph-hover-preview');
hoverPreview.hidden=true;hoverPreview.tabIndex=0;hoverPreview.setAttribute('aria-label','Saved chat preview');
$('overview-map').append(hoverPreview);
let previewRows=[];
let hoverPreviewPager,selectedTranscriptPager;
const previewController=createHoverPreview({
  load:({conversationId})=>invoke('get_conversation',{id:conversationId}),
  show:(target,detail,error)=>showRelatedPreview(target,detail,error),
  hide:()=>{hoverPreviewPager?.destroy();hoverPreviewPager=null;hoverPreview.hidden=true;hoverPreview.replaceChildren();},
});
function showRelatedPreview(target,detail,error){
  hoverPreviewPager?.destroy();hoverPreviewPager=null;hoverPreview.replaceChildren();hoverPreview.hidden=false;hoverPreview.scrollTop=0;
  const mapRect=$('overview-map').getBoundingClientRect(),rowRect=target.anchor?.getBoundingClientRect();
  const inspectorRect=$('graph-inspector').getBoundingClientRect();
  const available=inspectorRect.left-mapRect.left-14;
  const width=Math.min(370,available);
  if(width<180){hoverPreview.hidden=true;return;}
  const left=Math.max(8,inspectorRect.left-mapRect.left-width-8);
  const height=Math.min(520,Math.max(160,mapRect.height-20));
  const top=Math.max(8,Math.min(mapRect.height-height-8,(rowRect?.top-mapRect.top||10)-10));
  hoverPreview.style.left=`${left}px`;hoverPreview.style.top=`${top}px`;hoverPreview.style.width=`${width}px`;hoverPreview.style.height=`${height}px`;
  hoverPreview.append(element('h3',target.title||target.label||'Saved chat'));
  if(error||!detail?.conversation){hoverPreview.append(element('p','A preview is unavailable for this saved chat.'));return;}
  const provenance=element('p','Saved locally · preview begins at the start','graph-preview-note');hoverPreview.append(provenance);
  try{
    const body=element('div',undefined,'graph-preview-messages');body.tabIndex=0;body.setAttribute('role','region');body.setAttribute('aria-label','Saved chat preview content');
    hoverPreview.append(body);
    hoverPreviewPager=createTranscriptPager(body,orderedMessages(detail),{batchSize:2,disableLinks:true});
  }catch{
    hoverPreview.append(element('p','This saved chat is too large or incomplete to preview here.'));
  }
}
const view = createGraphView($('graph-canvas'), (node, open) => {
  if (open) return openNode(node);
  showSelection(node);
}, showEdge);
const mapSearch=createMapSearch({searchVisible:query=>view.search(query),
  searchLibrary:async query=>groupSearchConversations(normalizeSearch(await invoke('list_conversations',{...filters(),query,cursor:null})).items),
  render:renderSearchResults});
function state(text) { $('overview-state').textContent = text; $('catalog-status').textContent = text; }
function openChat(id, leafNodeId, anchorNodeId) {
  window.dispatchEvent(new CustomEvent('chat-hub:open-conversation', {detail: {id, leafNodeId, anchorNodeId}}));
}
function openNode(node) {
  if (node.kind === 'chat') openChat(node.conversation_id,node.search_leaf_node_id,node.search_node_id);
  else if(node.entity_id)showSelection(node);
  else if (node.message_id) openChat(messageDetail.conversation.id, node.leaf_node_id, node.id);
}
function leave() {
  document.body.classList.remove('overview-open'); panel.hidden = true;
  $('show-overview').setAttribute('aria-pressed', 'false');
}
function enter() {
  window.dispatchEvent(new Event('chat-hub:show-local'));
  document.body.classList.add('overview-open', 'mobile-reader'); panel.hidden = false;
  $('show-overview').setAttribute('aria-pressed', 'true');
}
function applyChatDisplayMode(){
  if($('overview-kind').value!=='chats')return;
  const list=displayMode==='list';
  $('overview-map').hidden=list;
  $('graph-caption').hidden=list;
  $('overview-recent-section').hidden=false;
  $('overview-recent-section').open=list;
  $('overview-recent-title').textContent=list?'Saved chats':'Chats in this map';
  panel.classList.toggle('overview-list-mode',list);
}
export function setOverviewDisplayMode(value){
  if(!['graph','list'].includes(value))return;
  displayMode=overviewModeForAction(value==='list'?'show-list':'show-graph',displayMode);
  try{localStorage.setItem(DISPLAY_MODE_KEY,value);}catch{}
  applyChatDisplayMode();
  if($('overview-kind').value==='chats')window.dispatchEvent(new Event(value==='list'?'chat-hub:show-chat-list':'chat-hub:show-chat-graph'));
}
export function getOverviewDisplayMode(){return displayMode;}
export function forceOverviewGraph(){
  displayMode=overviewModeForAction('graph-drilldown',displayMode);
  try{localStorage.setItem(DISPLAY_MODE_KEY,displayMode);}catch{}
  applyChatDisplayMode();
  window.dispatchEvent(new Event('chat-hub:force-overview-graph'));
}
export function showOverview(focus = true) {
  focusId = null; messageDetail = null; entityDetail=null; panel.classList.remove('message-mode', 'connections-mode', 'selection-open','entity-mode','entity-category-mode');
  window.dispatchEvent(new Event('chat-hub:show-chat-map'));
  $('overview-title').textContent = 'Library map';
  $('overview-back').hidden = $('overview-read-chat').hidden = true;
  $('overview-stats').hidden = false;
  $('overview-recent-title').textContent = 'Chats in this map';
  enter(); applyChatDisplayMode();
  if(displayMode==='list')window.dispatchEvent(new Event('chat-hub:show-chat-list'));
  else refresh(true);
  if (focus) panel.focus();
}
async function focusChat(id) {
  forceOverviewGraph();focusId = id; messageDetail = null; entityDetail=null;window.dispatchEvent(new Event('chat-hub:show-chat-map'));panel.classList.remove('message-mode','entity-mode','entity-category-mode'); panel.classList.add('connections-mode');
  $('overview-title').textContent = 'Chat connections';
  $('overview-stats').hidden = false; $('overview-back').hidden = false;
  $('overview-read-chat').hidden = true;
  enter(); await refresh(true);
}
function renderStats(stats) {
  const root = $('overview-stats'); root.replaceChildren();
  const categories = [['chats','Chats','chats'],['repository','Repos','repositories'],['pull_request','PRs','pull_requests'],['issue','Issues','issues'],['domain','Domains','domains'],['file_artifact','Files','file_artifacts']];
  for (const [kind,label,key] of categories) {
    const button=element('button');button.type='button';button.className='index-category';
    button.append(element('strong',Number(stats[key]||0).toLocaleString()),element('span',label));
    button.setAttribute('aria-label',`${label}: ${Number(stats[key]||0).toLocaleString()}`);
    button.addEventListener('click',()=>{$('overview-kind').value=kind;$('overview-kind').dispatchEvent(new Event('change'));});
    root.append(button);
  }
}
async function rebuild(manual = false) {
  if (rebuilding) return rebuilding;
  if (!native()) { state('Open the desktop app to index your saved chats.'); return; }
  $('rebuild-catalog').disabled = true;
  state('Indexing saved chats locally… You can keep reading while this runs.');
  rebuilding = invoke('rebuild_catalog_index').then(() => {
    state('Chat index is ready.');
    window.dispatchEvent(new Event('chat-hub:catalog-indexed'));
  }).catch(() => {
    state('The index could not rebuild. Your saved chats are available; retry in Settings.');
  }).finally(() => { rebuilding = null; $('rebuild-catalog').disabled = false; });
  await rebuilding;
  if (manual && !messageDetail) await refresh(false);
}
async function refresh(allowRebuild = false) {
  if (!native() || messageDetail || entityDetail || $('overview-kind').value!=='chats') return;
  if(displayMode==='list'){window.dispatchEvent(new Event('chat-hub:show-chat-list'));return;}
  const token = ++request, args = filters(), requestedFocus = focusId;
  const project = $('project-filter').selectedOptions?.[0]?.textContent || 'All projects';
  $('overview-scope').textContent = `${args.source === 'codex' ? 'Codex' : 'Chat'} · ${project}${args.tag?` · #${args.tag}`:''}`;
  try {
    let overview = await invoke('catalog_overview', args);
    if (token !== request || messageDetail) return;
    if (!overview?.index || !overview?.stats) throw Error('invalid_catalog');
    if (overview.index.stale && allowRebuild) {
      await rebuild();
      if (token !== request || messageDetail) return;
      overview = await invoke('catalog_overview', args);
    }
    const data = await invoke('catalog_graph', {...args, focusId:requestedFocus, limit:100});
    if (token !== request || messageDetail) return;
    if (!Array.isArray(data?.nodes) || !Array.isArray(data?.edges)) throw Error('invalid_catalog_graph');
    graph = prepareConnections(data); renderStats(overview.stats);$('overview-stats').hidden=!!args.tag; renderGraph();applyChatDisplayMode();
    $('overview-recent-title').textContent = `${graph.nodes.length} chats in this map`;
    renderList(graph.nodes.filter(node => node.kind === 'chat'));
    const focused = graph.nodes.find(node => node.id === graph.focus_id);
    if (focused) showSelection(focused);
    $('graph-caption').textContent = requestedFocus
      ? `${Math.max(0, graph.nodes.length - 1)} related chats · Select a line for its evidence. Back returns to the library map.`
      : `${graph.nodes.length} chats · ${graph.edges.length} connections · ${graph.clusters} clusters${graph.unmapped_count ? ` · ${graph.unmapped_count} other chats in the sidebar` : ''}. Select to inspect; double-click to open.`;
    state(overview.index.stale ? 'Index needs refreshing. Rebuild it in Settings.' : '');
  } catch (error) {
    console.error('Chat map could not render:', String(error?.message || error).slice(0,180));
    if (token === request) state('Connections could not load. Your saved chats remain available in the sidebar.');
  }
}
function renderList(nodes) {
  const root = $('overview-recent'); root.replaceChildren();
  for (const node of nodes) {
    const button = element('button', undefined, 'overview-chat'); button.type = 'button';
    button.append(element('span', node.kind === 'message' ? `${node.label}: ${node.excerpt || 'Context'}` : node.label),
      element('small', 'Inspect'));
    button.addEventListener('click', () => view.select(node.id)); root.append(button);
  }
}
function renderGraph() {
  selectedTranscriptPager?.destroy();selectedTranscriptPager=null;
  panel.classList.remove('chat-selection-open');
  for(const unbind of previewRows.splice(0))unbind();previewController.close();
  panel.classList.remove('selection-open');
  $('map-search').value='';mapSearch.reset();
  const category=panel.classList.contains('entity-category-mode');
  $('graph-selection').replaceChildren(element('h2', messageDetail ? 'Explore a message' : category?'Explore an indexed item':'Explore a chat'),
    element('p', messageDetail ? 'Select a message to read it on its original branch.' : category?'Select an item to inspect its source chats. Select a line for its reference.':'Select a chat to find related conversations. Select a line to see the connection.'));
  view.render(graph, Boolean(messageDetail));
}
function appendAction(root, text, action) {
  const button = element('button', text); button.type = 'button'; button.addEventListener('click', action); root.append(button);
}
function showEdge(edge) {
  ++selectionRequest;
  panel.classList.add('selection-open');
  const root = $('graph-selection'); root.replaceChildren(element('h2', edge.basis === 'entity_reference'?'Saved reference':edge.basis === 'shared_entity'?'Shared indexed item':edge.basis === 'project' ? 'Same project' : edge.basis === 'parent' ? 'Message sequence' : 'Shared topics'));
  const names = [edge.source, edge.target].map(id => graph.nodes.find(node => node.id === id)?.label || 'Chat');
  root.append(element('p', names.join(' ↔ ')), element('p', edge.reason || 'Original stored parent relationship.'));
  if (edge.evidence?.length) root.append(element('p', `Evidence: ${edge.evidence.join(', ')}`));
  for (const id of [edge.source, edge.target]) {
    const node = graph.nodes.find(node => node.id === id);
    if (node) appendAction(root, `Open ${node.label}`, () => openNode(node));
  }
}
function showSelection(node) {
  const token=++selectionRequest;
  selectedTranscriptPager?.destroy();selectedTranscriptPager=null;
  panel.classList.remove('chat-selection-open');
  $('graph-inspector').scrollTop=0;
  for(const unbind of previewRows.splice(0))unbind();
  previewController.close();
  panel.classList.add('selection-open');
  const root = $('graph-selection'); root.classList.remove('chat-inspector');root.scrollTop=0;root.replaceChildren(element('h2', node.label));
  if(node.entity_id){
    if(entityDetail?.entity?.id===node.entity_id){showEntitySelection(root,entityDetail,token);return;}
    root.append(element('p','Loading saved references…'));
    invoke('catalog_entity',{id:node.entity_id}).then(detail=>{
      if(token!==selectionRequest)return;
      entityDetail=detail;showSelection(node);
    }).catch(()=>{if(token===selectionRequest)root.replaceChildren(element('h2',node.label),element('p','This indexed item could not be opened. Try again after the current library task.'));});
    return;
  }
  entityDetail=null;
  if (node.kind === 'chat') {
    panel.classList.add('chat-selection-open');root.classList.add('chat-inspector');
    root.append(element('p', node.cached_archive ? 'Saved transcript. Original message boundaries and branches are unavailable.' : node.message_count===undefined?'Saved conversation':`${node.message_count || 0} stored messages · ${node.branch_count || 0} branch points`));
    const actions=element('div',undefined,'inspector-actions');
    const addCompactAction=(label,action,attrs={})=>{const button=element('button',label);button.type='button';button.setAttribute('aria-label',attrs.aria||label);button.addEventListener('click',action);actions.append(button);return button;};
    addCompactAction('Open',()=>openNode(node),{aria:'Open saved chat'});
    const tagsButton=addCompactAction('Tags',()=>dispatchHoverTags(node,tagsButton),{aria:'Edit chat tags'});
    const hoverTags=()=>dispatchHoverTags(node,tagsButton);
    tagsButton.addEventListener('pointerenter',hoverTags);tagsButton.addEventListener('focus',hoverTags);
    root.append(actions);
    if (node.conversation_id !== focusId) addCompactAction('Explore', () => focusChat(node.conversation_id),{aria:'Explore all chat connections'});
    const transcriptSection=element('section',undefined,'inspector-transcript-section');
    transcriptSection.append(element('h3','Saved conversation · beginning'));
    const transcript=element('div',undefined,'inspector-transcript-scroll saved-transcript-scroll');
    transcript.tabIndex=0;transcript.setAttribute('role','region');transcript.setAttribute('aria-label',`Saved conversation: ${node.label}`);
    transcript.append(element('p','Loading saved messages…','saved-transcript-status'));transcriptSection.append(transcript);root.append(transcriptSection);
    const connectionsSection=element('section',undefined,'inspector-connections-section');connectionsSection.append(element('h3','Related chats'));
    const connectionList=element('div',undefined,'inspector-connections-scroll');connectionList.tabIndex=0;connectionList.setAttribute('role','region');connectionList.setAttribute('aria-label','Related saved chats');
    connectionsSection.append(connectionList);root.append(connectionsSection);
    const detailPromise=invoke('get_conversation',{id:node.conversation_id}).then(detail=>{
      if(token!==selectionRequest)return null;
      transcript.replaceChildren();
      const messages=orderedMessages(detail);
      selectedTranscriptPager=createTranscriptPager(transcript,messages,{batchSize:4});
      return detail;
    }).catch(()=>{
      if(token===selectionRequest){transcript.replaceChildren(element('p','Saved messages could not be loaded. This conversation may exceed the detail size limit.','saved-transcript-status'));}
      return null;
    });
    addCompactAction('Messages', async () => {const detail=await detailPromise;if(detail)showMessages(detail);},{aria:'Open message graph'});
    const connections = graph.edges.filter(edge => edge.source === node.id || edge.target === node.id);
    if (!connections.length) connectionList.append(element('p', 'No strong connections found in this scope. Try All projects, or explore another chat.'));
    for (const edge of connections) {
      const other = graph.nodes.find(item => item.id === (edge.source === node.id ? edge.target : edge.source));
      if (!other) continue;
      const entry = element('div', undefined, 'connection-evidence');
      appendAction(entry, other.label, () => view.select(other.id));
      entry.append(element('p', edge.reason || 'Same project'));
      if (['shared_keyword','shared_entity'].includes(edge.basis) && edge.evidence?.length) entry.append(element('p', edge.evidence.join(' · ')));
       if(other.conversation_id)previewRows.push(previewController.bind(entry,hoverPreview,{conversationId:other.conversation_id,title:other.label}));
       connectionList.append(entry);
    }
  } else if (node.kind === 'message') {
    root.append(element('p', node.excerpt || 'Context node without message text.'));
    if (node.message_id) appendAction(root, 'Open message', () => openNode(node));
  } else if (node.kind === 'project') {
    appendAction(root, 'Filter this project', () => {
      $('project-filter').value = node.project_key; $('project-filter').dispatchEvent(new Event('change', {bubbles:true}));
    });
  }
}
function dispatchHoverTags(node,anchor){window.dispatchEvent(new CustomEvent('chat-hub:hover-tags',{detail:{targetKind:'conversation',targetId:node.conversation_id,title:node.label,anchor,keywords:node.keywords||[],entities:node.entities||[]}}));}
function showEntitySelection(root,detail,token){
  const item=detail.entity,kind=item.kind==='file_artifact'?'file':'entity';
  root.append(element('p',`${entityKinds[item.kind]||'Indexed item'} · ${item.reference_count||detail.references.length} references`));
  const tagRoot=element('div',undefined,'tag-chips');root.append(tagRoot);
  invoke('get_target_tags',{targetKind:kind,targetId:item.id}).then(result=>{if(token!==selectionRequest)return;
    for(const name of result.tags||[])tagRoot.append(element('span',name,'tag-chip'));
  }).catch(()=>{});
  const tagAction=element('button','Tags');tagAction.type='button';tagAction.setAttribute('aria-label','Edit item tags');
  const suggestionConversationIds=[...new Set(detail.references.map(reference=>reference.conversation_id).filter(Boolean))].slice(0,2);
  const hoverTags=()=>window.dispatchEvent(new CustomEvent('chat-hub:hover-tags',{detail:{targetKind:kind,targetId:item.id,title:item.label,anchor:tagAction,suggestionConversationIds}}));
  for(const event of ['pointerenter','focus','click'])tagAction.addEventListener(event,hoverTags);
  root.append(tagAction);
  if(item.kind==='file_artifact'){
    root.append(element('span',item.content_saved?'Saved locally':item.artifact_state==='metadata_present'?'Content not saved':'File link only','entity-file-state'));
    if(item.content_saved){
      appendAction(root,'Open local copy',()=>fileAction('open_artifact_file',{id:item.id}));
      appendAction(root,'Reveal local copy',()=>fileAction('open_artifact_file',{id:item.id,reveal:true}));
      appendAction(root,'Save a copy…',()=>fileAction('export_artifact_file',{id:item.id}));
    }else{
      root.append(element('p','Download the file in its original chat, then attach that copy to keep it available offline.'));
      const ref=detail.references.find(r=>r.conversation_id);
      if(ref)appendAction(root,'Attach downloaded copy…',()=>fileAction('attach_artifact_file',{id:item.id,conversationId:ref.conversation_id,mimeType:item.mime_type}));
    }
  }else if(item.canonical_url){
    root.append(element('p',item.canonical_url,'entity-source-link'));
    appendAction(root,'Open link',()=>fileAction('open_catalog_link',{id:item.id}));
    appendAction(root,'Copy link',async()=>{try{await navigator.clipboard.writeText(item.canonical_url);}catch{state('The link could not be copied.');}});
  }
  if(item.repository_id&&item.repository_id!==item.id)appendAction(root,'View repository',()=>window.dispatchEvent(new CustomEvent('chat-hub:inspect-entity',{detail:{id:item.repository_id}})));
  root.append(element('h3','Referenced in'));
  const seen=new Set();for(const ref of detail.references){
    const key=JSON.stringify([ref.conversation_id,ref.node_id]);if(seen.has(key))continue;seen.add(key);
    const entry=element('div',undefined,'entity-reference');
    appendAction(entry,ref.title||'Open saved chat',()=>openChat(ref.conversation_id,undefined,ref.node_id||undefined));
    entry.append(element('small',ref.relation==='produces'?'Produced here':ref.relation==='attaches'?'Attached here':'Mentioned here'));root.append(entry);
    if(ref.conversation_id)previewRows.push(previewController.bind(entry,hoverPreview,{conversationId:ref.conversation_id,title:ref.title||'Saved chat'}));
    if(seen.size>=20)break;
  }
  if(detail.references_truncated)root.append(element('p','More source references are stored in the index.'));
}
async function fileAction(command,args){
  const buttons=[...$('graph-selection').querySelectorAll('button')];buttons.forEach(button=>button.disabled=true);
  try{const result=await invoke(command,args);
    if(command==='attach_artifact_file'&&!result.cancelled)window.dispatchEvent(new Event('chat-hub:artifact-saved'));
  }catch(error){const code=String(error?.message||error);state(code.includes('file_too_large')?'Files up to 64 MiB can be saved locally.':code.includes('busy')?'The library is busy. Try again after the current task finishes.':'The file or link action could not finish. Your saved copy is unchanged.');}
  finally{buttons.forEach(button=>button.disabled=false);}
}
export function showEntityGraph(detail){
  if(!detail?.entity?.id)return;messageDetail=null;entityDetail=detail;focusId=null;++request;enter();
  panel.classList.remove('message-mode','connections-mode','entity-category-mode');panel.classList.add('entity-mode');
  $('overview-title').textContent=detail.entity.label;$('overview-scope').textContent='Indexed references · saved on this Mac';
  $('overview-stats').hidden=true;$('overview-read-chat').hidden=true;$('overview-back').hidden=false;
  graph=entityGraph(detail);renderGraph();renderList(graph.nodes.filter(n=>n.kind==='chat'));
  $('overview-recent-title').textContent='Source chats';$('graph-caption').textContent=`${graph.nodes.length-1} source chats · select a chat or reference to inspect it.`;
  showSelection(graph.nodes[0]);state('');
}
export function showCatalogEntityGraph(data,label){
  if(!Array.isArray(data?.nodes)||!Array.isArray(data?.edges))throw Error('Invalid category graph');
  messageDetail=null;entityDetail=null;focusId=null;++request;++selectionRequest;enter();
  panel.classList.remove('message-mode','connections-mode','entity-mode','selection-open');
  panel.classList.add('entity-category-mode');
  $('overview-title').textContent=label||'Indexed items';
  $('overview-scope').textContent='Indexed items · exact saved references';
  $('overview-stats').hidden=true;$('overview-read-chat').hidden=true;$('overview-back').hidden=true;
  graph=prepareEntityConnections(data);renderGraph();renderList(graph.nodes);
  $('overview-recent-title').textContent='Items in this graph';
  const total=Number.isSafeInteger(data.total_entities)?data.total_entities:graph.nodes.filter(node=>node.entity_id).length;
  const shown=graph.nodes.filter(node=>node.entity_id).length;
  $('graph-caption').textContent=`${total} indexed items · showing ${shown} items and ${graph.nodes.filter(node=>node.kind==='chat').length} source chats · ${graph.edges.length} saved references${graph.truncated?' in this bounded view':''}. Select an item to inspect it; select a chat to open it.`;
  state('');
}
function showMessages(detail) {
  if (!detail?.conversation?.id) return;
  forceOverviewGraph();window.dispatchEvent(new Event('chat-hub:show-chat-map'));entityDetail=null;messageDetail = detail; ++request; enter(); panel.classList.remove('connections-mode','entity-mode','entity-category-mode'); panel.classList.add('message-mode');
  $('overview-title').textContent = detail.conversation.title || 'Message graph';
  $('overview-scope').textContent = 'Message graph · original stored relationships';
  $('overview-back').hidden = $('overview-read-chat').hidden = false;
  $('overview-stats').hidden = true;
  graph = messageGraph(detail); renderGraph(); renderList(graph.nodes); state('');
  $('overview-recent-title').textContent = 'Messages in this view';
  $('graph-caption').textContent = graph.archived
    ? 'This import contains one transcript. Original message boundaries and alternate branches are unavailable.'
    : `${graph.nodes.length} stored nodes${graph.truncated ? ' shown (view limit)' : ''}. Arrows follow original parent links; the selected branch is highlighted.`;
}
$('show-overview').addEventListener('click', () => showOverview());
$('overview-back').addEventListener('click', () => {if(window.dispatchEvent(new Event('chat-hub:overview-back',{cancelable:true})))showOverview();});
$('overview-read-chat').addEventListener('click', () => openChat(messageDetail?.conversation?.id));
$('rebuild-catalog').addEventListener('click', () => rebuild(true));
$('graph-zoom-in').addEventListener('click', () => view.zoom(1.25));
$('graph-zoom-out').addEventListener('click', () => view.zoom(.8));
$('graph-reset').addEventListener('click', () => view.fit());
$('graph-inspector-close').addEventListener('click', () => panel.classList.remove('selection-open'));
$('graph-inspector').addEventListener('wheel',event=>event.stopPropagation(),{passive:true});
$('map-search').addEventListener('input', () => {
  mapSearch.update($('map-search').value);
});
$('map-search').addEventListener('keydown',event=>{
  if(event.key==='Escape'){event.target.value='';mapSearch.reset();}
  if(event.key==='Enter'){const first=$('map-search-results').querySelector('button');if(first){event.preventDefault();first.click();}}
});
function renderSearchResults(rows,{query,loading,error}){
  const root=$('map-search-results');root.replaceChildren();root.hidden=!query;if(!query)return;
  for(const row of rows){const button=element('button');button.type='button';button.append(element('span',row.label),element('small',row.inMap?'In this map':'Saved chat · explore connections'));
    button.addEventListener('click',async()=>{
      root.hidden=true;const hit={search_leaf_node_id:row.search_leaf_node_id,search_node_id:row.search_node_id,snippet:row.snippet};
      const existing=graph?.nodes.find(node=>node.conversation_id===row.conversation_id);
      if(existing)view.select(existing.id,hit);
      else{await focusChat(row.conversation_id);const node=graph?.nodes.find(n=>n.conversation_id===row.conversation_id);if(node)view.select(node.id,hit);}
    });root.append(button);
  }
  if(!rows.length)root.append(element('p',loading?'Searching saved chats…':error?'Library search is busy. Try again shortly.':'No saved chats match.'));
}
for (const event of ['chat-hub:show-local','chat-hub:show-live','chat-hub:live-open','chat-hub:connect']) window.addEventListener(event, leave);
window.addEventListener('chat-hub:show-message-graph', event => showMessages(event.detail));
window.addEventListener('chat-hub:show-chat-connections', event => focusChat(event.detail.id));
window.addEventListener('chat-hub:appearance-changed',()=>{if(graph)view.render(graph,Boolean(messageDetail));});
window.addEventListener('chat-hub:history-updated', () => {
  clearTimeout(refreshTimer); refreshTimer = setTimeout(() => {if (!panel.hidden) refresh(true);}, 750);
});
for (const id of ['source-chat','source-codex']) $(id).addEventListener('click', () => {if (!panel.hidden&&$('overview-kind').value==='chats') showOverview(false);});
for(const id of['project-filter','tag-filter'])$(id).addEventListener('change',()=>{if(!panel.hidden&&$('overview-kind').value==='chats')showOverview(false);});
if (native()) showOverview(false);
