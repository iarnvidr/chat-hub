export const entityKinds={repository:'Repositories',pull_request:'Pull requests',issue:'Issues',domain:'Domains',link:'Links',file_artifact:'Files'};
export function entityGraph(detail){
  const entity=detail?.entity;if(!entity?.id)throw Error('Invalid indexed item');
  const groups=new Map();
  for(const ref of detail.references||[]){if(typeof ref.conversation_id!=='string')continue;
    const id=typeof ref.canonical_conversation_id==='string'?ref.canonical_conversation_id:ref.conversation_id;
    if(!groups.has(id))groups.set(id,[]);groups.get(id).push(ref);
  }
  const nodes=[{id:entity.id,kind:entity.kind,entity_id:entity.id,label:entity.label,entity,degree:groups.size,hub:true,landmark:true,cluster:0}],edges=[];
  for(const[id,refs]of[...groups].slice(0,60)){
    const ref=refs.find(item=>item.conversation_id===id)||refs[0],nodeId=`chat:${id}`;
    nodes.push({id:nodeId,kind:'chat',conversation_id:id,source_id:ref.source_id,provider:ref.provider,
      search_conversation_id:ref.conversation_id,
      label:ref.title||'Untitled chat',cached_archive:ref.role==='archive',degree:1,cluster:0,landmark:groups.size<12});
    const relation=refs.some(r=>r.relation==='produces')?'produces':refs.some(r=>r.relation==='attaches')?'attaches':'mentions';
    const reference=refs.find(item=>item.relation===relation)||ref;
    edges.push({source:entity.id,target:nodeId,basis:'entity_reference',relation,
      reason:relation==='produces'?'Produced in this chat':relation==='attaches'?'Attached in this chat':`Referenced in ${refs.length} saved source record${refs.length===1?'':'s'}`,
      evidence:[],reference});
  }
  return{nodes,edges,focus_id:entity.id,clusters:groups.size?1:0};
}
export function branchForAnchor(detail,anchor){
  if(!anchor)return undefined;
  if((detail.thread_node_ids||[]).includes(anchor))return detail.selection?.selected_node_id;
  const nodes=new Map((detail.nodes||[]).map(n=>[n.id,n]));
  for(const branch of detail.branches||[]){const leaf=typeof branch==='string'?branch:branch.leaf_node_id;
    const seen=new Set();let id=leaf;while(id&&!seen.has(id)){if(id===anchor)return leaf;seen.add(id);id=nodes.get(id)?.parent_id;}
  }
  return undefined;
}
