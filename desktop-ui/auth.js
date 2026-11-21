import {httpBridge} from './data.js';

const bridge = httpBridge();
const gate = document.getElementById('server-login');
const form = document.getElementById('server-login-form');
const tokenField = document.getElementById('server-token');
const message = document.getElementById('server-login-message');

function applyCapabilities(value = {}) {
  const caps = value.capabilities && typeof value.capabilities === 'object' ? value.capabilities : value;
  window.ChatHub.capabilities = caps;
  const aliases = {provider_view:'desktop',import_export:'desktop',native_file_dialog:'desktop',updater:'desktop',desktop_diagnostics:'desktop'};
  for (const element of document.querySelectorAll('[data-capability]')) {
    const name = aliases[element.dataset.capability] || element.dataset.capability;
    element.hidden = caps[name] !== true;
  }
  document.querySelectorAll('[data-server-only]').forEach(element => { element.hidden = false; });
}

async function resume() {
  try {
    const caps = await bridge.capabilities();
    if (caps?.authenticated !== true) return false;
    applyCapabilities(caps);
    const loginRequired = caps.login_required !== false;
    document.getElementById('server-logout').hidden = !loginRequired;
    const syncState = document.getElementById('workspace-sync-state');
    if (syncState) syncState.textContent = loginRequired ? 'Server session active' : 'Server connected';
    if (caps.dev_mode === true || caps.service_worker !== true) {
      await navigator.serviceWorker?.getRegistrations().then(registrations => Promise.all(registrations.map(registration => registration.unregister())));
    } else if ('serviceWorker' in navigator) {
      navigator.serviceWorker.register('./service-worker.js').catch(() => {});
    }
    gate.hidden = true;
    document.querySelector('.skip-link').hidden = false;
    window.dispatchEvent(new Event('chat-hub:authenticated'));
    return true;
  } catch {
    return false;
  }
}

if (window.ChatHub?.transport === 'http') {
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const token = tokenField.value;
    if (!token) { message.textContent = 'Enter the server access token.'; return; }
    form.querySelector('button[type="submit"]').disabled = true;
    message.textContent = 'Signing in…';
    try {
      await bridge.login(token);
      tokenField.value = '';
      if (!await resume()) throw new Error('session_unavailable');
    } catch {
      tokenField.value = '';
      message.textContent = 'Sign-in failed. Check the token and try again.';
      tokenField.focus();
    } finally {
      form.querySelector('button[type="submit"]').disabled = false;
    }
  });

  document.getElementById('server-logout')?.addEventListener('click', async () => {
    try { await bridge.logout(); } catch { /* Hide private data locally even if the request fails. */ }
    window.location.reload();
  });

  document.querySelector('.skip-link').hidden = true;
  resume().then(authenticated => {
    if (!authenticated) {
      gate.hidden = false;
      tokenField.focus();
    }
  });
}
