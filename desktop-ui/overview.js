import {prepareConnections, messageGraph} from './graph-model.js';
import {createGraphView} from './graph-view.js';

const $ = id => document.getElementById(id);
const panel = $('library-overview');
let request = 0, rebuilding, refreshTimer, graph, messageDetail, focusId;
const native = () => typeof window.__TAURI_INTERNALS__?.invoke === 'function';
const invoke = (command, args = {}) => window.ChatHub.invoke(command, args);
const filters = () => ({source: $('history-source').value, projectKey: $('project-filter').value || null});
const element = (tag, text, cls) => {
  const node = document.createElement(tag); if (text !== undefined) node.textContent = text;
  if (cls) node.className = cls; return node;
};
const view = createGraphView($('graph-canvas'), (node, open) => {
  if (open) return openNode(node);
  showSelection(node);
}, showEdge);
function state(text) { $('overview-state').textContent = text; $('catalog-status').textContent = text; }
function openChat(id, leafNodeId, anchorNodeId) {
  window.dispatchEvent(new CustomEvent('chat-hub:open-conversation', {detail: {id, leafNodeId, anchorNodeId}}));
}
function openNode(node) {
  if (node.kind === 'chat') openChat(node.conversation_id);
  else if (node.message_id) openChat(messageDetail.conversation.id, node.leaf_node_id, node.id);
}
function leave() {
  document.body.classList.remove('overview-open'); panel.hidden = true;
  $('show-overview').setAttribute('aria-pressed', 'false');
}
function enter() {
  window.dispatchEvent(new Event('chat-hub:show-local'));
  document.body.classList.add('overview-open', 'mobile-reader'); panel.hidden = false;
  $('show-overview').setAttribute('aria-pressed', 'true');
}
function showOverview(focus = true) {
  focusId = null; messageDetail = null; panel.classList.remove('message-mode', 'connections-mode', 'selection-open');
  $('overview-title').textContent = 'Library map';
  $('overview-back').hidden = $('overview-read-chat').hidden = true;
  $('overview-stats').hidden = false;
  $('overview-recent-title').textContent = 'Chats in this map';
  enter(); refresh(true); if (focus) panel.focus();
}
async function focusChat(id) {
  focusId = id; messageDetail = null; panel.classList.remove('message-mode'); panel.classList.add('connections-mode');
  $('overview-title').textContent = 'Chat connections';
  $('overview-stats').hidden = false; $('overview-back').hidden = false;
  $('overview-read-chat').hidden = true;
  enter(); await refresh(true);
}
function renderStats(stats) {
  const root = $('overview-stats'); root.replaceChildren();
  root.textContent = `${Number(stats.chats || 0).toLocaleString()} chats · ${Number(stats.projects || 0).toLocaleString()} projects`;
}
async function rebuild(manual = false) {
  if (rebuilding) return rebuilding;
  if (!native()) { state('Open the desktop app to index your saved chats.'); return; }
  $('rebuild-catalog').disabled = true;
  state('Indexing saved chats locally… You can keep reading while this runs.');
  rebuilding = invoke('rebuild_catalog_index').then(() => {
    state('Chat index is ready.');
    window.dispatchEvent(new Event('chat-hub:catalog-indexed'));
  }).catch(() => {
    state('The index could not rebuild. Your saved chats are available; retry in Settings.');
  }).finally(() => { rebuilding = null; $('rebuild-catalog').disabled = false; });
  await rebuilding;
  if (manual && !messageDetail) await refresh(false);
}
async function refresh(allowRebuild = false) {
  if (!native() || messageDetail) return;
  const token = ++request, args = filters(), requestedFocus = focusId;
  const project = $('project-filter').selectedOptions?.[0]?.textContent || 'All projects';
  $('overview-scope').textContent = `${args.source === 'codex' ? 'Codex' : 'Chat'} · ${project}`;
  try {
    let overview = await invoke('catalog_overview', args);
    if (token !== request || messageDetail) return;
    if (!overview?.index || !overview?.stats) throw Error('invalid_catalog');
    if (overview.index.stale && allowRebuild) {
      await rebuild();
      if (token !== request || messageDetail) return;
      overview = await invoke('catalog_overview', args);
    }
    const data = await invoke('catalog_graph', {...args, focusId:requestedFocus, limit:100});
    if (token !== request || messageDetail) return;
    if (!Array.isArray(data?.nodes) || !Array.isArray(data?.edges)) throw Error('invalid_catalog_graph');
    graph = prepareConnections(data); renderStats(overview.stats); renderGraph();
    $('overview-recent-title').textContent = `${graph.nodes.length} chats in this map`;
    renderList(graph.nodes.filter(node => node.kind === 'chat'));
    const focused = graph.nodes.find(node => node.id === graph.focus_id);
    if (focused) showSelection(focused);
    $('graph-caption').textContent = requestedFocus
      ? `${Math.max(0, graph.nodes.length - 1)} related chats · Select a line for its evidence. Back returns to the library map.`
      : `${graph.nodes.length} chats · ${graph.edges.length} connections · ${graph.clusters} clusters${graph.unmapped_count ? ` · ${graph.unmapped_count} other chats in the sidebar` : ''}. Select to inspect; double-click to open.`;
    state(overview.index.stale ? 'Index needs refreshing. Rebuild it in Settings.' : '');
  } catch (error) {
    console.error('Chat map could not render:', String(error?.message || error).slice(0,180));
    if (token === request) state('Connections could not load. Your saved chats remain available in the sidebar.');
  }
}
function renderList(nodes) {
  const root = $('overview-recent'); root.replaceChildren();
  for (const node of nodes) {
    const button = element('button', undefined, 'overview-chat'); button.type = 'button';
    button.append(element('span', node.kind === 'message' ? `${node.label}: ${node.excerpt || 'Context'}` : node.label),
      element('small', 'Inspect'));
    button.addEventListener('click', () => view.select(node.id)); root.append(button);
  }
}
function renderGraph() {
  panel.classList.remove('selection-open');
  $('map-search').value='';$('map-search-results').hidden=true;
  $('graph-selection').replaceChildren(element('h2', messageDetail ? 'Explore a message' : 'Explore a chat'),
    element('p', messageDetail ? 'Select a message to read it on its original branch.' : 'Select a chat to find related conversations. Select a line to see the connection.'));
  view.render(graph, Boolean(messageDetail));
}
function appendAction(root, text, action) {
  const button = element('button', text); button.type = 'button'; button.addEventListener('click', action); root.append(button);
}
function showEdge(edge) {
  panel.classList.add('selection-open');
  const root = $('graph-selection'); root.replaceChildren(element('h2', edge.basis === 'project' ? 'Same project' : edge.basis === 'parent' ? 'Message sequence' : 'Shared topics'));
  const names = [edge.source, edge.target].map(id => graph.nodes.find(node => node.id === id)?.label || 'Chat');
  root.append(element('p', names.join(' ↔ ')), element('p', edge.reason || 'Original stored parent relationship.'));
  if (edge.evidence?.length) root.append(element('p', `Evidence: ${edge.evidence.join(', ')}`));
  for (const id of [edge.source, edge.target]) {
    const node = graph.nodes.find(node => node.id === id);
    if (node) appendAction(root, `Open ${node.label}`, () => openNode(node));
  }
}
function showSelection(node) {
  panel.classList.add('selection-open');
  const root = $('graph-selection'); root.replaceChildren(element('h2', node.label));
  if (node.kind === 'chat') {
    root.append(element('p', node.cached_archive ? 'Saved transcript. Original message boundaries and branches are unavailable.' : `${node.message_count || 0} stored messages · ${node.branch_count || 0} branch points`));
    appendAction(root, 'Open chat', () => openNode(node));
    if (node.conversation_id !== focusId) appendAction(root, 'Explore all connections', () => focusChat(node.conversation_id));
    appendAction(root, 'Message graph', async () => {
      try { showMessages(await invoke('get_conversation', {id:node.conversation_id})); }
      catch { state('This message graph could not open. Try reading the saved conversation.'); }
    });
    const connections = graph.edges.filter(edge => edge.source === node.id || edge.target === node.id);
    if (!connections.length) root.append(element('p', 'No strong connections found in this scope. Try All projects, or explore another chat.'));
    for (const edge of connections) {
      const other = graph.nodes.find(item => item.id === (edge.source === node.id ? edge.target : edge.source));
      if (!other) continue;
      const entry = element('div', undefined, 'connection-evidence');
      appendAction(entry, other.label, () => view.select(other.id));
      entry.append(element('p', edge.reason || 'Same project'));
      if (edge.basis === 'shared_keyword' && edge.evidence?.length) entry.append(element('p', edge.evidence.join(' · ')));
      root.append(entry);
    }
  } else if (node.kind === 'message') {
    root.append(element('p', node.excerpt || 'Context node without message text.'));
    if (node.message_id) appendAction(root, 'Open message', () => openNode(node));
  } else if (node.kind === 'project') {
    appendAction(root, 'Filter this project', () => {
      $('project-filter').value = node.project_key; $('project-filter').dispatchEvent(new Event('change', {bubbles:true}));
    });
  }
}
function showMessages(detail) {
  if (!detail?.conversation?.id) return;
  messageDetail = detail; ++request; enter(); panel.classList.remove('connections-mode'); panel.classList.add('message-mode');
  $('overview-title').textContent = detail.conversation.title || 'Message graph';
  $('overview-scope').textContent = 'Message graph · original stored relationships';
  $('overview-back').hidden = $('overview-read-chat').hidden = false;
  $('overview-stats').hidden = true;
  graph = messageGraph(detail); renderGraph(); renderList(graph.nodes); state('');
  $('overview-recent-title').textContent = 'Messages in this view';
  $('graph-caption').textContent = graph.archived
    ? 'This import contains one transcript. Original message boundaries and alternate branches are unavailable.'
    : `${graph.nodes.length} stored nodes${graph.truncated ? ' shown (view limit)' : ''}. Arrows follow original parent links; the selected branch is highlighted.`;
}
$('show-overview').addEventListener('click', () => showOverview());
$('overview-back').addEventListener('click', () => showOverview());
$('overview-read-chat').addEventListener('click', () => openChat(messageDetail?.conversation?.id));
$('rebuild-catalog').addEventListener('click', () => rebuild(true));
$('graph-zoom-in').addEventListener('click', () => view.zoom(1.25));
$('graph-zoom-out').addEventListener('click', () => view.zoom(.8));
$('graph-reset').addEventListener('click', () => view.fit());
$('graph-inspector-close').addEventListener('click', () => panel.classList.remove('selection-open'));
$('map-search').addEventListener('input', () => {
  const results=$('map-search-results');results.replaceChildren();
  const query=$('map-search').value.trim();const hits=view.search(query);results.hidden=!query;
  for(const node of hits) appendAction(results,node.label,()=>{view.select(node.id);results.hidden=true;});
  if(query&&!hits.length)results.append(element('p','No match in this map. Search all chats in the sidebar.'));
});
$('map-search').addEventListener('keydown',event=>{
  if(event.key==='Escape'){$('map-search-results').hidden=true;event.target.value='';view.search('');}
  if(event.key==='Enter'){const first=$('map-search-results').querySelector('button');if(first){event.preventDefault();first.click();}}
});
for (const event of ['chat-hub:show-local','chat-hub:show-live','chat-hub:live-open','chat-hub:connect']) window.addEventListener(event, leave);
window.addEventListener('chat-hub:show-message-graph', event => showMessages(event.detail));
window.addEventListener('chat-hub:show-chat-connections', event => focusChat(event.detail.id));
window.addEventListener('chat-hub:history-updated', () => {
  clearTimeout(refreshTimer); refreshTimer = setTimeout(() => {if (!panel.hidden) refresh(true);}, 750);
});
for (const id of ['source-chat','source-codex']) $(id).addEventListener('click', () => {if (!panel.hidden) showOverview(false);});
$('project-filter').addEventListener('change', () => {if (!panel.hidden) showOverview(false);});
if (native()) showOverview(false);
