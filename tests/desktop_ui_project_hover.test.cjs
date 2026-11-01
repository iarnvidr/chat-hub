const test = require('node:test');
const assert = require('node:assert/strict');

class Element {
  constructor(tag, doc) {
    this.tagName = tag.toUpperCase(); this.doc = doc; this.children = []; this.attributes = {};
    this.dataset = {}; this.style = {}; this.events = {}; this.hidden = false; this.disabled = false;
    this.className = ''; this.value = ''; this.textContent = '';
  }
  get classList() { return {contains: name => this.className.split(' ').includes(name)}; }
  get isConnected() { return this === this.doc.body || Boolean(this.parent?.isConnected); }
  get childElementCount() { return this.children.length; }
  append(...children) { for (const child of children) { child.parent = this; this.children.push(child); } }
  replaceChildren(...children) { for (const child of this.children) child.parent = null; this.children = []; this.append(...children); }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); this.parent = null; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  addEventListener(type, handler) { (this.events[type] ??= []).push(handler); }
  async fire(type, extra = {}) { const event = {type, target: this, preventDefault() { this.prevented = true; }, ...extra}; for (const handler of this.events[type] || []) await handler(event); return event; }
  focus() { this.doc.activeElement = this; }
  contains(target) { return target === this || this.children.some(child => child.contains(target)); }
  matches(selector) {
    if (selector === 'button:not(:disabled)') return this.tagName === 'BUTTON' && !this.disabled;
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    return this.tagName === selector.toUpperCase();
  }
  querySelectorAll(selector) {
    const options = selector.split(',').map(value => value.trim());
    return this.children.flatMap(child => [
      ...(options.some(option => child.matches(option)) ? [child] : []),
      ...child.querySelectorAll(selector),
    ]);
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  getBoundingClientRect() { return {left: 200, right: 320, top: 80, bottom: 110, height: 240, width: 120}; }
}

function environment() {
  const doc = {
    events: {},
    createElement(tag) { return new Element(tag, this); },
    addEventListener(type, handler) { (this.events[type] ??= []).push(handler); },
    getElementById(id) {
      const visit = element => element.id === id ? element : element.children.map(visit).find(Boolean) || null;
      return visit(this.body);
    },
  };
  doc.body = new Element('body', doc);
  const status = new Element('div', doc); status.id = 'app-status'; doc.body.append(status);
  const win = {innerWidth: 1000, innerHeight: 700, events: {}, addEventListener(type, handler) { this.events[type] = handler; }};
  return {doc, win, status};
}

const chat = {id: 'local-conversation-1', title: 'Village research', project: null};
async function setup(invoke) {
  const {createProjectHoverPicker} = await import('../desktop-ui/project-hover.js');
  const {doc, win, status} = environment();
  const anchor = doc.createElement('button'); anchor.id = 'set-chat-project'; doc.body.append(anchor);
  let changed = 0;
  const picker = createProjectHoverPicker({invoke, doc, win, onChanged: () => changed++});
  return {picker, doc, win, status, anchor, get changed() { return changed; }, menu: picker.menu};
}

test('reader picker searches ChatGPT projects and only sends verified upstream assignments', async () => {
  const calls = [];
  const ui = await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'list_projects') return {projects: [
      {project_key: 'project:village', title: 'Village research'},
      {project_key: 'project:other', title: 'Other work'},
    ]};
    return {scope: 'upstream', verified: true, library_revision: 8};
  });
  await ui.picker.open(chat, ui.anchor);
  assert.equal(ui.menu.hidden, false);
  assert.equal(ui.anchor.getAttribute('aria-expanded'), 'true');
  assert.equal(ui.menu.querySelectorAll('button').length, 3);
  assert.equal(ui.menu.querySelector('input').placeholder, 'Find a project…');
  assert.equal(ui.menu.querySelectorAll('select').length, 0);

  const buttons = ui.menu.querySelectorAll('button');
  await buttons[1].fire('click');
  assert.equal(ui.changed, 1);
  assert.equal(ui.menu.hidden, true);
  assert.deepEqual(calls.at(-1), ['set_conversation_project', {
    conversationId: chat.id, projectKey: 'project:village', scope: 'upstream',
  }]);
});

test('No project clears the ChatGPT assignment through the same upstream path', async () => {
  const calls = [];
  const ui = await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'list_projects') return {projects: []};
    return {scope: 'upstream', verified: true, library_revision: 9};
  });
  await ui.picker.open(chat, ui.anchor);
  await ui.menu.querySelector('button').fire('click');
  assert.deepEqual(calls.at(-1), ['set_conversation_project', {
    conversationId: chat.id, projectKey: null, scope: 'upstream',
  }]);
  assert.equal(ui.changed, 1);
});

test('assignment errors identify the failing ChatGPT step and invalid IDs are not reported as name errors', async () => {
  const ui = await setup(async command => {
    if (command === 'list_projects') return {projects: [{project_key: 'project:village', title: 'Village research'}]};
    throw Error('invalid_request');
  });
  await ui.picker.open(chat, ui.anchor);
  await ui.menu.querySelectorAll('button')[1].fire('click');
  assert.match(ui.menu.querySelector('.thread-menu-status').textContent, /conversation or project could not be validated/i);
  assert.doesNotMatch(ui.menu.querySelector('.thread-menu-status').textContent, /project name|120 characters/i);
  assert.equal(ui.changed, 0);
});

test('unavailable ChatGPT project list is stated explicitly and can be retried', async () => {
  const ui = await setup(async () => { throw Error('project_catalog_unavailable'); });
  await ui.picker.open(chat, ui.anchor);
  assert.match(ui.menu.querySelector('.thread-menu-status').textContent, /could not load its project list/i);
  assert.equal(ui.changed, 0);
});

test('malformed project rows fail closed instead of becoming a clear-assignment action', async () => {
  const ui = await setup(async () => ({projects: [{title: 'Broken fixture row'}]}));
  await ui.picker.open(chat, ui.anchor);
  assert.match(ui.menu.querySelector('.thread-menu-status').textContent, /could not load its project list/i);
  assert.equal(ui.menu.querySelectorAll('button').length, 0);
  assert.equal(ui.changed, 0);
});

test('closing while project data is loading prevents a stale picker from reopening', async () => {
  let finish;
  const ui = await setup(() => new Promise(resolve => { finish = resolve; }));
  const request = ui.picker.open(chat, ui.anchor);
  ui.picker.close(false);
  finish({projects: []});
  await request;
  assert.equal(ui.menu.hidden, true);
  assert.equal(ui.menu.children.length, 0);
});
