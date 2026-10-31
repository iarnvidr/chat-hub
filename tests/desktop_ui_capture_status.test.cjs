const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(require.resolve('../desktop-ui/capture-status.js'), 'utf8')
  .replace('export async function', 'async function')
  .replace('export function', 'function')
  + '\nglobalThis.captureStatus={formatCaptureStatus,refreshCaptureStatus};';
const context = vm.createContext({ Number, Promise });
vm.runInContext(source, context);
const { formatCaptureStatus, refreshCaptureStatus } = context.captureStatus;

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
