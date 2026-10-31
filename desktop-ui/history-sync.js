const button = document.getElementById('sync-history');
const status = document.getElementById('history-sync-status') || document.getElementById('app-status');
const appStatus = document.getElementById('app-status');
const workspaceStatus = document.getElementById('workspace-sync-state');
let pollTimer, cooldownTimer, autoRetryTimer, lastChanges = 0, lastExports = 0, running = false, pendingConnection = false;
let startPromise = null, interactiveSync = false, cooldownUntil = 0, automaticSyncUntil = 0, lastConnectionStart = 0;
let configured = true;
let sessionTimer;
const hasNative = typeof window.__TAURI_INTERNALS__?.invoke === 'function';
const AUTO_EVENT_DEBOUNCE_MS = 5000;
const AUTO_SYNC_COALESCE_MS = 5 * 60 * 1000;
const MAX_AUTOMATIC_RETRIES = 3;
const MAX_COOLDOWN_SECONDS = 30 * 60;
const AUTOMATIC_BACKOFF_BASE_SECONDS = 15;
const COOLDOWN_STORAGE_KEY = 'chat-hub.chatgpt-history-cooldown.v1';
let automaticRetriesRemaining = MAX_AUTOMATIC_RETRIES;
let automaticRetryPaused = false;
let verificationNeeded = false;

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

function persistCooldown() {
  try {
    if (cooldownUntil > Date.now() || automaticRetryPaused) {
      // This provider-scoped timestamp contains no account or session identifier.
      // The app currently exposes no safe account key in the status DTO.
      window.localStorage?.setItem(COOLDOWN_STORAGE_KEY, JSON.stringify({
        provider: 'chatgpt', until: cooldownUntil > Date.now() ? cooldownUntil : 0,
        automatic_retry_paused: automaticRetryPaused,
        verification_needed: verificationNeeded,
      }));
    } else window.localStorage?.removeItem(COOLDOWN_STORAGE_KEY);
  } catch { /* Cooldown persistence is best-effort; the native service remains authoritative. */ }
}

function restoreCooldown() {
  try {
    const saved = JSON.parse(window.localStorage?.getItem(COOLDOWN_STORAGE_KEY) || 'null');
    if (saved?.provider !== 'chatgpt') return;
    automaticRetryPaused = saved.automatic_retry_paused === true;
    verificationNeeded = saved.verification_needed === true;
    if (Number.isFinite(saved.until)) {
      const remaining = Math.ceil((saved.until - Date.now()) / 1000);
      if (remaining > 0) cooldownUntil = saved.until;
      else if (!automaticRetryPaused) window.localStorage?.removeItem(COOLDOWN_STORAGE_KEY);
    } else if (!automaticRetryPaused) window.localStorage?.removeItem(COOLDOWN_STORAGE_KEY);
  } catch { /* Ignore corrupt or unavailable local state. */ }
}

function scheduleAutomaticRetry() {
  clearTimeout(autoRetryTimer);
  autoRetryTimer = null;
  if (!configured || automaticRetriesRemaining <= 0 || automaticRetryPaused) {
    footer(verificationNeeded ? 'ChatGPT verification needed' : 'Sync paused — choose Sync history to retry');
    if (!automaticRetryPaused) {
      say('Automatic retries are paused after repeated ChatGPT limits. Your fetched progress is saved; use Sync history to resume.');
    }
    return;
  }
  footer('History sync waiting to retry');
  autoRetryTimer = setTimeout(async () => {
    autoRetryTimer = null;
    if (!configured || running || cooldownUntil > Date.now()) return;
    // Recheck the native deadline immediately before resuming. Another library
    // task or a refreshed server cooldown can extend it while this timer waits.
    const result = await poll({allowConnect: false});
    if (!result) return;
    if (result.automatic_retry_paused || result.retry_after_too_long) automaticRetryPaused = true;
    if (automaticRetryPaused) return;
    if (cooldownUntil > Date.now() || result?.retry_after > 0) return;
    if (!configured || running) return;
    if (result.error_code === 'signed_out' || result.error_code === 'login_required') {
      footer('Connect ChatGPT in Settings');
      say('Connect ChatGPT in Settings to resume history sync. Your saved chats remain available.');
      return;
    }
    if (['error', 'failed'].includes(result.state) && result.error_code !== 'rate_limited') return;
    automaticRetriesRemaining -= 1;
    await start({automatic: true, retry: true});
  }, 0);
}

function automaticBackoffSeconds() {
  if (automaticRetriesRemaining <= 0) return 0;
  const retriesUsed = MAX_AUTOMATIC_RETRIES - automaticRetriesRemaining;
  return Math.min(MAX_COOLDOWN_SECONDS, AUTOMATIC_BACKOFF_BASE_SECONDS * (2 ** retriesUsed));
}

