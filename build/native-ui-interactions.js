// Runs inside the actual Tauri webview through the native-smoke eval bridge.
(async () => {
  const button = document.querySelector('[data-testid="check-updates"]');
  const message = document.querySelector('[data-testid="update-message"]');
  const version = document.querySelector('[data-testid="app-version"]');
  if (!button || !message || !version) throw new Error('native UI test selectors missing');
  if (!version.textContent.includes('Version')) throw new Error('version was not rendered');
  button.click();
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const text = message.textContent || '';
    if (text.startsWith('Update check failed:') || text === 'No update available' || text === 'Update available') {
      const state = await window.ChatHub.invoke('update_status');
      if (!state || !state.version || !text.includes(state.message)) throw new Error('update DOM does not match native IPC state');
      return {clicked: true, message: text, version: version.textContent, state};
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`update UI did not reach result state: ${message.textContent}`);
})()