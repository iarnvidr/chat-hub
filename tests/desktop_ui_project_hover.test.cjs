const test=require('node:test'),assert=require('node:assert/strict');

class Button {
  constructor(){this.handlers={};}
  addEventListener(type,handler){(this.handlers[type]??=[]).push(handler);}
  fire(type,event={}){for(const handler of this.handlers[type]||[])handler({type,...event});}
}
function timers(){let next=0;const callbacks=new Map();return {
  window:{setTimeout(fn){const id=++next;callbacks.set(id,fn);return id;},clearTimeout(id){callbacks.delete(id);}},
  run(id){const fn=callbacks.get(id);if(!fn)return false;callbacks.delete(id);fn();return true;},
  pending(){return [...callbacks.keys()]}
};}

test('project picker hover waits for intent, then keeps its grace when moving to the picker',async()=>{
  const {bindProjectHoverIntent}=await import('../desktop-ui/project-hover.js');
  const button=new Button(),clock=timers(),events=[];
  bindProjectHoverIntent(button,{window:clock.window,delay:180,grace:280,onOpen:()=>events.push('open'),onLeave:()=>events.push('close')});
  button.fire('pointerenter',{pointerType:'mouse'});assert.deepEqual(events,[]);
  const openTimer=clock.pending()[0];assert.equal(clock.run(openTimer),true);assert.deepEqual(events,['open']);
  button.fire('pointerleave');assert.equal(clock.pending().length,1);
  const closeTimer=clock.pending()[0];clock.run(closeTimer);assert.deepEqual(events,['open','close']);
});

test('touch hover is ignored while keyboard focus opens the picker immediately',async()=>{
  const {bindProjectHoverIntent}=await import('../desktop-ui/project-hover.js');
  const button=new Button(),clock=timers(),events=[];
  bindProjectHoverIntent(button,{window:clock.window,onOpen:()=>events.push('open'),onLeave:()=>events.push('close')});
  button.fire('pointerenter',{pointerType:'touch'});assert.deepEqual(events,[]);assert.deepEqual(clock.pending(),[]);
  button.fire('focus');assert.deepEqual(events,['open']);
});
