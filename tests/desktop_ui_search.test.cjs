const test = require('node:test');
const assert = require('node:assert/strict');
const load = () => import('../desktop-ui/search.js');
test('search presents one chat with a message target and combines hits across pages', async () => {
  const {groupSearchConversations: group} = await load();
  const hits = [
    {id: 'a', title: 'Matched title', search_node_id: null, snippet: 'Matched title'},
    {id: 'b', search_node_id: 'b1', search_message_id: 'mb', snippet: 'Another chat'},
    {id: 'a', search_node_id: 'a1', search_message_id: 'm1', search_leaf_node_id: 'leaf', snippet: 'Matched message'},
    {id: 'a', search_node_id: 'a2', search_message_id: 'm2', snippet: 'Other message'},
  ];
  const rows = group([...hits, hits[2]]);
  assert.deepEqual(rows.map(row => row.id), ['a', 'b']);
  assert.equal(rows[0].search_node_id, 'a1');
  assert.equal(rows[0].search_leaf_node_id, 'leaf');
  assert.equal(rows[0].snippet, 'Matched message');
  assert.equal(rows[0].matched_message_count, 2);
  assert.equal(rows[0].matching_hits.length, 3);
});
test('highlighting treats input as text and escapes regular expression operators', async () => {
  const {appendSearchHighlight: highlight} = await load();
  const nodes = [], doc = {createTextNode: text => ({text}), createElement: tag => ({tag, textContent: ''})};
  highlight({append: node => nodes.push(node)}, '<img onerror=alert(1)> C++ code', 'C++', doc);
  assert.deepEqual(nodes.filter(node => node.tag).map(node => [node.tag, node.textContent]), [['mark', 'C++']]);
  assert.equal(nodes[0].text, '<img onerror=alert(1)> ');
});