function setCooldown(seconds, _message, manual = false) {
  clearTimeout(sessionTimer);
  const requestedDelay = Number(seconds);
  const serverDelay = Number.isFinite(requestedDelay) && requestedDelay > 0 ? requestedDelay : 0;
  // MAX_COOLDOWN_SECONDS limits our own exponential backoff, never the
  // provider's Retry-After deadline.
  const delay = Math.max(serverDelay, automaticBackoffSeconds());
  if (delay <= 0) { scheduleAutomaticRetry(); return; }
  cooldownUntil = Math.max(cooldownUntil, Date.now() + delay * 1000);
  persistCooldown();
  const copy = verificationNeeded
    ? 'ChatGPT returned a verification challenge. Open ChatGPT in Settings when you choose to verify; no conversation will be opened automatically.'
    : automaticRetryPaused
      ? 'Automatic sync is paused because ChatGPT supplied a very long retry window. Sync manually later; fetched progress was saved.'
      : `ChatGPT is limiting history requests. Fetched progress was saved. Retry in ${delay} seconds.`;
  say(copy, {manual});
  footer(verificationNeeded ? 'ChatGPT verification needed'
    : automaticRetryPaused ? 'Sync paused — retry manually later' : 'History sync waiting');
  clearTimeout(cooldownTimer);
  const tick = () => {
    const remaining = Math.max(0, Math.ceil((cooldownUntil - Date.now()) / 1000));
    updateButton();
    if (remaining > 0) cooldownTimer = setTimeout(tick, Math.min(1000, remaining * 1000));
    else {
      cooldownUntil = 0;
      persistCooldown();
      if (!automaticRetryPaused) {
        say('The rate-limit wait ended. Checking the shared cooldown before resuming.', {manual});
        footer('History sync checking retry window');
      }
      updateButton();
      scheduleAutomaticRetry();
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
      if (result?.error_code === 'rate_limited') {
        if (result.automatic_retry_paused || result.retry_after_too_long || result.challenge_detected) {
          automaticRetryPaused = true;
          verificationNeeded = result.challenge_detected === true;
          persistCooldown();
          if (result.challenge_detected) {
            say('ChatGPT returned a verification challenge. Open ChatGPT in Settings when you choose to verify; no conversation will be opened automatically.', {manual: allowConnect});
            footer('ChatGPT verification needed');
          } else {
            say('Automatic sync is paused because ChatGPT supplied a very long retry window. Sync manually later; fetched progress was saved.', {manual: allowConnect});
            footer('Sync paused — retry manually later');
          }
        } else {
          say(result.retry_after > 0 ? `ChatGPT is limiting history requests. Fetched progress was saved. Retry in ${result.retry_after} seconds.` : 'The rate-limit wait ended. Checking the shared cooldown before resuming.', {manual: allowConnect});
          footer('History sync waiting to retry');
        }
        if (result.retry_after > 0) setCooldown(result.retry_after, result.message, allowConnect);
      } else {
        say(copy, {manual: allowConnect});
        footer('Sync needs attention');
        if ((result?.error_code === 'signed_out' || result?.error_code === 'login_required') && allowConnect) {
          pendingConnection = true;
          window.dispatchEvent(new Event('chat-hub:connect'));
        }
      }
    } else if (result?.state === 'idle') {
      footer(cooldownUntil > Date.now() ? 'History sync waiting' : 'Open ChatGPT to sync history');
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
      if (!result?.retry_after && !result?.automatic_retry_paused && !result?.retry_after_too_long) setCooldown(60, 'ChatGPT is limiting history requests. Fetched progress was saved.', interactiveSync);
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

function start({automatic = false, retry = false} = {}) {
  if (startPromise || running || cooldownUntil > Date.now() || !configured) return startPromise || Promise.resolve();
  if (automatic && automaticRetryPaused) return Promise.resolve();
  if (!automatic) {
    automaticRetriesRemaining = MAX_AUTOMATIC_RETRIES;
    automaticRetryPaused = false;
    verificationNeeded = false;
    persistCooldown();
    clearTimeout(autoRetryTimer);
    autoRetryTimer = null;
  }
  clearTimeout(sessionTimer);
  if (automatic && !retry && automaticSyncUntil > Date.now()) return Promise.resolve();
  startPromise = runStart(automatic).finally(() => { startPromise = null; });
  return startPromise;
}

async function startup() {
  if (!hasNative) return;
  const result = await poll({allowConnect: false});
  if (automaticRetryPaused) {
    say(verificationNeeded
      ? 'ChatGPT previously returned a verification challenge. Open ChatGPT in Settings only when you choose to verify; no conversation will be opened automatically.'
      : 'Automatic sync is paused because ChatGPT supplied a very long retry window. Sync manually later; fetched progress was saved.');
    footer(verificationNeeded ? 'ChatGPT verification needed' : 'Sync paused — choose Sync history to retry');
    return;
  }
  if (running || !configured || cooldownUntil > Date.now()) return;
  try {
    const connection = await window.ChatHub.invoke('get_chatgpt_connection');
    if (connection?.state === 'connected') await start({automatic: true});
    else if (connection?.state === 'not_open' || connection?.state === 'loading') {
      // Initialize only an untouched hidden session. Native code refuses to
      // navigate a page that already hosts a live chat or an unsent draft.
      await window.ChatHub.invoke('prepare_chatgpt_session');
      const deadline = Date.now() + 60000;
      const probe = async () => {
        if (running || cooldownUntil > Date.now()) return;
        try {
          const session = await window.ChatHub.invoke('get_chatgpt_connection');
          if (session?.state === 'connected') { await start({automatic: true}); return; }
          if (session?.state === 'signed_out') {
            footer('Connect ChatGPT in Settings');
            say('Connect ChatGPT in Settings to sync history. Your saved chats are available offline.');
            return;
          }
          if (Date.now() < deadline) sessionTimer = setTimeout(probe, 2000);
        } catch { /* Startup remains bounded and never opens a sign-in window. */ }
      };
      sessionTimer = setTimeout(probe, 2000);
    }
  } catch { /* Startup never interrupts the active conversation. */ }
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

restoreCooldown();
if (hasNative) {
  if (cooldownUntil > Date.now()) setCooldown(Math.ceil((cooldownUntil - Date.now()) / 1000), 'Persisted ChatGPT cooldown', false);
  void startup();
}
