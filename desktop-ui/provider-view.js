const library = document.getElementById('library-reader');
const panel = document.getElementById('provider-shell');
const host = document.getElementById('provider-host');
const status = document.getElementById('provider-view-status');
const libraryButton = document.getElementById('show-library');
const providerButton = document.getElementById('show-chatgpt');
let showing = false, sequence = 0, queue = Promise.resolve(), poll, resize;

function native() {
  const invoke = window.ChatHub?.invoke;
  if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function' || typeof invoke !== 'function') {
    throw new Error('desktop_required');
  }
  return invoke;
}

async function update(reload = false) {
  const rect = host.getBoundingClientRect();
  return native()('set_provider_view', {
    visible: showing,
    x: Math.max(0, rect.x), y: Math.max(0, rect.y),
    width: Math.max(1, rect.width), height: Math.max(1, rect.height), reload,
  });
}

function report(result) {
  status.textContent = result?.state === 'ready'
    ? 'ChatGPT is open. Sign in here to use your account.'
    : result?.message || 'Loading ChatGPT…';
}

async function navigate(show) {
  const token = ++sequence;
  clearTimeout(poll);
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
  library.hidden = true; panel.hidden = false;
  document.body.classList.add('provider-open');
  providerButton.classList.add('active'); libraryButton.classList.remove('active');
  providerButton.setAttribute('aria-current', 'page'); libraryButton.removeAttribute('aria-current');
  status.textContent = 'Loading ChatGPT…';
  try {
    const result = await update();
    if (token !== sequence) return;
    report(result);
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

function go(show) { queue = queue.catch(() => {}).then(() => navigate(show)); }
providerButton.addEventListener('click', () => {
  window.dispatchEvent(new Event('chat-hub:live-open'));
  go(true);
});
window.addEventListener('chat-hub:show-live', () => go(true));
window.addEventListener('chat-hub:show-local', () => go(false));
libraryButton.addEventListener('click', () => go(false));
document.getElementById('provider-back').addEventListener('click', () => go(false));
document.getElementById('provider-reload').addEventListener('click', async () => {
  if (!showing) return;
  clearTimeout(poll);
  status.textContent = 'Reloading ChatGPT…';
  try { report(await update(true)); watch(sequence, Date.now() + 25000); }
  catch { status.textContent = 'ChatGPT could not reload. Check your connection and try again.'; }
});
new ResizeObserver(() => {
  clearTimeout(resize);
  if (showing) resize = setTimeout(() => update().catch(() => {}), 80);
}).observe(host);
