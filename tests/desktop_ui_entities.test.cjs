const test=require('node:test'),assert=require('node:assert/strict');
test('entity graph keeps exact source chats and distinguishes produced files from mentions',async()=>{
  const {entityGraph}=await import('../desktop-ui/entity-model.js');
  const result=entityGraph({entity:{id:'entity:file1',kind:'file_artifact',label:'report.xlsx'},references:[
    {conversation_id:'a',title:'Report',relation:'mentions',node_id:'m1'},
    {conversation_id:'a',title:'Report',relation:'produces',node_id:'m2'},
    {conversation_id:'b',title:'Followup',relation:'mentions',node_id:'m3'}]});
  assert.equal(result.nodes.length,3);assert.equal(result.nodes[0].entity_id,'entity:file1');
  assert.equal(result.edges.find(e=>e.target==='chat:a').relation,'produces');
  assert.equal(result.edges.find(e=>e.target==='chat:b').relation,'mentions');
  assert.equal(result.edges.find(e=>e.target==='chat:b').reference.node_id,'m3');
});
test('source links find the original alternate branch without inventing roots or looping cycles',async()=>{
  const {branchForAnchor}=await import('../desktop-ui/entity-model.js');
  const detail={nodes:[{id:'root'},{id:'selected',parent_id:'root'},{id:'alternate',parent_id:'root'},{id:'leaf',parent_id:'alternate'}],
    branches:[{leaf_node_id:'selected'},{leaf_node_id:'leaf'}],thread_node_ids:['root','selected'],selection:{selected_node_id:'selected'}};
  assert.equal(branchForAnchor(detail,'alternate'),'leaf');assert.equal(branchForAnchor(detail,'root'),'selected');
  assert.equal(branchForAnchor({...detail,nodes:[{id:'leaf',parent_id:'leaf'}]},'absent'),undefined);
});
test('entity maps group saved copies without losing the original file reference',async()=>{
  const {entityGraph}=await import('../desktop-ui/entity-model.js');
  const archive={conversation_id:'archive',canonical_conversation_id:'current',title:'Old title',role:'archive',relation:'produces',source_node_id:'old-message'};
  const current={conversation_id:'current',title:'Current title',relation:'mentions',node_id:'new-message'};
  const result=entityGraph({entity:{id:'file',kind:'file_artifact',label:'report.xlsx'},references:[archive,current]});
  assert.equal(result.nodes.length,2);
  assert.equal(result.nodes[1].conversation_id,'current');
  assert.equal(result.nodes[1].search_conversation_id,'current');
  assert.equal(result.nodes[1].label,'Current title');
  assert.equal(result.edges.length,1);
  assert.equal(result.edges[0].relation,'produces');
  assert.equal(result.edges[0].reference.conversation_id,'archive');
  assert.equal(result.edges[0].reference.source_node_id,'old-message');
  const olderOnly=entityGraph({entity:{id:'domain',kind:'domain',label:'example.test'},references:[archive]});
  assert.equal(olderOnly.nodes[1].conversation_id,'current');
  assert.equal(olderOnly.nodes[1].search_conversation_id,'archive','an older-only reference still opens its saved source');
});
test('map search ranks saved hits, deduplicates chats, and preserves message anchors',async()=>{
  const {mergeMapSearch}=await import('../desktop-ui/map-search.js');
  const result=mergeMapSearch([{id:'chat:a',conversation_id:'a',label:'A',kind:'chat'}],
    [{id:'b',title:'Older chat',search_node_id:'branch-node',search_leaf_node_id:'leaf'},{id:'a',title:'A',snippet:'match'}]);
  assert.deepEqual(result.map(r=>r.conversation_id),['b','a']);assert.equal(result[0].search_node_id,'branch-node');
  assert.equal(result[1].inMap,true);assert.equal(result[0].inMap,false);
});
test('slow prior map search cannot overwrite a newer query or repopulate after reset',async()=>{
  const {createMapSearch}=await import('../desktop-ui/map-search.js');
  const jobs=[],pending=new Map(),rendered=[];
  const control=createMapSearch({searchVisible:()=>[],searchLibrary:q=>new Promise(resolve=>pending.set(q,resolve)),
    render:(rows,state)=>rendered.push({rows,state}),schedule:fn=>{jobs.push(fn);return jobs.length;},cancel:()=>{}});
  control.update('old');const old=jobs.shift()();control.update('new');const recent=jobs.shift()();
  pending.get('new')([{id:'new-chat',title:'New'}]);await recent;
  pending.get('old')([{id:'old-chat',title:'Old'}]);await old;
  assert.equal(rendered.at(-1).rows[0].id,'new-chat');
  control.update('later');const later=jobs.shift()();control.reset();pending.get('later')([{id:'late',title:'Late'}]);await later;
  assert.equal(rendered.at(-1).state.query,'');assert.deepEqual(rendered.at(-1).rows,[]);
});
test('map view cache preserves exploration across Back and never shares mutable positions',async()=>{
  const {MapViewState,mapKey}=await import('../desktop-ui/map-state.js');
  const a={nodes:[{id:'a'},{id:'b'}],edges:[{source:'a',target:'b',basis:'project'}]},b={...a,nodes:[{id:'b',label:'Renamed'},{id:'a'}]};
  assert.equal(mapKey(a),mapKey(b));const cache=new MapViewState(1),snapshot={positions:{a:{x:1,y:2}},zoom:.8,pan:{x:10,y:20}};
  cache.save('a',snapshot);snapshot.positions.a.x=99;assert.equal(cache.get('a').positions.a.x,1);
  const read=cache.get('a');read.positions.a.x=77;assert.equal(cache.get('a').positions.a.x,1);
  cache.save('b',snapshot);assert.equal(cache.get('a'),null);cache.save('invalid',{positions:{a:{x:NaN,y:1}},zoom:1});assert.equal(cache.get('invalid'),null);
});
