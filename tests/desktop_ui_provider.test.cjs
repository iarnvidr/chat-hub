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
  const context = vm.createContext({
    document: {getElementById: element, body: element('body')},
    window: {ChatHub: {invoke}, __TAURI_INTERNALS__: {invoke},
      addEventListener: (name, cb) => listeners.set(name, cb),
      dispatchEvent: event => listeners.get(event.type)?.(),
    },
    Event: class { constructor(type) { this.type = type; } },
    ResizeObserver: class { observe() {} }, setTimeout: () => 1, clearTimeout() {}, Date,
  });
  vm.runInContext(fs.readFileSync(require.resolve('../desktop-ui/provider-view.js'), 'utf8'), context);
  return {element, flush: () => new Promise(resolve => setImmediate(resolve))};
}

test('ChatGPT opens on demand within the main layout and hides before returning to library', async () => {
  const calls = [];
  const ui = boot(async (command, args) => { calls.push({command, args}); return {state: args.visible ? 'ready' : 'hidden'}; });
  assert.equal(calls.length, 0, 'startup must not open a provider popup');
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.equal(calls[0].command, 'set_provider_view');
  assert.equal(calls[0].args.visible, true);
  assert.equal(calls[0].args.x, 200);
  assert.equal(calls[0].args.width, 900);
  assert.equal(ui.element('library-reader').hidden, true);
  assert.equal(ui.element('provider-shell').hidden, false);
  assert.match(ui.element('provider-view-status').textContent, /ChatGPT is open/);
  ui.element('provider-back').click(); await ui.flush();
  assert.equal(calls[1].args.visible, false);
  assert.equal(ui.element('library-reader').hidden, false);
  assert.equal(ui.element('provider-shell').hidden, true);
});

test('failed provider creation offers recovery and keeps the library reachable', async () => {
  const ui = boot(async () => { throw new Error('provider unavailable'); });
  ui.element('show-chatgpt').click(); await ui.flush();
  assert.match(ui.element('provider-view-status').textContent, /could not open.*Reload ChatGPT/);
  ui.element('provider-back').click(); await ui.flush();
  assert.equal(ui.element('library-reader').hidden, false);
});
