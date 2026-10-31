import cytoscape from './vendor/cytoscape.esm.min.mjs';

export function createGraphView(container, onNode, onEdge) {
  let cy;
  const observer = new ResizeObserver(() => {if(cy){cy.resize();cy.fit(undefined,25);}}); observer.observe(container);
  function render(graph, messageMode = false) {
    cy?.destroy(); cy = undefined; container.replaceChildren();
    if (!graph.nodes.length) {
      const empty = document.createElement('p'); empty.className = 'graph-empty';
      empty.textContent = 'No chats match this view.'; container.append(empty); return;
    }
    const elements = graph.nodes.map(node => ({data:{...node, label:shorten(node.label), fullLabel:node.label},
      classes:[node.kind, node.selected ? 'on-branch' : '', node.id === graph.focus_id ? 'focus-chat' : ''].join(' ')}));
    for (const [index, edge] of graph.edges.entries()) elements.push({data:{...edge,id:`edge:${index}`},classes:edge.basis});
    cy = cytoscape({container, elements, minZoom:.25, maxZoom:2.5, wheelSensitivity:.16,
      style:[
        {selector:'node',style:{shape:'round-rectangle',width:168,height:54,'background-color':'#26323e','border-width':1.5,'border-color':'#526776',
          label:'data(label)',color:'#edf0f3','font-family':'system-ui, sans-serif','font-size':12,'text-wrap':'wrap','text-max-width':150,'text-valign':'center','text-halign':'center',padding:8}},
        {selector:'node.project',style:{width:150,height:40,'background-color':'#392e47','border-color':'#a28bbb','font-size':11}},
        {selector:'node.focus-chat',style:{'background-color':'#21424c','border-color':'#74d4dd','border-width':3,'font-weight':600}},
        {selector:'node.message',style:{width:145,height:42,'background-color':'#28323c','font-size':11}},
        {selector:'node.on-branch',style:{'border-color':'#74d4dd','border-width':2}},
        {selector:'edge',style:{width:1.7,'line-color':'#758896','curve-style':'bezier','target-arrow-shape':'none','arrow-scale':.8}},
        {selector:'edge.project',style:{'line-color':'#a28bbb'}},
        {selector:'edge.shared_keyword',style:{'line-color':'#6c9fa9','line-style':'dashed'}},
        {selector:'edge.parent',style:{'target-arrow-shape':'triangle','target-arrow-color':'#758896'}},
        {selector:':selected',style:{'border-color':'#b4f2f4','border-width':3,'line-color':'#b4f2f4','target-arrow-color':'#b4f2f4'}},
        {selector:'.muted',style:{opacity:.22}}
      ],layout:{name:'grid',padding:30,avoidOverlap:true,spacingFactor:1.3}});
    const layout = messageMode
      ? {name:'breadthfirst',directed:true,spacingFactor:1.35,padding:36,avoidOverlap:true,nodeDimensionsIncludeLabels:true,animate:false}
      : graph.focus_id
        ? {name:'grid',cols:container.clientWidth < 520 ? 2 : 3,spacingFactor:1.3,padding:25,avoidOverlap:true,nodeDimensionsIncludeLabels:true,animate:false,
          sort:(a,b)=>Number(b.id()===graph.focus_id)-Number(a.id()===graph.focus_id)}
        : {name:'grid',cols:container.clientWidth < 520 ? 2 : 3,spacingFactor:1.15,padding:25,avoidOverlap:true,nodeDimensionsIncludeLabels:true,animate:false};
    cy.layout(layout).run();
    cy.on('tap','node',event=>{
      const node=event.target; cy.elements().removeClass('muted');
      cy.elements().difference(node.closedNeighborhood()).addClass('muted');
      onNode({...node.data(),label:node.data('fullLabel')});
    });
    cy.on('tap','edge',event=>onEdge(event.target.data()));
    cy.on('dbltap','node',event=>onNode({...event.target.data(),label:event.target.data('fullLabel')},true));
    container.setAttribute('role','img');
    container.setAttribute('aria-label',messageMode ? 'Stored message and branch relationships. Use the message list below for keyboard access.' : 'Labelled chat connections. Use the chat buttons below for keyboard access.');
  }
  return {render,zoom:factor=>{if(cy)cy.zoom({level:Math.max(.25,Math.min(2.5,cy.zoom()*factor)),renderedPosition:{x:cy.width()/2,y:cy.height()/2}});},
    fit:()=>cy?.fit(undefined,36),resize:()=>cy?.resize(),destroy:()=>{observer.disconnect();cy?.destroy();}};
}
function shorten(text) {text=String(text||'Untitled');return text.length>60?text.slice(0,57)+'…':text;}
