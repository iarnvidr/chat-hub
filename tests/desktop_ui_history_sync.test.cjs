const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function boot(invoke) {
  const callbacks = new Map(), events = [], button = {addEventListener: (_, fn) => {button.click = fn;}}, status = {};
  vm.runInNewContext(fs.readFileSync(require.resolve('../desktop-ui/history-sync.js'), 'utf8'), {
    document: {getElementById: id => id === 'sync-history' ? button : status},
    window: {__TAURI_INTERNALS__: {invoke}, ChatHub: {invoke},
      addEventListener: (name, fn) => callbacks.set(name, fn),
      dispatchEvent: event => {events.push(event.type); callbacks.get(event.type)?.(event);}},
    Event: class {constructor(type) {this.type = type;}},
    setTimeout: () => 1, clearTimeout() {},
  });
  return {button, status, events, dispatch: name => callbacks.get(name)?.(),
    flush: () => new Promise(resolve => setImmediate(resolve))};
}

test('startup keeps saved history usable without opening ChatGPT or a plugin', async () => {
  const calls = [], ui = boot(async command => {
    calls.push(command); return {state: 'idle', configured: true, auto_sync: false};
  });
  await ui.flush();
  assert.deepEqual(calls, ['chatgpt_history_status']);
  assert.deepEqual(ui.events, []);
});

test('explicit sync connects the app account and resumes fetching when sign-in completes', async () => {
  const calls = []; let connected = false;
  const ui = boot(async command => {
    calls.push(command);
    if (command === 'sync_chatgpt_history' && !connected) throw Error('chatgpt_not_open');
    if (command === 'chatgpt_history_status') return connected
      ? {state: 'completed', imported: 2, updated: 0, unchanged: 0}
      : {state: 'idle', configured: true, auto_sync: false};
    return {};
  });
  await ui.flush(); await ui.button.click();
  assert.ok(ui.events.includes('chat-hub:connect'));
  connected = true; ui.dispatch('chat-hub:chatgpt-connected'); await ui.flush();
  assert.equal(calls.filter(x => x === 'sync_chatgpt_history').length, 2);
  assert.ok(ui.events.includes('chat-hub:history-updated'));
  assert.match(ui.status.textContent, /2 added/);
});

test('rate-limit cooldown reports saved progress without immediate retries', async () => {
  let starts = 0;
  const ui = boot(async command => {
    if (command === 'sync_chatgpt_history') {starts++; throw Error('history_rate_limited');}
    return {state: 'partial', error_code: 'rate_limited', retry_after: 60,
      message: 'Fetched progress was saved.', imported: 3};
  });
  await ui.flush(); await ui.button.click();
  assert.equal(starts, 1);
  assert.match(ui.status.textContent, /progress was saved.*60 seconds/);
});
