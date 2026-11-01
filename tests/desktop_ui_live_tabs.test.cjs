const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const ROOT = require('node:path').resolve(__dirname, '..');
const UUID = '123e4567-e89b-12d3-a456-426614174000';
const SCOPE = 'chatgpt:observed:' + 'a'.repeat(64);

function bootApp({records = () => [], detail = id => ({conversation: {id, title: 'Saved chat', provider: 'chatgpt', account_scope: SCOPE, source_id: UUID}, selection: {selected_node_id: 'branch-a'}})} = {}) {
  const elements = new Map(), listeners = new Map(), pending = [];
  class Element {
    constructor(id = '') {
      this.id = id; this.hidden = false; this.textContent = ''; this.value = id === 'history-source' ? 'chatgpt' : '';
      this.disabled = false; this.children = []; this.attributes = {}; this.events = {};
      const classes = new Set();
      this.classList = {add: x => classes.add(x), remove: x => classes.delete(x), contains: x => classes.has(x)};
    }
    addEventListener(name, fn) { this.events[name] = fn; }
    setAttribute(name, value) { this.attributes[name] = value; }
    getAttribute(name) { return this.attributes[name] ?? null; }
    removeAttribute(name) { delete this.attributes[name]; }
    replaceChildren(...children) { this.children = children; }
    append(...children) { this.children.push(...children); }
    closest() { return {hidden: false}; }
    focus() {}
    scrollIntoView() {}
    querySelector() { return null; }
    querySelectorAll() { return []; }
  }
  const getElementById = id => {
    if (!elements.has(id)) elements.set(id, new Element(id));
    return elements.get(id);
  };
  const invoke = async (command, args) => {
    if (command === 'list_conversations') return {items: records(), next_cursor: null, library_revision: 7};
    if (command === 'get_conversation') return detail(args.id);
    if (command === 'get_chatgpt_connection') return {state:'connected',account_scope:SCOPE};
    return {};
  };
  const pendingRows=[];
  const createPendingConversationStore=()=>({
    list:()=>pendingRows.map(row=>({...row})),get:id=>{const row=pendingRows.find(item=>item.pending_id===id);return row?{...row}:null;},
    createDraft:(pending_id,created_at_ms=Date.now())=>{let row=pendingRows.find(item=>item.pending_id===pending_id);if(!row){row={pending_id,source_id:null,account_scope:null,created_at_ms};pendingRows.push(row);}return {...row};},
    attachProviderConversation:(pending_id,source_id,account_scope)=>{let row=pendingRows.find(item=>item.pending_id===pending_id);if(!row){row={pending_id,created_at_ms:Date.now()};pendingRows.push(row);}Object.assign(row,{source_id,account_scope});return {...row};},
    setAccountScope:(pending_id,account_scope)=>{const row=pendingRows.find(item=>item.pending_id===pending_id);if(row){row.account_scope=account_scope;return true;}return false;},
    removeUnstartedDraft:pending_id=>{const i=pendingRows.findIndex(row=>row.pending_id===pending_id&&!row.source_id);if(i<0)return false;pendingRows.splice(i,1);return true;},
    reconcileCaptured:items=>{const before=pendingRows.length;for(let i=pendingRows.length-1;i>=0;i--){const row=pendingRows[i];if(row.source_id&&row.account_scope&&items.filter(item=>item.provider==='chatgpt'&&item.source_id===row.source_id&&item.account_scope===row.account_scope).length===1)pendingRows.splice(i,1);}return before-pendingRows.length;},
  });
  const window = {
    __TAURI_INTERNALS__: {invoke},
    addEventListener(name, fn) { listeners.set(name, fn); },
    dispatchEvent(event) {
      const result = listeners.get(event.type)?.(event);
      if (result?.then) pending.push(result);
      return true;
    },
  };
  class TestEvent { constructor(type) { this.type = type; } }
  class TestCustomEvent extends TestEvent { constructor(type, options = {}) { super(type); this.detail = options.detail; } }
  const context = vm.createContext({
    window, document: {body: getElementById('body'), activeElement: null, getElementById,
      querySelectorAll: () => [], createElement: () => new Element()},
    location: {hostname: 'chat-hub.local', search: ''}, URLSearchParams,
    Event: TestEvent, CustomEvent: TestCustomEvent, Date, Promise, Map, Set, Array, Object, String,
    Number, Math, JSON, Error, setTimeout: () => 1, clearTimeout() {},
    fixtureAdapter: () => ({}), normalizeList: x => x, normalizeSearch: x => x,
    chatgptConversationId: c => c?.source_id || null,
    appendPinMarker: (title, pinned) => { if (pinned) title.append(new Element('pin-marker')); },
    branchChoices: () => [], orderedMessages: () => [], messageText: () => '',
    hasUnsupportedContent: () => false, renderMarkdown: () => {},
    updateStatus: async () => {},
    createRowActions: () => ({wrap: (chat, button) => button, close() {}}),
    createPendingConversationStore,pendingConversationItem:row=>({id:`pending:${row.pending_id}`,pending_id:row.pending_id,source_id:row.source_id||null,account_scope:row.account_scope||null,provider:'chatgpt-pending',title:row.source_id?'ChatGPT conversation':'New ChatGPT conversation',updated_at_ms:row.created_at_ms,pending:true}),
  });
  const tabs = fs.readFileSync(`${ROOT}/desktop-ui/conversation-tabs.js`, 'utf8')
    .replaceAll('export function', 'function');
  const shortcuts = fs.readFileSync(`${ROOT}/desktop-ui/tab-shortcuts.js`, 'utf8')
    .replaceAll('export function', 'function');
  const app = fs.readFileSync(`${ROOT}/desktop-ui/app.js`, 'utf8')
    .replace(/^import .*;\s*$/gm, '') + '\nglobalThis.__tabs = conversationTabs;';
  vm.runInContext(`${tabs}\n${shortcuts}\n${app}`, context);
  return {
    tabs: context.__tabs, elements, window,
    async flush() {
      for (let i = 0; i < 10; i++) {
        await Promise.resolve();
        const active = pending.splice(0);
        if (active.length) await Promise.all(active);
        await new Promise(resolve => setImmediate(resolve));
      }
    },
    dispatch(type, detail) { window.dispatchEvent({type, detail}); },
  };
}

