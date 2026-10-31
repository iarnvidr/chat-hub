const TAB_CLOSE_EVENT = 'chat-hub:menu-close-tab';

/** Install the small set of application tab shortcuts without intercepting text-editing keys. */
export function installTabShortcuts({target = globalThis.window, tabs, onNewTab = () => {}}) {
  const onKeyDown = event => {
    if (!(event.metaKey || event.ctrlKey) || event.altKey || event.isComposing || event.defaultPrevented) return;

    const key = String(event.key || '').toLowerCase();
    let handled = false;
    if (key === 'w' && !event.shiftKey) {
      tabs.closeSelected();
      handled = true;
    } else if (key === 't' && !event.shiftKey) {
      onNewTab();
      handled = true;
    } else if (event.shiftKey && (event.code === 'BracketLeft' || event.key === '[' || event.key === '{')) {
      tabs.selectRelative(-1);
      handled = true;
    } else if (event.shiftKey && (event.code === 'BracketRight' || event.key === ']' || event.key === '}')) {
      tabs.selectRelative(1);
      handled = true;
    } else if (!event.shiftKey && /^[1-9]$/.test(event.key)) {
      tabs.selectNumber(Number(event.key));
      handled = true;
    }

    if (handled) event.preventDefault();
  };
  const onNativeCloseTab = () => tabs.closeSelected();
  const onNativeNewTab = () => onNewTab();
  const onNativePreviousTab = () => tabs.selectRelative(-1);
  const onNativeNextTab = () => tabs.selectRelative(1);
  const onNativeSelectTab = event => tabs.selectNumber(Number(event.detail));
  target.addEventListener('keydown', onKeyDown);
  target.addEventListener(TAB_CLOSE_EVENT, onNativeCloseTab);
  target.addEventListener('chat-hub:menu-new-tab', onNativeNewTab);
  target.addEventListener('chat-hub:menu-previous-tab', onNativePreviousTab);
  target.addEventListener('chat-hub:menu-next-tab', onNativeNextTab);
  target.addEventListener('chat-hub:menu-select-tab', onNativeSelectTab);
  return () => {
    target.removeEventListener('keydown', onKeyDown);
    target.removeEventListener(TAB_CLOSE_EVENT, onNativeCloseTab);
    target.removeEventListener('chat-hub:menu-new-tab', onNativeNewTab);
    target.removeEventListener('chat-hub:menu-previous-tab', onNativePreviousTab);
    target.removeEventListener('chat-hub:menu-next-tab', onNativeNextTab);
    target.removeEventListener('chat-hub:menu-select-tab', onNativeSelectTab);
  };
}
