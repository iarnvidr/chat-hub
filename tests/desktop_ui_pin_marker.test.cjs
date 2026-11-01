const test=require('node:test'),assert=require('node:assert/strict');

class Element {
  constructor(tag){this.tagName=tag;this.children=[];this.attributes={};this.className='';}
  setAttribute(name,value){this.attributes[name]=String(value);}
  append(...nodes){this.children.push(...nodes);}
}
const doc={createElement:tag=>new Element(tag),createElementNS:(_ns,tag)=>new Element(tag)};

test('a confirmed pinned chat gets a visible, accessible, noninteractive marker beside its title',async()=>{
  const {appendPinMarker}=await import('../desktop-ui/pin-marker.js');
  const title=new Element('strong');title.ownerDocument=doc;title.textContent='Saved chat';
  const marker=appendPinMarker(title,true,doc);
  assert.equal(title.children[0],marker);
  assert.equal(marker.className,'conversation-pin-marker');
  assert.equal(marker.attributes.role,'img');
  assert.equal(marker.attributes['aria-label'],'Pinned in ChatGPT');
  assert.equal(marker.children[0].tagName,'svg');
  assert.equal(marker.children[0].children[0].tagName,'path');
  assert.equal(typeof marker.addEventListener,'undefined');
});

test('unknown or unpinned chats receive no pin marker',async()=>{
  const {appendPinMarker}=await import('../desktop-ui/pin-marker.js');
  const title=new Element('strong');
  assert.equal(appendPinMarker(title,false,doc),null);
  assert.equal(appendPinMarker(title,undefined,doc),null);
  assert.deepEqual(title.children,[]);
});
