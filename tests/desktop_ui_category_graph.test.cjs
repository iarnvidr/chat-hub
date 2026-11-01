const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');

const idRegistry=new Map();
class FakeElement {
  constructor(id=''){
    this.id=id;this.children=[];this.parentNode=null;this.attributes={};this.handlers=new Map();this.hidden=false;this.value='';this.textContent='';this.style={};this.dataset={};this.selectedOptions=[{textContent:'All projects'}];
    const owner=this;this.classList={
      add(...names){const set=new Set((owner.className||'').split(/\s+/).filter(Boolean));for(const name of names)set.add(name);owner.className=[...set].join(' ');},
      remove(...names){const set=new Set((owner.className||'').split(/\s+/).filter(Boolean));for(const name of names)set.delete(name);owner.className=[...set].join(' ');},
      contains(name){return(owner.className||'').split(/\s+/).includes(name);},
      toggle(name,force){const has=this.contains(name),on=force===undefined?!has:!!force;if(on)this.add(name);else this.remove(name);return on;},
    };
  }
  set id(value){this._id=value;if(value)idRegistry.set(value,this);}
  get id(){return this._id;}
  addEventListener(type,fn){if(!this.handlers.has(type))this.handlers.set(type,[]);this.handlers.get(type).push(fn);}
  removeEventListener(type,fn){this.handlers.set(type,(this.handlers.get(type)||[]).filter(item=>item!==fn));}
  dispatchEvent(event){for(const fn of this.handlers.get(event.type)||[])fn(event);return true;}
  append(...nodes){for(const node of nodes){if(node&&typeof node==='object'){node.parentNode=this;this.children.push(node);}}}
  appendChild(node){this.append(node);return node;}
  replaceChildren(...nodes){this.children=[];this.textContent='';this.append(...nodes);}
  after(node){node.parentNode=this.parentNode;this.parentNode?.children.push(node);}
  setAttribute(name,value){this.attributes[name]=String(value);}
  getAttribute(name){return this.attributes[name]??null;}
  querySelector(){return null;}
  querySelectorAll(){return [];}
  focus(){}
}
class FakeWindow {
  constructor(){this.handlers=new Map();this.__testInvocations=[];this.ChatHub={invoke:async()=>({})};}
  addEventListener(type,fn){if(!this.handlers.has(type))this.handlers.set(type,[]);this.handlers.get(type).push(fn);}
  dispatchEvent(event){for(const fn of this.handlers.get(event.type)||[])fn(event);return true;}
}

function scriptSource(relativePath,append=''){
  return fs.readFileSync(require.resolve(relativePath),'utf8')
    .replace(/^import\s+.*?;\s*$/gm,'')
    .replace(/\bexport\s+function\b/g,'function')+append;
}

test('category graph sends scoped filters with a supported limit and selecting an entity keeps Graph mode',async()=>{
  const window=new FakeWindow();
  const document={body:new FakeElement('body'),createElement:()=>new FakeElement(),getElementById(id){if(!idRegistry.has(id))new FakeElement(id);return idRegistry.get(id);},querySelector:()=>null};
  const storage=new Map([['chat-hub.overview-display-mode','graph']]);
  const context=vm.createContext({window,document,localStorage:{getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)},Event,CustomEvent,console,setTimeout,clearTimeout,Map,Set,Math,JSON,Number,String,Array,Promise});
  const [graphModel,entityModel,search]=await Promise.all([
    import('../desktop-ui/graph-model.js'),import('../desktop-ui/entity-model.js'),import('../desktop-ui/search.js'),
  ]);
  context.prepareConnections=graphModel.prepareConnections;
  context.prepareEntityConnections=graphModel.prepareEntityConnections;
  context.messageGraph=graphModel.messageGraph;
  context.entityGraph=entityModel.entityGraph;context.entityKinds=entityModel.entityKinds;
  context.normalizeSearch=(value)=>value;context.groupSearchConversations=search.groupSearchConversations;context.conversationOpenTarget=search.conversationOpenTarget;
  context.normalizeOverviewMode=value=>value==='list'?'list':'graph';context.overviewModeForAction=(action,current)=>action==='show-list'?'list':action==='show-graph'||action==='graph-drilldown'?'graph':current;
  context.createMapSearch=()=>({reset(){},update(){}});
  context.createHoverPreview=()=>({close(){},bind(){return()=>{};}});
  context.orderedMessages=()=>[];context.createTranscriptPager=()=>({destroy(){}});
  context.createGraphView=(_container,onNode)=>{
    window.__testOnNode=onNode;
    return {render(graph){window.__testGraph=graph;},search:()=>[],select(){},fit(){},zoom(){},destroy(){}};
  };
  context.window.ChatHub.invoke=async(command,args)=>{
    window.__testInvocations.push({command,args});
    if(command==='catalog_entity_graph')return {total_entities:1,truncated:false,nodes:[{id:'entity:file',kind:'file_artifact',label:'Report.csv',entity_id:'entity:file'}],edges:[]};
    if(command==='catalog_entity')return {entity:{id:'entity:file',kind:'file_artifact',label:'Report.csv',reference_count:1,artifact_state:'metadata_present',content_saved:false},references:[],references_truncated:false};
    if(command==='get_target_tags')return {tags:[]};
    return {};
  };
  context.showOverview=(...args)=>context.__overview.showOverview(...args);
  context.showCatalogEntityGraph=(...args)=>context.__overview.showCatalogEntityGraph(...args);
  context.showEntityGraph=(...args)=>context.__overview.showEntityGraph(...args);
  context.getOverviewDisplayMode=()=>storage.get('chat-hub.overview-display-mode')||'graph';
  context.setOverviewDisplayMode=value=>storage.set('chat-hub.overview-display-mode',value);
  try{
    const overview=scriptSource('../desktop-ui/overview.js','\nglobalThis.__overview={showOverview,showCatalogEntityGraph,showEntityGraph};');
    vm.runInContext(`{${overview}}`,context,{filename:'overview.js'});
    const catalog=scriptSource('../desktop-ui/catalog-items.js');
    vm.runInContext(`{${catalog}}`,context,{filename:'catalog-items.js'});
    document.getElementById('history-source').value='codex';
    document.getElementById('project-filter').value='project:alpha';
    document.getElementById('tag-filter').value='reviewed';
    document.getElementById('artifact-state-filter').value='metadata_present';
    window.dispatchEvent(new CustomEvent('chat-hub:show-chat-files',{detail:{id:'local-chat-id'}}));
    await new Promise(resolve=>setTimeout(resolve,0));
    const searchInput=document.getElementById('catalog-search');searchInput.value='needle';searchInput.dispatchEvent(new Event('input'));
    await new Promise(resolve=>setTimeout(resolve,230));
    const call=window.__testInvocations.filter(item=>item.command==='catalog_entity_graph').at(-1);
    assert.deepEqual(call.args,{source:'codex',projectKey:'project:alpha',tag:'reviewed',kind:'file_artifact',conversationId:'local-chat-id',query:'needle',artifactState:'metadata_present',limit:40});
    assert.equal(window.__testGraph.nodes[0].kind,'file_artifact');
    window.__testOnNode(window.__testGraph.nodes[0],false);
    await new Promise(resolve=>setTimeout(resolve,0));
    assert.equal(document.getElementById('overview-mode-graph').getAttribute('aria-pressed'),'true');
    assert.equal(document.getElementById('overview-mode-list').getAttribute('aria-pressed'),'false');
    assert.ok(document.getElementById('library-overview').classList.contains('entity-category-mode'));
    assert.equal(document.getElementById('overview-map').hidden,false);
  }finally{idRegistry.clear();}
});
