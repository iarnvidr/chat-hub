const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(require.resolve('../desktop-ui/capture-status.js'), 'utf8')
  .replace('export async function', 'async function')
  .replaceAll('export function', 'function')
  + '\nglobalThis.captureStatus={formatCaptureStatus,refreshCaptureStatus,bindCapturePreferences};';
const context = vm.createContext({ Number, Promise });
vm.runInContext(source, context);
const { formatCaptureStatus, refreshCaptureStatus, bindCapturePreferences } = context.captureStatus;

function element() {
  const listeners = new Map();
  return {
    checked: false,
    disabled: false,
    hidden: false,
    textContent: '',
    addEventListener(name, callback) { listeners.set(name, callback); },
    async click() { await listeners.get('click')?.(); },
    change() { listeners.get('change')?.(); },
  };
}

function preferencesUI(invoke, confirmation = true) {
  const elements = {
    toggle: element(),
    saveButton: element(),
    restartButton: element(),
    status: element(),
  };
  const prompts = [];
  const binding = bindCapturePreferences(invoke, elements, message => {
    prompts.push(message);
    return confirmation;
  });
  return { elements, prompts, load: binding.load };
}

test('capture status exposes enabled state and committed total only', () => {
  assert.equal(formatCaptureStatus({
    status: 'available', enabled: true,
    counters: { committed_imported: 2, committed_updated: 1, last_error_code: 'private-detail' },
  }), 'Capture on · 3 conversations saved');
  assert.equal(formatCaptureStatus({ status: 'available', enabled: false, counters: {} }), 'Capture off');
});

test('capture status handles transient service health and invalid counters safely', async () => {
  let shown;
  await refreshCaptureStatus(async () => ({
    healthy: true,
    capture: { status: 'available', enabled: true, counters: { committed_imported: Number.MAX_SAFE_INTEGER + 1 } },
  }), text => { shown = text; });
  assert.equal(shown, 'Capture on · 0 conversations saved');
  await refreshCaptureStatus(async () => { throw Error('offline'); }, text => { shown = text; });
  assert.equal(shown, 'Capture status unavailable');
});

test('saving enable preference shows restart action and cancelling does not restart', async () => {
  const calls = [];
  const ui = preferencesUI(async (command, args) => {
    calls.push({ command, args });
    if (command === 'get_capture_preferences') return { enabled: false, active: false, restart_required: false };
    if (command === 'set_capture_preferences') return { enabled: true, active: false, restart_required: true };
    throw Error(`unexpected command ${command}`);
  }, false);
  await ui.load();
  assert.equal(ui.elements.toggle.disabled, false);
  ui.elements.toggle.checked = true;
  ui.elements.toggle.change();
  assert.equal(ui.elements.saveButton.disabled, false);
  await ui.elements.saveButton.click();
  assert.equal(ui.elements.restartButton.hidden, false);
  assert.match(ui.elements.status.textContent, /still off until you restart/);
  await ui.elements.restartButton.click();
  assert.equal(ui.prompts.length, 1);
  assert.match(ui.prompts[0], /unsent draft/);
  assert.deepEqual(calls.map(call => call.command), ['get_capture_preferences', 'set_capture_preferences']);
});

test('confirmed restart invokes the guarded restart command', async () => {
  const calls = [];
  const ui = preferencesUI(async (command, args) => {
    calls.push({ command, args });
    if (command === 'get_capture_preferences') return { enabled: false, active: false, restart_required: false };
    if (command === 'set_capture_preferences') return { enabled: true, active: false, restart_required: true };
    if (command === 'restart_capture_settings') return undefined;
    throw Error(`unexpected command ${command}`);
  });
  await ui.load();
  ui.elements.toggle.checked = true;
  ui.elements.toggle.change();
  await ui.elements.saveButton.click();
  await ui.elements.restartButton.click();
  assert.deepEqual(calls.map(call => call.command), [
    'get_capture_preferences', 'set_capture_preferences', 'restart_capture_settings',
  ]);
});

test('undoing an unsaved toggle retains a previously pending restart', async () => {
  const ui = preferencesUI(async () => ({enabled: true, active: false, restart_required: true}));
  await ui.load();
  assert.equal(ui.elements.restartButton.hidden, false);
  ui.elements.toggle.checked = false;
  ui.elements.toggle.change();
  assert.equal(ui.elements.restartButton.hidden, true);
  ui.elements.toggle.checked = true;
  ui.elements.toggle.change();
  assert.equal(ui.elements.restartButton.hidden, false);
  assert.equal(ui.elements.saveButton.disabled, true);
});

test('failed safe shutdown tells the user to reopen after the service stops', async () => {
  const ui = preferencesUI(async command => {
    if (command === 'get_capture_preferences') return { enabled: false, active: false, restart_required: false };
    if (command === 'set_capture_preferences') return { enabled: true, active: false, restart_required: true };
    if (command === 'restart_capture_settings') throw Error('capture_restart_shutdown_incomplete');
    throw Error(`unexpected command ${command}`);
  });
  await ui.load();
  ui.elements.toggle.checked = true;
  ui.elements.toggle.change();
  await ui.elements.saveButton.click();
  await ui.elements.restartButton.click();
  assert.match(ui.elements.status.textContent, /Reopen Chat Hub after the local service stops/);
});

test('environment override is shown and cannot be changed in the app', async () => {
  const calls = [];
  const ui = preferencesUI(async command => {
    calls.push(command);
    return { enabled: true, active: true, restart_required: false, environment_managed: true };
  });
  await ui.load();
  assert.equal(ui.elements.toggle.checked, true);
  assert.equal(ui.elements.toggle.disabled, true);
  assert.match(ui.elements.status.textContent, /CHAT_HUB_CAPTURE_ENABLED/);
  assert.deepEqual(calls, ['get_capture_preferences']);
});
