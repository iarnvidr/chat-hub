const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const ROOT = path.resolve(__dirname, '..');

function loadModule(relativePath, names) {
  const source = fs.readFileSync(path.join(ROOT, relativePath), 'utf8').replaceAll('export function', 'function');
  const context = vm.createContext({globalThis: {}, Map, Set, Array, Object, String, Number});
  vm.runInContext(`${source}\nglobalThis.__exports = {${names.join(',')}};`, context);
  return vm.runInContext('globalThis.__exports', context);
}

class MockElement {
  constructor() { this.children = []; this.listeners = {}; this.hidden = false; this.attributes = {}; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(type, fn) { this.listeners[type] = fn; }
  replaceChildren(...children) { this.children = children; }
  append(...children) { this.children.push(...children); }
  focus() { this.focused = true; }
}

function setup() {
  const {createConversationTabs} = loadModule('desktop-ui/conversation-tabs.js', ['createConversationTabs']);
  const {installTabShortcuts} = loadModule('desktop-ui/tab-shortcuts.js', ['installTabShortcuts']);
  const target = {
    listeners: new Map(),
    addEventListener(type, fn) { this.listeners.set(type, fn); },
    removeEventListener(type) { this.listeners.delete(type); },
    dispatch(event) { this.listeners.get(event.type)?.(event); },
  };
  const selected = [], emptied = [];
  const tabs = createConversationTabs({
    container: new MockElement(), doc: {createElement: () => new MockElement()},
    onSelect: tab => selected.push(tab.id), onEmpty: () => emptied.push(true),
  });
  let newTabs = 0;
  const uninstall = installTabShortcuts({target, tabs, onNewTab: () => newTabs++});
  function key({key, code, metaKey = true, ctrlKey = false, shiftKey = false, altKey = false, isComposing = false}) {
    let prevented = false;
    target.dispatch({type: 'keydown', key, code, metaKey, ctrlKey, shiftKey, altKey, isComposing,
      defaultPrevented: false, preventDefault() { prevented = true; }});
    return prevented;
  }
  return {tabs, target, selected, emptied, key, get newTabs() { return newTabs; }, uninstall};
}

test('Cmd+W closes the active tab, chooses its right neighbor, and leaves the app open when empty', () => {
  const ui = setup();
  assert.equal(ui.key({key: 'w'}), true);
  assert.equal(ui.tabs.all().length, 0);
  assert.equal(ui.emptied.length, 0);
  assert.equal(ui.newTabs, 0);

  ui.tabs.open({id: 'one', title: 'One'});
  ui.tabs.open({id: 'two', title: 'Two'});
  ui.tabs.open({id: 'three', title: 'Three'});
  assert.equal(ui.key({key: 'w'}), true);
  assert.deepEqual(ui.tabs.all().map(tab => tab.id), ['one', 'two']);
  assert.equal(ui.tabs.selected().id, 'two'); // closing the rightmost tab selects its left neighbor
  assert.equal(ui.emptied.length, 0);

  ui.key({key: 'w'});
  ui.key({key: 'w'});
  assert.equal(ui.tabs.all().length, 0);
  assert.equal(ui.emptied.length, 1);
  assert.equal(ui.newTabs, 0);
});

test('Cmd+T opens a draft and bracket / number shortcuts navigate tabs', () => {
  const ui = setup();
  for (let i = 1; i <= 10; i++) ui.tabs.open({id: `tab-${i}`, title: `Tab ${i}`});
  assert.equal(ui.key({key: '1'}), true);
  assert.equal(ui.tabs.selected().id, 'tab-1');
  assert.equal(ui.key({key: '9'}), true);
  assert.equal(ui.tabs.selected().id, 'tab-10');
  assert.equal(ui.key({key: '{', code: 'BracketLeft', shiftKey: true}), true);
  assert.equal(ui.tabs.selected().id, 'tab-9');
  assert.equal(ui.key({key: '}', code: 'BracketRight', shiftKey: true}), true);
  assert.equal(ui.tabs.selected().id, 'tab-10');
  assert.equal(ui.key({key: 't'}), true);
  assert.equal(ui.newTabs, 1);
});

test('native Close Tab event and Ctrl+W use the same no-tab-safe behavior', () => {
  const ui = setup();
  ui.target.dispatch({type: 'chat-hub:menu-close-tab'});
  assert.equal(ui.emptied.length, 0);
  ui.tabs.open({id: 'saved', title: 'Saved'});
  assert.equal(ui.key({key: 'w', metaKey: false, ctrlKey: true}), true);
  assert.equal(ui.tabs.all().length, 0);
  assert.equal(ui.emptied.length, 1);
  ui.target.dispatch({type: 'chat-hub:menu-close-tab'});
  assert.equal(ui.emptied.length, 1);
});

test('native menu actions reach tab navigation while a child webview has focus', () => {
  const ui = setup();
  ui.target.dispatch({type: 'chat-hub:menu-new-tab'});
  assert.equal(ui.newTabs, 1);
  for (let i = 1; i <= 3; i++) ui.tabs.open({id: `tab-${i}`, title: `Tab ${i}`});
  ui.target.dispatch({type: 'chat-hub:menu-previous-tab'});
  assert.equal(ui.tabs.selected().id, 'tab-2');
  ui.target.dispatch({type: 'chat-hub:menu-next-tab'});
  assert.equal(ui.tabs.selected().id, 'tab-3');
  ui.target.dispatch({type: 'chat-hub:menu-select-tab', detail: 1});
  assert.equal(ui.tabs.selected().id, 'tab-1');
  ui.target.dispatch({type: 'chat-hub:menu-select-tab', detail: 9});
  assert.equal(ui.tabs.selected().id, 'tab-3');
  ui.target.dispatch({type: 'chat-hub:menu-select-tab', detail: 0});
  assert.equal(ui.tabs.selected().id, 'tab-3');
});

test('ordinary editing shortcuts and modified/composition keys are not intercepted', () => {
  const ui = setup();
  for (const key of ['a', 'c', 'v', 'x', 'z', 'f']) {
    assert.equal(ui.key({key}), false, `Cmd+${key} should remain an editing/search shortcut`);
  }
  assert.equal(ui.key({key: 'w', shiftKey: true}), false);
  assert.equal(ui.key({key: 't', altKey: true}), false);
  assert.equal(ui.key({key: 't', isComposing: true}), false);
  ui.uninstall();
  assert.equal(ui.target.listeners.size, 0);
});
