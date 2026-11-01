export function createTagHoverEditor({invoke,onSaved=()=>{},doc=document,win=window}) {
  const node=(tag,text,cls)=>{const n=doc.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
  const panel=node('section',undefined,'tag-hover-panel');panel.hidden=true;panel.setAttribute('role','dialog');panel.setAttribute('aria-label','Chat tags');doc.body.append(panel);
  let current=null,anchor=null,tags=[],suggestions=[],generation=0,openTimer,closeTimer,pending=false,loading=false,custom=false;
  const key=s=>String(s).trim().toLocaleLowerCase();
  function position(){if(!anchor||panel.hidden)return;const rect=anchor.getBoundingClientRect();const sidebar=doc.querySelector('.list-pane')?.getBoundingClientRect();const inSidebar=sidebar&&rect.right<=sidebar.right+2;const width=Math.min(inSidebar?sidebar.width-20:320,win.innerWidth-16);panel.style.width=`${width}px`;panel.style.left=`${Math.max(8,Math.min(rect.right-width,win.innerWidth-width-8))}px`;panel.style.maxHeight=`${win.innerHeight-20}px`;const height=panel.getBoundingClientRect().height;panel.style.top=`${rect.bottom+height+7<win.innerHeight?rect.bottom+7:Math.max(10,rect.top-height-7)}px`;}
  function close(){clearTimeout(openTimer);clearTimeout(closeTimer);++generation;panel.hidden=true;current=null;anchor=null;}
  function leave(){clearTimeout(openTimer);clearTimeout(closeTimer);closeTimer=setTimeout(close,180);}
  function keep(){clearTimeout(closeTimer);}
  function message(text){const el=panel.querySelector('.tag-hover-status');if(el)el.textContent=text;position();}
  async function toggle(name){
    if(!current||pending||loading)return;const target=current,token=generation;const remove=tags.some(t=>key(t)===key(name));pending=true;
    for(const button of panel.querySelectorAll('button,input'))button.disabled=true;message('Saving…');
    try{
      const latest=await invoke('get_target_tags',{targetKind:target.targetKind,targetId:target.targetId});
      const next=(latest.tags||[]).filter(t=>key(t)!==key(name));if(!remove)next.push(name);
      await invoke('set_target_tags',{targetKind:target.targetKind,targetId:target.targetId,tags:next});
      if(token===generation&&current===target){tags=next;custom=false;draw();[...panel.querySelectorAll('button')].find(button=>button.textContent===name)?.focus();}onSaved();
    }catch{if(token===generation)message('Tags could not be saved. Try again.');}
    finally{pending=false;for(const button of panel.querySelectorAll('button,input'))button.disabled=loading;}
  }
  function chip(name,selected,reason){const button=node('button',name,'tag-choice'+(selected?' is-selected':''));button.type='button';button.setAttribute('aria-pressed',String(selected));button.setAttribute('aria-label',`${selected?'Remove':'Add'} tag ${name}`);if(reason){button.setAttribute('aria-description',reason);button.addEventListener('pointerenter',()=>message(reason));button.addEventListener('focus',()=>message(reason));}button.addEventListener('click',()=>toggle(name));return button;}
  function draw(){
    if(!current)return;panel.replaceChildren();const head=node('header');head.append(node('strong','Tags'),node('span','Suggestions from saved content'));panel.append(head);
    const selected=node('div',undefined,'tag-choice-group');selected.setAttribute('aria-label','Applied tags');
    for(const tag of tags)selected.append(chip(tag,true));if(tags.length)panel.append(selected);
    const choices=node('div',undefined,'tag-choice-group');choices.setAttribute('aria-label','Suggested tags');const seen=new Set(tags.map(key));
    for(const value of suggestions){
      const name=value?.name;if(!name||seen.has(key(name)))continue;seen.add(key(name));choices.append(chip(name,false,value.reason));if(choices.childElementCount>=12)break;
    }
    panel.append(choices);
    if(!choices.childElementCount&&!tags.length)panel.append(node('p','No suggestions yet. Add your own tag below.','tag-hover-empty'));
    const create=node('button','＋ Create a custom tag','tag-choice tag-choice-custom');create.type='button';create.setAttribute('aria-expanded',String(custom));create.addEventListener('click',()=>{custom=!custom;draw();if(custom)panel.querySelector('input')?.focus();});panel.append(create);
    if(custom){const form=node('form',undefined,'tag-custom-form'),label=node('label','New tag'),input=node('input'),add=node('button','Create tag');input.id='tag-hover-custom-name';input.maxLength=80;input.autocomplete='off';label.setAttribute('for',input.id);add.type='submit';form.append(label,input,add);form.addEventListener('submit',event=>{event.preventDefault();const value=input.value.trim().replace(/\s+/g,' ');if(value)toggle(value);});panel.append(form);}
    const state=node('p',undefined,'tag-hover-status');state.setAttribute('role','status');state.setAttribute('aria-live','polite');panel.append(state);for(const button of panel.querySelectorAll('button,input'))button.disabled=pending||loading;position();
  }
  async function read(command,args){
    for(let attempt=0;;attempt++){
      try{return await invoke(command,args);}catch(error){
        if(attempt>=2||!/busy|service_unavailable/.test(String(error?.message||error)))throw error;
        await new Promise(resolve=>setTimeout(resolve,120*(attempt+1)));
      }
    }
  }
  async function show(detail,element,retry=false){
    if(!detail?.targetId||!element)return;
    clearTimeout(openTimer);keep();
    if(!retry&&current?.targetId===detail.targetId&&current?.targetKind===detail.targetKind&&!panel.hidden){anchor=element;position();return;}
    current=detail;anchor=element;const token=++generation;tags=[];suggestions=[];custom=false;loading=true;panel.hidden=false;draw();message('Finding suggestions…');
    try{
      const contextIds=detail.targetKind==='conversation'?[detail.targetId]:[...new Set(detail.suggestionConversationIds||[])].filter(id=>typeof id==='string'&&id).slice(0,2);
      const results=await Promise.allSettled([
        read('get_target_tags',{targetKind:detail.targetKind,targetId:detail.targetId}),
        ...contextIds.map(conversationId=>read('catalog_tag_suggestions',{conversationId,limit:contextIds.length>1?6:12})),
      ]);
      if(token!==generation||current!==detail)return;
      if(results[0].status!=='fulfilled')throw Error('tags_unavailable');
      tags=results[0].value.tags||[];suggestions=results.slice(1).flatMap(result=>result.status==='fulfilled'?result.value.items||[]:[]);loading=false;draw();
      if(results.slice(1).some(result=>result.status!=='fulfilled'))retryLoad('Some suggestions could not load.');
    }catch{if(token===generation){loading=false;draw();retryLoad('Tags could not load.');}}
    function retryLoad(text){message(text);const button=node('button','Try again','tag-retry');button.type='button';button.addEventListener('click',()=>show(detail,element,true));panel.append(button);position();}
  }
  function bind(element,getDetail){
    element.addEventListener('pointerenter',()=>{keep();clearTimeout(openTimer);openTimer=setTimeout(()=>show(getDetail(),element),200);});
    element.addEventListener('pointerleave',leave);
    element.addEventListener('focus',()=>show(getDetail(),element));
    element.addEventListener('click',()=>show(getDetail(),element));
    element.setAttribute('aria-haspopup','dialog');
  }
  panel.addEventListener('pointerenter',keep);panel.addEventListener('pointerleave',leave);panel.addEventListener('focusin',keep);
  panel.addEventListener('focusout',()=>{setTimeout(()=>{if(!panel.contains(doc.activeElement)&&doc.activeElement!==anchor)leave();},0);});
  panel.addEventListener('keydown',event=>{if(event.key==='Escape'){event.preventDefault();anchor?.focus?.();close();}});
  doc.addEventListener('pointerdown',event=>{if(!panel.hidden&&!panel.contains(event.target)&&!anchor?.contains(event.target))close();},true);
  win.addEventListener('resize',close);
  return {show,bind,close,leave,keep};
}
