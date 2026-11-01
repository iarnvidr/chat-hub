export function createProjectHover({document:doc=document,window:win=window,className='project-hover'}) {
  const panel=doc.createElement('section');panel.className=className;panel.hidden=true;panel.setAttribute('role','dialog');
  panel.setAttribute('aria-label','Project actions');doc.body.append(panel);
  let anchor=null,closeTimer=0;
  function place(){
    if(!anchor||panel.hidden)return;
    const r=anchor.getBoundingClientRect(),w=Math.min(320,win.innerWidth-20);
    panel.style.width=`${w}px`;
    const rect=panel.getBoundingClientRect();
    panel.style.left=`${Math.max(10,Math.min(r.left,win.innerWidth-w-10))}px`;
    panel.style.top=`${Math.max(10,Math.min(r.bottom+6,win.innerHeight-rect.height-10))}px`;
  }
  function close(){
    clearTimeout(closeTimer);closeTimer=0;
    if(panel.hidden)return;
    panel.hidden=true;panel.replaceChildren();anchor?.setAttribute('aria-expanded','false');anchor=null;
  }
  function open(button,render){
    close();anchor=button;panel.hidden=false;panel.replaceChildren();
    button.setAttribute('aria-expanded','true');render(panel);place();
  }
  function deferClose(delay=280){clearTimeout(closeTimer);closeTimer=win.setTimeout(()=>{
    const menu=doc.getElementById('thread-actions-menu');
    if(!panel.matches(':hover')&&!menu?.matches(':hover')&&!panel.contains(doc.activeElement))close();
  },delay);}
  doc.addEventListener('pointerdown',event=>{if(!panel.hidden&&!panel.contains(event.target)&&!anchor?.contains(event.target))close();},true);
  doc.addEventListener('keydown',event=>{if(event.key==='Escape'&&!panel.hidden){event.preventDefault();close();}});
  win.addEventListener('resize',place);
  panel.addEventListener('pointerenter',()=>clearTimeout(closeTimer));
  panel.addEventListener('pointerleave',()=>deferClose());
  return {open,close,deferClose,panel};
}

export function bindProjectHoverIntent(button,{onOpen,onLeave,delay=180,grace=280,window:win=window}) {
  let openTimer=0,leaveTimer=0;
  button.addEventListener('pointerenter',event=>{
    if(event.pointerType==='touch')return;
    clearTimeout(leaveTimer);clearTimeout(openTimer);
    openTimer=win.setTimeout(()=>{openTimer=0;onOpen(event);},delay);
  });
  button.addEventListener('pointerleave',()=>{
    clearTimeout(openTimer);openTimer=0;
    leaveTimer=win.setTimeout(()=>{leaveTimer=0;onLeave();},grace);
  });
  button.addEventListener('focus',event=>{clearTimeout(openTimer);onOpen(event);});
  return ()=>{clearTimeout(openTimer);clearTimeout(leaveTimer);};
}
