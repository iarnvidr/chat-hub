export function mapKey(graph,messageMode=false){
  return JSON.stringify([messageMode,graph.focus_id||null,
    graph.nodes.map(n=>n.id).sort(),graph.edges.map(e=>[e.source,e.target,e.basis].join('\u0000')).sort()]);
}
export class MapViewState{
  constructor(limit=8){this.limit=limit;this.views=new Map();}
  save(key,snapshot){
    if(!key||!snapshot||!Number.isFinite(snapshot.zoom)||snapshot.zoom<=0)return;
    if(!Object.values(snapshot.positions||{}).every(p=>Number.isFinite(p.x)&&Number.isFinite(p.y)))return;
    this.views.delete(key);this.views.set(key,structuredClone(snapshot));
    while(this.views.size>this.limit)this.views.delete(this.views.keys().next().value);
  }
  get(key){const value=this.views.get(key);return value?structuredClone(value):null;}
}
