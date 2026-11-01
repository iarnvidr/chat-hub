const test=require('node:test'),assert=require('node:assert/strict');

class Element {
  constructor(tag,doc){this.tagName=tag.toUpperCase();this.doc=doc;this.children=[];this.attributes={};this.dataset={};this.style={};this.events={};this.hidden=false;this.disabled=false;this.className='';this.value='';}
  get classList(){return {contains:name=>this.className.split(' ').includes(name),toggle:(name,value)=>{const c=new Set(this.className.split(' ').filter(Boolean));if(value)c.add(name);else c.delete(name);this.className=[...c].join(' ');}};}
  get isConnected(){return this===this.doc.body||Boolean(this.parent?.isConnected);}
  get childElementCount(){return this.children.length;}
  append(...children){for(const child of children){child.parent=this;this.children.push(child);}}
  replaceChildren(...children){for(const child of this.children)child.parent=null;this.children=[];this.append(...children);}
  remove(){if(this.parent)this.parent.children=this.parent.children.filter(c=>c!==this);this.parent=null;}
  setAttribute(name,value){this.attributes[name]=String(value);}
  getAttribute(name){return this.attributes[name]??null;}
  addEventListener(type,handler){(this.events[type]??=[]).push(handler);}
  async fire(type,extra={}){const event={type,target:this,preventDefault(){this.prevented=true;},...extra};for(const handler of this.events[type]||[])await handler(event);return event;}
  focus(){this.doc.activeElement=this;}
  contains(target){return target===this||this.children.some(c=>c.contains(target));}
  matches(selector){if(selector==='button:not(:disabled)')return this.tagName==='BUTTON'&&!this.disabled;if(selector.startsWith('.'))return this.classList.contains(selector.slice(1));return this.tagName===selector.toUpperCase();}
  querySelectorAll(selector){const choices=selector.split(',');return this.children.flatMap(c=>[...(choices.some(s=>c.matches(s))?[c]:[]),...c.querySelectorAll(selector)]);}
  querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
  getBoundingClientRect(){return {left:200,right:260,top:80,bottom:110,height:240,width:246};}
}
function environment(){const doc={events:{},createElement(tag){return new Element(tag,this);},createElementNS(ns,tag){return this.createElement(tag);},getElementById(id){return [this.body,...this.body.querySelectorAll('*')].find(n=>n.id===id)||this.status;},addEventListener(type,handler){(this.events[type]??=[]).push(handler);}};doc.body=new Element('body',doc);doc.status=new Element('div',doc);doc.status.id='app-status';doc.body.append(doc.status);const win={innerWidth:1100,innerHeight:760,events:{},addEventListener(type,fn){this.events[type]=fn;},dispatchEvent(){},navigator:{clipboard:{writeText:async()=>{}}}};return {doc,win};}
const chat={id:'local-chat',source_id:'a1a1a1a1-a1a1-4a1a-8a1a-a1a1a1a1a1a1',provider:'chatgpt',account_scope:'chatgpt:observed:owner',title:'A chat',pinned:false,archived:false};
async function setup(invoke){const {createRowActions}=await import('../desktop-ui/row-actions.js');const {doc,win}=environment();let changed=0;const actions=createRowActions({doc,win,invoke,onChanged:()=>changed++});const primary=doc.createElement('button');primary.className='conversation-item';const row=actions.wrap(chat,primary);doc.body.append(row);return {doc,win,row,primary,actions,get changed(){return changed;},menu:doc.body.children.find(n=>n.id==='thread-actions-menu')};}

