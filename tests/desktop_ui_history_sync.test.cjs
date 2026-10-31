const {test} = require('bun:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function boot(invoke, {buttonPresent = true, storage = new Map(), initialTime = Date.now()} = {}) {
  const callbacks = new Map(), events = [], timers = new Map();
  let nextTimer = 1;
  let now = initialTime;
  const localStorage = {
    getItem: key => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, String(value)),
    removeItem: key => storage.delete(key),
  };
  class TestDate extends Date { static now() { return now; } }
  const button = buttonPresent ? {addEventListener: (_, fn) => {button.click = fn;}} : null;
  const settingsStatus = {}, appStatus = {}, footer = {};
  vm.runInNewContext(fs.readFileSync(require.resolve('../desktop-ui/history-sync.js'), 'utf8'), {
    document: {getElementById: id => id === 'sync-history' ? button
      : id === 'history-sync-status' ? settingsStatus
        : id === 'app-status' ? appStatus
          : id === 'workspace-sync-state' ? footer : null},
    window: {__TAURI_INTERNALS__: {invoke}, ChatHub: {invoke}, localStorage,
      addEventListener: (name, fn) => callbacks.set(name, fn),
      dispatchEvent: event => {events.push(event.type); callbacks.get(event.type)?.(event);}},
    Event: class {constructor(type) {this.type = type;}},
    Date: TestDate,
    setTimeout: (fn, ms) => {const id = nextTimer++; timers.set(id, {fn, ms}); return id;},
    clearTimeout: id => timers.delete(id),
    setImmediate,
  });
  return {
    button, status: settingsStatus, appStatus, footer, events, timers,
    dispatch: name => callbacks.get(name)?.({type: name}),
    advance: ms => {now += ms;},
    now: () => now,
    runTimer: async predicate => {
      const selected = [...timers.entries()].find(([, timer]) => predicate(timer));
      if (!selected) return false;
      const [id, timer] = selected;
      timers.delete(id);
      await timer.fn();
      await new Promise(resolve => setImmediate(resolve));
      await new Promise(resolve => setImmediate(resolve));
      return true;
    },
    storage,
    flush: async () => {await new Promise(resolve => setImmediate(resolve)); await new Promise(resolve => setImmediate(resolve));},
  };
}

test('startup prepares a hidden session without opening the live view or starting an unready sync', async () => {
  const calls = [], ui = boot(async command => {
    calls.push(command);
    if (command === 'chatgpt_history_status') return {state: 'idle', configured: true, auto_sync: false};
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    return {};
  });
  await ui.flush();
  assert.deepEqual(calls, ['chatgpt_history_status', 'get_chatgpt_connection', 'prepare_chatgpt_session']);
  assert.equal(calls.includes('sync_chatgpt_history'), false);
  assert.deepEqual(ui.events, []);
});

test('a hidden startup session becomes connected and syncs without opening a tab', async () => {
  let probes = 0, starts = 0;
  const ui = boot(async command => {
    if (command === 'chatgpt_history_status') return {state: starts ? 'completed' : 'idle'};
    if (command === 'get_chatgpt_connection') return {state: probes++ ? 'connected' : 'not_open'};
    if (command === 'prepare_chatgpt_session') return true;
    if (command === 'sync_chatgpt_history') starts++;
    return {};
  });
  await ui.flush();
  const pending = [...ui.timers.values()].find(timer => timer.ms === 2000);
  assert.ok(pending);
  await pending.fn(); await ui.flush();
  assert.equal(starts, 1);
  assert.equal(ui.events.includes('chat-hub:show-live'), false);
  assert.equal(ui.events.includes('chat-hub:connect'), false);
});

test('a signed-in session found at startup starts a background history sync', async () => {
  const calls = [], events = [];
  const ui = boot(async command => {
    calls.push(command);
    if (command === 'chatgpt_history_status') return {state: 'completed', imported: 3, updated: 1, unchanged: 0};
    if (command === 'get_chatgpt_connection') return {state: 'connected'};
    return {};
  });
  ui.events.push(...events);
  await ui.flush();
  assert.equal(calls.filter(command => command === 'sync_chatgpt_history').length, 1);
  assert.ok(ui.events.includes('chat-hub:history-updated'));
  assert.match(ui.status.textContent, /3 added, 1 updated/);
  assert.equal(ui.events.includes('chat-hub:connect'), false);
});

test('a newly connected account automatically syncs without changing the active view', async () => {
  const calls = [];
  const ui = boot(async command => {
    calls.push(command);
    if (command === 'chatgpt_history_status') return {state: 'completed', imported: 2, updated: 0, unchanged: 0};
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    return {};
  });
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected');
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected');
  await ui.flush();
  assert.equal(calls.filter(command => command === 'sync_chatgpt_history').length, 1);
  assert.ok(ui.events.includes('chat-hub:history-updated'));
  assert.equal(ui.events.includes('chat-hub:connect'), false);
  assert.equal(ui.events.includes('chat-hub:show-live'), false);
});

