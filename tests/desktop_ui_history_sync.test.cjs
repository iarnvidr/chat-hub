const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function boot(invoke, {buttonPresent = true} = {}) {
  const callbacks = new Map(), events = [], timers = new Map();
  let nextTimer = 1;
  let now = Date.now();
  class TestDate extends Date { static now() { return now; } }
  const button = buttonPresent ? {addEventListener: (_, fn) => {button.click = fn;}} : null;
  const settingsStatus = {}, appStatus = {}, footer = {};
  vm.runInNewContext(fs.readFileSync(require.resolve('../desktop-ui/history-sync.js'), 'utf8'), {
    document: {getElementById: id => id === 'sync-history' ? button
      : id === 'history-sync-status' ? settingsStatus
        : id === 'app-status' ? appStatus
          : id === 'workspace-sync-state' ? footer : null},
    window: {__TAURI_INTERNALS__: {invoke}, ChatHub: {invoke},
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
    flush: async () => {await new Promise(resolve => setImmediate(resolve)); await new Promise(resolve => setImmediate(resolve));},
  };
}

test('startup checks the provider session without opening ChatGPT or starting a sync when it is not open', async () => {
  const calls = [], ui = boot(async command => {
    calls.push(command);
    if (command === 'chatgpt_history_status') return {state: 'idle', configured: true, auto_sync: false};
    if (command === 'get_chatgpt_connection') return {state: 'not_open'};
    return {};
  });
  await ui.flush();
  assert.deepEqual(calls, ['chatgpt_history_status', 'get_chatgpt_connection']);
  assert.equal(calls.includes('sync_chatgpt_history'), false);
  assert.deepEqual(ui.events, []);
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

test('rate-limit cooldown blocks automatic and manual retries without polling loops', async () => {
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
  assert.equal(ui.button.disabled, true);
  await ui.dispatch('chat-hub:chatgpt-connected');
  await ui.button.click();
  await ui.flush();
  assert.equal(starts, 1);
  assert.equal([...ui.timers.values()].some(timer => timer.ms === 2000), false);
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
