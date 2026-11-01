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
test('server search envelopes are normalized and list-mode grouping keeps exact open anchors',async()=>{
  const [{normalizeSearch},{groupSearchConversations:group,conversationOpenTarget}]=await Promise.all([
    import('../desktop-ui/data.js'),import('../desktop-ui/search.js'),
  ]);
  const page={next_cursor:'next',items:[
    {conversation:{id:'a',title:'Matched'},node_id:'node-a',leaf_node_id:'leaf-a',message_id:'msg-a',snippet:'first'},
    {conversation:{id:'a',title:'Matched'},node_id:'node-b',leaf_node_id:'leaf-b',message_id:'msg-b',snippet:'second'},
  ]};
  const rows=group(normalizeSearch(page).items);
  assert.equal(rows.length,1);
  assert.equal(rows[0].id,'a');
  assert.equal(rows[0].search_node_id,'node-a');
  assert.equal(rows[0].search_leaf_node_id,'leaf-a');
  assert.equal(rows[0].matching_hits.length,2);
  assert.deepEqual(conversationOpenTarget(rows[0]),{id:'a',leafNodeId:'leaf-a',anchorNodeId:'node-a'});
  assert.equal(page.next_cursor,'next','pagination cursor remains the API hit cursor');
});
test('highlighting treats input as text and escapes regular expression operators', async () => {
  const {appendSearchHighlight: highlight} = await load();
  const nodes = [], doc = {createTextNode: text => ({text}), createElement: tag => ({tag, textContent: ''})};
  highlight({append: node => nodes.push(node)}, '<img onerror=alert(1)> C++ code', 'C++', doc);
  assert.deepEqual(nodes.filter(node => node.tag).map(node => [node.tag, node.textContent]), [['mark', 'C++']]);
  assert.equal(nodes[0].text, '<img onerror=alert(1)> ');
});

test('logical chat search retains the saved copy that owns each matching message', async () => {
  const [{normalizeSearch},{groupSearchConversations:group,conversationOpenTarget}]=await Promise.all([
    import('../desktop-ui/data.js'),import('../desktop-ui/search.js'),
  ]);
  const hits=normalizeSearch({items:[
    {conversation:{id:'current',title:'Current title'},node_id:null,snippet:'title'},
    {conversation:{id:'current',title:'Current title'},matched_conversation_id:'archive',node_id:'same-node',leaf_node_id:'old-leaf',message_id:'same-message',snippet:'older-only content'},
    {conversation:{id:'current',title:'Current title'},node_id:'same-node',leaf_node_id:'new-leaf',message_id:'same-message',snippet:'current content'},
  ]}).items;
  const rows=group([...hits,hits[1]]);
  assert.equal(rows.length,1);
  assert.equal(rows[0].id,'current','row actions retain the current identity');
  assert.equal(rows[0].matching_hits.length,3,'equal node IDs in different saved copies stay distinct');
  assert.equal(rows[0].matched_message_count,2);
  assert.deepEqual(conversationOpenTarget(rows[0]),{id:'archive',leafNodeId:'old-leaf',anchorNodeId:'same-node'});
  assert.equal(rows[0].snippet,'older-only content');
});
