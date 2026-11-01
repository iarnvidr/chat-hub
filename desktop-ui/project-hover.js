export function assignmentErrorMessage(error) {
  const code = String(error?.code || error?.message || error);
  if (/rate_limit|cooldown/.test(code)) return 'ChatGPT is limiting requests. Try again after the current cooldown.';
  if (/signed_out|login_required|not_open|not_ready|loading/.test(code)) return 'Connect ChatGPT in Settings, then try again.';
  if (/account_changed|account_mismatch/.test(code)) return 'The connected ChatGPT account changed. Reopen this conversation and try again.';
  if (/account_unavailable/.test(code)) return 'ChatGPT could not confirm the connected account. Reconnect and try again.';
  if (/project_catalog_unavailable/.test(code)) return 'ChatGPT could not load its project list. Check the connection and try again.';
  if (/project_unavailable|unknown project/.test(code)) return 'That project is no longer available in ChatGPT. Reopen the picker and choose a current project.';
  if (/conversation_unavailable|unknown conversation/.test(code)) return 'That conversation is no longer available in ChatGPT. Refresh the library and try again.';
  if (/verification_failed/.test(code)) return 'ChatGPT did not confirm the project change. The library was not updated.';
  if (/project_assignment_timeout/.test(code)) return 'ChatGPT did not finish confirming the project change in time. Reopen the conversation to check its current project.';
  if (/invalid_assignment_context|invalid_assignment|invalid_project|invalid_request/.test(code)) return 'The selected conversation or project could not be validated. Refresh the library and try again.';
  if (/chatgpt_page_changed/.test(code)) return 'The ChatGPT page changed before the project update finished. Reopen the conversation and try again.';
  if (/busy|unavailable|unsupported|shape|request_failed|service_unavailable/.test(code)) return 'ChatGPT could not complete the project change. Check the connection and try again.';
  return 'ChatGPT did not confirm the project change. The library was not updated.';
}

