const schemes=[
  {id:'amber',name:'Classified amber',color:'#d2a24c',background:'#0e0f12'},
  {id:'phosphor',name:'Phosphor',color:'#8bd6a4',background:'#0b1210'},
  {id:'arctic',name:'Arctic',color:'#8fbfff',background:'#0c111b'},
  {id:'violet',name:'Violet',color:'#c0a4ef',background:'#121019'},
  {id:'paper',name:'Field notes',color:'#7b4f12',background:'#f2efe7'}
];
const storageKey='chat-hub:color-scheme';
let saved='amber';try{saved=localStorage.getItem(storageKey)||saved;}catch{}
function apply(id,persist=false){
  const scheme=schemes.find(item=>item.id===id)||schemes[0];
  document.documentElement.dataset.scheme=scheme.id;
  for(const button of document.querySelectorAll('#color-schemes button'))button.setAttribute('aria-pressed',String(button.dataset.scheme===scheme.id));
  if(persist){
    try{localStorage.setItem(storageKey,scheme.id);document.getElementById('color-scheme-status').textContent=`${scheme.name} saved.`;}
    catch{document.getElementById('color-scheme-status').textContent=`${scheme.name} applied for this session.`;}
    window.dispatchEvent(new Event('chat-hub:appearance-changed'));
  }
}
const root=document.getElementById('color-schemes');
for(const scheme of schemes){
  const button=document.createElement('button');button.type='button';button.dataset.scheme=scheme.id;
  const swatch=document.createElement('span');swatch.className='scheme-swatch';swatch.setAttribute('aria-hidden','true');swatch.style.background=scheme.background;swatch.style.color=scheme.color;swatch.textContent='Aa';
  const label=document.createElement('span');label.textContent=scheme.name;button.append(swatch,label);
  button.addEventListener('click',()=>apply(scheme.id,true));root.append(button);
}
apply(saved);
window.addEventListener('storage',event=>{if(event.key===storageKey){apply(event.newValue);window.dispatchEvent(new Event('chat-hub:appearance-changed'));}});
