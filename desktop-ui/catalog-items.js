import {entityKinds} from './entity-model.js';
import {showCatalogEntityGraph,showEntityGraph,showOverview,getOverviewDisplayMode,setOverviewDisplayMode} from './overview.js';
import {normalizeSearch} from './data.js';
import {groupSearchConversations,conversationOpenTarget} from './search.js';
const $=id=>document.getElementById(id),panel=$('library-overview');
const invoke=(command,args={})=>window.ChatHub.invoke(command,args);
let mode='chats',conversationId=null,cursor=null,items=[],searchHits=[],generation=0,selected=null,loading=false,searchTimer,viewMode=getOverviewDisplayMode();
let refreshTimer;
const element=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
const filters=()=>({source:$('history-source').value,projectKey:$('project-filter').value||null,tag:$('tag-filter').value||null});
const display=element('div',undefined,'overview-display-mode');display.id='overview-display-mode';display.setAttribute('role','group');display.setAttribute('aria-label','Overview view');
const graphButton=element('button','Graph');graphButton.type='button';graphButton.id='overview-mode-graph';
const listButton=element('button','List');listButton.type='button';listButton.id='overview-mode-list';
display.append(graphButton,listButton);$('overview-kind').after(display);
function updateDisplayButtons(){for(const [button,value]of[[graphButton,'graph'],[listButton,'list']]){button.setAttribute('aria-pressed',String(viewMode===value));button.classList.toggle('selected',viewMode===value);}}
function controls(list,detail=false){
  const chats=mode==='chats';
  const showList=chats?viewMode==='list':list;
  panel.classList.toggle('entity-list-open',showList);panel.classList.toggle('category-graph-open',!chats&&!list&&!detail);
  $('catalog-items-panel').hidden=!(showList&&(!chats||!detail));
  $('overview-map').hidden=chats?viewMode==='list':list;
  $('catalog-search').hidden=detail||(chats&&viewMode!=='list');
  $('catalog-search').placeholder=chats?'Search saved chats…':'Search indexed items…';
  $('overview-recent-section').hidden=chats?viewMode==='list':list;
  $('graph-caption').hidden=showList;
  $('artifact-state-filter').hidden=mode!=='file_artifact'||detail;
  if(chats&&viewMode==='list')$('overview-recent-section').open=false;
  updateDisplayButtons();
}
function title(){return entityKinds[mode]||'Indexed items';}
async function showItems(reset=true){
  if(mode==='chats')return viewMode==='list'?showChatList(reset):undefined;
  const token=++generation;loading=true;if(reset){cursor=null;items=[];selected=null;$('catalog-items-list').replaceChildren();}
  window.dispatchEvent(new Event('chat-hub:show-local'));document.body.classList.add('overview-open','mobile-reader');panel.hidden=false;
  $('show-overview').setAttribute('aria-pressed','true');controls(viewMode==='list');panel.classList.remove('selection-open');
  $('overview-title').textContent=title();$('overview-back').hidden=!conversationId;$('overview-read-chat').hidden=true;
  $('overview-stats').hidden=false;$('overview-state').textContent='';$('catalog-items-more').hidden=true;
  $('catalog-items-state').textContent=viewMode==='graph'?'Loading category graph…':'Loading indexed items…';$('catalog-items-more').disabled=true;
  const query=$('catalog-search').value.trim()||null;
  try{
    if(viewMode==='graph'){
      const result=await invoke('catalog_entity_graph',{...filters(),kind:mode,conversationId,query,artifactState:mode==='file_artifact'?$('artifact-state-filter').value||null:null,limit:40});
      if(token!==generation)return;controls(false);showCatalogEntityGraph(result,title());
      if(result.truncated)$('graph-caption').textContent+=' More indexed items are omitted from this bounded graph.';
      return;
    }
    const result=await invoke('catalog_entities',{...filters(),kind:mode,conversationId,query,artifactState:mode==='file_artifact'?$('artifact-state-filter').value||null:null,cursor,limit:40});
    if(token!==generation)return;cursor=result.next_cursor;items.push(...result.items||[]);
    $('overview-stats').textContent=`${Number(result.total||0).toLocaleString()} ${title().toLocaleLowerCase()}${conversationId?' in this chat':''}`;
    $('catalog-items-state').textContent=result.coverage_truncated?'Some very large chats were only partially indexed.':'';
    if(!items.length)$('catalog-items-state').textContent='No indexed items match these filters.';
    draw();$('catalog-items-more').hidden=!cursor;
  }catch(error){if(token===generation){const message='The index is unavailable. Rebuild it in Settings, or try again once the current library task finishes.';$('catalog-items-state').textContent=message;$('overview-state').textContent=message;}}
  finally{if(token===generation){loading=false;$('catalog-items-more').disabled=false;}}
}
function draw(){
  const root=$('catalog-items-list');root.replaceChildren();
  for(const item of items){const button=element('button',undefined,'catalog-item');button.type='button';
    if(mode==='chats'){
      const info=element('span',undefined,'catalog-item-title');info.append(element('strong',item.title||'Untitled chat'));
      info.append(element('small',item.cached_archive?'Saved transcript':`${item.provider==='codex'?'Codex':'Chat'} · ${item.message_count??'Saved'}`));
      button.append(info,element('span','Open →','catalog-item-count'));
      button.addEventListener('click',()=>window.dispatchEvent(new CustomEvent('chat-hub:open-conversation',{detail:conversationOpenTarget(item)})));
      root.append(button);continue;
    }
    const info=element('span',undefined,'catalog-item-title');info.append(element('strong',item.label||item.filename||'Indexed item'));
    const origin=item.produced_reference_count?'Produced file':item.attachment_reference_count?'Attachment':'File reference';
    const secondary=item.kind==='file_artifact'
      ? `${origin} · ${item.content_saved?'Saved locally':item.artifact_state==='metadata_present'?'Content not saved':'Link only'}`
      :item.canonical_url||item.domain||'';
    info.append(element('small',secondary));button.append(info,element('span',`${item.reference_count||0} reference${item.reference_count===1?'':'s'} →`,'catalog-item-count'));
    button.addEventListener('click',()=>openItem(item.id));root.append(button);
  }
}
async function showChatList(reset=true){
  mode='chats';const token=++generation;loading=true;
  if(reset){cursor=null;items=[];searchHits=[];$('catalog-items-list').replaceChildren();}
  window.dispatchEvent(new Event('chat-hub:show-local'));document.body.classList.add('overview-open','mobile-reader');panel.hidden=false;
  $('show-overview').setAttribute('aria-pressed','true');controls(true);panel.classList.remove('selection-open');
  $('overview-title').textContent='Saved chats';$('overview-back').hidden=true;$('overview-read-chat').hidden=true;
  $('overview-stats').hidden=false;$('overview-state').textContent='';$('catalog-items-state').textContent='Loading saved chats…';$('catalog-items-more').disabled=true;
  $('overview-recent-section').hidden=true;$('graph-caption').hidden=true;
  const archiveState=$('show-archived')?.getAttribute('aria-pressed')==='true'?'archived':'active';
  const query=$('catalog-search').value.trim();
  try{
    const result=await invoke('list_conversations',{...filters(),query,cursor,archiveState});
    if(token!==generation)return;cursor=result.next_cursor;
    if(query){searchHits.push(...normalizeSearch(result).items);items=groupSearchConversations(searchHits);}
    else items.push(...result.items||[]);
    $('overview-stats').textContent=`Loaded ${items.length}${cursor?'+':''} ${items.length===1?'saved chat':'saved chats'}`;
    $('catalog-items-state').textContent=items.length?'':'No saved chats match these filters.';
    draw();$('catalog-items-more').hidden=!cursor;
  }catch{if(token===generation)$('catalog-items-state').textContent='Saved chats could not load. Try again when the current library task finishes.';}
  finally{if(token===generation){loading=false;$('catalog-items-more').disabled=false;}}
}
async function openItem(id){
  const token=++generation;$('catalog-items-state').textContent='Opening references…';
  try{const detail=await invoke('catalog_entity',{id});if(token!==generation)return;selected=id;controls(false,true);
    $('overview-back').hidden=false;$('overview-back').textContent=`← ${title()}`;showEntityGraph(detail);
  }catch{if(token===generation)$('catalog-items-state').textContent='This indexed item could not open. Try again after the current library task.';}
}
function switchMode(value){
  mode=value;selected=null;conversationId=null;++generation;$('overview-kind').value=value;
  $('catalog-search').value='';clearTimeout(searchTimer);
  if(value==='chats'){controls(false);$('artifact-state-filter').hidden=true;showOverview(false);}
  else showItems(true);
}
function setViewMode(value){
  if(!['graph','list'].includes(value))return;
  viewMode=value;setOverviewDisplayMode(value);updateDisplayButtons();
  if(mode==='chats')return;
  selected=null;showItems(true);
}
graphButton.addEventListener('click',()=>setViewMode('graph'));
listButton.addEventListener('click',()=>setViewMode('list'));
window.addEventListener('chat-hub:show-chat-list',()=>{mode='chats';selected=null;showChatList(true);});
window.addEventListener('chat-hub:show-chat-graph',()=>{mode='chats';selected=null;showOverview(false);});
window.addEventListener('chat-hub:force-overview-graph',()=>{
  viewMode='graph';updateDisplayButtons();panel.classList.remove('entity-list-open','category-graph-open');
  $('catalog-items-panel').hidden=true;$('catalog-search').hidden=true;$('overview-map').hidden=false;
  $('overview-recent-section').hidden=false;$('graph-caption').hidden=false;
});
$('overview-kind').addEventListener('change',()=>switchMode($('overview-kind').value));
$('artifact-state-filter').addEventListener('change',()=>showItems(true));
$('catalog-search').addEventListener('input',()=>{clearTimeout(searchTimer);++generation;searchTimer=setTimeout(()=>mode==='chats'?showChatList(true):showItems(true),180);});
$('catalog-items-more').addEventListener('click',()=>{if(!loading&&cursor)showItems(false);});
window.addEventListener('chat-hub:overview-back',event=>{
  if(mode==='chats')return;event.preventDefault();
  if(selected){selected=null;showItems(true);}else{conversationId=null;switchMode('chats');}
});
window.addEventListener('chat-hub:show-chat-map',()=>{mode='chats';conversationId=null;selected=null;++generation;$('overview-kind').value='chats';controls(false);$('artifact-state-filter').hidden=true;});
window.addEventListener('chat-hub:show-chat-files',event=>{mode='file_artifact';conversationId=event.detail.id;selected=null;$('catalog-search').value='';$('overview-kind').value=mode;showItems(true);});
window.addEventListener('chat-hub:inspect-entity',event=>openItem(event.detail.id));
for(const name of['chat-hub:tags-updated','chat-hub:catalog-indexed','chat-hub:artifact-saved'])window.addEventListener(name,()=>{
  if(mode!=='chats'&&!panel.hidden){if(selected)openItem(selected);else showItems(true);}
});
window.addEventListener('chat-hub:history-updated',()=>{
  clearTimeout(refreshTimer);refreshTimer=setTimeout(async()=>{
    if(panel.hidden)return;
    if(mode==='chats'){if(viewMode==='list')showChatList(true);return;}
    try{await invoke('rebuild_catalog_index');if(selected)openItem(selected);else showItems(true);}catch{$('catalog-items-state').textContent='Index update is waiting for the current library task. Try again shortly.';}
  },750);
});
for(const id of['project-filter','tag-filter'])$(id).addEventListener('change',()=>{if(mode!=='chats'&&!panel.hidden){if(selected)openItem(selected);else showItems(true);}});
for(const id of['source-chat','source-codex'])$(id).addEventListener('click',()=>{if(mode!=='chats'&&!panel.hidden){if(selected)openItem(selected);else showItems(true);}});
const archiveButton=$('show-archived');archiveButton?.addEventListener('click',()=>setTimeout(()=>{if(mode==='chats'&&viewMode==='list'&&!panel.hidden)showChatList(true);},0));
updateDisplayButtons();
