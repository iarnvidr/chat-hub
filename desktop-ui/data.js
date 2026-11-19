export function normalizeList(response) {
  if (!response || !Array.isArray(response.items)) throw new Error('Invalid conversation list response');
  return response;
}

// Search results are hits, not conversation summaries. Preserve the hit metadata
// while exposing the summary fields used by the conversation-list renderer.
export function normalizeSearch(response) {
  if (!response || !Array.isArray(response.items)) throw new Error('Invalid search response');
  return { ...response, items: response.items.map(hit => {
    const summary = hit.conversation;
    if (!summary || typeof summary !== 'object') throw new Error('Invalid search hit');
    return { ...summary, snippet: hit.snippet, score: hit.score,
      selected_branch: hit.selected_branch, search_conversation_id: hit.matched_conversation_id ?? summary.id,
      search_node_id: hit.node_id ?? null,
      search_message_id: hit.message_id ?? null, search_leaf_node_id: hit.leaf_node_id ?? null,
      search_hit: true };
  }) };
}

// Browser edition transport. The access token is submitted once to the session
// endpoint and is never retained by this client; subsequent requests rely on
// the server's HTTP-only cookie.
export function httpBridge({ fetchImpl = globalThis.fetch, base = '' } = {}) {
  const csrfToken = () => globalThis.document?.cookie?.split(';').map(part => part.trim())
    .find(part => part.startsWith('chat_hub_csrf='))?.slice('chat_hub_csrf='.length) || '';
  async function request(path, options = {}) {
    const mutating = ['POST', 'PUT', 'PATCH', 'DELETE'].includes(options.method);
    const response = await fetchImpl(`${base}${path}`, {
      credentials: 'same-origin',
      cache: 'no-store',
      ...options,
      headers: { Accept: 'application/json', ...(options.body ? { 'Content-Type': 'application/json' } : {}), ...(mutating && !path.endsWith('/session') ? { 'X-CSRF-Token': csrfToken() } : {}), ...(options.headers || {}) },
    });
    let body = null;
    if (response.status !== 204) {
      try { body = await response.json(); } catch { body = null; }
    }
    if (!response.ok) {
      const error = new Error(body?.code || body?.error || `http_${response.status}`);
      error.code = body?.code || body?.error || `http_${response.status}`;
      error.status = response.status;
      throw error;
    }
    return body;
  }

  return {
    invoke(command, args = {}) {
      if (typeof command !== 'string' || !/^[a-z][a-z0-9_]*$/.test(command)) throw new Error('invalid_command');
      return request(`/api/bridge/${command}`, { method: 'POST', body: JSON.stringify({ args }) });
    },
    capabilities() { return request('/api/capabilities'); },
    login(token) { return request('/api/session', { method: 'POST', body: JSON.stringify({ token }) }); },
    logout() { return request('/api/session', { method: 'DELETE', headers: { 'X-CSRF-Token': csrfToken() } }); },
  };
}

export function fixtureAdapter() {
  const load = async path => {
    const response = await fetch(new URL(path, import.meta.url));
    if (!response.ok) throw new Error('Fixture unavailable');
    return response.json();
  };
  return {
    async listConversations(query = '', cursor = null) {
      if (query) {
        const data = await load('../tests/fixtures/api-v1/search-page-1.json');
        const next = await load('../tests/fixtures/api-v1/search-page-2.json'); return {...data, items:[...data.items,...next.items]};
      }
      return normalizeList(await load(`../tests/fixtures/api-v1/${cursor ? 'conversations-page-2.json' : 'conversations-page-1.json'}`));
    },
    async getConversation(id, leafNodeId) {
      const fixture = id === 'cv-b' ? 'conversation-fallback.json' : id === 'cv-a' ? (leafNodeId === 'n-b' ? 'conversation-alternate.json' : 'conversation-selected.json') : null;
      if (!fixture) throw new Error('Conversation not found');
      const detail = await load(`../tests/fixtures/api-v1/${fixture}`);
      if (detail.conversation.id !== id) throw new Error('Conversation not found');
      return detail;
    },
    async importChatgptExport() { return { cancelled: true, result: null }; }
  };
}
