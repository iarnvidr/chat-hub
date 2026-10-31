const button = document.getElementById('sync-history');
const status = document.getElementById('app-status');
let timer, lastChanges = 0, running = false, pendingConnection = false;
const hasNative = typeof window.__TAURI_INTERNALS__?.invoke === 'function';

async function poll() {
  if (!hasNative) return;
  try {
    const result = await window.ChatHub.invoke('chatgpt_history_status');
    running = result?.state === 'running' || result?.state === 'syncing';
    button.disabled = running;
    button.textContent = running ? 'Syncing history…' : 'Sync history';
    const changes = (result?.imported || 0) + (result?.updated || 0);
    if (changes !== lastChanges) {
      lastChanges = changes;
      window.dispatchEvent(new Event('chat-hub:history-updated'));
    }
    if (running) status.textContent = result.message || 'Fetching your history from ChatGPT…';
    else if (result?.state === 'complete' || result?.state === 'completed') {
      status.textContent = `History ready: ${result.imported || 0} added, ${result.updated || 0} updated, ${result.unchanged || 0} already saved.`;
    } else if (result?.state === 'error' || result?.state === 'failed' || result?.state === 'partial') {
      status.textContent = result.message || result.error || 'History could not sync. Your saved conversations are still available.';
      if (result?.error_code === 'signed_out' || result?.error_code === 'login_required') {
        pendingConnection = true;
        window.dispatchEvent(new Event('chat-hub:connect'));
      }
      if (result?.error_code === 'rate_limited' && result?.retry_after > 0) {
        status.textContent += ` Wait ${result.retry_after} seconds before retrying.`;
      }
    }
    if (running) timer = setTimeout(poll, 2000);
    return result;
  } catch {
    running = false; button.disabled = false; button.textContent = 'Sync history';
    status.textContent = 'History sync is unavailable. Your saved conversations are still available.';
  }
}

async function start() {
  if (!hasNative || running) return;
  clearTimeout(timer); running = true; button.disabled = true;
  button.textContent = 'Syncing history…';
  status.textContent = 'Fetching your conversation history directly from ChatGPT…';
  try { await window.ChatHub.invoke('sync_chatgpt_history'); await poll(); }
  catch (error) {
    running = false; button.disabled = false; button.textContent = 'Sync history';
    const code = String(error?.message || error);
    if (code.includes('chatgpt_not_open')) {
      pendingConnection = true;
      status.textContent = 'Connect ChatGPT in this app to fetch your history.';
      window.dispatchEvent(new Event('chat-hub:connect'));
    } else if (code.includes('history_rate_limited')) {
      await poll();
    } else status.textContent = 'ChatGPT history could not start. Your saved conversations are still available.';
  }
}
button.addEventListener('click', start);
window.addEventListener('chat-hub:chatgpt-connected', () => {
  if (pendingConnection) { pendingConnection = false; start(); }
});
if (hasNative) poll().then(result => {
  if (result?.state === 'idle' && result?.configured === true && result?.auto_sync === true) start();
});
