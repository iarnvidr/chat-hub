export function normalizeOverviewMode(value){
  return value==='list'?'list':'graph';
}

export function overviewModeForAction(action,current){
  if(action==='show-list')return 'list';
  if(action==='show-graph'||action==='graph-drilldown')return 'graph';
  return normalizeOverviewMode(current);
}
