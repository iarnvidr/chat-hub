const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');

class Element {
  constructor(id=''){this.id=id;this.children=[];this.handlers={};this.attributes={};this.hidden=false;this.disabled=false;this.value='';this.textContent='';this.className='';this.dataset={};this.style={};this.isConnected=true;}
  addEventListener(type,fn){(this.handlers[type]??=[]).push(fn);}
  fire(type,event={}){let result;for(const handler of this.handlers[type]||[]){result=handler({type,target:this,currentTarget:this,preventDefault(){},...event});}this.lastPromise=result;return result;}
  append(...children){for(const child of children){child.parent=this;this.children.push(child);}}
  replaceChildren(...children){this.children=[];this.append(...children);}
  setAttribute(name,value){this.attributes[name]=String(value);}
  getAttribute(name){return this.attributes[name]??null;}
  focus(){}
  select(){}
  querySelector(){return null;}
}

async function loadProjectsUi(renameResult){
  const ids=new Map(),calls=[],windowEvents={};
  const document={body:new Element('body'),getElementById(id){if(!ids.has(id))ids.set(id,new Element(id));return ids.get(id);},createElement:()=>new Element()};
  const window={ChatHub:{invoke:async(command,args)=>{calls.push({command,args});if(command==='list_projects')return {projects:[{project_key:'project:["paseo-chatgpt","paseo-cache:owner","g-p-legacy"]',display_name:'Old name',title:'Old upstream'}]};return renameResult;}},addEventListener(type,fn){(windowEvents[type]??=[]).push(fn);},dispatchEvent(event){for(const fn of windowEvents[event.type]||[])fn(event);}};
  let editorAnchor=null,editorPanel=null;
  const createProjectHover=()=>({panel:new Element('hover-panel'),open(anchor,render){editorAnchor=anchor;this.panel.hidden=false;editorPanel=this.panel;render(this.panel);},close(){this.panel.hidden=true;this.panel.children=[];},deferClose(){}});
  const context=vm.createContext({window,document,Event,CustomEvent,requestAnimationFrame:fn=>fn(),createProjectHover,bindProjectHoverIntent:()=>()=>{},console,Promise,Array,JSON,Number,String});
  const source=fs.readFileSync(require.resolve('../desktop-ui/projects.js'),'utf8').replace(/^import\s+.*?;\s*$/gm,'');
  vm.runInContext(source,context,{filename:'projects.js'});
  const filter=document.getElementById('project-filter');filter.value='project:["paseo-chatgpt","paseo-cache:owner","g-p-legacy"]';
  const anchor=document.getElementById('edit-project-name');
  const event={type:'click',target:anchor,currentTarget:anchor,isTrusted:false,preventDefault(){}};
  const promise=anchor.handlers.click[0](event);
  // Browsers clear Event.currentTarget as soon as dispatch returns, before awaited metadata resolves.
  event.currentTarget=null;
  await promise;
  return {window,document,calls,anchor,panel:editorPanel,editorAnchor};
}

test('project rename retains its clicked anchor across async metadata and saves upstream only',async()=>{
  const ui=await loadProjectsUi({scope:'upstream',verified:true,library_revision:12});
  assert.equal(ui.editorAnchor,ui.anchor);
  const form=ui.panel.children[0];
  const nameInput=form.children[2];assert.equal(nameInput.value,'Old name');nameInput.value='New name';
  assert.doesNotMatch(ui.panel.children.map(child=>child.textContent).join(' '),/local only|Apply to/i);
  const submit=form.handlers.submit[0];await submit({preventDefault(){}});
  assert.deepEqual(ui.calls.at(-1),{command:'rename_project',args:{projectKey:'project:["paseo-chatgpt","paseo-cache:owner","g-p-legacy"]',name:'New name',scope:'upstream'}});
  assert.equal(ui.document.getElementById('app-status').textContent,'Project renamed in ChatGPT.');
});

test('project rename does not report success without the verified upstream receipt',async()=>{
  const ui=await loadProjectsUi({scope:'local',verified:false,library_revision:12});
  const form=ui.panel.children[0],nameInput=form.children[2];nameInput.value='New name';
  await form.handlers.submit[0]({preventDefault(){}});
  assert.match(form.children[3].textContent,/did not confirm/);
  assert.notEqual(ui.document.getElementById('app-status').textContent,'Project renamed in ChatGPT.');
});
