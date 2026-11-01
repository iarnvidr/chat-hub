const test = require('node:test');
const assert = require('node:assert/strict');

function fakeElement() {
  const handlers = new Map();
  return {
    addEventListener(type, fn) { if (!handlers.has(type)) handlers.set(type, new Set()); handlers.get(type).add(fn); },
    removeEventListener(type, fn) { handlers.get(type)?.delete(fn); },
    contains(node) { return node === this; },
    emit(type, extra = {}) { for (const fn of handlers.get(type) || []) fn({relatedTarget:null,...extra}); },
  };
}
function fakeClock() {
  let next = 0;
  const jobs = new Map();
  return {
    schedule(fn) { const id=++next;jobs.set(id,fn);return id; },
    cancel(id) { jobs.delete(id); },
    run() { const queued=[...jobs.values()];jobs.clear();for(const fn of queued)fn(); },
  };
}

test('related-chat preview waits for intent and transfers hover between row and pane', async () => {
  const {createHoverPreview}=await import('../desktop-ui/graph-interactions.js');
  const clock=fakeClock(),row=fakeElement(),pane=fakeElement(),events=[];
  const controller=createHoverPreview({schedule:clock.schedule,cancel:clock.cancel,load:async target=>target.title,show:target=>events.push(`show:${target.title}`),hide:()=>events.push('hide')});
  controller.bind(row,pane,{conversationId:'local-chat',title:'Saved title'});
  row.emit('pointerenter');assert.deepEqual(events,[]);
  clock.run();await Promise.resolve();assert.deepEqual(events,['show:Saved title']);
  row.emit('pointerleave',{relatedTarget:pane});pane.emit('pointerenter');clock.run();
  assert.deepEqual(events,['show:Saved title']);
  pane.emit('pointerleave',{relatedTarget:row});row.emit('pointerleave');row.emit('pointerenter');clock.run();
  assert.deepEqual(events,['show:Saved title']);
  row.emit('pointerleave');clock.run();assert.deepEqual(events,['show:Saved title','hide']);
  controller.destroy();
});

test('switching related rows hides the old preview immediately and ignores its late response', async () => {
  const {createHoverPreview}=await import('../desktop-ui/graph-interactions.js');
  const clock=fakeClock(),rowA=fakeElement(),rowSlow=fakeElement(),rowB=fakeElement(),pane=fakeElement(),events=[],pending=[];
  const controller=createHoverPreview({schedule:clock.schedule,cancel:clock.cancel,
    load:target=>new Promise(resolve=>pending.push({title:target.title,resolve})),
    show:target=>events.push(`show:${target.title}`),hide:()=>events.push('hide')});
  controller.bind(rowA,pane,{conversationId:'a',title:'A'});
  rowA.emit('pointerenter');clock.run();await Promise.resolve();
  pending.shift().resolve('A detail');await Promise.resolve();await Promise.resolve();
  assert.equal(events.at(-1),'show:A');
  controller.bind(rowSlow,pane,{conversationId:'slow',title:'Slow'});
  rowA.emit('pointerleave');rowSlow.emit('pointerenter');
  assert.equal(events.at(-1),'hide');clock.run();await Promise.resolve();
  controller.bind(rowB,pane,{conversationId:'b',title:'B'});
  rowSlow.emit('pointerleave');rowB.emit('pointerenter');
  assert.equal(events.at(-1),'hide','old preview closes as soon as a different row is entered');
  clock.run();await Promise.resolve();
  const [requestA,requestB]=pending;
  requestA.resolve('late slow detail');await Promise.resolve();await Promise.resolve();
  assert.ok(!events.includes('show:Slow'));
  requestB.resolve('B detail');await Promise.resolve();await Promise.resolve();
  assert.equal(events.at(-1),'show:B');
  controller.destroy();
});

test('late preview responses are discarded after the pointer leaves', async () => {
  const {createHoverPreview}=await import('../desktop-ui/graph-interactions.js');
  const clock=fakeClock(),row=fakeElement(),events=[];let resolve;
  const controller=createHoverPreview({schedule:clock.schedule,cancel:clock.cancel,load:()=>new Promise(done=>{resolve=done;}),show:()=>events.push('show'),hide:()=>events.push('hide')});
  controller.bind(row,null,{conversationId:'large-local-chat',title:'Large saved chat'});
  row.emit('pointerenter');clock.run();
  row.emit('pointerleave');clock.run();
  resolve({conversation:{id:'large-local-chat'}});await Promise.resolve();await Promise.resolve();
  assert.deepEqual(events,['hide']);
  controller.destroy();
});

test('preview pane exit grace does not hide while focus or pointer transfers back to its row', async () => {
  const {createHoverPreview}=await import('../desktop-ui/graph-interactions.js');
  const clock=fakeClock(),row=fakeElement(),pane=fakeElement(),events=[];
  const controller=createHoverPreview({schedule:clock.schedule,cancel:clock.cancel,load:async()=>({}),show:()=>events.push('show'),hide:()=>events.push('hide')});
  controller.bind(row,pane,{conversationId:'chat',title:'Chat'});
  row.emit('focusin');clock.run();await Promise.resolve();assert.equal(events.at(-1),'show');
  row.emit('focusout',{relatedTarget:pane});pane.emit('focusin');pane.emit('focusout',{relatedTarget:row});row.emit('focusin');clock.run();
  assert.equal(events.includes('hide'),false);
  row.emit('focusout');clock.run();assert.equal(events.at(-1),'hide');
  controller.destroy();
});