test('tab switches do not repeat a background history fetch for five minutes, while manual sync remains available', async () => {
  let starts = 0;
  const ui = boot(async command => {
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    if (command === 'sync_chatgpt_history') {starts++; return {};}
    return {state: 'completed', imported: 0, updated: 0, unchanged: 4};
  });
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(starts, 1);

  ui.advance(6_000); // More than the event debounce, less than the fetch coalesce window.
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(starts, 1);

  await ui.button.click(); await ui.flush();
  assert.equal(starts, 2);
});

test('explicit sync connects the app account and resumes fetching when sign-in completes', async () => {
  const calls = []; let connected = false;
  const ui = boot(async command => {
    calls.push(command);
    if (command === 'get_chatgpt_connection') return {state: connected ? 'connected' : 'not_open'};
    if (command === 'sync_chatgpt_history' && !connected) throw Error('chatgpt_not_open');
    if (command === 'chatgpt_history_status') return connected
      ? {state: 'completed', imported: 2, updated: 0, unchanged: 0}
      : {state: 'idle', configured: true, auto_sync: false};
    return {};
  });
  await ui.flush(); await ui.button.click();
  assert.ok(ui.events.includes('chat-hub:connect'));
  connected = true;
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(calls.filter(x => x === 'sync_chatgpt_history').length, 2);
  assert.ok(ui.events.includes('chat-hub:history-updated'));
  assert.match(ui.status.textContent, /2 added/);
});

test('rate-limit cooldown waits quietly and resumes automatically after native deadline', async () => {
  let starts = 0, statusCalls = 0;
  const ui = boot(async command => {
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    if (command === 'sync_chatgpt_history') {starts++; throw Error('history_rate_limited');}
    if (command === 'chatgpt_history_status' && statusCalls++ === 0)
      return {state: 'idle', configured: true};
    return {state: 'partial', error_code: 'rate_limited', retry_after: 60,
      message: 'Fetched progress was saved.', imported: 3};
  });
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(starts, 1);
  assert.match(ui.status.textContent, /progress was saved.*60 seconds/);
  assert.equal(ui.footer.textContent, 'History sync waiting');
  assert.equal(ui.button.disabled, true);
  await ui.dispatch('chat-hub:chatgpt-connected');
  await ui.button.click();
  await ui.flush();
  assert.equal(starts, 1);
  assert.equal([...ui.timers.values()].some(timer => timer.ms === 2000), false);
  assert.ok([...ui.timers.values()].some(timer => timer.ms === 1000), 'cooldown schedules a bounded timer');
});

test('a server Retry-After longer than the backoff cap is persisted without shortening', async () => {
  const storage=new Map(), startTime=Date.now();
  let failed=false;
  const ui=boot(async command=>{
    if(command==='get_chatgpt_connection')return{state:'not_open'};
    if(command==='chatgpt_history_status')return failed
      ? {state:'partial',error_code:'rate_limited',retry_after:3600,message:'progress saved'}
      : {state:'idle',configured:true};
    if(command==='sync_chatgpt_history'){failed=true;throw Error('history_rate_limited');}
    return{};
  },{storage,initialTime:startTime});
  await ui.flush();
  await ui.button.click();
  await ui.flush();
  const saved=JSON.parse(storage.get('chat-hub.chatgpt-history-cooldown.v1'));
  assert.equal(saved.until,startTime+3_600_000);
  assert.match(ui.status.textContent,/3600 seconds/);
  ui.advance(30*60*1000);
  assert.equal(await ui.runTimer(timer=>timer.ms>0),true);
  assert.equal(ui.button.disabled,true,'the local 30 minute backoff cap cannot resume early');
});

test('verification challenges pause background retry and never open the provider automatically', async () => {
  let starts=0;
  const ui=boot(async command=>{
    if(command==='get_chatgpt_connection')return{state:'not_open'};
    if(command==='sync_chatgpt_history'){starts++;throw Error('history_rate_limited');}
    if(command==='chatgpt_history_status')return starts
      ? {state:'partial',error_code:'rate_limited',retry_after:60,challenge_detected:true,automatic_retry_paused:true}
      : {state:'idle',configured:true};
    return{};
  });
  await ui.flush();
  await ui.button.click();
  await ui.flush();
  assert.equal(starts,1);
  assert.match(ui.status.textContent,/verification challenge/);
  assert.equal(ui.footer.textContent,'ChatGPT verification needed');
  assert.equal(ui.events.includes('chat-hub:connect'),false);
  ui.advance(60_000);
  assert.equal(await ui.runTimer(timer=>timer.ms>0),true);
  assert.equal(ui.footer.textContent,'ChatGPT verification needed');
  assert.equal(await ui.runTimer(timer=>timer.ms===0),false);
  assert.equal(starts,1,'verification-required cooldown does not launch background sync');
});