test('pin only refreshes after a verified native receipt; a rejection leaves the row unchanged',async()=>{
  let accept;const calls=[];const ui=await setup((command,args)=>{calls.push([command,args]);return new Promise(resolve=>accept=resolve);});
  const button=ui.row.querySelector('.thread-pin');const request=button.fire('click');assert.equal(ui.changed,0);assert.equal(button.disabled,true);assert.equal(button.getAttribute('aria-pressed'),'false');
  accept({verified:true,library_revision:3});await request;assert.equal(ui.changed,1);assert.deepEqual(calls,[['set_conversation_state',{conversationId:chat.id,pinned:true}]]);
  const failed=await setup(async()=>({verified:false,library_revision:4}));await failed.row.querySelector('.thread-pin').fire('click');assert.equal(failed.changed,0);assert.match(failed.doc.status.textContent,/did not confirm/);
});
test('row action clicks never open the primary conversation and Escape returns focus to its menu trigger',async()=>{
  const ui=await setup(async()=>({}));let opened=0;ui.primary.addEventListener('click',()=>opened++);
  const more=ui.row.querySelector('.thread-more');await more.fire('click');assert.equal(opened,0);assert.equal(ui.menu.hidden,false);assert.equal(more.getAttribute('aria-expanded'),'true');
  await ui.menu.fire('keydown',{key:'Escape'});assert.equal(ui.menu.hidden,true);assert.equal(ui.doc.activeElement,more);assert.equal(more.getAttribute('aria-expanded'),'false');
});
test('project submenu includes No project and sends null with verified upstream scope',async()=>{
  const calls=[];const ui=await setup(async(command,args)=>{calls.push([command,args]);if(command==='list_projects')return {projects:[{project_key:'project:["chatgpt","chatgpt:observed:owner","g-p-example"]',title:'Example'}]};return {scope:'upstream',verified:true,library_revision:4};});
  await ui.row.querySelector('.thread-more').fire('click');await ui.menu.querySelectorAll('button').find(n=>n.dataset.action==='project').fire('click');
  const choices=ui.menu.querySelectorAll('button').filter(n=>n.dataset.action==='choose-project');assert.equal(choices.length,2);await choices[0].fire('click');assert.equal(ui.changed,1);assert.deepEqual(calls.at(-1),['set_conversation_project',{conversationId:chat.id,projectKey:null,scope:'upstream'}]);
});
test('reader project picker is anchored, upstream-only, and filters other account scopes',async()=>{
  const calls=[];const ui=await setup(async(command,args)=>{calls.push([command,args]);return command==='list_projects'?{projects:[
    {project_key:'project:["chatgpt","chatgpt:observed:owner","g-p-mine"]',title:'My project'},
    {project_key:'project:["chatgpt","chatgpt:observed:other","g-p-other"]',title:'Other account'},
    {project_key:'project:["paseo-codex_local","chatgpt:observed:owner","local"]',title:'Codex'}
  ]}:{scope:'upstream',verified:true,library_revision:8};});
  const anchor=ui.doc.createElement('button');ui.doc.body.append(anchor);
  ui.win.events['chat-hub:open-project-picker']({detail:{chat,anchor,focus:false}});
  await Promise.resolve();await Promise.resolve();
  assert.equal(ui.menu.hidden,false);assert.equal(anchor.getAttribute('aria-expanded'),'true');
  const choices=ui.menu.querySelectorAll('button').filter(n=>n.dataset.action==='choose-project');
  assert.deepEqual(choices.map(n=>n.children[1]?.textContent),['No project','My project']);
  await choices[1].fire('click');
  assert.deepEqual(calls.at(-1),['set_conversation_project',{conversationId:chat.id,projectKey:'project:["chatgpt","chatgpt:observed:owner","g-p-mine"]',scope:'upstream'}]);
});
test('closing a project request before it resolves cannot reopen an old row menu',async()=>{
  let finish;const ui=await setup(()=>new Promise(resolve=>finish=resolve));await ui.row.querySelector('.thread-more').fire('click');const request=ui.menu.querySelectorAll('button').find(n=>n.dataset.action==='project').fire('click');ui.actions.close();finish({projects:[]});await request;assert.equal(ui.menu.hidden,true);assert.equal(ui.menu.children.length,0);
});
