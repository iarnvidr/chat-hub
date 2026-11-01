const test = require('node:test');
const assert = require('node:assert/strict');

test('category graph preserves bounded entity and exact source-chat nodes and links', async () => {
  const {prepareEntityConnections} = await import('../desktop-ui/graph-model.js');
  const raw = {
    focus_id: 'entity:repo-a',
    nodes: [
      {id:'entity:repo-a',kind:'entity',entity_kind:'repository',label:'Muninn'},
      {id:'entity:issue-a',kind:'issue',label:'Issue #12',entity_id:'entity:issue-a'},
      {id:'chat:one',kind:'chat',conversation_id:'one',label:'First chat'},
      {id:'term:noise',kind:'keyword',label:'noise'},
    ],
    edges: [
      {source:'entity:repo-a',target:'chat:one',basis:'entity_reference',relation:'mentions',reference_count:1},
      {source:'entity:issue-a',target:'chat:one',basis:'entity_reference',reason:'Mentioned in this chat'},
      {source:'chat:one',target:'term:noise',basis:'shared_keyword'},
      {source:'entity:issue-a',target:'chat:missing',basis:'entity_reference'},
    ],
  };
  const graph=prepareEntityConnections(raw);
  assert.deepEqual(graph.nodes.map(node=>node.kind),['repository','issue','chat']);
  assert.equal(graph.nodes[0].entity_id,'entity:repo-a');
  assert.equal(graph.edges.length,2);
  assert.ok(graph.edges.every(edge=>edge.basis==='entity_reference'));
  assert.equal(graph.edges[0].reason,'Referenced in 1 saved reference');
  assert.equal(graph.focus_id,'entity:repo-a');
  assert.equal(graph.nodes.find(node=>node.id==='chat:one').degree,2);
  assert.equal(raw.nodes[0].kind,'entity','preparation leaves the server response unchanged');
});

test('category graph enforces a bounded node and edge set without retaining dangling edges', async () => {
  const {prepareEntityConnections}=await import('../desktop-ui/graph-model.js');
  const entities=Array.from({length:360},(_,i)=>({id:`entity:${i}`,kind:'domain',label:`domain ${i}`}));
  const chats=Array.from({length:20},(_,i)=>({id:`chat:${i}`,kind:'chat',conversation_id:`${i}`,label:`chat ${i}`}));
  const edges=entities.flatMap(entity=>chats.map(chat=>({source:entity.id,target:chat.id,basis:'entity_reference'})));
  const graph=prepareEntityConnections({nodes:[...entities,...chats],edges});
  assert.equal(graph.nodes.length,300);
  assert.ok(graph.edges.length<=700);
  const ids=new Set(graph.nodes.map(node=>node.id));
  assert.ok(graph.edges.every(edge=>ids.has(edge.source)&&ids.has(edge.target)));
  assert.equal(graph.truncated,true);
});
