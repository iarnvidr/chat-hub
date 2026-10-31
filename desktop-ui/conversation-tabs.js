export function findCapturedChatRecord(items, sourceConversationId, accountScope) {
  if (typeof sourceConversationId !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(sourceConversationId)) return null;
  if (typeof accountScope !== 'string' ||
      !/^chatgpt:observed:[0-9a-f]{64}$/.test(accountScope)) return null;
  let matches = (Array.isArray(items) ? items : []).filter(item =>
    item?.provider === 'chatgpt' && item.source_id === sourceConversationId &&
    item.account_scope === accountScope);
  return matches.length === 1 ? {...matches[0]} : null;
}

export function createConversationTabs({container, onSelect, onEmpty}) {
  const tabs = new Map();
  let selected = null;
  function render() {
    container.replaceChildren();
    container.hidden = tabs.size === 0;
    for (const tab of tabs.values()) {
      const group = document.createElement('div');
      group.className = 'conversation-tab' + (tab.id === selected ? ' active' : '');
      const button = document.createElement('button');
      button.type = 'button'; button.setAttribute('role', 'tab');
      button.setAttribute('aria-selected', String(tab.id === selected));
      button.tabIndex = tab.id === selected ? 0 : -1;
      button.textContent = tab.title || 'Untitled conversation';
      button.addEventListener('click', () => { selected = tab.id; render(); onSelect({...tab}); });
      button.addEventListener('keydown', event => {
        const ids = [...tabs.keys()], index = ids.indexOf(tab.id);
        let next;
        if (event.key === 'ArrowRight') next = ids[(index + 1) % ids.length];
        if (event.key === 'ArrowLeft') next = ids[(index + ids.length - 1) % ids.length];
        if (event.key === 'Home') next = ids[0];
        if (event.key === 'End') next = ids.at(-1);
        if (next) { event.preventDefault(); selected = next; render(); onSelect({...tabs.get(next)}); container.querySelector('[aria-selected="true"]')?.focus(); }
      });
      const close = document.createElement('button');
      close.type = 'button'; close.className = 'close-tab'; close.textContent = '×';
      close.setAttribute('aria-label', `Close ${tab.title || 'conversation'}`);
      close.addEventListener('click', () => {
        const ids = [...tabs.keys()], index = ids.indexOf(tab.id);
        tabs.delete(tab.id);
        if (selected === tab.id) {
          selected = ids[index + 1] || ids[index - 1] || null;
          if (selected) onSelect({...tabs.get(selected)}); else onEmpty();
        }
        render();
      });
      group.append(button, close); container.append(group);
    }
  }
  return {
    open(tab) { const existing = tabs.get(tab.id); tabs.set(tab.id, {...existing, ...tab}); selected = tab.id; render(); },
    update(id, changes) { if (tabs.has(id)) { tabs.set(id, {...tabs.get(id), ...changes}); render(); } },
    get(id) { const tab = tabs.get(id); return tab ? {...tab} : null; },
    selected() { const tab = tabs.get(selected); return tab ? {...tab} : null; },
    all() { return [...tabs.values()].map(tab => ({...tab})); },
    rekey(oldId, newId, changes = {}) {
      if (!tabs.has(oldId) || (oldId !== newId && tabs.has(newId))) return false;
      const entries = [...tabs.entries()];
      const updated = new Map();
      for (const [id, tab] of entries) {
        if (id === oldId) updated.set(newId, {...tab, ...changes, id: newId});
        else updated.set(id, tab);
      }
      tabs.clear();
      for (const [id, tab] of updated) tabs.set(id, tab);
      if (selected === oldId) selected = newId;
      render();
      return true;
    },
  };
}
