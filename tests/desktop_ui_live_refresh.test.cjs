const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

test('new captures refresh the list and reread the selected conversation', () => {
  let listener;
  let clicked = 0;
  let observer;
  const selected = { click: () => { clicked += 1; } };
  const list = { querySelector: () => selected };
  const search = { dispatchEvent: () => observer.callback() };
  class MutationObserver {
    constructor(callback) { this.callback = callback; observer = this; }
    observe() {}
    disconnect() {}
  }
  const context = vm.createContext({
    document: { getElementById: id => id === 'conversation-list' ? list : search },
    window: { addEventListener: (name, callback) => { if (name === 'chat-hub-capture-updated') listener = callback; } },
    MutationObserver,
    Event: class Event {},
    setTimeout: () => 1,
    clearTimeout: () => {},
  });
  vm.runInContext(fs.readFileSync(require.resolve('../desktop-ui/live-refresh.js'), 'utf8'), context);
  listener();
  assert.equal(clicked, 1);
});
