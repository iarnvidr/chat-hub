const button = document.getElementById('sync-history');
const status = document.getElementById('history-sync-status') || document.getElementById('app-status');
const appStatus = document.getElementById('app-status');
const workspaceStatus = document.getElementById('workspace-sync-state');
let pollTimer, cooldownTimer, lastChanges = 0, lastExports = 0, running = false, pendingConnection = false;
let startPromise = null, interactiveSync = false, cooldownUntil = 0, automaticSyncUntil = 0, lastConnectionStart = 0;
let configured = true;
const hasNative = typeof window.__TAURI_INTERNALS__?.invoke === 'function';
const AUTO_EVENT_DEBOUNCE_MS = 5000;
const AUTO_SYNC_COALESCE_MS = 5 * 60 * 1000;

function say(message, {manual = false} = {}) {
  if (status) status.textContent = message;
  if (manual && appStatus) appStatus.textContent = message;
}

function footer(message) {
  if (workspaceStatus) workspaceStatus.textContent = message;
}

function updateButton() {
  if (!button) return;
  const remaining = Math.max(0, Math.ceil((cooldownUntil - Date.now()) / 1000));
  button.disabled = running || remaining > 0 || !configured;
  button.textContent = running ? 'Syncing history…'
    : remaining > 0 ? `Try again in ${remaining}s`
      : configured ? 'Sync history' : 'History sync is off';
}

function setCooldown(seconds, _message, manual = false) {
  const delay = Math.max(0, Math.min(1800, Number(seconds) || 0));
  if (delay <= 0) return;
  cooldownUntil = Math.max(cooldownUntil, Date.now() + delay * 1000);
  const copy = `ChatGPT is limiting history requests. Fetched progress was saved. Retry in ${delay} seconds.`;
  say(copy, {manual});
  footer('Sync needs attention');
  clearTimeout(cooldownTimer);
  const tick = () => {
    const remaining = Math.max(0, Math.ceil((cooldownUntil - Date.now()) / 1000));
    updateButton();
    if (remaining > 0) cooldownTimer = setTimeout(tick, Math.min(1000, remaining * 1000));
    else {
      cooldownUntil = 0;
      say('History sync can run again. Fetched progress was saved.', {manual});
      footer('History sync ready');
      updateButton();
    }
  };
  updateButton();
  cooldownTimer = setTimeout(tick, Math.min(1000, delay * 1000));
}

function publishChanges(result) {
  const changes = (result?.imported || 0) + (result?.updated || 0);
  const exports = result?.processed_exports || 0;
  if ((changes !== lastChanges && changes > 0) || (exports !== lastExports && exports > 0)) {
    window.dispatchEvent(new Event('chat-hub:history-updated'));
  }
  lastChanges = changes;
  lastExports = exports;
}

async function poll({allowConnect = interactiveSync} = {}) {
  if (!hasNative) return;
  try {
    const result = await window.ChatHub.invoke('chatgpt_history_status');
    configured = result?.configured !== false;
    running = result?.state === 'running' || result?.state === 'syncing';
    updateButton();
    publishChanges(result);
    if (running) {
      say(result.message || 'Fetching your history from ChatGPT…', {manual: allowConnect});
      footer('Syncing history…');
    }
    else if (result?.state === 'complete' || result?.state === 'completed') {
      const copy = `History ready: ${result.imported || 0} added, ${result.updated || 0} updated, ${result.unchanged || 0} already saved.`;
      say(copy, {manual: allowConnect});
      footer('History up to date');
    } else if (result?.state === 'error' || result?.state === 'failed' || result?.state === 'partial') {
      const copy = result.message || result.error || 'History could not sync. Your saved conversations are still available.';
      say(copy, {manual: allowConnect});
      footer('Sync needs attention');
      if ((result?.error_code === 'signed_out' || result?.error_code === 'login_required') && allowConnect) {
        pendingConnection = true;
        window.dispatchEvent(new Event('chat-hub:connect'));
      }
      if (result?.error_code === 'rate_limited' && result?.retry_after > 0) {
        setCooldown(result.retry_after, result.message, allowConnect);
      }
    } else if (result?.state === 'idle') {
      footer('Open ChatGPT to sync history');
    }
    if (running) pollTimer = setTimeout(() => poll({allowConnect}), 2000);
    return result;
  } catch {
    running = false;
    interactiveSync = false;
    updateButton();
    footer('Sync needs attention');
    say('History sync is unavailable. Your saved conversations are still available.');
  }
}

function errorCode(error) {
  return String(error?.message || error || '');
}

async function runStart(automatic) {
  if (!hasNative || running || !configured) return;
  if (cooldownUntil > Date.now()) {
    updateButton();
    return;
  }
  clearTimeout(pollTimer);
  running = true;
  interactiveSync = !automatic;
  updateButton();
  const startingMessage = automatic
    ? 'Saving your ChatGPT history in the background…'
    : 'Fetching your conversation history from ChatGPT…';
  say(startingMessage, {manual: !automatic});
  footer('Syncing history…');
  try {
    await window.ChatHub.invoke('sync_chatgpt_history');
    automaticSyncUntil = Date.now() + AUTO_SYNC_COALESCE_MS;
    await poll({allowConnect: interactiveSync});
  } catch (error) {
    running = false;
    updateButton();
    const code = errorCode(error);
    if (code.includes('history_rate_limited')) {
      const result = await poll({allowConnect: interactiveSync});
      if (!result?.retry_after) setCooldown(60, 'ChatGPT is limiting history requests. Fetched progress was saved.', interactiveSync);
    } else if (code.includes('chatgpt_not_open') || code.includes('provider_not_ready')) {
      if (automatic) {
        say('Open ChatGPT once to sync your history. Your saved chats remain available.');
        footer('Open ChatGPT to sync history');
      } else {
        pendingConnection = true;
        say('Open ChatGPT in the app to sync your history.', {manual: true});
        footer('Open ChatGPT to sync history');
        window.dispatchEvent(new Event('chat-hub:connect'));
      }
    } else if (code.includes('archive_busy')) {
      say('Another library task is finishing. History sync will be available shortly.', {manual: !automatic});
      footer('Sync needs attention');
    } else {
      say('ChatGPT history could not start. Your saved conversations are still available.', {manual: !automatic});
      footer('Sync needs attention');
    }
  } finally {
    interactiveSync = false;
  }
}

function start({automatic = false} = {}) {
  if (startPromise || running || cooldownUntil > Date.now() || !configured) return startPromise || Promise.resolve();
  if (automatic && automaticSyncUntil > Date.now()) return Promise.resolve();
  startPromise = runStart(automatic).finally(() => { startPromise = null; });
  return startPromise;
}

async function startup() {
  if (!hasNative) return;
  const result = await poll({allowConnect: false});
  if (running || !configured || cooldownUntil > Date.now()) return;
  try {
    const connection = await window.ChatHub.invoke('get_chatgpt_connection');
    if (connection?.state === 'connected') await start({automatic: true});
  } catch { /* Startup probing never navigates or interrupts the current view. */ }
}

if (button) button.addEventListener('click', () => start({automatic: false}));

window.addEventListener('chat-hub:chatgpt-connected', () => {
  if (!configured || running || cooldownUntil > Date.now()) return;
  const now = Date.now();
  if (now - lastConnectionStart < AUTO_EVENT_DEBOUNCE_MS) return;
  lastConnectionStart = now;
  if (pendingConnection) {
    pendingConnection = false;
    start({automatic: false});
  } else {
    start({automatic: true});
  }
});

if (hasNative) void startup();
