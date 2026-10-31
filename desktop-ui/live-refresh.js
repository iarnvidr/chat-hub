if (typeof document !== 'undefined') {
  window.addEventListener('chat-hub-capture-updated', () => {
    const list = document.getElementById('conversation-list');
    const search = document.getElementById('search');
    if (!list || !search) return;
    const observer = new MutationObserver(() => {
      const selected = list.querySelector('button.conversation-item.selected');
      if (!selected) return;
      observer.disconnect();
      clearTimeout(timeout);
      selected.click();
    });
    observer.observe(list, { childList: true });
    const timeout = setTimeout(() => observer.disconnect(), 2000);
    search.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
