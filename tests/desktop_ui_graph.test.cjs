const test = require('node:test'), assert = require('node:assert/strict');
const load = () => import('../desktop-ui/graph-model.js');

test('connections keeps only named chat/project nodes and declared reasons', async () => {
  const {prepareConnections} = await load();
  const graph = {
    focus_id: 'chat:a',
    nodes: [
      {id:'chat:a',kind:'chat',label:'First chat'},
      {id:'project:p',kind:'project',label:'Research'},
      {id:'term:t',kind:'keyword',label:'noisy term'},
    ],
    edges: [
      {source:'chat:a',target:'project:p',basis:'project',reason:'Same project: Research',evidence:['Research']},
      {source:'chat:a',target:'term:t',basis:'shared_keyword',reason:'Shared keyword'},
      {source:'chat:a',target:'missing',basis:'project',reason:'invalid target'},
      {source:'chat:a',target:'project:p',basis:'made_up',reason:'invalid basis'},
    ],
  };
  const prepared = prepareConnections(graph);
  assert.deepEqual(prepared.nodes.map(node => node.id), ['chat:a','project:p']);
  assert.equal(prepared.edges.length, 1);
  assert.equal(prepared.edges[0].reason, 'Same project: Research');
  assert.deepEqual(prepared.edges[0].evidence, ['Research']);
  assert.equal(prepared.focus_id, 'chat:a');
  assert.equal(graph.nodes.length, 3, 'preparation does not mutate the indexed response');
});

test('overview never materializes generic keyword hubs and caps graph size', async () => {
  const {prepareConnections} = await load();
  const chats = Array.from({length:130},(_,i)=>({id:`chat:${i}`,kind:'chat',label:`Chat ${i}`}));
  const projects = [{id:'project:p',kind:'project',label:'Project'}];
  const terms = Array.from({length:40},(_,i)=>({id:`term:${i}`,kind:'keyword',label:`generic ${i}`}));
  const edges = [
    ...chats.slice(0,31).map(chat=>({source:chat.id,target:'project:p',basis:'project',reason:'Same project'})),
    ...terms.flatMap(term=>chats.slice(0,2).map(chat=>({source:chat.id,target:term.id,basis:'shared_keyword'}))),
  ];
  const graph = prepareConnections({nodes:[...chats,...projects,...terms],edges});
  assert.equal(graph.nodes.length, 100);
  assert.ok(graph.nodes.every(node=>['chat','project'].includes(node.kind)));
  assert.ok(graph.edges.length <= 240);
  assert.ok(graph.edges.every(edge=>['project','shared_keyword'].includes(edge.basis)));
});

test('map clusters and node degrees reflect declared connections only', async () => {
  const {prepareConnections}=await load();
  const graph=prepareConnections({unmapped_count:9,nodes:['a','b','c','d'].map(id=>({id,kind:'chat',label:id})),
    edges:[{source:'a',target:'b',basis:'shared_keyword'},{source:'c',target:'d',basis:'project'}]});
  assert.equal(graph.clusters,2);assert.equal(graph.unmapped_count,9);
  assert.equal(graph.nodes.find(n=>n.id==='a').cluster,graph.nodes.find(n=>n.id==='b').cluster);
  assert.notEqual(graph.nodes.find(n=>n.id==='a').cluster,graph.nodes.find(n=>n.id==='c').cluster);
  assert.ok(graph.nodes.every(n=>n.degree===1));assert.equal(graph.nodes.filter(n=>n.hub).length,2);
});

test('message graph preserves null context, branches and exact navigation anchors', async () => {
  const {messageGraph} = await load();
  const detail={nodes:[{id:'b',parent_id:'root',message_id:'mb'},{id:'a',parent_id:'root',message_id:'ma'},{id:'root',parent_id:null,message_id:null}],
    messages:[{id:'ma',role:'assistant',content:'selected'},{id:'mb',role:'assistant',content:'alternate'}],
    branches:[{leaf_node_id:'a'},{leaf_node_id:'b'}],thread_node_ids:['root','a'],selection:{selected_node_id:'a'}};
  const graph=messageGraph(detail);
  assert.equal(graph.edges.length,2);
  assert.equal(graph.nodes.find(node=>node.id==='root').label,'context');
  assert.equal(graph.nodes.find(node=>node.id==='a').leaf_node_id,'a');
  assert.equal(graph.nodes.find(node=>node.id==='b').leaf_node_id,'b');
  assert.ok(graph.nodes.find(node=>node.id==='a').selected);
});

test('cached transcripts stay a single archive node and corrupt cycles cannot loop', async () => {
  const {messageGraph} = await load();
  const archive=messageGraph({nodes:[{id:'cache',parent_id:null,message_id:'m'}],messages:[{id:'m',role:'archive',content:'User: text\nAssistant: text'}]});
  assert.equal(archive.nodes.length,1); assert.equal(archive.archived,true); assert.equal(archive.edges.length,0);
  const cycle=messageGraph({nodes:[{id:'a',parent_id:'b'},{id:'b',parent_id:'a'}]},1);
  assert.equal(cycle.nodes.length,1); assert.equal(cycle.truncated,true);
});
