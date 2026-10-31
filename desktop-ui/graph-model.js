export function prepareConnections(graph) {
  const nodes = (graph.nodes || []).filter(node => ['chat','project'].includes(node.kind)).slice(0, 30)
    .map(node => ({...node}));
  const ids = new Set(nodes.map(node => node.id));
  const edges = (graph.edges || []).filter(edge => ids.has(edge.source) && ids.has(edge.target)
    && ['project','shared_keyword'].includes(edge.basis)).slice(0, 60).map(edge => ({...edge}));
  return {nodes, edges, focus_id:graph.focus_id || null};
}

export function messageGraph(detail, limit = 240) {
  const all = new Map((detail.nodes || []).map(node => [node.id, node]));
  const messages = new Map((detail.messages || []).map(message => [message.id, message]));
  const selected = new Set(detail.thread_node_ids || []);
  const leaves = (detail.branches || []).map(branch => typeof branch === 'string' ? branch : branch.leaf_node_id);
  const children = new Map();
  for (const node of all.values()) {
    if (!children.has(node.parent_id)) children.set(node.parent_id, []);
    children.get(node.parent_id).push(node.id);
  }
  for (const ids of children.values()) ids.sort();
  const roots = [...all.values()].filter(node => !all.has(node.parent_id)).map(node => node.id).sort();
  const queue = roots.map(id => ({id, depth: 0})), visited = new Set(), rows = new Map();
  for (let i = 0; i < queue.length && visited.size < limit; i++) {
    const item = queue[i];
    if (visited.has(item.id)) continue;
    visited.add(item.id);
    if (!rows.has(item.depth)) rows.set(item.depth, []);
    rows.get(item.depth).push(item.id);
    for (const id of children.get(item.id) || []) queue.push({id, depth: item.depth + 1});
  }
  // A corrupt cycle has no root. Keep it visibly disconnected, never invent a parent.
  for (const id of [...all.keys()].sort()) if (!visited.has(id) && visited.size < limit) {
    visited.add(id); if (!rows.has(0)) rows.set(0, []); rows.get(0).push(id);
  }
  const leafFor = nodeId => {
    if (selected.has(nodeId)) return detail.selection?.selected_node_id;
    for (const leaf of leaves) {
      const seen = new Set(); let id = leaf;
      while (id && !seen.has(id)) {
        if (id === nodeId) return leaf;
        seen.add(id); id = all.get(id)?.parent_id;
      }
    }
    return undefined;
  };
  const width = Math.max(720, ...[...rows.values()].map(ids => ids.length * 170));
  const height = Math.max(220, rows.size * 76 + 40), nodes = [];
  for (const [depth, ids] of rows) ids.forEach((id, index) => {
    const node = all.get(id), message = messages.get(node.message_id);
    const role = message?.role || 'context';
    nodes.push({id, kind: 'message', label: role === 'archive' ? 'Imported transcript' : role,
      message_id: node.message_id, excerpt: (message?.content || '').slice(0, 240),
      leaf_node_id: leafFor(id), selected: selected.has(id),
      x: width * (index + 1) / (ids.length + 1), y: 35 + depth * 76});
  });
  const edges = [...visited].map(id => all.get(id)).filter(node => visited.has(node.parent_id))
    .map(node => ({source: node.parent_id, target: node.id, basis: 'parent'}));
  return {nodes, edges, width, height, truncated: all.size > visited.size,
    archived: nodes.some(node => node.label === 'Imported transcript')};
}
