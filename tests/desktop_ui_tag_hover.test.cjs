const test=require('node:test'),assert=require('node:assert/strict');
class Element{
  constructor(tag,doc){this.tagName=tag.toUpperCase();this.doc=doc;this.children=[];this.events={};this.attributes={};this.style={};this.hidden=false;this.className='';}
  append(...children){for(const child of children){child.parent=this;this.children.push(child);}}
  replaceChildren(...children){this.children=[];this.append(...children);}
  setAttribute(k,v){this.attributes[k]=String(v);}
  getAttribute(k){return this.attributes[k];}
  addEventListener(k,fn){(this.events[k]??=[]).push(fn);}
  async fire(k,extra={}){for(const fn of this.events[k]||[])await fn({target:this,preventDefault(){},...extra});}
  get childElementCount(){return this.children.length;}
  contains(n){return n===this||this.children.some(c=>c.contains(n));}
  querySelectorAll(s){return this.children.flatMap(c=>[...(s.split(',').some(p=>p[0]==='.'?c.className.split(' ').includes(p.slice(1)):c.tagName===p.toUpperCase())?[c]:[]),...c.querySelectorAll(s)]);}
  querySelector(s){return this.querySelectorAll(s)[0]||null;}
  getBoundingClientRect(){return{left:400,right:430,top:80,bottom:110,width:300,height:160};}
  focus(){this.doc.activeElement=this;}
}
async function setup(invoke){const {createTagHoverEditor}=await import('../desktop-ui/tag-hover.js');const doc={createElement(tag){return new Element(tag,this);},querySelector(){return null;},addEventListener(){}};doc.body=new Element('body',doc);const win={innerWidth:1100,innerHeight:760,addEventListener(){}};let saved=0;const editor=createTagHoverEditor({invoke,doc,win,onSaved:()=>saved++});return{doc,editor,panel:doc.body.children[0],anchor:new Element('button',doc),get saved(){return saved;}};}
const tick=()=>new Promise(resolve=>setTimeout(resolve,0));
test('tag hover shows indexed suggestions, dedupes applied tags, excludes unrelated library tags, and makes custom creation distinct',async()=>{
  const ui=await setup(async command=>command==='get_target_tags'?{tags:['Rust']}:command==='catalog_tag_suggestions'?{items:[{name:'Rust'},{name:'SQLite',reason:'Mentioned in this chat'}]}:{items:[{name:'sqlite'},{name:'release'}]});
  await ui.editor.show({targetKind:'conversation',targetId:'chat-a'},ui.anchor);
  const choices=ui.panel.querySelectorAll('.tag-choice');assert.deepEqual(choices.map(c=>c.textContent),['Rust','SQLite','＋ Create a custom tag']);assert.equal(choices[0].getAttribute('aria-pressed'),'true');assert.match(choices.at(-1).className,/tag-choice-custom/);assert.equal(choices[1].getAttribute('aria-description'),'Mentioned in this chat');
});
test('busy suggestion loads retry briefly and persistent failures offer an explicit retry',async()=>{
  let reads=0,fail=false;const ui=await setup(async command=>{
    if(command==='get_target_tags')return{tags:[]};
    if(command==='catalog_tag_suggestions'){reads++;if(fail)throw Error('unavailable');if(reads===1)throw Error('busy');return{items:[{name:'Schema migrations'}]};}
    throw Error('unrelated_library_request');
  });
  await ui.editor.show({targetKind:'conversation',targetId:'a'},ui.anchor);assert.equal(reads,2);assert.equal(ui.panel.querySelector('.tag-choice').textContent,'Schema migrations');
  fail=true;await ui.editor.show({targetKind:'conversation',targetId:'b'},ui.anchor);assert.match(ui.panel.querySelector('.tag-hover-status').textContent,/suggestions could not load/);const retry=ui.panel.querySelector('.tag-retry');assert.ok(retry);fail=false;await retry.fire('click');assert.equal(ui.panel.querySelector('.tag-choice').textContent,'Schema migrations');
});
test('entity suggestions use bounded referenced-chat context while saves remain on the entity',async()=>{
  const calls=[];const ui=await setup(async(command,args)=>{calls.push([command,args]);return command==='catalog_tag_suggestions'?{items:[{name:'Village research'}]}:{tags:[]};});
  await ui.editor.show({targetKind:'entity',targetId:'domain-a',suggestionConversationIds:['chat-a','chat-a','chat-b','chat-c']},ui.anchor);
  assert.deepEqual(calls.filter(([command])=>command==='catalog_tag_suggestions').map(([,args])=>args),[{conversationId:'chat-a',limit:6},{conversationId:'chat-b',limit:6}]);
  await ui.panel.querySelector('.tag-choice').fire('click');assert.deepEqual(calls.at(-1),['set_target_tags',{targetKind:'entity',targetId:'domain-a',tags:['Village research']}]);
});
test('tag saves merge current persisted tags and only acknowledge after successful persistence',async()=>{
  let reads=0,write;const calls=[];const ui=await setup(async(command,args)=>{calls.push([command,args]);if(command==='get_target_tags')return{tags:++reads===1?['Rust']:['Rust','Added elsewhere']};if(command==='set_target_tags')return new Promise(resolve=>write=resolve);return{items:[{name:'SQLite'}]};});
  await ui.editor.show({targetKind:'conversation',targetId:'chat-a'},ui.anchor);const button=ui.panel.querySelectorAll('button').find(c=>c.textContent==='SQLite');const save=button.fire('click');await tick();assert.equal(ui.saved,0);assert.equal(button.disabled,true);assert.deepEqual(calls.at(-1),['set_target_tags',{targetKind:'conversation',targetId:'chat-a',tags:['Rust','Added elsewhere','SQLite']}]);write({});await save;assert.equal(ui.saved,1);
});
test('stale tag metadata cannot replace the newly hovered chat and choices wait for initial metadata',async()=>{
  let resolveA;const ui=await setup(async(command,args)=>{if(command==='get_target_tags')return args.targetId==='a'?new Promise(resolve=>resolveA=resolve):{tags:['B tag']};return{items:[]};});
  const old=ui.editor.show({targetKind:'conversation',targetId:'a'},ui.anchor);assert.equal(ui.panel.querySelector('button').disabled,true);await ui.editor.show({targetKind:'conversation',targetId:'b'},ui.anchor);resolveA({tags:['A tag']});await old;assert.equal(ui.panel.querySelector('.tag-choice').textContent,'B tag');
  await ui.panel.fire('keydown',{key:'Escape'});assert.equal(ui.panel.hidden,true);assert.equal(ui.doc.activeElement,ui.anchor);
});
