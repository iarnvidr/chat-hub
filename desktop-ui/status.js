export async function updateStatus({ invoke, setVersion, setMessage, setHealth, setRetryVisible = () => {}, maxPolls = 3, delay = ms => new Promise(r => setTimeout(r, ms)) }) {
  setRetryVisible(false);
  if (typeof invoke !== 'function') { setMessage('Desktop bridge unavailable'); setHealth('Local service unavailable'); setRetryVisible(true); return; }
  try {
    const s = await invoke('update_status');
    await invoke('service_info');
    setVersion(`Version ${s.version}`);
    setMessage(s.message);
  } catch (_) { setHealth('Local service unavailable'); setRetryVisible(true); return; }
  for (let i = 0; i < maxPolls; i++) {
    try {
      const health = await invoke('service_health');
      if (health.healthy === true) {
        setHealth('Local service READY.');
        setRetryVisible(false);
        return;
      }
    } catch (_) {}
    if (i + 1 < maxPolls) await delay(500);
  }
  setHealth('Local service unavailable.'); setRetryVisible(true);
}
