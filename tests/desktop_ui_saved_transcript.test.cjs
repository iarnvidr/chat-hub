const test=require('node:test');
const assert=require('node:assert/strict');

class FakeElement {
  constructor(tag='div'){this.tagName=tag.toUpperCase();this.nodeType=1;this.children=[];this.parentNode=null;this.dataset={};this.attributes={};this.handlers=new Map();this.className='';this.clientHeight=100;this.scrollHeight=500;this.scrollTop=0;this.hidden=false;this._text='';}
  set textContent(value){this._text=String(value??'');this.children=[];}
  get textContent(){return this._text+this.children.map(child=>child.textContent||'').join('');}
  append(...nodes){for(const node of nodes){if(!node)continue;node.parentNode=this;this.children.push(node);}}
  removeChild(node){this.children=this.children.filter(child=>child!==node);node.parentNode=null;return node;}
  replaceChildren(...nodes){for(const child of this.children)child.parentNode=null;this.children=[];this._text='';this.append(...nodes);}
  addEventListener(type,fn){if(!this.handlers.has(type))this.handlers.set(type,[]);this.handlers.get(type).push(fn);}
  removeEventListener(type,fn){this.handlers.set(type,(this.handlers.get(type)||[]).filter(candidate=>candidate!==fn));}
  dispatchEvent(event){for(const fn of this.handlers.get(event.type)||[])fn(event);}
  setAttribute(name,value){this.attributes[name]=String(value);}
}
const doc={createElement:tag=>new FakeElement(tag),createTextNode:text=>({nodeType:3,textContent:String(text),parentNode:null})};

test('message author objects are rendered as roles or safe names, never object stringification',async()=>{
  const {speakerLabel}=await import('../desktop-ui/saved-transcript.js');
  assert.equal(speakerLabel({author:{role:'user'}}).label,'YOU');
  assert.equal(speakerLabel({author:{role:'assistant'}}).label,'ASSISTANT');
  assert.equal(speakerLabel({role:'user',author:{role:'assistant'}}).label,'YOU');
  assert.equal(speakerLabel({author:{name:'ChatGPT'}}).label,'ChatGPT');
  assert.equal(speakerLabel({author:{provider:{id:1}}}).label,'Message');
  assert.notEqual(speakerLabel({author:{provider:{id:1}}}).label,'[object Object]');
});

test('transcript pager starts with the first long-message chunk and appends the full text on scroll',async()=>{
  const {createTranscriptPager}=await import('../desktop-ui/saved-transcript.js');
  const content='opening paragraph\n'+('long saved message '.repeat(400));
  const message={role:'assistant',author:{role:'assistant'},content};
  const root=new FakeElement('div');
  const pager=createTranscriptPager(root,[message],{batchSize:1,maxChunkChars:1024,doc});
  try{
    assert.equal(pager.loaded,1);
    const turn=root.children.find(child=>child.className==='saved-turn');
    assert.ok(turn.textContent.startsWith('ASSISTANTopening paragraph'));
    assert.ok(turn.textContent.length<content.length,'initial DOM remains bounded');
    root.scrollTop=450;root.dispatchEvent({type:'scroll'});
    while(root.children.some(child=>child.className==='saved-transcript-more'))pager.loadNext();
    const fullText=turn.children.find(child=>child.className==='saved-turn__content').textContent;
    assert.equal(fullText.replace(/\s+/g,' ').trim(),content.replace(/\s+/g,' ').trim());
    assert.equal(pager.loaded>1,true);
  }finally{pager.destroy();}
});

test('long fenced code stays readable across progressive chunk boundaries',async()=>{
  const {createTranscriptPager}=await import('../desktop-ui/saved-transcript.js');
  const codeLines=Array.from({length:120},(_,index)=>`const value_${index} = ${index};`);
  const root=new FakeElement('div');
  const pager=createTranscriptPager(root,[{role:'assistant',content:`\`\`\`ts\n${codeLines.join('\n')}\n\`\`\``}],{batchSize:1,maxChunkChars:180,doc});
  try{
    const turn=root.children.find(child=>child.className==='saved-turn');
    while(root.children.some(child=>child.className==='saved-transcript-more'))pager.loadNext();
    const visible=turn.children.find(child=>child.className==='saved-turn__content').textContent;
    assert.ok(visible.includes('const value_0 = 0;'));
    assert.ok(visible.includes('const value_119 = 119;'));
    assert.equal((visible.match(/const value_\d+ = \d+;/g)||[]).length,120);
  }finally{pager.destroy();}
});
