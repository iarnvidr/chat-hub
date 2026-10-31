const filter=document.getElementById('project-filter');
const options=document.getElementById('project-options');
const disclosure=document.getElementById('project-disclosure');
function render(){
  const selected=filter.selectedOptions[0];
  document.getElementById('project-selection').textContent=selected?.textContent||'All projects';
  options.replaceChildren();
  for(const option of filter.options){
    const button=document.createElement('button');button.type='button';button.textContent=option.textContent;
    button.dataset.projectKey=option.value;button.setAttribute('aria-pressed',String(option.value===filter.value));
    button.addEventListener('click',()=>{
      if(filter.value===option.value)return;
      filter.value=option.value;filter.dispatchEvent(new Event('change',{bubbles:true}));
    });
    options.append(button);
  }
}
filter.addEventListener('change',render);
new MutationObserver(render).observe(filter,{childList:true,subtree:true,characterData:true});
for(const id of ['source-chat','source-codex'])document.getElementById(id).addEventListener('click',render);
disclosure.addEventListener('keydown',event=>{
  if(event.key==='Escape'){disclosure.open=false;disclosure.querySelector('summary').focus();}
});
render();
