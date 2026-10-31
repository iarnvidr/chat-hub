// Runs inside the actual Tauri webview through the native-smoke eval bridge.
(async () => {
  const button = document.querySelector('[data-testid="check-updates"]');
  const message = document.querySelector('[data-testid="update-message"]');
  const version = document.querySelector('[data-testid="app-version"]');
  if (!button || !message || !version) throw new Error('native UI test selectors missing');
  const versionDeadline = Date.now() + 15000;
  while (!version.textContent.includes('Version') && Date.now() < versionDeadline) {
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!version.textContent.includes('Version')) throw new Error(`version was not rendered: ${JSON.stringify({
    versionText: version.textContent, bridge: typeof window.ChatHub?.invoke,
    health: document.getElementById('health')?.textContent,
    status: document.getElementById('app-status')?.textContent,
  })}`);
  button.click();
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const text = message.textContent || '';
    if (text.startsWith('Update check failed:') || text === 'No update available' || text === 'Update available') {
      // Failed checks return a rejected IPC promise, but update_status retains
      // the authoritative native error status/message. Compare the rendered
      // failure prefix to that state without requiring the DOM to expose raw
      // platform error details verbatim.
      const state = await window.ChatHub.invoke('update_status');
      const matches = state && state.version && (
        (state.status === 'error' && text === `Update check failed: ${state.message}`) ||
        (state.status !== 'error' && text === state.message)
      );
      if (!matches) throw new Error(`update DOM does not match native IPC state: ${JSON.stringify({text, state})}`);
      return {clicked: true, message: text, version: version.textContent, state};
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`update UI did not reach result state: ${message.textContent}`);
})()
