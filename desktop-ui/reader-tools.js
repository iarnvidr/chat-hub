const root = document.getElementById('messages');
const input = document.getElementById('branch-find');
const count = document.getElementById('branch-find-count');
const previous = document.getElementById('branch-find-prev');
const next = document.getElementById('branch-find-next');
let hits = [], current = -1, timer;

function selectMatch(index, scroll = true) {
  hits[current]?.classList.remove('current-match');
  current = hits.length ? (index + hits.length) % hits.length : -1;
  hits[current]?.classList.add('current-match');
  count.textContent = !input.value.trim() ? '' : hits.length ? `${current + 1} of ${hits.length}${hits.length === 5000 ? '+' : ''}` : 'No matches';
  previous.disabled = next.disabled = !hits.length;
  if (scroll) hits[current]?.scrollIntoView({block: 'center', behavior: 'smooth'});
}

function find(scroll = true) {
  root.querySelectorAll('mark.branch-match').forEach(mark => mark.replaceWith(document.createTextNode(mark.textContent)));
  root.normalize();
  hits = []; current = -1;
  const query = input.value.trim().toLowerCase();
  if (query) {
    for (const body of root.querySelectorAll('.markdown')) {
      const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
      const nodes = [];
      while (walker.nextNode()) {
        if (!walker.currentNode.parentElement.closest('button')) nodes.push(walker.currentNode);
      }
      for (const node of nodes) {
        const text = node.textContent, lower = text.toLowerCase();
        let start = 0, position = lower.indexOf(query), found = false;
        const fragment = document.createDocumentFragment();
        while (position !== -1 && hits.length < 5000) {
          found = true;
          fragment.append(document.createTextNode(text.slice(start, position)));
          const mark = document.createElement('mark');
          mark.className = 'branch-match';
          mark.textContent = text.slice(position, position + query.length);
          fragment.append(mark); hits.push(mark);
          start = position + query.length;
          position = lower.indexOf(query, start);
        }
        if (found) {
          fragment.append(document.createTextNode(text.slice(start)));
          node.replaceWith(fragment);
        }
        if (hits.length === 5000) break;
      }
      if (hits.length === 5000) break;
    }
  }
  selectMatch(0, scroll);
}

input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(find, 120); });
input.addEventListener('keydown', event => {
  if (event.key === 'Enter') { event.preventDefault(); clearTimeout(timer); if (!hits.length) find(); else selectMatch(current + (event.shiftKey ? -1 : 1)); }
  if (event.key === 'Escape') { input.value = ''; clearTimeout(timer); find(false); }
});
previous.addEventListener('click', () => selectMatch(current - 1));
next.addEventListener('click', () => selectMatch(current + 1));
new MutationObserver(() => { clearTimeout(timer); find(false); }).observe(root, {childList: true});

document.getElementById('copy-branch').addEventListener('click', async () => {
  const status = document.getElementById('app-status');
  const messages = [...root.querySelectorAll('.message')];
  if (!messages.length) { status.textContent = 'There are no messages in this branch to copy.'; return; }
  const text = [document.getElementById('title').textContent, ...messages.map(message => {
    const body = message.querySelector('.markdown').cloneNode(true);
    body.querySelectorAll('button').forEach(button => button.remove());
    body.querySelectorAll('br').forEach(br => br.replaceWith(document.createTextNode('\n')));
    body.querySelectorAll('p,pre,li,h1,h2,h3,h4,h5,h6,blockquote').forEach(block => block.append(document.createTextNode('\n\n')));
    return `${message.querySelector('.role').textContent}\n\n${body.textContent.trim()}`;
  })].join('\n\n---\n\n');
  try { await navigator.clipboard.writeText(text); status.textContent = `Copied ${messages.length} messages from this branch.`; }
  catch { status.textContent = 'Clipboard access was unavailable. Select message text to copy it.'; }
});
