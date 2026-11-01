import { createTagHoverEditor } from './tag-hover.js';
const $=id=>document.getElementById(id);
const invoke=(command,args={})=>window.ChatHub.invoke(command,args);
const native=typeof window.__TAURI_INTERNALS__?.invoke==='function';
let currentChat,tagLoad=0,chatLoad=0;
const node=(tag,text)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;return n;};
const editor=createTagHoverEditor({invoke,onSaved:()=>window.dispatchEvent(new Event('chat-hub:tags-updated'))});
function chips(root,tags){root.replaceChildren();for(const tag of tags){const chip=node('span',tag);chip.className='tag-chip';root.append(chip);}}
async function loadTags(){
  if(!native)return;const token=++tagLoad;
  try{const data=await invoke('list_local_tags');if(token!==tagLoad)return;
    const select=$('tag-filter'),value=select.value;select.replaceChildren(new Option('All tags',''));
    for(const item of data.items||[])select.append(new Option(`${item.name} · ${item.target_count}`,item.name));
    if([...select.options].some(option=>option.value===value))select.value=value;
  }catch{/* Keep the current filter while the library is busy. */}
}
async function loadChatTags(){
  const chat=currentChat,token=++chatLoad;if(!chat||!native)return;
  try{const result=await invoke('get_target_tags',{targetKind:'conversation',targetId:chat.id});
    if(token===chatLoad&&currentChat?.id===chat.id)chips($('reader-tags'),result.tags||[]);
  }catch{if(token===chatLoad)$('reader-tags').replaceChildren();}
}
editor.bind($('edit-chat-tags'),()=>currentChat&&({targetKind:'conversation',targetId:currentChat.id,title:currentChat.title}));
const bound=new WeakMap();
function edit(event){
  const detail=event.detail;if(!native||!detail?.targetId)return;const anchor=detail.anchor||document.activeElement;
  if(anchor){
    if(event.type==='chat-hub:hover-tags'){
      if(!bound.has(anchor))editor.bind(anchor,()=>bound.get(anchor));
      bound.set(anchor,detail);
    }else if(!bound.has(anchor)){
      anchor.addEventListener('pointerleave',editor.leave);bound.set(anchor,null);
    }
  }
  editor.show(detail,anchor);
}
window.addEventListener('chat-hub:edit-tags',edit);
window.addEventListener('chat-hub:hover-tags',edit);
$('show-chat-files').addEventListener('click',()=>{if(currentChat)window.dispatchEvent(new CustomEvent('chat-hub:show-chat-files',{detail:currentChat}));});
window.addEventListener('chat-hub:conversation-loading',()=>{editor.close();currentChat=null;++chatLoad;$('reader-tags').replaceChildren();$('edit-chat-tags').disabled=$('show-chat-files').disabled=true;});
window.addEventListener('chat-hub:conversation-loaded',event=>{currentChat=event.detail;$('edit-chat-tags').disabled=$('show-chat-files').disabled=false;loadChatTags();});
window.addEventListener('chat-hub:tags-updated',()=>{loadTags();loadChatTags();});
window.addEventListener('chat-hub:catalog-indexed',loadTags);
if(native)loadTags();
