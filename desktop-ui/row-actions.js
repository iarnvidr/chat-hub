import {chatgptConversationId} from './continuation.js';

export function actionError(error) {
  const code=String(error?.code||error?.message||error);
  if(/rate_limit|cooldown/.test(code))return 'ChatGPT is limiting requests. Try again after the current cooldown.';
  if(/signed_out|login_required|not_open|not_ready|loading/.test(code))return 'Connect ChatGPT in Settings, then try again.';
  if(/account|scope_mismatch/.test(code))return 'The connected ChatGPT account does not match this chat.';
  if(/pin_limit/.test(code))return 'ChatGPT’s pin limit has been reached. Unpin another chat first.';
  if(/verification|unconfirmed/.test(code))return 'ChatGPT did not confirm the change. Try again to check its current state.';
  if(/unavailable|unsupported|shape/.test(code))return 'ChatGPT could not complete this action. Try again after reconnecting.';
  if(/busy/.test(code))return 'Another library action is finishing. Try again shortly.';
  return 'This change could not be confirmed with ChatGPT. Try again.';
}

const observedChatgptScope = value => typeof value === 'string' && /^chatgpt:observed:[0-9a-f]{64}$/.test(value);

function chatgptProjectIdentity(projectKey) {
  if (typeof projectKey !== 'string' || !projectKey.startsWith('project:')) return null;
  try {
    const parts = JSON.parse(projectKey.slice('project:'.length));
    if (!Array.isArray(parts) || parts.length !== 3) return null;
    const [provider, accountScope, projectId] = parts;
    if (!['chatgpt', 'paseo-chatgpt'].includes(provider) ||
        typeof accountScope !== 'string' ||
        !(observedChatgptScope(accountScope) || /^paseo-cache:[A-Za-z0-9:_-]{1,256}$/.test(accountScope)) ||
        typeof projectId !== 'string' || !/^g-p-[A-Za-z0-9_-]{1,128}$/.test(projectId)) return null;
    return {provider, accountScope, projectId};
  } catch {
    return null;
  }
}

function projectPickerScope(chat) {
  if (observedChatgptScope(chat?.account_scope)) return {observedScope:chat.account_scope,legacyScope:null};
  if (!['chatgpt', 'paseo-chatgpt'].includes(chat?.provider) ||
      typeof chat.account_scope !== 'string' || !/^paseo-cache:[A-Za-z0-9:_-]{1,256}$/.test(chat.account_scope)) return null;
  const currentProject = chatgptProjectIdentity(chat.project?.project_key);
  if (currentProject?.provider === 'chatgpt' && observedChatgptScope(currentProject.accountScope) &&
      (typeof chat.project.project_id !== 'string' || chat.project.project_id === currentProject.projectId) &&
      (typeof chat.project.account_scope !== 'string' || chat.project.account_scope === currentProject.accountScope)) {
    return {observedScope:currentProject.accountScope,legacyScope:chat.account_scope};
  }
  return {observedScope:null,legacyScope:chat.account_scope};
}