export function createProjectHoverPicker({invoke, onChanged = () => {}, doc = document, win = window}) {
  const node = (tag, text, cls) => {
    const element = doc.createElement(tag);
    if (text !== undefined) element.textContent = text;
    if (cls) element.className = cls;
    return element;
  };
  const menu = node('div', undefined, 'thread-menu');
  menu.id = 'reader-project-menu';
  menu.hidden = true;
  menu.setAttribute('role', 'menu');
  doc.body.append(menu);

  let target = null, trigger = null, generation = 0, busy = false;

  function close(restoreFocus = true) {
    if (menu.hidden) return;
    const prior = trigger;
    ++generation;
    menu.hidden = true;
    menu.replaceChildren();
    target = null;
    trigger = null;
    prior?.setAttribute('aria-expanded', 'false');
    if (restoreFocus && prior?.isConnected) prior.focus();
  }

  function position() {
    if (!trigger || menu.hidden) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(280, win.innerWidth - 16);
    menu.style.width = `${width}px`;
    menu.style.maxHeight = `${win.innerHeight - 16}px`;
    menu.style.left = `${Math.max(8, Math.min(rect.right - width, win.innerWidth - width - 8))}px`;
    const height = menu.getBoundingClientRect().height;
    menu.style.top = `${rect.bottom + height + 6 <= win.innerHeight - 8 ? rect.bottom + 6 : Math.max(8, rect.top - height - 6)}px`;
  }

  function status(message) {
    let element = menu.querySelector('.thread-menu-status');
    if (!element) {
      element = node('p', undefined, 'thread-menu-status');
      element.setAttribute('role', 'status');
      element.setAttribute('aria-live', 'polite');
      menu.append(element);
    }
    element.textContent = message;
    position();
  }

  function header(chat) {
    const element = node('div', undefined, 'thread-menu-heading');
    const title = node('strong', 'Move to project');
    const subtitle = node('span', chat.title || 'Untitled conversation');
    element.append(title, subtitle);
    return element;
  }

  function choice(project, label, selected) {
    const button = node('button', undefined, 'thread-menu-item');
    button.type = 'button';
    button.dataset.action = 'choose-project';
    button.setAttribute('role', 'menuitemradio');
    button.setAttribute('aria-checked', String(selected));
    button.disabled = busy;
    button.append(node('span', label));
    if (selected) button.append(node('span', '✓'));
    button.addEventListener('click', async () => await assign(project?.project_key ?? null));
    return button;
  }

  async function assign(projectKey) {
    if (!target || busy) return;
    const chat = target, token = generation;
    busy = true;
    for (const control of menu.querySelectorAll('button,input')) control.disabled = true;
    status('Updating ChatGPT…');
    try {
      const result = await invoke('set_conversation_project', {
        conversationId: chat.id,
        projectKey,
        scope: 'upstream',
      });
      if (result?.scope !== 'upstream' || result?.verified !== true || !Number.isSafeInteger(result.library_revision)) {
        throw Error('update_unconfirmed');
      }
      const stillOpen = target === chat && generation === token;
      if (stillOpen) close(false);
      onChanged();
      const appStatus = doc.getElementById('app-status');
      if (appStatus) appStatus.textContent = projectKey ? 'Project updated in ChatGPT.' : 'Project removed in ChatGPT.';
    } catch (error) {
      if (target === chat && generation === token) status(assignmentErrorMessage(error));
      else {
        const appStatus = doc.getElementById('app-status');
        if (appStatus) appStatus.textContent = assignmentErrorMessage(error);
      }
    } finally {
      busy = false;
      if (target === chat && generation === token) {
        for (const control of menu.querySelectorAll('button,input')) control.disabled = false;
      }
    }
  }

  async function open(chat, anchor) {
    if (!chat?.id || !anchor) return;
    close(false);
    target = chat;
    trigger = anchor;
    const token = ++generation;
    menu.hidden = false;
    menu.replaceChildren(header(chat));
    menu.setAttribute('aria-label', 'Move conversation to a ChatGPT project');
    anchor.setAttribute('aria-haspopup', 'menu');
    anchor.setAttribute('aria-expanded', 'true');
    status('Loading projects…');
    position();
    try {
      const result = await invoke('list_projects', {source: 'chatgpt'});
      if (token !== generation || target !== chat) return;
      if (!Array.isArray(result?.projects)
          || result.projects.some(project => !project || typeof project.project_key !== 'string' || !project.project_key)) {
        throw Error('project_catalog_unavailable');
      }
      menu.replaceChildren(header(chat));
      const search = node('input');
      search.type = 'search';
      search.placeholder = 'Find a project…';
      search.setAttribute('aria-label', 'Find a ChatGPT project');
      search.className = 'thread-project-search';
      menu.append(search);
      const list = node('div', undefined, 'thread-project-options');
      menu.append(list);
      const choices = [null, ...result.projects];
      const selected = chat.project?.project_key ?? null;
      const draw = () => {
        list.replaceChildren();
        const query = search.value.trim().toLocaleLowerCase();
        const options = choices.map((project, index) => ({
          project,
          label: project ? (project.display_name || project.title || `Unnamed project ${index}`) : 'No project',
        }));
        for (const option of options) {
          if (query && !option.label.toLocaleLowerCase().includes(query)) continue;
          list.append(choice(option.project, option.label, (option.project?.project_key ?? null) === selected));
        }
        if (!list.childElementCount) list.append(node('p', 'No projects match.', 'thread-menu-status'));
        position();
      };
      search.addEventListener('input', draw);
      draw();
      search.focus();
    } catch (error) {
      if (token === generation && target === chat) status(assignmentErrorMessage(error));
    }
  }

  menu.addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key === 'Tab') { const prior = trigger; close(false); prior?.focus(); return; }
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    if (event.target?.tagName === 'INPUT' && !['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    if (!buttons.length) return;
    event.preventDefault();
    let index = buttons.indexOf(doc.activeElement);
    index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
      : (index + (event.key === 'ArrowUp' ? -1 : 1) + buttons.length) % buttons.length;
    buttons[index].focus();
  });
  doc.addEventListener('pointerdown', event => {
    if (!menu.hidden && !menu.contains(event.target) && !trigger?.contains(event.target)) close(false);
  }, true);
  doc.addEventListener('scroll', event => {
    if (!menu.hidden && !menu.contains(event.target)) close(false);
  }, true);
  win.addEventListener('resize', () => close(false));

  return {open, close, menu};
}
