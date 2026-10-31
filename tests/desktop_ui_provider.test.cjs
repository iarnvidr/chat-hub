const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function boot(invoke) {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) {
      const classes = new Set(), attributes = new Map();
      elements.set(id, {hidden: id === 'provider-shell', textContent: '',
        classList: {add: x => classes.add(x), remove: x => classes.delete(x)},
        setAttribute: (k, v) => attributes.set(k, v), removeAttribute: k => attributes.delete(k),
        getBoundingClientRect: () => ({x: 200, y: 120, width: 900, height: 600}),
        addEventListener(event, callback) { this[event] = callback; },
      });
    }
    return elements.get(id);
  };
  const listeners = new Map();
  const timers = new Map(); let nextTimer = 0;
  const context = vm.createContext({
    document: {getElementById: element, body: element('body')},
    window: {ChatHub: {invoke}, __TAURI_INTERNALS__: {invoke},
      addEventListener: (name, cb) => listeners.set(name, cb),
      dispatchEvent: event => listeners.get(event.type)?.(event),
    },
    Event: class { constructor(type) { this.type = type; } },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options?.detail; } },
    ResizeObserver: class { observe() {} },
    setTimeout: (fn, delay) => { const id = ++nextTimer; timers.set(id, {fn, delay}); return id; },
    clearTimeout: id => timers.delete(id), Date,
  });
  vm.runInContext(fs.readFileSync(require.resolve('../desktop-ui/provider-view.js'), 'utf8'), context);
  return {element, dispatch: (type, detail) => listeners.get(type)?.({type, detail}),
    listen: (type, callback) => listeners.set(type, callback),
    async runTimer(delay) { const item = [...timers].find(([, timer]) => timer.delay === delay); if (!item) return false; timers.delete(item[0]); await item[1].fn(); return true; },
    flush: () => new Promise(resolve => setImmediate(resolve))};
}

test('ChatGPT opens on demand within the main layout and hides before returning to library', async () => {
  const calls = [];
  const ui = boot(async (command, args) => { calls.push({command, args}); return {state: args.visible ? 'ready' : 'hidden'}; });
  assert.equal(calls.length, 0, 'startup must not open a provider popup');
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.equal(calls[0].command, 'open_provider_conversation');
  assert.equal(calls[0].args.conversationId, null);
  assert.equal(calls[1].command, 'set_provider_view');
  assert.equal(calls[1].args.visible, true);
  assert.equal(calls[1].args.x, 200);
  assert.equal(calls[1].args.width, 900);
  assert.equal(ui.element('library-reader').hidden, true);
  assert.equal(ui.element('provider-shell').hidden, false);
  assert.match(ui.element('provider-view-status').textContent, /ChatGPT is open/);
  ui.element('provider-back').click(); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'set_provider_view').at(-1).args.visible, false);
  assert.equal(ui.element('library-reader').hidden, false);
  assert.equal(ui.element('provider-shell').hidden, true);
});

test('saved chat targets its conversation, preserves it across local reading and returns to the same saved tab', async () => {
  const calls = [], ui = boot(async (command, args) => {
    calls.push({command, args}); return {state: args.visible ? 'ready' : 'hidden'};
  });
  const detail = {conversationId: '123e4567-e89b-12d3-a456-426614174000', title: 'Saved conversation', localId: 'local:1', leafNodeId: 'alternate-reply'};
  ui.dispatch('chat-hub:show-live', detail); await ui.flush();
  assert.equal(calls[0].command, 'open_provider_conversation');
  assert.equal(calls[0].args.conversationId, detail.conversationId);
  assert.equal(ui.element('provider-title').textContent, detail.title);
  assert.equal(ui.element('provider-back').textContent, 'Read saved copy');
  assert.match(ui.element('provider-view-status').textContent, /same account/);
  ui.dispatch('chat-hub:show-local'); await ui.flush();
  ui.dispatch('chat-hub:show-live', detail); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'open_provider_conversation').length, 1, 'returning to the same live chat preserves its webview');
  let returned;
  ui.listen('chat-hub:read-saved', event => { returned = event.detail; });
  ui.element('provider-back').click(); await ui.flush();
  assert.equal(returned.localId, detail.localId);
  assert.equal(returned.leafNodeId, detail.leafNodeId);
  assert.equal(ui.element('library-reader').hidden, false);
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'open_provider_conversation').at(-1).args.conversationId, null);
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'open_provider_conversation').length, 3, 'explicit New conversation always opens a new draft');
});

test('failed provider creation offers recovery and keeps the library reachable', async () => {
  const ui = boot(async () => { throw new Error('provider unavailable'); });
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.match(ui.element('provider-view-status').textContent, /could not open.*Reload ChatGPT/);
  ui.element('provider-back').click(); await ui.flush();
  assert.equal(ui.element('library-reader').hidden, false);
});

test('Settings hides the native chat and restores it without navigating or losing the draft', async () => {
  const calls = [], ui = boot(async (command, args) => {
    calls.push({command, args});
    return {state: args?.visible ? 'ready' : 'hidden'};
  });
  ui.element('show-chatgpt').click(); await ui.flush();
  ui.dispatch('chat-hub:workspace-settings', {open: true}); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'set_provider_view').at(-1).args.visible, false);
  ui.dispatch('chat-hub:workspace-settings', {open: false}); await ui.flush();
  assert.equal(calls.filter(c => c.command === 'set_provider_view').at(-1).args.visible, true);
  assert.equal(calls.filter(c => c.command === 'open_provider_conversation').length, 1);
});

test('Connect ChatGPT opens account sign-in without creating a conversation tab', async () => {
  const calls = [], ui = boot(async (command, args) => {
    calls.push({command, args});
    if (command === 'get_chatgpt_connection') return {state: 'signed_out'};
    return {state: args.visible ? 'ready' : 'hidden'};
  });
  let drafts = 0;
  ui.listen('chat-hub:live-open', () => { drafts++; });
  ui.element('connect-chatgpt').click(); await ui.flush();
  assert.equal(drafts, 0);
  assert.equal(ui.element('provider-title').textContent, 'Connect ChatGPT');
  assert.match(ui.element('provider-view-status').textContent, /Sign in.*fetch your history/);
  assert.equal(calls.filter(c => c.command === 'open_provider_conversation').length, 1);
});

test('visible ChatGPT view polls its guarded native URL status and dispatches a conversation UUID once', async () => {
  let currentId = null; const received = [];
  const ui = boot(async (command, args) => {
    if (command === 'get_chatgpt_connection') return {state: 'connected'};
    if (command === 'set_provider_view') return {state: args.visible ? 'ready' : 'hidden', conversation_id: currentId};
    return {};
  });
  ui.listen('chat-hub:provider-conversation', event => received.push(event.detail.conversationId));
  ui.element('show-chatgpt').click(); await ui.flush();
  currentId = '123e4567-e89b-12d3-a456-426614174000';
  assert.equal(await ui.runTimer(3000), true);
  assert.deepEqual(received, [currentId]);
  assert.equal(await ui.runTimer(3000), true);
  assert.deepEqual(received, [currentId], 'unchanged location is reported only once');
});
