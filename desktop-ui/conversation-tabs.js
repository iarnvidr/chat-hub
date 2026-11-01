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

export function createConversationTabs({container, onSelect, onEmpty, onClose=()=>{}, doc=globalThis.document}) {
  const tabs = new Map();
  let selected = null;
  function render(focusSelected = false) {
    container.replaceChildren();
    container.hidden = tabs.size === 0;
    let selectedButton = null;
    for (const tab of tabs.values()) {
      const group = doc.createElement('div');
      group.className = 'conversation-tab' + (tab.id === selected ? ' active' : '');
      const button = doc.createElement('button');
      button.type = 'button'; button.setAttribute('role', 'tab');
      button.setAttribute('aria-selected', String(tab.id === selected));
      button.tabIndex = tab.id === selected ? 0 : -1;
      button.textContent = tab.title || 'Untitled conversation';
      if (tab.id === selected) selectedButton = button;
      button.addEventListener('click', () => select(tab.id));
      button.addEventListener('keydown', event => {
        const ids = [...tabs.keys()], index = ids.indexOf(tab.id);
        let next;
        if (event.key === 'ArrowRight') next = ids[(index + 1) % ids.length];
        if (event.key === 'ArrowLeft') next = ids[(index + ids.length - 1) % ids.length];
        if (event.key === 'Home') next = ids[0];
        if (event.key === 'End') next = ids.at(-1);
        if (next) { event.preventDefault(); select(next, true); }
      });
      const close = doc.createElement('button');
      close.type = 'button'; close.className = 'close-tab'; close.textContent = '×';
      close.setAttribute('aria-label', `Close ${tab.title || 'conversation'}`);
      close.addEventListener('click', () => closeTab(tab.id));
      group.append(button, close); container.append(group);
    }
    if (focusSelected) selectedButton?.focus();
  }
  function select(id, focus = false) {
    const tab = tabs.get(id);
    if (!tab) return false;
    selected = id;
    onSelect({...tab});
    render(focus);
    return true;
  }
  function closeTab(id) {
    const ids = [...tabs.keys()], index = ids.indexOf(id);
    if (index < 0) return false;
    const wasSelected = selected === id;
    const closing={...tabs.get(id)};
    tabs.delete(id);
    onClose(closing);
    if (wasSelected) {
      selected = ids[index + 1] || ids[index - 1] || null;
      if (selected) onSelect({...tabs.get(selected)}); else onEmpty();
    }
    render(wasSelected && selected !== null);
    return true;
  }
  return {
    open(tab) { const existing = tabs.get(tab.id); tabs.set(tab.id, {...existing, ...tab}); selected = tab.id; render(); },
    update(id, changes) { if (tabs.has(id)) { tabs.set(id, {...tabs.get(id), ...changes}); render(); } },
    select,
    selectRelative(delta) {
      const ids = [...tabs.keys()];
      if (!ids.length || !Number.isInteger(delta)) return false;
      const index = ids.indexOf(selected);
      const next = ids[(Math.max(index, 0) + delta % ids.length + ids.length) % ids.length];
      return select(next, true);
    },
    selectNumber(number) {
      const ids = [...tabs.keys()];
      if (!Number.isInteger(number) || number < 1 || number > 9 || !ids.length) return false;
      const index = number === 9 ? ids.length - 1 : number - 1;
      return ids[index] ? select(ids[index], true) : false;
    },
    closeTab,
    closeSelected() { return selected === null ? false : closeTab(selected); },
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
