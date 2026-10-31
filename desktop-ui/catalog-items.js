import {entityKinds} from './entity-model.js';
import {showEntityGraph,showOverview} from './overview.js';
const $=id=>document.getElementById(id),panel=$('library-overview');
const invoke=(command,args={})=>window.ChatHub.invoke(command,args);
let mode='chats',conversationId=null,cursor=null,items=[],generation=0,selected=null,loading=false,searchTimer;
let refreshTimer;
const element=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
const filters=()=>({source:$('history-source').value,projectKey:$('project-filter').value||null,tag:$('tag-filter').value||null});
function controls(list){
  panel.classList.toggle('entity-list-open',list);$('catalog-items-panel').hidden=!list;$('overview-map').hidden=list;
  $('catalog-search').hidden=!list;
  $('overview-recent-section').hidden=list;$('graph-caption').hidden=list;
  $('artifact-state-filter').hidden=mode!=='file_artifact'||!list;
}
function title(){return entityKinds[mode]||'Indexed items';}
async function showItems(reset=true){
  if(mode==='chats')return;
  const token=++generation;loading=true;if(reset){cursor=null;items=[];selected=null;$('catalog-items-list').replaceChildren();}
  window.dispatchEvent(new Event('chat-hub:show-local'));document.body.classList.add('overview-open','mobile-reader');panel.hidden=false;
  $('show-overview').setAttribute('aria-pressed','true');controls(true);panel.classList.remove('selection-open');
  $('overview-title').textContent=title();$('overview-back').hidden=!conversationId;$('overview-read-chat').hidden=true;
  $('overview-stats').hidden=false;$('overview-state').textContent='';
  $('catalog-items-state').textContent='Loading indexed items…';$('catalog-items-more').disabled=true;
  try{const result=await invoke('catalog_entities',{...filters(),kind:mode,conversationId,query:$('catalog-search').value.trim()||null,artifactState:mode==='file_artifact'?$('artifact-state-filter').value||null:null,cursor,limit:40});
    if(token!==generation)return;cursor=result.next_cursor;items.push(...result.items||[]);
    $('overview-stats').textContent=`${Number(result.total||0).toLocaleString()} ${title().toLocaleLowerCase()}${conversationId?' in this chat':''}`;
    $('catalog-items-state').textContent=result.coverage_truncated?'Some very large chats were only partially indexed.':'';
    if(!items.length)$('catalog-items-state').textContent='No indexed items match these filters.';
    draw();$('catalog-items-more').hidden=!cursor;
  }catch{if(token===generation)$('catalog-items-state').textContent='The index is unavailable. Rebuild it in Settings, or try again once the current library task finishes.';}
  finally{if(token===generation){loading=false;$('catalog-items-more').disabled=false;}}
}
function draw(){
  const root=$('catalog-items-list');root.replaceChildren();
  for(const item of items){const button=element('button',undefined,'catalog-item');button.type='button';
    const info=element('span',undefined,'catalog-item-title');info.append(element('strong',item.label||item.filename||'Indexed item'));
    const origin=item.produced_reference_count?'Produced file':item.attachment_reference_count?'Attachment':'File reference';
    const secondary=item.kind==='file_artifact'
      ? `${origin} · ${item.content_saved?'Saved locally':item.artifact_state==='metadata_present'?'Content not saved':'Link only'}`
      :item.canonical_url||item.domain||'';
    info.append(element('small',secondary));button.append(info,element('span',`${item.reference_count||0} reference${item.reference_count===1?'':'s'} →`,'catalog-item-count'));
    button.addEventListener('click',()=>openItem(item.id));root.append(button);
  }
}
async function openItem(id){
  const token=++generation;$('catalog-items-state').textContent='Opening references…';
  try{const detail=await invoke('catalog_entity',{id});if(token!==generation)return;selected=id;controls(false);
    $('overview-back').hidden=false;$('overview-back').textContent=`← ${title()}`;showEntityGraph(detail);
  }catch{if(token===generation)$('catalog-items-state').textContent='This indexed item could not open. Try again after the current library task.';}
}
function switchMode(value){
  mode=value;selected=null;conversationId=null;++generation;$('overview-kind').value=value;
  $('catalog-search').value='';clearTimeout(searchTimer);
  if(value==='chats'){controls(false);$('artifact-state-filter').hidden=true;showOverview(false);}
  else showItems(true);
}
$('overview-kind').addEventListener('change',()=>switchMode($('overview-kind').value));
$('artifact-state-filter').addEventListener('change',()=>showItems(true));
$('catalog-search').addEventListener('input',()=>{clearTimeout(searchTimer);++generation;searchTimer=setTimeout(()=>showItems(true),180);});
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
    if(mode==='chats'||panel.hidden)return;
    try{await invoke('rebuild_catalog_index');if(selected)openItem(selected);else showItems(true);}catch{$('catalog-items-state').textContent='Index update is waiting for the current library task. Try again shortly.';}
  },750);
});
for(const id of['project-filter','tag-filter'])$(id).addEventListener('change',()=>{if(mode!=='chats'&&!panel.hidden)showItems(true);});
for(const id of['source-chat','source-codex'])$(id).addEventListener('click',()=>{if(mode!=='chats'&&!panel.hidden)showItems(true);});
