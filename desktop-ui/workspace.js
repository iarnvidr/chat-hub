const settings = document.getElementById('settings-page');
const trigger = document.getElementById('show-settings');
let returnFocus;
function showSettings(open) {
  if (open === !settings.hidden) return;
  if (open) returnFocus = document.activeElement;
  settings.hidden = !open;
  document.body.classList.toggle('settings-open', open);
  trigger.setAttribute('aria-pressed', String(open));
  window.dispatchEvent(new CustomEvent('chat-hub:workspace-settings', {detail: {open}}));
  if (open) settings.focus();
  else if (returnFocus?.isConnected) returnFocus.focus();
}
trigger.addEventListener('click', () => showSettings(settings.hidden));
document.getElementById('workspace-sync-state').addEventListener('click', () => showSettings(true));
document.getElementById('settings-back').addEventListener('click', () => showSettings(false));
for (const name of ['chat-hub:show-local', 'chat-hub:show-live', 'chat-hub:live-open', 'chat-hub:connect']) {
  window.addEventListener(name, () => showSettings(false));
}
document.getElementById('connect-chatgpt').addEventListener('click', () => showSettings(false));
document.querySelector('.brand').addEventListener('click', event => {
  event.preventDefault(); showSettings(false);
  document.getElementById('show-library').click();
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape' && !event.defaultPrevented && !settings.hidden && !document.querySelector('details[open]')) {
    showSettings(false); event.preventDefault();
  }
});