export function createRowActions({invoke,onChanged=()=>{},doc=document,win=window}) {
  const paths={pin:'M12 17v5M8 3h8v6l3 4v3H5v-3l3-4V3Z',more:'M5 12h.01M12 12h.01M19 12h.01',folder:'M3 7V4h6l3 3h9v13H3V7Z',archive:'M3 4h18v4H3zM5 8v12h14V8M10 12h4',tag:'M20 13l-7 7L3 10V3h7l10 10ZM7 7h.01',link:'M10 13a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-2 2M14 11a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l2-2',back:'m14 5-7 7 7 7',check:'m5 12 4 4L19 6'};
  const node=(tag,text,cls)=>{const n=doc.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
  function icon(kind){const svg=doc.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 24 24');svg.setAttribute('aria-hidden','true');const path=doc.createElementNS('http://www.w3.org/2000/svg','path');path.setAttribute('d',paths[kind]||paths.more);svg.append(path);return svg;}
  const menu=node('div',undefined,'thread-menu');menu.id='thread-actions-menu';menu.hidden=true;menu.setAttribute('role','menu');doc.body.append(menu);
  const pending=new Set();let target=null,trigger=null,view='actions',generation=0,busy=false,hoverPicker=false,hoverCloseTimer=0,hoverOpenTimer=0;
  function close(restoreFocus=true){
    clearTimeout(hoverOpenTimer);
    if(menu.hidden)return;
    clearTimeout(hoverCloseTimer);hoverPicker=false;
    const prior=trigger;++generation;menu.hidden=true;menu.replaceChildren();target=null;trigger=null;view='actions';
    prior?.setAttribute('aria-expanded','false');
    if(restoreFocus&&prior?.isConnected)prior.focus();
  }
  function position(){
    if(!trigger||menu.hidden)return;
    const r=trigger.getBoundingClientRect(),width=Math.min(246,win.innerWidth-16);
    menu.style.width=`${width}px`;menu.style.maxHeight=`${win.innerHeight-16}px`;
    menu.style.left=`${Math.max(8,Math.min(r.right-width,win.innerWidth-width-8))}px`;
    const height=menu.getBoundingClientRect().height;
    menu.style.top=`${r.bottom+height+6<=win.innerHeight-8?r.bottom+6:Math.max(8,r.top-height-6)}px`;
  }
  function status(text){let el=menu.querySelector('.thread-menu-status');if(!el){el=node('p',undefined,'thread-menu-status');el.setAttribute('role','status');el.setAttribute('aria-live','polite');menu.append(el);}el.textContent=text;position();}
  function item(label,kind,action,options={}){
    const button=node('button',undefined,'thread-menu-item');button.type='button';button.setAttribute('role','menuitem');button.dataset.action=options.action||kind;
    button.append(icon(kind),node('span',label));button.disabled=busy||Boolean(options.disabled);
    if(options.checked)button.append(icon('check'));button.addEventListener('click',action);return button;
  }
  function heading(title){const header=node('div',undefined,'thread-menu-heading');header.append(node('strong',title),node('span',target?.title||'Untitled conversation'));menu.append(header);}
  function checkReceipt(result){if(result?.verified!==true||!Number.isSafeInteger(result.library_revision))throw Error('update_unconfirmed');}
  async function mutate(chat,changes,row=null){
    if(busy||pending.has(chat.id))return;pending.add(chat.id);busy=true;
    if(row){row.dataset.busy='true';for(const b of row.querySelectorAll('.thread-action'))b.disabled=true;}
    if(target?.id===chat.id){for(const b of menu.querySelectorAll('button'))b.disabled=true;status('Updating ChatGPT…');}
    try{
      const result=await invoke('set_conversation_state',{conversationId:chat.id,...changes});checkReceipt(result);
      if(target?.id===chat.id)close(false);
      onChanged();
      doc.getElementById('app-status').textContent=changes.archived===true?'Archived in ChatGPT.':changes.archived===false?'Restored from archive.':changes.pinned?'Pinned in ChatGPT.':'Unpinned in ChatGPT.';
    }catch(error){
      const text=actionError(error);if(target?.id===chat.id)status(text);else doc.getElementById('app-status').textContent=text;
    }finally{
      pending.delete(chat.id);busy=false;
      if(row){delete row.dataset.busy;for(const b of row.querySelectorAll('.thread-action'))b.disabled=false;}
      if(target)for(const b of menu.querySelectorAll('button,input'))b.disabled=false;
    }
  }
  function actions(){
    if(!target)return;view='actions';menu.replaceChildren();menu.setAttribute('aria-label','Chat actions');heading('Chat actions');const chat=target;
    if(chatgptConversationId(chat)){
      menu.append(item('Move to project','folder',()=>projects(),{action:'project'}),
        item(chat.pinned?'Unpin':'Pin','pin',()=>mutate(chat,{pinned:!chat.pinned}),{action:'pin'}),
        item(chat.archived?'Restore from archive':'Archive','archive',()=>mutate(chat,{archived:!chat.archived}),{action:'archive'}));
      menu.append(node('div',undefined,'thread-menu-separator'));
    }
    menu.append(item('Tags','tag',()=>{const anchor=trigger;close(false);win.dispatchEvent(new CustomEvent('chat-hub:edit-tags',{detail:{targetKind:'conversation',targetId:chat.id,title:chat.title,anchor}}));}));
    const id=chatgptConversationId(chat);
    if(id)menu.append(item('Copy ChatGPT link','link',async()=>{try{await win.navigator.clipboard.writeText(`https://chatgpt.com/c/${id}`);close();doc.getElementById('app-status').textContent='ChatGPT link copied.';}catch{status('The link could not be copied.');}},{action:'copy-link'}));
    position();
  }
  async function projects(){
    if(!target||busy)return;view='projects';const chat=target,token=++generation;menu.replaceChildren();menu.setAttribute('aria-label','Move chat to project');
    const back=item('Chat actions','back',()=>{++generation;actions();menu.querySelector('button')?.focus();},{action:'back'});menu.append(back);heading('Move to project');status('Loading projects…');
    try{
      const result=await invoke('list_projects',{source:'chatgpt'});if(token!==generation||target!==chat||view!=='projects')return;
      if(!Array.isArray(result?.projects))throw Error('unavailable');
      menu.querySelector('.thread-menu-status')?.remove();
      const search=node('input');search.type='search';search.placeholder='Find a project…';search.setAttribute('aria-label','Find a project');search.className='thread-project-search';menu.append(search);
      const list=node('div',undefined,'thread-project-options');menu.append(list);
        const pickerScope=projectPickerScope(chat);
        const choices=[{project_key:null,display_name:'No project'},...result.projects.filter(project=>{
          const identity=chatgptProjectIdentity(project?.project_key);
          if(!identity||!pickerScope)return false;
          const observedChoice=pickerScope.observedScope===identity.accountScope&&
            (identity.provider==='chatgpt'||(identity.provider==='paseo-chatgpt'&&chat.provider==='chatgpt'));
          const legacyChoice=pickerScope.legacyScope===identity.accountScope&&identity.provider==='paseo-chatgpt';
          if(!observedChoice&&!legacyChoice)return false;
          return typeof project.account_scope!=='string'||project.account_scope===identity.accountScope;
        })];
      function draw(){
        list.replaceChildren();const query=search.value.trim().toLocaleLowerCase();
        for(const project of choices){const label=project.display_name||project.title||'Project name pending';if(query&&!label.toLocaleLowerCase().includes(query))continue;
          list.append(item(label,'folder',()=>move(chat,project.project_key),{action:'choose-project',checked:(chat.project?.project_key||null)===project.project_key}));
        }
        if(!list.childElementCount)list.append(node('p','No projects match.','thread-menu-status'));position();
      }
       search.addEventListener('input',draw);draw();if(!hoverPicker)search.focus();
    }catch(error){if(token===generation&&target===chat)status(actionError(error));}
  }
  async function move(chat,projectKey){
    if(busy||target!==chat)return;busy=true;for(const b of menu.querySelectorAll('button,input'))b.disabled=true;status('Updating ChatGPT…');
    try{const result=await invoke('set_conversation_project',{conversationId:chat.id,projectKey,scope:'upstream'});checkReceipt(result);if(result.scope!=='upstream')throw Error('update_unconfirmed');if(target===chat)close(false);onChanged();doc.getElementById('app-status').textContent=projectKey?'Moved to project in ChatGPT.':'Removed from project in ChatGPT.';}
    catch(error){if(target===chat)status(actionError(error));else doc.getElementById('app-status').textContent=actionError(error);}
    finally{busy=false;if(target)for(const b of menu.querySelectorAll('button,input'))b.disabled=false;}
  }
  function open(chat,button,hover=false){
    if(target?.id===chat.id&&trigger===button&&!menu.hidden){if(!hover)close();return;}
    close(false);target=chat;trigger=button;hoverPicker=hover;menu.hidden=false;button.setAttribute('aria-expanded','true');actions();if(!hover)menu.querySelector('button:not(:disabled)')?.focus();
  }
  win.addEventListener('chat-hub:open-project-picker',event=>{
    const chat=event.detail?.chat,button=event.detail?.anchor;
    if(!chatgptConversationId(chat)||!button)return;
    close(false);target=chat;trigger=button;hoverPicker=event.detail?.focus===false;
    menu.hidden=false;button.setAttribute('aria-expanded','true');projects();
  });
  win.addEventListener('chat-hub:close-project-picker',()=>{
    if(view==='projects'&&hoverPicker)close(false);
  });
  menu.addEventListener('pointerenter',()=>{clearTimeout(hoverCloseTimer);});
  menu.addEventListener('pointerleave',()=>{
    if(!hoverPicker)return;
    clearTimeout(hoverCloseTimer);hoverCloseTimer=win.setTimeout(()=>{
      if(!menu.matches(':hover')&&!trigger?.matches(':hover')&&!menu.contains(doc.activeElement))close(false);
    },280);
  });
  function wrap(chat,primary){
    const row=node('div',undefined,'conversation-row');row.dataset.id=chat.id;row.dataset.pinned=String(Boolean(chat.pinned));row.classList.toggle('selected',primary.classList.contains('selected'));
    const rail=node('div',undefined,'thread-action-rail');rail.setAttribute('role','group');rail.setAttribute('aria-label','Chat actions');
    const more=node('button',undefined,'thread-action thread-more');more.type='button';more.append(icon('more'));more.setAttribute('aria-label',`Actions for ${chat.title||'conversation'}`);more.setAttribute('aria-haspopup','menu');more.setAttribute('aria-expanded','false');more.setAttribute('aria-controls',menu.id);more.addEventListener('click',()=>open(chat,more));rail.append(more);
    rail.addEventListener('pointerenter',event=>{if(event.pointerType==='touch')return;clearTimeout(hoverCloseTimer);clearTimeout(hoverOpenTimer);hoverOpenTimer=setTimeout(()=>open(chat,more,true),180);});
    rail.addEventListener('pointerleave',()=>{clearTimeout(hoverOpenTimer);if(target?.id!==chat.id||!hoverPicker)return;clearTimeout(hoverCloseTimer);hoverCloseTimer=setTimeout(()=>{if(!menu.matches(':hover')&&!menu.contains(doc.activeElement))close(false);},220);});
    more.addEventListener('keydown',event=>{if(event.key==='ArrowDown'){event.preventDefault();open(chat,more);}});
    row.append(primary,rail);
    row.addEventListener('contextmenu',event=>{event.preventDefault();open(chat,more);});
    primary.addEventListener('keydown',event=>{if(event.key==='ContextMenu'||(event.shiftKey&&event.key==='F10')){event.preventDefault();open(chat,more);}});
    return row;
  }
  menu.addEventListener('keydown',event=>{
    if(event.key==='Escape'){event.preventDefault();close();return;}
    if(event.key==='ArrowLeft'&&view==='projects'){event.preventDefault();++generation;actions();menu.querySelector('button')?.focus();return;}
    if(event.key==='Tab'){const prior=trigger;close(false);prior?.focus();return;}
    if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;
    if(event.target.tagName==='INPUT'&&!['ArrowDown','ArrowUp'].includes(event.key))return;
    const buttons=[...menu.querySelectorAll('button:not(:disabled)')];if(!buttons.length)return;event.preventDefault();let index=buttons.indexOf(doc.activeElement);
    index=event.key==='Home'?0:event.key==='End'?buttons.length-1:(index+(event.key==='ArrowUp'?-1:1)+buttons.length)%buttons.length;buttons[index].focus();
  });
  doc.addEventListener('pointerdown',event=>{if(!menu.hidden&&!menu.contains(event.target)&&!trigger?.contains(event.target))close(false);},true);
  doc.addEventListener('scroll',event=>{if(!menu.hidden&&!menu.contains(event.target))close(false);},true);
  win.addEventListener('resize',()=>close(false));
  return {wrap,close};
}
