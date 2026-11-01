import {bindProjectHoverIntent,createProjectHover} from './project-hover.js';

const $ = id => document.getElementById(id);
const invoke = (command, args) => window.ChatHub.invoke(command, args);
let projects = [], renameBusy = false, renameRequest = 0;

function projectParts(project) {
  try {
    const key = project?.project_key;
    return typeof key === 'string' && key.startsWith('project:') ? JSON.parse(key.slice(8)) : null;
  } catch (_) { return null; }
}
function projectCanBeRenamed(project) {
  const parts = projectParts(project);
  return Array.isArray(parts) && ['chatgpt','paseo-chatgpt'].includes(parts[0]) &&
    typeof parts[1] === 'string' && parts[1] !== '' && typeof parts[2] === 'string' && parts[2] !== '';
}
function errorMessage(error, action = 'rename') {
  const code = String(error?.code || error?.message || error);
  if (/rate_limit|cooldown/.test(code)) return 'ChatGPT is limiting requests. Wait before trying again.';
  if (/signed_out|login_required|not_open|not_ready|loading/.test(code)) return 'Connect ChatGPT in Settings, then try again.';
  if (/account|scope_mismatch/.test(code)) return 'The connected ChatGPT account does not match this project or chat.';
  if (/project_shape_unavailable/.test(code)) return 'ChatGPT returned an unrecognized project record. Nothing was changed.';
  if (/verification_failed|project_rename_failed|update_unconfirmed/.test(code)) return 'ChatGPT did not confirm the change. Chat Hub was not updated.';
  if (/invalid_name|name_too_long/.test(code)) return 'Project names must be between 1 and 120 characters.';
  if (/invalid|project_not_available|not_found/.test(code)) return action === 'assign'
    ? 'That project is not available to this chat in the connected account.'
    : 'This project is unavailable in the connected ChatGPT account.';
  return action === 'assign'
    ? 'ChatGPT did not confirm the project assignment. The saved assignment has not changed.'
    : 'ChatGPT did not confirm the new name. The saved project name has not changed.';
}
async function loadProjects() {
  const result = await invoke('list_projects', {source:'chatgpt'});
  if (!Array.isArray(result?.projects)) throw Error('project_list_unavailable');
  projects = result.projects;
  return projects;
}
function verifyRename(result) {
  if (!result || result.scope !== 'upstream' || result.verified !== true || !Number.isSafeInteger(result.library_revision)) {
    throw Error('update_unconfirmed');
  }
}

const editor = createProjectHover({document, window, className:'project-rename-hover'});
$('project-filter').addEventListener('change', () => {
  $('edit-project-name').disabled = !$('project-filter').value || $('project-filter').value === '__none__';
});
$('edit-project-name').addEventListener('click', async event => {
  if (renameBusy) return;
  const anchor = event.currentTarget;
  const request = ++renameRequest;
  try {
    await loadProjects();
    if(request!==renameRequest)return;
    const editingProject = projects.find(project => project.project_key === $('project-filter').value);
    if (!editingProject) return;
    if (!projectCanBeRenamed(editingProject)) throw Error('project_not_available');
    editor.open(anchor, panel => {
      const form = document.createElement('form');
      form.className = 'project-rename-form';
      const title = document.createElement('strong'); title.textContent = 'Rename in ChatGPT';
      const help = document.createElement('p'); help.textContent = 'This changes the ChatGPT project and its name in Chat Hub.';
      const input = document.createElement('input'); input.type='text'; input.maxLength=120; input.required=true;
      input.autocomplete='off'; input.setAttribute('aria-label','Project name');
      input.value = editingProject.display_name || editingProject.title || '';
      const status = document.createElement('p'); status.className='project-hover-status'; status.setAttribute('role','status'); status.setAttribute('aria-live','polite');
      const actions = document.createElement('div'); actions.className='project-hover-actions';
      const cancel = document.createElement('button'); cancel.type='button'; cancel.textContent='Cancel'; cancel.addEventListener('click',()=>editor.close());
      const save = document.createElement('button'); save.type='submit'; save.className='primary'; save.textContent='Rename';
      actions.append(cancel,save); form.append(title,help,input,status,actions);
      form.addEventListener('submit', async submit => {
        submit.preventDefault(); if (renameBusy) return;
        const name = input.value.trim();
        if (!name || name.length > 120) { status.textContent='Enter a project name of up to 120 characters.'; input.focus(); return; }
        renameBusy = true; input.disabled = cancel.disabled = save.disabled = true; status.textContent='Updating ChatGPT…';
        try {
          const result = await invoke('rename_project', {projectKey:editingProject.project_key,name,scope:'upstream'});
          verifyRename(result); editor.close();
          window.dispatchEvent(new Event('chat-hub:history-updated'));
          $('app-status').textContent='Project renamed in ChatGPT.';
        } catch (error) { status.textContent=errorMessage(error); }
        finally { renameBusy=false; input.disabled=cancel.disabled=save.disabled=false; }
      });
      panel.append(form);
      if(event.isTrusted)requestAnimationFrame(()=>{if(!editor.panel.hidden){input.focus();input.select();}});
    });
  } catch (error) { $('app-status').textContent=errorMessage(error); }
});
bindProjectHoverIntent($('edit-project-name'),{
  onOpen:()=>$('edit-project-name').click(),
  onLeave:()=>{++renameRequest;editor.deferClose();}
});

window.addEventListener('chat-hub:edit-chat-project', event => {
  const chat = event.detail;
  if (!chat?.id) return;
  window.dispatchEvent(new CustomEvent('chat-hub:open-project-picker', {
    detail:{chat, anchor:$('set-chat-project'), focus:false}
  }));
});
bindProjectHoverIntent($('set-chat-project'),{
  onOpen:()=>$('set-chat-project').click(),
  onLeave:()=>window.dispatchEvent(new Event('chat-hub:close-project-picker'))
});
