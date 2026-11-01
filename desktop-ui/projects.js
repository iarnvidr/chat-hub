const $ = id => document.getElementById(id);
const renameDialog = $('project-editor'), assignDialog = $('chat-project-editor');
let projects = [], editingProject, editingChat, returnToChat = false, busy = false;
const invoke = (command, args) => window.ChatHub.invoke(command, args);
const label = (project, index = 0) => project.display_name || project.title || `Unnamed project ${index + 1}`;
function projectProvider(project) {
  try {
    const key = project?.project_key;
    return typeof key === 'string' && key.startsWith('project:') ? JSON.parse(key.slice(8))?.[0] : null;
  } catch (_) { return null; }
}
const upstreamRenameAvailable = project => ['chatgpt','paseo-chatgpt'].includes(projectProvider(project));
function errorMessage(error) {
  const code = String(error?.code || error?.message || error);
  if (/rate_limit/.test(code)) return 'ChatGPT is limiting requests. Wait before trying again. Nothing has been changed locally.';
  if (/signed_out|login_required|not_open|not_ready/.test(code)) return 'Connect ChatGPT in Settings, then try again.';
  if (/account|scope_mismatch/.test(code)) return 'The connected ChatGPT account does not match this project or conversation.';
  if (/project_shape_unavailable/.test(code)) return 'ChatGPT returned an unrecognized project record. Nothing was renamed.';
  if (/verification_failed|project_rename_failed|update_unconfirmed/.test(code)) return 'ChatGPT did not confirm the new name. Chat Hub was not updated.';
  if (/invalid|name/.test(code)) return 'Enter a project name of up to 120 characters.';
  if (/not_found/.test(code)) return 'This project or conversation is unavailable in the connected account.';
  return 'ChatGPT did not confirm the update. The saved project assignment or name has not changed.';
}
async function loadProjects() {
  const result = await invoke('list_projects', {source:'chatgpt'});
  if (!Array.isArray(result?.projects)) throw Error('project_list_unavailable');
  projects = result.projects; return projects;
}
async function openDialog(dialog) {
  returnToChat = !document.body.classList.contains('settings-open');
  window.dispatchEvent(new Event('chat-hub:open-settings'));
  await new Promise(resolve => requestAnimationFrame(resolve));
  dialog.showModal();
}
function closeDialog(dialog) {
  if (busy) return;
  dialog.close();
  if (returnToChat) window.dispatchEvent(new Event('chat-hub:close-settings'));
}
function beginBusy(value) {
  busy = value;
  for (const id of ['project-editor-save','project-editor-cancel','chat-project-save','chat-project-cancel']) $(id).disabled = value;
}
function verifyReceipt(result, scope) {
  if (!result || typeof result !== 'object' || !Number.isSafeInteger(result.library_revision)) throw Error('update_unconfirmed');
  if (scope === 'upstream' && (result.scope !== 'upstream' || result.verified !== true)) throw Error('update_unconfirmed');
}
$('project-filter').addEventListener('change', () => { $('edit-project-name').disabled = !$('project-filter').value || $('project-filter').value === '__none__'; });
$('edit-project-name').addEventListener('click', async () => {
  try {
    await loadProjects(); editingProject = projects.find(project => project.project_key === $('project-filter').value);
    if (!editingProject) return;
    $('project-name-input').value = editingProject.display_name || editingProject.title || '';
    const upstreamOption = $('project-edit-scope').querySelector('option[value="upstream"]');
    upstreamOption.disabled = !upstreamRenameAvailable(editingProject);
    $('project-edit-scope').value = upstreamRenameAvailable(editingProject) && editingProject.name_source !== 'local' ? 'upstream' : 'local';
    $('project-editor-status').textContent = '';
    $('project-editor-save').textContent = $('project-edit-scope').value === 'local' ? 'Save local label' : 'Rename in ChatGPT';
    $('project-editor-description').textContent = $('project-edit-scope').value === 'local'
      ? 'Only changes the label in this library. Leave blank to use the ChatGPT name.'
      : 'Changes the name in ChatGPT and Chat Hub. It also clears this project’s saved local label.';
    await openDialog(renameDialog); $('project-name-input').focus();
  } catch (error) { $('app-status').textContent = errorMessage(error); }
});
$('project-edit-scope').addEventListener('change', () => {
  const local = $('project-edit-scope').value === 'local';
  $('project-editor-save').textContent = local ? 'Save local label' : 'Rename in ChatGPT';
  $('project-editor-description').textContent = local ? 'Only changes the label in this library. Leave blank to use the ChatGPT name.' : 'Changes the name in ChatGPT and Chat Hub. It also clears this project’s saved local label.';
});
$('project-editor-cancel').addEventListener('click', () => closeDialog(renameDialog));
$('project-editor-form').addEventListener('submit', async event => {
  event.preventDefault(); if (busy || !editingProject) return;
  const name = $('project-name-input').value.trim(), scope = $('project-edit-scope').value;
  if (scope === 'upstream' && !name) { $('project-editor-status').textContent = 'Enter a project name.'; return; }
  beginBusy(true); $('project-editor-status').textContent = scope === 'upstream' ? 'Updating ChatGPT…' : 'Saving local label…';
  try {
    const result = await invoke('rename_project', {projectKey:editingProject.project_key, name:name || null, scope});
    verifyReceipt(result, scope); beginBusy(false); closeDialog(renameDialog);
    window.dispatchEvent(new Event('chat-hub:history-updated'));
    $('app-status').textContent = scope === 'upstream' ? 'Project renamed in ChatGPT.' : 'Local project label saved.';
  } catch (error) { $('project-editor-status').textContent = errorMessage(error); }
  finally { beginBusy(false); }
});
window.addEventListener('chat-hub:edit-chat-project', async event => {
  editingChat = event.detail; if (!editingChat?.id) return;
  try {
    await loadProjects();
    const select = $('chat-project-choice'); select.replaceChildren();
    const none = document.createElement('option'); none.value = ''; none.textContent = 'No project'; select.append(none);
    projects.forEach((project, index) => {
      const option = document.createElement('option'); option.value = project.project_key; option.textContent = label(project,index);
      option.selected = option.value === editingChat.project?.project_key; select.append(option);
    });
    $('chat-project-title').textContent = editingChat.title || 'Conversation'; $('chat-project-status').textContent = '';
    $('chat-project-scope').value = 'upstream'; $('chat-project-save').textContent = 'Update in ChatGPT';
    await openDialog(assignDialog);
  } catch (error) { $('app-status').textContent = errorMessage(error); }
});
$('chat-project-scope').addEventListener('change', () => {
  $('chat-project-save').textContent = $('chat-project-scope').value === 'local' ? 'Save locally' : 'Update in ChatGPT';
});
$('chat-project-cancel').addEventListener('click', () => closeDialog(assignDialog));
$('chat-project-form').addEventListener('submit', async event => {
  event.preventDefault(); if (busy || !editingChat) return;
  const scope = $('chat-project-scope').value; beginBusy(true);
  $('chat-project-status').textContent = scope === 'upstream' ? 'Updating and checking ChatGPT…' : 'Saving assignment…';
  try {
    const result = await invoke('set_conversation_project', {conversationId:editingChat.id, projectKey:$('chat-project-choice').value || null, scope});
    verifyReceipt(result, scope); beginBusy(false); closeDialog(assignDialog);
    window.dispatchEvent(new Event('chat-hub:history-updated'));
    $('app-status').textContent = scope === 'upstream' ? 'Project assignment updated in ChatGPT.' : 'Project assignment saved locally.';
  } catch (error) { $('chat-project-status').textContent = errorMessage(error); }
  finally { beginBusy(false); }
});
for (const dialog of [renameDialog,assignDialog]) dialog.addEventListener('cancel', event => {
  event.preventDefault(); closeDialog(dialog);
});
