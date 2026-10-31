const library = document.getElementById('library-reader');
const panel = document.getElementById('provider-shell');
const host = document.getElementById('provider-host');
const status = document.getElementById('provider-view-status');
const libraryButton = document.getElementById('show-library');
const providerButton = document.getElementById('show-chatgpt');
let showing = false, sequence = 0, queue = Promise.resolve(), poll, resize, connectionPoll, locationPoll, connectDeadline = 0;
let activeTarget, current = {conversationId: null, title: 'New conversation', localId: null};
let reportedConversationId = null;
let settingsOpen = false;

function native() {
  const invoke = window.ChatHub?.invoke;
  if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function' || typeof invoke !== 'function') {
    throw new Error('desktop_required');
  }
  return invoke;
}

async function update(reload = false) {
  const rect = host.getBoundingClientRect();
  const token = sequence;
  const result = await native()('set_provider_view', {
    visible: showing && !settingsOpen,
    x: Math.max(0, rect.x), y: Math.max(0, rect.y),
    width: Math.max(1, rect.width), height: Math.max(1, rect.height), reload,
  });
  if (showing && token === sequence) await observeConversationLocation(result, token);
  return result;
}

async function observeConversationLocation(result, token) {
  const id = result?.conversation_id;
  if (!showing || token !== sequence || typeof id !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id) ||
      id === reportedConversationId) return;
  reportedConversationId = id;
  let accountScope = null;
  try {
    const connection = await native()('get_chatgpt_connection', {});
    if (connection?.state === 'connected' &&
        typeof connection.account_scope === 'string' &&
        /^chatgpt:observed:[0-9a-f]{64}$/.test(connection.account_scope)) {
      accountScope = connection.account_scope;
    }
  } catch { /* Unknown identity stays unlinked to local history. */ }
  if (!showing || token !== sequence) return;
  window.dispatchEvent(new CustomEvent('chat-hub:provider-conversation', {detail: {conversationId: id, accountScope}}));
}

function watchConversationLocation(token) {
  clearTimeout(locationPoll);
  if (!showing || token !== sequence) return;
  locationPoll = setTimeout(async () => {
    if (!showing || token !== sequence) return;
    try { await update(); } catch { /* Location metadata is optional; keep the live view usable. */ }
    if (showing && token === sequence) watchConversationLocation(token);
  }, 3000);
}

async function report(result) {
  status.textContent = result?.state === 'ready'
    ? current.localId ? 'Continue this chat in ChatGPT. Use the same account as the saved conversation.'
      : 'ChatGPT is open. Sign in here to use your account.'
    : result?.message || 'Loading ChatGPT…';
  if (result?.state === 'ready') {
    const token = sequence;
    try {
      const connection = await native()('get_chatgpt_connection', {});
      if (!showing || token !== sequence) return;
      if (connection?.state === 'connected') {
        current.connecting = false;
        document.getElementById('connect-chatgpt').textContent = 'ChatGPT connected';
        status.textContent = current.localId
          ? 'Continue this conversation in ChatGPT. Read saved copy returns to your local history.'
          : 'ChatGPT is connected. Start a new chat here, or sync your history.';
        window.dispatchEvent(new Event('chat-hub:chatgpt-connected'));
      } else if (connection?.state === 'signed_out') {
        document.getElementById('connect-chatgpt').textContent = 'Connect ChatGPT';
        status.textContent = 'Sign in to ChatGPT here to fetch your history and continue your chats.';
      }
      if (current.connecting && Date.now() < connectDeadline) {
        clearTimeout(connectionPoll);
        connectionPoll = setTimeout(() => {
          if (showing && token === sequence) report({state: 'ready'});
        }, 5000);
      }
    } catch { /* The provider remains usable if the session probe is unavailable. */ }
  }
}

