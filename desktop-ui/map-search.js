export function mergeMapSearch(visible,library,limit=8){
  const onMap=new Map(visible.filter(n=>n.conversation_id).map(n=>[n.conversation_id,n]));
  const result=[],seen=new Set();
  for(const item of [...library,...visible.map(n=>({...n,id:n.conversation_id,title:n.label}))]){
    if(typeof item.id!=='string'||seen.has(item.id))continue;seen.add(item.id);
    result.push({...onMap.get(item.id),...item,conversation_id:item.id,label:item.title||item.label||'Untitled chat',inMap:onMap.has(item.id)});
    if(result.length>=limit)break;
  }
  return result;
}
export function createMapSearch({searchVisible,searchLibrary,render,schedule=fn=>setTimeout(fn,180),cancel=clearTimeout}){
  let version=0,timer;
  function reset(){version++;cancel(timer);searchVisible('');render([],{query:'',loading:false});}
  function update(text){
    const query=String(text||'').trim(),token=++version;cancel(timer);
    const visible=searchVisible(query).filter(n=>n.kind==='chat');
    if(!query){render([],{query:'',loading:false});return;}
    render(mergeMapSearch(visible,[]),{query,loading:true});
    timer=schedule(async()=>{
      try{const rows=await searchLibrary(query);if(token!==version)return;
        render(mergeMapSearch(visible,rows),{query,loading:false});
      }catch{if(token===version)render(mergeMapSearch(visible,[]),{query,loading:false,error:true});}
    });
  }
  return{update,reset};
}
