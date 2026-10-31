export function formatCaptureStatus(capture) {
  if (capture?.status === 'unavailable') return 'Capture status temporarily unavailable';
  if (capture?.status !== 'available') return 'Capture status unavailable';
  if (capture.enabled !== true) return 'Capture off';
  const saved = committedTotal(capture);
  return `Capture on · ${saved} conversations saved`;
}

function committedTotal(capture) {
  const counters = capture?.counters || {};
  return ['committed_imported', 'committed_updated'].reduce((sum, key) => {
    const value = counters[key];
    return sum + (Number.isSafeInteger(value) && value > 0 ? value : 0);
  }, 0);
}

export async function refreshCaptureStatus(invoke, setText) {
  if (typeof invoke !== 'function') {
    setText('');
    return null;
  }
  try {
    const health = await invoke('service_health');
    if (health?.healthy !== true) {
      setText('Capture status unavailable');
      return null;
    }
    setText(formatCaptureStatus(health.capture));
    return health.capture;
  } catch (_) {
    setText('Capture status unavailable');
    return null;
  }
}

if (typeof document !== 'undefined') {
  const bridge = window.__TAURI_INTERNALS__;
  const invoke = typeof bridge?.invoke === 'function' ? (...args) => bridge.invoke(...args) : null;
  const status = document.getElementById('capture-health');
  let previousSaved = null;
  const refresh = async () => {
    const capture = await refreshCaptureStatus(invoke, text => { status.textContent = text; });
    if (capture?.status !== 'available' || capture.enabled !== true) return;
    const saved = committedTotal(capture);
    if (previousSaved !== null && saved !== previousSaved) {
      window.dispatchEvent(new Event('chat-hub-capture-updated'));
    }
    previousSaved = saved;
  };
  refresh();
  setInterval(refresh, 3000);
}