test('one manual session gets at most three automatic retries after a rate limit', async () => {
  let starts = 0, statusCalls = 0, failedSinceStatus = false;
  const ui = boot(async command => {
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    if (command === 'sync_chatgpt_history') { starts++; failedSinceStatus=true; throw Error('history_rate_limited'); }
    if (command === 'chatgpt_history_status') {
      if (statusCalls++ === 0) return {state:'idle',configured:true};
      const retry_after=failedSinceStatus?1:0;
      failedSinceStatus=false;
      return {state:'partial',error_code:'rate_limited',retry_after,message:'Fetched progress was saved.'};
    }
    return {};
  });
  await ui.flush();
  await ui.button.click(); // explicit attempt, then background retries are bounded
  await ui.flush();
  assert.equal(starts,1);
  for (const waitMs of [15_000, 30_000, 60_000]) {
    ui.advance(waitMs);
    assert.equal(await ui.runTimer(timer=>timer.ms>0),true);
    const auto=await ui.runTimer(timer=>timer.ms===0);
    assert.equal(auto,true);
  }
  ui.advance(1000);
  assert.equal(await ui.runTimer(timer=>timer.ms===1000),true);
  assert.equal(starts,4,'one explicit attempt plus at most three automatic attempts');
  assert.match(ui.footer.textContent,/Sync paused/);
  assert.equal([...ui.timers.values()].some(timer=>timer.ms===0),false);
});

test('provider cooldown deadline persists locally across app restart without account data', async () => {
  const storage=new Map();
  const startTime=Date.now();
  let failed=false;
  const first=boot(async command=>{
    if(command==='get_chatgpt_connection')return{state:'not_open'};
    if(command==='chatgpt_history_status'){
      if(!failed)return{state:'idle',configured:true};
      return{state:'partial',error_code:'rate_limited',retry_after:90,message:'progress saved'};
    }
    if(command==='sync_chatgpt_history'){failed=true;throw Error('history_rate_limited');}
    return{};
  },{storage,initialTime:startTime});
  await first.flush();
  await first.button.click();
  await first.flush();
  const saved=JSON.parse(storage.get('chat-hub.chatgpt-history-cooldown.v1'));
  assert.deepEqual(Object.keys(saved).sort(),['automatic_retry_paused','provider','until','verification_needed']);
  assert.equal(saved.provider,'chatgpt');
  assert.equal(saved.automatic_retry_paused,false);

  let starts=0;
  const second=boot(async command=>{
    if(command==='chatgpt_history_status')return{state:'idle',configured:true};
    if(command==='get_chatgpt_connection')return{state:'connected'};
    if(command==='sync_chatgpt_history')starts++;
    return{};
  },{storage,initialTime:startTime+1000});
  await second.flush();
  assert.equal(starts,0,'restart cannot bypass the saved cooldown');
  assert.equal(second.button.disabled,true);
  assert.equal(second.footer.textContent,'History sync waiting');
});

test('a challenge pause survives restart and startup does not sync until manual action', async () => {
  const storage=new Map();
  let attempted=false;
  const first=boot(async command=>{
    if(command==='get_chatgpt_connection')return{state:'not_open'};
    if(command==='chatgpt_history_status')return attempted
      ? {state:'partial',error_code:'rate_limited',retry_after:0,challenge_detected:true,automatic_retry_paused:true}
      : {state:'idle',configured:true};
    if(command==='sync_chatgpt_history'){attempted=true;throw Error('history_rate_limited');}
    return{};
  },{storage});
  await first.flush();
  await first.button.click();
  await first.flush();
  assert.equal(JSON.parse(storage.get('chat-hub.chatgpt-history-cooldown.v1')).verification_needed,true);

  let starts=0;
  const restarted=boot(async command=>{
    if(command==='chatgpt_history_status')return{state:'idle',configured:true};
    if(command==='get_chatgpt_connection')return{state:'connected'};
    if(command==='sync_chatgpt_history')starts++;
    return{};
  },{storage});
  await restarted.flush();
  assert.equal(starts,0);
  assert.equal(restarted.footer.textContent,'ChatGPT verification needed');
});

test('background sign-in failures do not navigate away from the current view', async () => {
  const ui = boot(async command => {
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    if (command === 'sync_chatgpt_history') return {};
    return {state: 'failed', error_code: 'signed_out', message: 'Sign in to ChatGPT in this app.'};
  });
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(ui.events.includes('chat-hub:connect'), false);
  assert.equal(ui.events.includes('chat-hub:show-live'), false);
  assert.match(ui.status.textContent, /Sign in to ChatGPT/);
});

test('history status remains safe when the Settings subpage has no sync button mounted', async () => {
  const ui = boot(async command => command === 'chatgpt_history_status'
    ? {state: 'idle', configured: true}
    : {state: 'not_open'}, {buttonPresent: false});
  await ui.flush();
  await ui.dispatch('chat-hub:chatgpt-connected');
  await ui.flush();
  assert.match(ui.status.textContent, /history/);
});
