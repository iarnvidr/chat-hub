const button = document.getElementById('restore-library');
const status = document.getElementById('app-status');
let busy = false;
let recoveryRequired = false;

button?.addEventListener('click', async () => {
  const invoke = window.ChatHub?.invoke;
  if (typeof invoke !== 'function' || busy || recoveryRequired) return;
  const confirmed = window.confirm(
    'Restoring replaces the conversations currently in this library. Save a backup first if you may need them. Continue?'
  );
  if (!confirmed) return;

  busy = true;
  button.disabled = true;
  button.textContent = 'Restoring…';
  status.textContent = 'Choose a Chat Hub SQLite backup to restore.';
  try {
    const receipt = await invoke('restore_library_backup');
    if (!receipt || typeof receipt.cancelled !== 'boolean') {
      throw new Error('invalid_receipt');
    }
    if (receipt.cancelled) {
      status.textContent = 'Restore cancelled.';
      return;
    }
    if (!Number.isSafeInteger(receipt.conversation_count) || receipt.conversation_count < 0) {
      throw new Error('invalid_receipt');
    }
    status.textContent = `Restored ${receipt.conversation_count} conversations. Restarting Chat Hub…`;
  } catch (error) {
    const message = String(error?.message || error);
    const code = message.toLowerCase();
    const messages = {
      invalid_backup: 'That backup could not be opened or is not a valid Chat Hub backup.',
      invalid_receipt: 'The restore did not return a valid result.',
      library_unavailable: 'The local library is unavailable. Restart Chat Hub and try again.',
      restore_failed: 'The backup could not be restored. Your existing library was kept.',
      archive_busy: 'Another library backup or restore is in progress. Try again when it finishes.',
      restore_shutdown_incomplete: 'Restore stopped because the library is still closing. Your library has not been replaced. Close Chat Hub fully before trying again.',
    };
    recoveryRequired = code.startsWith('restore_recovery_required:');
    status.textContent = recoveryRequired
      ? `Restore needs recovery. ${message.slice('restore_recovery_required:'.length)}`
      : messages[code] || 'The backup could not be restored. Check the library before trying again.';
  } finally {
    busy = false;
    button.disabled = recoveryRequired;
    button.textContent = 'Restore backup';
  }
});