test('a created draft rekeys to its provider UUID and New keeps the prior live tab', async () => {
  const ui = bootApp();
  await ui.flush();
  ui.dispatch('chat-hub:live-open');
  const draft = ui.tabs.selected();
  assert.match(draft.id, /^draft:chatgpt:/);
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  assert.deepEqual(Array.from(ui.tabs.all(), tab => tab.id), [`live:chatgpt:${UUID}`]);
  ui.dispatch('chat-hub:live-open');
  assert.equal(ui.tabs.all().length, 2);
  assert.equal(ui.tabs.all()[0].id, `live:chatgpt:${UUID}`);
  assert.match(ui.tabs.selected().id, /^draft:chatgpt:/);
});

test('a new ChatGPT draft appears in the sidebar immediately, resets filters, and upgrades without a duplicate',async()=>{
  let captured=false;const ui=bootApp({records:()=>captured?[{id:`chatgpt:${UUID}`,provider:'chatgpt',source_id:UUID,account_scope:SCOPE,title:'Captured chat'}]:[]});
  await ui.flush();
  ui.elements.get('history-source').value='codex';ui.elements.get('search').value='old search';
  ui.elements.get('project-filter').value='project:one';ui.elements.get('tag-filter').value='important';
  ui.elements.get('show-archived').setAttribute('aria-pressed','true');
  ui.dispatch('chat-hub:live-open');await ui.flush();
  assert.equal(ui.elements.get('history-source').value,'chatgpt');
  assert.equal(ui.elements.get('search').value,'');assert.equal(ui.elements.get('project-filter').value,'');
  assert.equal(ui.elements.get('tag-filter').value,'');assert.equal(ui.elements.get('show-archived').getAttribute('aria-pressed'),'false');
  let rows=ui.elements.get('conversation-list').children;
  assert.equal(rows.length,1);assert.match(rows[0].className,/pending-conversation/);
  assert.match(rows[0].children[1].textContent,/Not saved to library yet/);
  ui.dispatch('chat-hub:provider-conversation',{conversationId:UUID,accountScope:SCOPE});await ui.flush();
  rows=ui.elements.get('conversation-list').children;assert.equal(rows.length,1);assert.match(rows[0].className,/pending-conversation/);
  captured=true;ui.dispatch('chat-hub:history-updated');await ui.flush();
  rows=ui.elements.get('conversation-list').children;assert.equal(rows.length,1);assert.doesNotMatch(rows[0].className,/pending-conversation/);
  assert.equal(rows[0].children[0].textContent,'Captured chat');
});

test('closing an unsent blank draft removes its ephemeral sidebar row',async()=>{
  const ui=bootApp();await ui.flush();ui.dispatch('chat-hub:live-open');await ui.flush();
  assert.equal(ui.elements.get('conversation-list').children.length,1);
  assert.equal(ui.tabs.closeSelected(),true);await ui.flush();
  assert.equal(ui.elements.get('conversation-list').children.length,0);
});

test('capture refresh adopts one unambiguous account record without switching away from live ChatGPT', async () => {
  const localId = `chatgpt:${UUID}`;
  const ui = bootApp({records: () => [{id: localId, provider: 'chatgpt', source_id: UUID,
    account_scope: SCOPE, title: 'Captured title'}],
    detail: id => ({conversation: {id, title: 'Captured title', provider: 'chatgpt', account_scope: SCOPE, source_id: UUID}, selection: {selected_node_id: 'branch-a'}})});
  await ui.flush();
  const localFocusEvents = [];
  ui.window.addEventListener('chat-hub:show-local', event => localFocusEvents.push(event));
  ui.dispatch('chat-hub:live-open');
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  ui.dispatch('chat-hub:history-updated');
  await ui.flush();
  const selected = ui.tabs.selected();
  assert.equal(selected.mode, 'live');
  assert.equal(selected.id, localId);
  assert.equal(selected.localId, localId);
  assert.equal(selected.accountScope, SCOPE);
  assert.equal(selected.title, 'Captured title');
  assert.equal(localFocusEvents.length, 0);
});

