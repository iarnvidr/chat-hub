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
  };
}
