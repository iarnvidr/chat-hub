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

export function bindCapturePreferences(invoke, elements, confirmRestart) {
  const { toggle, saveButton, restartButton, status } = elements;
  let savedEnabled = false;
  let managedByEnvironment = false;
  let busy = false;
  let pendingRestart = false;

  const renderButtons = restartRequired => {
    toggle.disabled = busy || managedByEnvironment;
    saveButton.disabled = busy || managedByEnvironment || toggle.checked === savedEnabled;
    restartButton.hidden = !restartRequired;
    restartButton.disabled = busy;
  };

  const accept = preferences => {
    toggle.checked = preferences.enabled === true;
    savedEnabled = toggle.checked;
    managedByEnvironment = preferences.environment_managed === true;
    pendingRestart = preferences.restart_required === true;
    if (managedByEnvironment) {
      status.textContent = 'CHAT_HUB_CAPTURE_ENABLED at launch overrides the saved app setting.';
    } else if (preferences.restart_required === true) {
      status.textContent = `Saved. Capture is still ${preferences.active === true ? 'on' : 'off'} until you restart Chat Hub.`;
    } else {
      status.textContent = preferences.enabled === true
        ? 'Live capture is on. Captured replies are saved on this Mac.'
        : 'Live capture is off. New conversations are not saved in this library.';
    }
    renderButtons(preferences.restart_required === true);
  };

  const load = async () => {
    toggle.disabled = true;
    saveButton.disabled = true;
    try {
      accept(await invoke('get_capture_preferences'));
    } catch (_) {
      toggle.disabled = true;
      saveButton.disabled = true;
      restartButton.hidden = true;
      status.textContent = 'Live capture settings are unavailable.';
    }
  };

  toggle.addEventListener('change', () => {
    renderButtons(pendingRestart && toggle.checked === savedEnabled);
    status.textContent = toggle.checked === savedEnabled
      ? 'No unsaved setting changes.'
      : 'Save this preference, then restart Chat Hub to apply it.';
  });

  saveButton.addEventListener('click', async () => {
    if (busy || managedByEnvironment || toggle.checked === savedEnabled) return;
    busy = true;
    status.textContent = 'Saving live capture preference…';
    renderButtons(false);
    try {
      accept(await invoke('set_capture_preferences', { enabled: toggle.checked }));
    } catch (_) {
      status.textContent = 'Could not confirm this setting was saved. Current capture behavior has not changed.';
      renderButtons(false);
    } finally {
      busy = false;
      renderButtons(restartButton.hidden === false);
    }
  });

  restartButton.addEventListener('click', async () => {
    if (busy || restartButton.hidden || managedByEnvironment) return;
    const confirmed = confirmRestart(
      'Restart Chat Hub now to apply this setting? Any unsent draft in the ChatGPT window may be lost.'
    );
    if (!confirmed) return;
    busy = true;
    status.textContent = 'Saving is complete. Restarting Chat Hub to apply your setting…';
    renderButtons(true);
    try {
      await invoke('restart_capture_settings');
    } catch (error) {
      const code = String(error?.message || error);
      status.textContent = code.includes('archive_busy')
        ? 'A library operation is in progress. Finish it before restarting Chat Hub.'
        : code.includes('capture_settings_already_active')
          ? 'This setting is already active.'
          : 'Chat Hub could not stop its local writer safely. Your setting remains pending. Reopen Chat Hub after the local service stops.';
      busy = false;
      renderButtons(true);
    }
  });

  return { load };
}

if (typeof document !== 'undefined') {
  const bridge = window.__TAURI_INTERNALS__;
  const invoke = typeof bridge?.invoke === 'function' ? (...args) => bridge.invoke(...args) : null;
  const status = document.getElementById('capture-health');
  const capturePreferences = bindCapturePreferences(invoke, {
    toggle: document.getElementById('capture-enabled-toggle'),
    saveButton: document.getElementById('save-capture-setting'),
    restartButton: document.getElementById('restart-capture-settings'),
    status: document.getElementById('capture-preferences-status'),
  }, message => window.confirm(message));
  capturePreferences.load();
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