test('unknown browser account never adopts even a single stored account record', async () => {
  const ui = bootApp({records: () => [{id: 'only-record', provider: 'chatgpt', source_id: UUID,
    account_scope: SCOPE, title: 'Other account chat'}]});
  await ui.flush();
  ui.dispatch('chat-hub:live-open');
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID});
  ui.dispatch('chat-hub:history-updated');
  await ui.flush();
  assert.equal(ui.tabs.selected().id, `live:chatgpt:${UUID}`);
  assert.equal(ui.tabs.selected().localId, null);
  assert.equal(ui.tabs.selected().accountScope, null);
});

test('verified scope adopts only its matching record when multiple accounts share a UUID', async () => {
  const ui = bootApp({records: () => [
    {id: 'a', provider: 'chatgpt', source_id: UUID, account_scope: SCOPE},
    {id: 'b', provider: 'chatgpt', source_id: UUID, account_scope: 'chatgpt:observed:' + 'b'.repeat(64)},
  ]});
  await ui.flush();
  ui.dispatch('chat-hub:live-open');
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  ui.dispatch('chat-hub:history-updated');
  await ui.flush();
  assert.equal(ui.tabs.selected().id, 'a');
  assert.equal(ui.tabs.selected().localId, 'a');
  assert.equal(ui.tabs.selected().accountScope, SCOPE);
  assert.equal(ui.tabs.all().length, 1);
});

test('account change preserves the old scoped live tab and creates a separate placeholder', async () => {
  const other = 'chatgpt:observed:' + 'b'.repeat(64);
  const ui = bootApp();
  await ui.flush();
  ui.dispatch('chat-hub:live-open');
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  const old = ui.tabs.selected();
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: other});
  assert.equal(ui.tabs.all().length, 2);
  assert.equal(ui.tabs.get(old.id).accountScope, SCOPE);
  assert.equal(ui.tabs.get(old.id).sourceConversationId, UUID);
  assert.equal(ui.tabs.selected().accountScope, other);
  assert.equal(ui.tabs.selected().localId, null);
});

test('continuing an imported copy binds its live session within the same conversation tab', async () => {
  const cached = {id: 'cached', provider: 'paseo-chatgpt', source_id: UUID,
    account_scope: 'paseo-cache:owner', title: 'Imported chat'};
  const ui = bootApp({records: () => [cached],
    detail: id => ({conversation: {...cached, id}, selection: {selected_node_id: 'cached-leaf'}})});
  await ui.flush();
  ui.elements.get('conversation-list').children[0].events.click(); await ui.flush();
  ui.elements.get('continue-chatgpt').events.click();
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  assert.equal(ui.tabs.all().length, 1);
  assert.equal(ui.tabs.selected().localId, 'cached');
  assert.equal(ui.tabs.selected().accountScope, SCOPE);
  assert.equal(cached.account_scope, 'paseo-cache:owner');
});

test('rekeying a captured Paseo transcript to a verified raw capture clears its stale branch', async () => {
  let refreshed = false;
  const paseo = {id: 'paseo:cached', provider: 'paseo-chatgpt', source_id: UUID,
    account_scope: 'paseo:owner:old', title: 'Cached copy'};
  const direct = {id: 'chatgpt:direct', provider: 'chatgpt', source_id: UUID,
    account_scope: SCOPE, title: 'Raw capture'};
  const ui = bootApp({records: () => refreshed ? [paseo, direct] : [paseo],
    detail: id => ({conversation: {id, title: 'Cached copy', provider: 'paseo-chatgpt',
      source_id: UUID, account_scope: 'paseo:owner:old'}, selection: {selected_node_id: 'cached-leaf'}})});
  await ui.flush();
  ui.elements.get('conversation-list').children[0].events.click();
  await ui.flush();
  ui.elements.get('continue-chatgpt').events.click();
  const cachedLiveTab = ui.tabs.selected();
  assert.equal(cachedLiveTab.localId, 'paseo:cached');
  assert.equal(cachedLiveTab.leafNodeId, 'cached-leaf');
  ui.dispatch('chat-hub:provider-conversation', {conversationId: UUID, accountScope: SCOPE});
  assert.equal(ui.tabs.selected().localId, 'paseo:cached');
  refreshed = true;
  ui.dispatch('chat-hub:history-updated');
  await ui.flush();
  const adopted = ui.tabs.selected();
  assert.equal(adopted.id, 'chatgpt:direct');
  assert.equal(adopted.localId, 'chatgpt:direct');
  assert.equal(adopted.leafNodeId, null);
  assert.equal(adopted.anchorNodeId, null);
});