async function navigate(show, target) {
  const token = ++sequence;
  clearTimeout(poll);
  clearTimeout(connectionPoll);
  clearTimeout(locationPoll);
  if (!show) {
    showing = false;
    try { await update(); } catch { /* A missing provider does not prevent library use. */ }
    if (token !== sequence) return;
    panel.hidden = true; library.hidden = false;
    document.body.classList.remove('provider-open');
    libraryButton.classList.add('active'); providerButton.classList.remove('active');
    libraryButton.setAttribute('aria-current', 'page'); providerButton.removeAttribute('aria-current');
    return;
  }
  showing = true;
  current = target || current;
  if (target?.freshNavigation || (target && Object.prototype.hasOwnProperty.call(target, 'conversationId')))
    reportedConversationId = target.conversationId || null;
  library.hidden = true; panel.hidden = false;
  document.getElementById('provider-title').textContent = current.title || 'Conversation';
  document.getElementById('provider-back').textContent = current.localId ? 'Read saved copy' : 'Back to library';
  document.body.classList.add('provider-open');
  providerButton.classList.add('active'); libraryButton.classList.remove('active');
  providerButton.setAttribute('aria-current', 'page'); libraryButton.removeAttribute('aria-current');
  status.textContent = 'Loading ChatGPT…';
  try {
    if (target?.freshNavigation || activeTarget !== current.conversationId) {
      await native()('open_provider_conversation', {conversationId: current.conversationId});
      activeTarget = current.conversationId;
    }
    const result = await update();
    if (token !== sequence) return;
    report(result);
    watchConversationLocation(token);
    if (result?.state !== 'ready') watch(token, Date.now() + 25000);
  } catch (error) {
    status.textContent = String(error).includes('desktop_required')
      ? 'Open the Chat Hub desktop app to use ChatGPT.'
      : 'ChatGPT could not open. Use Reload ChatGPT to try again.';
  }
}

function watch(token, deadline) {
  poll = setTimeout(async () => {
    if (!showing || token !== sequence) return;
    try {
      const result = await update();
      if (!showing || token !== sequence) return;
      if (result?.state === 'ready') { report(result); return; }
    } catch { /* Keep the bounded loading deadline and recovery action visible. */ }
    if (Date.now() >= deadline) {
      status.textContent = 'ChatGPT has not finished loading. Check your connection, then use Reload ChatGPT.';
    } else watch(token, deadline);
  }, 2000);
}

function go(show, target) { queue = queue.catch(() => {}).then(() => navigate(show, target)); }
providerButton.addEventListener('click', () => {
  window.dispatchEvent(new Event('chat-hub:live-open'));
  go(true, {conversationId: null, title: 'New conversation', localId: null, freshNavigation: true});
});
window.addEventListener('chat-hub:show-live', event => go(true, {
  conversationId: event.detail?.conversationId ?? null,
  title: event.detail?.title || 'New conversation', localId: event.detail?.localId ?? null,
  leafNodeId: event.detail?.leafNodeId,
  freshNavigation: !!event.detail?.freshNavigation,
}));
window.addEventListener('chat-hub:show-local', () => go(false));
function connect() {
  connectDeadline = Date.now() + 5 * 60 * 1000;
  go(true, {conversationId: activeTarget ?? null, title: 'Connect ChatGPT',
    localId: current.localId, leafNodeId: current.leafNodeId, connecting: true});
}
document.getElementById('connect-chatgpt').addEventListener('click', connect);
window.addEventListener('chat-hub:connect', connect);
function readSaved() {
  go(false);
  if (current.localId) window.dispatchEvent(new CustomEvent('chat-hub:read-saved', {detail: {
    localId: current.localId, leafNodeId: current.leafNodeId,
  }}));
}
libraryButton.addEventListener('click', readSaved);
document.getElementById('provider-back').addEventListener('click', readSaved);
document.getElementById('provider-reload').addEventListener('click', async () => {
  if (!showing) return;
  clearTimeout(poll);
  status.textContent = 'Reloading ChatGPT…';
  try {
    if (activeTarget !== current.conversationId) {
      await native()('open_provider_conversation', {conversationId: current.conversationId});
      activeTarget = current.conversationId;
    }
    report(await update(true)); watchConversationLocation(sequence); watch(sequence, Date.now() + 25000);
  }
  catch { status.textContent = 'ChatGPT could not reload. Check your connection and try again.'; }
});
new ResizeObserver(() => {
  clearTimeout(resize);
  if (showing) resize = setTimeout(() => update().catch(() => {}), 80);
}).observe(host);
window.addEventListener('chat-hub:workspace-settings', event => {
  settingsOpen = !!event.detail?.open;
  if (showing) queue = queue.catch(() => {}).then(() => update()).catch(() => {});
});
