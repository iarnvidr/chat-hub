const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function boot({ confirmed = true, invoke } = {}) {
  let listener;
  const button = { disabled: false, textContent: 'Restore backup', addEventListener: (_name, fn) => { listener = fn; } };
  const status = { textContent: '' };
  const context = vm.createContext({
    document: { getElementById: id => id === 'restore-library' ? button : status },
    window: { confirm: () => confirmed, ChatHub: { invoke } },
  });
  vm.runInContext(fs.readFileSync(require.resolve('../desktop-ui/restore.js'), 'utf8'), context);
  return { click: listener, button, status };
}

test('desktop restore asks before replacing the library and reports restored conversations', async () => {
  let calls = 0;
  const ui = boot({ invoke: async name => { calls += 1; assert.equal(name, 'restore_library_backup'); return { cancelled: false, conversation_count: 7 }; } });
  await ui.click();
  assert.equal(calls, 1);
  assert.match(ui.status.textContent, /Restored 7 conversations/);
  assert.equal(ui.button.disabled, false);
  assert.equal(ui.button.textContent, 'Restore backup');
});

test('desktop restore does not call native code without replacement confirmation', async () => {
  let calls = 0;
  const ui = boot({ confirmed: false, invoke: async () => { calls += 1; } });
  await ui.click();
  assert.equal(calls, 0);
  assert.equal(ui.status.textContent, '');
});

test('invalid backup error keeps the existing library and is explained in the UI', async () => {
  const ui = boot({ invoke: async () => { throw new Error('invalid_backup'); } });
  await ui.click();
  assert.match(ui.status.textContent, /not a valid Chat Hub backup/);
  assert.equal(ui.button.disabled, false);
});

test('failed rollback shows the recovery path and prevents another restore', async () => {
  let calls = 0;
  const ui = boot({ invoke: async () => { calls += 1; throw new Error('restore_recovery_required:Recover /Library/.Previous.sqlite before restarting.'); } });
  await ui.click();
  assert.match(ui.status.textContent, /Restore needs recovery/);
  assert.match(ui.status.textContent, /\/Library\/\.Previous.sqlite/);
  assert.doesNotMatch(ui.status.textContent, /existing library was kept/);
  assert.equal(ui.button.disabled, true);
  await ui.click();
  assert.equal(calls, 1);
});
