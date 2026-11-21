'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const script = fs.readFileSync(path.join(__dirname, '../desktop-ui/auth.js'), 'utf8')
  .replace(/^import \{httpBridge\} from '\.\/data\.js';\n/, '');

class Element {
  constructor() {
    this.hidden = false;
    this.disabled = false;
    this.value = '';
    this.textContent = '';
    this.listeners = new Map();
    this.dataset = {};
    this.focused = false;
  }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  querySelector(selector) {
    if (selector === 'button[type="submit"]') return this.submit;
    throw new Error('Unexpected element selector: ' + selector);
  }
  focus() { this.focused = true; }
}

async function setup(capabilities, authMode = 'authelia') {
  const nodes = new Map();
  const el = key => {
    if (!nodes.has(key)) nodes.set(key, new Element());
    return nodes.get(key);
  };
  const form = el('server-login-form');
  form.submit = new Element();
  const gate = el('server-login');
  const reconnect = el('server-reconnect');
  const token = el('server-token');
  reconnect.hidden = true;
  gate.hidden = true;
  el('server-logout').hidden = true;
  const skip = new Element();
  let reloads = 0;
  let tokenLogins = 0;
  const bridge = {
    capabilities: async () => {
      if (capabilities instanceof Error) throw capabilities;
      return capabilities;
    },
    login: async () => { tokenLogins++; },
    logout: async () => {},
  };
  const context = {
    httpBridge: () => bridge,
    document: {
      getElementById: el,
      querySelector: selector => selector === '.skip-link' ? skip
        : selector === 'meta[name="chat-hub-auth-mode"]' ? {content:authMode} : null,
      querySelectorAll: () => [],
    },
    window: {
      ChatHub: {transport:'http'},
      dispatchEvent: () => {},
      location: {reload: () => { reloads++; }},
    },
    navigator: {
      serviceWorker: {getRegistrations: async () => []},
    },
    Event: class Event { constructor(type) { this.type = type; } },
  };
  vm.runInNewContext(script, context, {filename:'desktop-ui/auth.js'});
  await new Promise(resolve => setImmediate(resolve));
  return {gate, form, reconnect, token, skip, el, tokenLogins: () => tokenLogins,
    reloads: () => reloads};
}

test('Authelia-authenticated owner opens library with no extra token form', async () => {
  const h = await setup({authenticated:true,login_required:false,
    capabilities:{read:true,provider_view:false},dev_mode:true});
  assert.equal(h.gate.hidden, true);
  assert.equal(h.el('server-logout').hidden, true);
  assert.equal(h.skip.hidden, false);
  assert.equal(h.tokenLogins(), 0);
});

test('expired Authelia or failed proxy check offers reconnect, never token login', async () => {
  for (const status of [401, 403, 502, undefined]) {
    const error = Object.assign(new Error('server unavailable'), {status});
    const h = await setup(error);
    assert.equal(h.gate.hidden, false, 'gate must explain unavailable connection');
    assert.equal(h.form.hidden, true, 'token form must stay hidden behind Authelia');
    assert.equal(h.reconnect.hidden, false);
    assert.equal(h.token.focused, false);
    assert.equal(h.tokenLogins(), 0);
    h.el('server-reconnect-button').listeners.get('click')();
    assert.equal(h.reloads(), 1, 'full reload allows Authelia redirect');
  }
});

test('standalone token-auth development still uses its own login flow', async () => {
  const error = Object.assign(new Error('missing developer session'), {status:401});
  const h = await setup(error, 'token');
  assert.equal(h.gate.hidden, false);
  assert.equal(h.form.hidden, false);
  assert.equal(h.reconnect.hidden, true);
  assert.equal(h.token.focused, true);
});
