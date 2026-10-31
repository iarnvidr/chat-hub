const button = document.getElementById('sync-history');
const status = document.getElementById('app-status');
let timer, lastChanges = 0, running = false;
const hasNative = typeof window.__TAURI_INTERNALS__?.invoke === 'function';

async function poll() {
  if (!hasNative) return;
  try {
    const result = await window.ChatHub.invoke('paseo_history_status');
    running = result?.state === 'running' || result?.state === 'syncing';
    button.disabled = running;
    button.textContent = running ? 'Syncing history…' : 'Sync history';
    const changes = (result?.imported || 0) + (result?.updated || 0);
    if (changes !== lastChanges) {
      lastChanges = changes;
      window.dispatchEvent(new Event('chat-hub:history-updated'));
    }
    if (running) status.textContent = result.message || 'Syncing conversation history using Paseo…';
    else if (result?.state === 'complete' || result?.state === 'completed') {
      status.textContent = `History ready: ${result.imported || 0} added, ${result.updated || 0} updated, ${result.unchanged || 0} already saved.`;
    } else if (result?.state === 'error' || result?.state === 'failed') {
      status.textContent = result.message || result.error || 'History could not sync. Your saved conversations are still available.';
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
  status.textContent = 'Reading your history using the Paseo history worker…';
  try { await window.ChatHub.invoke('sync_paseo_history'); await poll(); }
  catch {
    running = false; button.disabled = false; button.textContent = 'Sync history';
    status.textContent = 'History sync could not start. Check the Paseo history source configuration.';
  }
}
button.addEventListener('click', start);
if (hasNative) poll().then(result => {
  if (result?.state === 'idle' && result?.configured === true) start();
});
