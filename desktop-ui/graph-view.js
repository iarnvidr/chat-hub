import cytoscape from './vendor/cytoscape.esm.min.mjs';

const palette=['#71c5ce','#b9a0d9','#e2b47d','#8db89a','#8daedb','#cf97ad','#c0be7b','#86bdbc'];
export function createGraphView(container, onNode, onEdge) {
  let cy, resizeFrame, labelFrame, renderFrame, updateLabels=()=>{};
  const measure=document.createElement('canvas').getContext('2d');if(measure)measure.font='11px system-ui';
  const tooltip=document.createElement('div');tooltip.className='map-tooltip';tooltip.hidden=true;
  const observer=new ResizeObserver(()=>{
    cancelAnimationFrame(resizeFrame);
    resizeFrame=requestAnimationFrame(()=>cy?.resize());
  });observer.observe(container);
  function choose(node,notify=true) {
    if(!cy||!node?.length)return;
    cy.elements().removeClass('muted active adjacent');
    const near=node.closedNeighborhood();cy.elements().difference(near).addClass('muted');
    node.addClass('active');near.nodes().difference(node).addClass('adjacent');
    near.edges().addClass('active');
    updateLabels();
    if(notify)onNode({...node.data(),label:node.data('fullLabel')});
  }
  function render(graph,messageMode=false) {
    cancelAnimationFrame(renderFrame);cancelAnimationFrame(labelFrame);
    cy?.destroy();cy=undefined;container.replaceChildren();tooltip.hidden=true;
    if(!graph.nodes.length){
      const empty=document.createElement('p');empty.className='graph-empty';
      empty.textContent='No strong connections found in this scope. Your chats are still available in the sidebar.';
      container.append(empty);return;
    }
    const elements=graph.nodes.map(node=>({data:{...node,
      label:shorten(messageMode&&node.excerpt?`${node.label} · ${node.excerpt}`:node.label),fullLabel:node.label,
      color:palette[(node.cluster||0)%palette.length],size:messageMode?12:Math.min(22,11+(node.degree||0)*2)},
      classes:[node.kind,node.hub?'hub':'',node.landmark||messageMode?'landmark':'',node.selected?'on-branch':'',node.id===graph.focus_id?'focus-chat':''].join(' ')}));
    for(const [i,edge] of graph.edges.entries())elements.push({data:{...edge,id:`edge:${i}`},classes:edge.basis});
    cy=cytoscape({container,elements,minZoom:.2,maxZoom:4,wheelSensitivity:.2,selectionType:'single',
      style:[
        {selector:'node',style:{shape:'ellipse',width:'data(size)',height:'data(size)',
          'background-color':'data(color)','border-width':1.5,'border-color':'#171d23',
          label:'',color:'#aab7c4','font-family':'system-ui, sans-serif','font-size':11,
          'text-valign':'bottom','text-halign':'center','text-margin-y':7,'text-wrap':'ellipsis','text-max-width':135,
          'text-outline-color':'#171d23','text-outline-width':2,'min-zoomed-font-size':0,'overlay-padding':8}},
        {selector:'node.label-visible,node.hovered',style:{label:'data(label)'}},
        {selector:'node.hub',style:{color:'#e3eaf0','font-weight':600,'font-size':12}},
        {selector:'node.focus-chat',style:{width:25,height:25,'border-width':3,'border-color':'#edfaff',color:'#edfaff'}},
        {selector:'node.message',style:{'background-color':'#8796a6','text-max-width':180}},
        {selector:'node.on-branch',style:{'background-color':'#72cbd4'}},
        {selector:'edge',style:{width:1,'line-color':'#738a99','curve-style':'bezier',opacity:.32,'overlay-padding':7}},
        {selector:'edge.project',style:{'line-color':'#af91cc',width:1.2,opacity:.5}},
        {selector:'edge.shared_keyword',style:{'line-color':'#6d9da4'}},
        {selector:'edge.parent',style:{'target-arrow-shape':'triangle','target-arrow-color':'#738a99','arrow-scale':.7}},
        {selector:'.muted',style:{opacity:.12}},
        {selector:'node.adjacent',style:{color:'#edf4f8','text-background-color':'#171d23','text-background-opacity':.9,'text-background-padding':3}},
        {selector:'node.active',style:{label:'data(label)','border-width':3,'border-color':'#f4ffff',color:'#ffffff','font-weight':600,'text-background-color':'#171d23','text-background-opacity':1,'text-background-padding':4}},
        {selector:'edge.active',style:{opacity:.95,width:2,'line-color':'#b4e2e7'}}
      ],layout:{name:'preset',fit:false}});
    container.append(tooltip);
    function readableLabels(){
      if(!cy)return;const z=cy.zoom();
      cy.nodes().style({'font-size':11/z,'text-max-width':155/z,'text-outline-width':1.5/z,'text-margin-y':6/z});
      cy.nodes().removeClass('label-visible');
      const boxes=[],width=cy.width(),height=cy.height();
      const candidates=cy.nodes().filter(n=>n.hasClass('active')||n.hasClass('adjacent')||n.data('landmark')||messageMode||graph.focus_id||z>1.5)
        .sort((a,b)=>Number(b.hasClass('active'))-Number(a.hasClass('active'))||Number(b.hasClass('adjacent'))-Number(a.hasClass('adjacent'))||Number(b.data('hub'))-Number(a.data('hub'))||(b.data('degree')||0)-(a.data('degree')||0));
      for(const node of candidates){
        const p=node.renderedPosition(),r=Math.max(3,node.renderedOuterWidth()/2);
        const w=Math.min(155,measure?.measureText(node.data('label')).width||130),h=15;
        const positions=[
          {x:p.x-w/2,y:p.y+r+7,v:'bottom',a:'center',dx:0,dy:7},
          {x:p.x-w/2,y:p.y-r-7-h,v:'top',a:'center',dx:0,dy:-7},
          {x:p.x+r+8,y:p.y-h/2,v:'center',a:'right',dx:8,dy:0},
          {x:p.x-r-8-w,y:p.y-h/2,v:'center',a:'left',dx:-8,dy:0}];
        const chosen=positions.find(b=>b.x>=4&&b.y>=4&&b.x+w<width-4&&b.y+h<height-4
          &&!boxes.some(a=>b.x<a.x+a.w+5&&b.x+w+5>a.x&&b.y<a.y+a.h+4&&b.y+h+4>a.y));
        if(chosen){
          boxes.push({...chosen,w,h});node.addClass('label-visible');
          node.style({'text-valign':chosen.v,'text-halign':chosen.a,'text-margin-x':chosen.dx/z,'text-margin-y':chosen.dy/z});
        }
      }
    }
    updateLabels=readableLabels;
    cy.on('viewport',()=>{cancelAnimationFrame(labelFrame);labelFrame=requestAnimationFrame(readableLabels);});
    cy.on('tap','node',event=>choose(event.target));
    cy.on('tap','edge',event=>{cy.elements().removeClass('muted active adjacent');event.target.addClass('active');readableLabels();onEdge(event.target.data());});
    cy.on('tap',event=>{if(event.target===cy){cy.elements().removeClass('muted active adjacent');tooltip.hidden=true;readableLabels();}});
    cy.on('dbltap','node',event=>onNode({...event.target.data(),label:event.target.data('fullLabel')},true));
    cy.on('mouseover','node',event=>{
      const node=event.target,p=node.renderedPosition();node.addClass('hovered');tooltip.textContent=node.data('fullLabel');tooltip.hidden=false;
      tooltip.style.left=`${Math.max(8,Math.min(container.clientWidth-240,p.x+16))}px`;
      tooltip.style.top=`${Math.max(8,p.y-32)}px`;container.style.cursor='pointer';
    });
    cy.on('mouseout','node',event=>{event.target.removeClass('hovered');tooltip.hidden=true;container.style.cursor='';});
    const selected=graph.focus_id&&cy.getElementById(graph.focus_id);if(selected?.length)choose(selected,false);
    container.setAttribute('role','img');
    container.setAttribute('aria-label',messageMode?'Stored message and branch graph. Use the message list for keyboard access.':'Connected chat map. Search the map or use the chat list for keyboard access.');
    // Wait for the workspace to be visible and sized before the library fits its layout.
    const instance=cy;
    renderFrame=requestAnimationFrame(()=>{
      if(cy!==instance)return;cy.resize();
      const layout=cy.layout(messageMode
        ? {name:'breadthfirst',directed:true,padding:45,spacingFactor:1.4,avoidOverlap:true,nodeDimensionsIncludeLabels:true,animate:false}
        : {name:'cose',animate:false,randomize:true,padding:42,nodeRepulsion:()=>11000,idealEdgeLength:()=>85,
          edgeElasticity:()=>90,gravity:.7,componentSpacing:90,numIter:1000,nodeDimensionsIncludeLabels:false});
      layout.one('layoutstop',()=>{
        if(cy!==instance)return;
        cy.resize();cy.fit(undefined,42);readableLabels();
      });
      layout.run();
    });
  }
  return {render,
    select:id=>{const node=cy?.getElementById(id);if(node?.length){choose(node);cy.center(node);}},
    search:text=>{
      if(!cy)return [];
      const q=String(text||'').toLocaleLowerCase().trim();cy.elements().removeClass('muted');
      if(!q)return [];
      const hits=cy.nodes().filter(n=>String(n.data('fullLabel')).toLocaleLowerCase().includes(q));
      cy.elements().difference(hits.closedNeighborhood()).addClass('muted');
      return hits.map(n=>({...n.data(),label:n.data('fullLabel')})).slice(0,8);
    },
    zoom:factor=>{if(cy)cy.zoom({level:Math.max(.2,Math.min(4,cy.zoom()*factor)),renderedPosition:{x:cy.width()/2,y:cy.height()/2}});},
    fit:()=>{if(cy){cy.elements().removeClass('muted active adjacent');cy.fit(undefined,42);updateLabels();}},
    resize:()=>cy?.resize(),destroy:()=>{observer.disconnect();cancelAnimationFrame(resizeFrame);cancelAnimationFrame(labelFrame);cancelAnimationFrame(renderFrame);cy?.destroy();}};
}
function shorten(text){text=String(text||'Untitled').replace(/\s+/g,' ');return text.length>48?text.slice(0,45)+'…':text;}
