// Keep API hit cursors untouched while presenting one result per conversation.
// The first ranked hit determines position; prefer a message hit as its target.
export function groupSearchConversations(hits) {
  const groups = new Map();
  for (const hit of hits) {
    let group = groups.get(hit.id);
    if (!group) {
      group = {...hit, matching_hits: [], matched_message_count: 0};
      groups.set(hit.id, group);
    }
    const key = JSON.stringify([hit.search_node_id ?? null, hit.search_message_id ?? null]);
    if (!group.matching_hits.some(item => item.key === key)) {
      group.matching_hits.push({key, node_id: hit.search_node_id, leaf_node_id: hit.search_leaf_node_id,
        message_id: hit.search_message_id, snippet: hit.snippet});
      if (hit.search_node_id != null) group.matched_message_count++;
    }
    if (group.search_node_id == null && hit.search_node_id != null) {
      for (const name of ['snippet', 'search_node_id', 'search_message_id', 'search_leaf_node_id', 'selected_branch']) group[name] = hit[name];
    }
  }
  return [...groups.values()];
}

export function conversationOpenTarget(result) {
  if(!result||typeof result.id!=='string')throw new Error('Invalid conversation search result');
  const target={id:result.id};
  if(typeof result.search_leaf_node_id==='string')target.leafNodeId=result.search_leaf_node_id;
  if(typeof result.search_node_id==='string')target.anchorNodeId=result.search_node_id;
  return target;
}

export function appendSearchHighlight(parent, text, query, doc = document) {
  text = String(text ?? '');
  const terms = [...new Set(String(query).trim().split(/\s+/).filter(term => /[\p{L}\p{N}]/u.test(term)))];
  if (!terms.length) { parent.textContent = text; return; }
  const pattern = terms.sort((a, b) => b.length - a.length).map(term => term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|');
  const re = new RegExp(pattern, 'giu');
  let index = 0, match;
  while ((match = re.exec(text))) {
    parent.append(doc.createTextNode(text.slice(index, match.index)));
    const mark = doc.createElement('mark'); mark.textContent = match[0]; parent.append(mark);
    index = re.lastIndex;
  }
  parent.append(doc.createTextNode(text.slice(index)));
}
