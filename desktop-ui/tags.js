const $=id=>document.getElementById(id),dialog=$('tags-editor');
const invoke=(command,args={})=>window.ChatHub.invoke(command,args);
const native=typeof window.__TAURI_INTERNALS__?.invoke==='function';
let target,selected=[],currentChat,pending=false,tagLoad=0,chatLoad=0,returnFocus;
const node=(tag,text)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;return n;};
function chips(root,tags,remove){
  root.replaceChildren();for(const tag of tags){const chip=node('span',tag);chip.className='tag-chip';
    if(remove){const button=node('button','×');button.type='button';button.setAttribute('aria-label',`Remove ${tag}`);button.onclick=()=>remove(tag);chip.append(button);}
    root.append(chip);
  }
}
async function loadTags(){
  if(!native)return;const token=++tagLoad;
  try{const data=await invoke('list_local_tags');if(token!==tagLoad)return;
    const value=$('tag-filter').value;const select=$('tag-filter'),suggestions=$('tag-suggestions');
    select.replaceChildren(new Option('All tags',''));suggestions.replaceChildren();
    for(const item of data.items||[]){select.append(new Option(`${item.name} · ${item.target_count}`,item.name));const option=node('option');option.value=item.name;suggestions.append(option);}
    if([...select.options].some(option=>option.value===value))select.value=value;
  }catch{/* Existing tags and chat reading remain available while the library is busy. */}
}
async function loadChatTags(){
  const chat=currentChat,token=++chatLoad;if(!chat||!native)return;
  try{const result=await invoke('get_target_tags',{targetKind:'conversation',targetId:chat.id});
    if(token===chatLoad&&currentChat?.id===chat.id)chips($('reader-tags'),result.tags||[]);
  }catch{if(token===chatLoad)$('reader-tags').replaceChildren();}
}
function drawSelected(){chips($('tags-editor-chips'),selected,tag=>{selected=selected.filter(v=>v!==tag);drawSelected();});}
function addTag(){
  const value=$('tag-name-input').value.trim().replace(/\s+/g,' ');if(!value)return;
  if(selected.length>=64){$('tags-editor-status').textContent='An item can have up to 64 tags.';return;}
  if(!selected.some(v=>v.toLocaleLowerCase()===value.toLocaleLowerCase()))selected.push(value);
  $('tag-name-input').value='';$('tags-editor-status').textContent='';drawSelected();
}
async function edit(detail){
  if(!native||!detail?.targetId||pending)return;
  target=detail;selected=[];returnFocus=document.activeElement;
  // The Settings page suspends the native provider surface behind this dialog.
  window.dispatchEvent(new Event('chat-hub:open-settings'));
  $('tags-editor-target').textContent=detail.title||'Saved item';$('tags-editor-status').textContent='Loading tags…';
  $('tag-name-input').value='';drawSelected();$('tags-editor-save').disabled=true;
  if(!dialog.open)dialog.showModal();
  try{const result=await invoke('get_target_tags',{targetKind:detail.targetKind,targetId:detail.targetId});
    if(target!==detail||!dialog.open)return;selected=result.tags||[];drawSelected();$('tags-editor-status').textContent='';$('tags-editor-save').disabled=false;$('tag-name-input').focus();
  }catch{$('tags-editor-status').textContent='Tags could not load. Close this dialog and try again.';}
}
function close(){if(pending)return;target=null;dialog.close();window.dispatchEvent(new Event('chat-hub:close-settings'));returnFocus?.focus?.();}
$('tags-editor-form').addEventListener('submit',async event=>{
  event.preventDefault();if(!target||pending)return;addTag();pending=true;$('tags-editor-save').disabled=true;$('tags-editor-cancel').disabled=true;
  try{await invoke('set_target_tags',{targetKind:target.targetKind,targetId:target.targetId,tags:selected});
    window.dispatchEvent(new Event('chat-hub:tags-updated'));pending=false;close();
  }catch{$('tags-editor-status').textContent='Tags could not be saved. Your previous tags are unchanged.';}
  finally{pending=false;$('tags-editor-save').disabled=false;$('tags-editor-cancel').disabled=false;}
});
$('add-tag').addEventListener('click',addTag);
$('tag-name-input').addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();addTag();}});
$('tags-editor-cancel').addEventListener('click',close);dialog.addEventListener('cancel',event=>{event.preventDefault();close();});
$('edit-chat-tags').addEventListener('click',()=>{if(currentChat)edit({targetKind:'conversation',targetId:currentChat.id,title:currentChat.title});});
$('show-chat-files').addEventListener('click',()=>{if(currentChat)window.dispatchEvent(new CustomEvent('chat-hub:show-chat-files',{detail:currentChat}));});
window.addEventListener('chat-hub:edit-tags',event=>edit(event.detail));
window.addEventListener('chat-hub:conversation-loading',()=>{currentChat=null;++chatLoad;$('reader-tags').replaceChildren();$('edit-chat-tags').disabled=$('show-chat-files').disabled=true;});
window.addEventListener('chat-hub:conversation-loaded',event=>{currentChat=event.detail;$('edit-chat-tags').disabled=$('show-chat-files').disabled=false;loadChatTags();});
window.addEventListener('chat-hub:tags-updated',()=>{loadTags();loadChatTags();});
window.addEventListener('chat-hub:catalog-indexed',loadTags);
if(native)loadTags();
