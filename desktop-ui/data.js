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
      selected_branch: hit.selected_branch, search_node_id: hit.node_id ?? null,
      search_message_id: hit.message_id ?? null, search_leaf_node_id: hit.leaf_node_id ?? null,
      search_hit: true };
  }) };
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
