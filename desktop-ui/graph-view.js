import cytoscape from './vendor/cytoscape.esm.min.mjs';
import {MapViewState,mapKey} from './map-state.js';

export function runFiniteGraphLayout(cy, messageMode, onComplete) {
  const layout=cy.layout(messageMode
    ? {name:'breadthfirst',directed:true,padding:45,spacingFactor:1.4,avoidOverlap:true,nodeDimensionsIncludeLabels:true,animate:false}
    : {name:'cose',animate:false,randomize:false,padding:42,nodeRepulsion:()=>11000,idealEdgeLength:()=>85,
      edgeElasticity:()=>90,gravity:.7,componentSpacing:90,numIter:350,nodeDimensionsIncludeLabels:false});
  layout.one('layoutstop',onComplete);
  layout.run();
  return layout;
}

export function createGraphView(container, onNode, onEdge) {
  let css=getComputedStyle(document.documentElement);const color=(name,fallback)=>css.getPropertyValue(name).trim()||fallback;
  const theme={canvas:color('--canvas','#0e0f12'),text:color('--text','#e6e9ec'),muted:color('--muted','#9aa3ac'),accent:color('--accent','#d2a24c'),mono:color('--mono','monospace')};
  const palette=[theme.accent,'#a99c84','#c4b597','#8b9294','#d6c292','#b09362','#a4a39b','#c3b084'];
  const kindColors={repository:color('--map-repository',theme.accent),pull_request:color('--map-pr',theme.accent),issue:color('--map-issue','#b07f28'),domain:color('--map-domain',theme.muted),file_artifact:color('--map-file',theme.muted)};
  const views=new MapViewState();let currentKey,layoutReady=false;
  let cy, resizeFrame, labelFrame, enterTimer, updateLabels=()=>{};
  const measure=document.createElement('canvas').getContext('2d');if(measure)measure.font=`11px ${theme.mono}`;
  const tooltip=document.createElement('div');tooltip.className='map-tooltip';tooltip.hidden=true;
  const observer=new ResizeObserver(()=>{
    cancelAnimationFrame(resizeFrame);
    resizeFrame=requestAnimationFrame(()=>{cy?.resize();updateLabels();});
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
    css=getComputedStyle(document.documentElement);
    for(const key of ['canvas','text','muted','accent','mono'])theme[key]=color(`--${key}`,theme[key]);
    palette[0]=theme.accent;
    Object.assign(kindColors,{repository:color('--map-repository',theme.accent),pull_request:color('--map-pr',theme.accent),issue:color('--map-issue',theme.accent),domain:color('--map-domain',theme.muted),file_artifact:color('--map-file',theme.muted)});
    cancelAnimationFrame(labelFrame);clearTimeout(enterTimer);
    if(cy&&layoutReady)views.save(currentKey,{positions:Object.fromEntries(cy.nodes().map(n=>[n.id(),n.position()])),zoom:cy.zoom(),pan:cy.pan(),width:cy.width(),height:cy.height()});
    currentKey=mapKey(graph,messageMode);const prior=views.get(currentKey);layoutReady=false;
    cy?.destroy();cy=undefined;container.replaceChildren();tooltip.hidden=true;
    if(!graph.nodes.length){
      const empty=document.createElement('p');empty.className='graph-empty';
      empty.textContent='No strong connections found in this scope. Your chats are still available in the sidebar.';
      container.append(empty);return;
    }
    const elements=graph.nodes.map(node=>({data:{...node,
      label:shorten(messageMode&&node.excerpt?`${node.label} · ${node.excerpt}`:node.label),fullLabel:node.label,
      color:kindColors[node.kind]||palette[(node.cluster||0)%palette.length],size:messageMode?12:Math.min(22,11+(node.degree||0)*2)},position:prior?.positions[node.id],
      classes:[node.kind,node.hub?'hub':'',node.landmark||messageMode?'landmark':'',node.selected?'on-branch':'',node.id===graph.focus_id?'focus-chat':''].join(' ')}));
    for(const [i,edge] of graph.edges.entries())elements.push({data:{...edge,id:`edge:${i}`},classes:edge.basis});
    cy=cytoscape({container,elements,minZoom:.2,maxZoom:4,wheelSensitivity:.2,selectionType:'single',
      style:[
        {selector:'node',style:{shape:'ellipse',width:'data(size)',height:'data(size)',
          'background-color':'data(color)','border-width':1.5,'border-color':theme.canvas,
          label:'',color:theme.muted,'font-family':theme.mono,'font-size':11,
          'text-valign':'bottom','text-halign':'center','text-margin-y':7,'text-wrap':'ellipsis','text-max-width':135,
           'text-outline-color':theme.canvas,'text-outline-width':2,'min-zoomed-font-size':0,'overlay-padding':8,'text-events':'yes'}},
        {selector:'node.repository',style:{shape:'diamond',width:25,height:25}},
        {selector:'node.pull_request',style:{shape:'hexagon',width:23,height:20}},
        {selector:'node.issue',style:{shape:'triangle',width:23,height:22}},
        {selector:'node.domain',style:{shape:'round-rectangle',width:25,height:20}},
        {selector:'node.file_artifact',style:{shape:'rectangle',width:20,height:25}},
        {selector:'node.label-visible,node.hovered',style:{label:'data(label)'}},
        {selector:'node.hub',style:{color:theme.text,'font-weight':600,'font-size':12}},
        {selector:'node.focus-chat',style:{width:25,height:25,'border-width':3,'border-color':theme.text,color:theme.text}},
        {selector:'node.message',style:{'background-color':'#8796a6','text-max-width':180}},
        {selector:'node.on-branch',style:{'background-color':theme.accent}},
        {selector:'edge',style:{width:1,'line-color':theme.muted,'curve-style':'bezier',opacity:.32,'overlay-padding':7}},
        {selector:'edge.project',style:{'line-color':theme.accent,width:1.2,opacity:.5}},
        {selector:'edge.shared_keyword',style:{'line-color':theme.muted}},
        {selector:'edge.shared_entity',style:{'line-color':kindColors.repository,width:1.5,opacity:.68}},
        {selector:'edge.entity_reference',style:{'target-arrow-shape':'triangle','target-arrow-color':theme.muted,'arrow-scale':.6}},
        {selector:'edge.parent',style:{'target-arrow-shape':'triangle','target-arrow-color':'#738a99','arrow-scale':.7}},
        {selector:'.muted',style:{opacity:.12}},
        {selector:'node.adjacent',style:{color:theme.text,'text-background-color':theme.canvas,'text-background-opacity':.9,'text-background-padding':3}},
        {selector:'node.active',style:{label:'data(label)','border-width':3,'border-color':theme.accent,color:theme.text,'font-weight':600,'text-background-color':theme.canvas,'text-background-opacity':1,'text-background-padding':4}},
        {selector:'edge.active',style:{opacity:.95,width:2,'line-color':theme.accent}}
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
    container.setAttribute('role','application');container.tabIndex=0;
    container.setAttribute('aria-label',messageMode?'Stored message and branch graph. Use arrow keys to inspect nodes, Enter to open the selected message, or use the message list.':'Connected chat map. Use arrow keys to inspect nodes, Enter to open the selected chat, or use the chat list.');
    container.onkeydown=event=>{
      if(!cy||!['ArrowRight','ArrowDown','ArrowLeft','ArrowUp','Enter'].includes(event.key))return;
      const nodes=cy.nodes().sort((a,b)=>String(a.id()).localeCompare(String(b.id())));if(!nodes.length)return;
      event.preventDefault();const current=nodes.findIndex(node=>node.hasClass('active'));
      if(event.key==='Enter'){const node=current>=0?nodes[current]:nodes[0];onNode({...node.data(),label:node.data('fullLabel')},true);return;}
      const backwards=event.key==='ArrowLeft'||event.key==='ArrowUp';const next=(current+(backwards?-1:1)+nodes.length)%nodes.length;choose(nodes[next]);
      cy.animate({center:{eles:nodes[next]}},{duration:reducedMotion()?0:220});
    };
    // Wait for the workspace to be visible and sized before the library fits its layout.
    const instance=cy;cy.resize();
    if(prior){
      if(Math.abs(prior.width-cy.width())<40&&Math.abs(prior.height-cy.height())<40)cy.viewport({zoom:prior.zoom,pan:prior.pan});
      else fitGraph();layoutReady=true;readableLabels();animateEntry();return;
    }
    runFiniteGraphLayout(cy,messageMode,()=>{
      if(cy!==instance)return;
      cy.resize();fitGraph();layoutReady=true;readableLabels();animateEntry();
    });
  }
  function animateEntry(){
    if(reducedMotion())return;
    container.classList.remove('graph-entering');void container.offsetWidth;container.classList.add('graph-entering');
    enterTimer=setTimeout(()=>container.classList.remove('graph-entering'),210);
  }
  function reducedMotion(){return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches||false;}
  function fitGraph(){if(!cy)return;cy.fit(undefined,42);if(cy.zoom()>1.5){cy.zoom(1.5);cy.center();}}
  return {render,
     select:(id,extra)=>{const node=cy?.getElementById(id);if(node?.length){if(extra)node.data(extra);choose(node);cy.animate({center:{eles:node}},{duration:reducedMotion()?0:220});}},
    search:text=>{
      if(!cy)return [];
      const q=String(text||'').toLocaleLowerCase().trim();cy.elements().removeClass('muted');
      if(!q)return [];
      const hits=cy.nodes().filter(n=>String(n.data('fullLabel')).toLocaleLowerCase().includes(q));
      cy.elements().difference(hits.closedNeighborhood()).addClass('muted');
      return hits.map(n=>({...n.data(),label:n.data('fullLabel')})).slice(0,8);
    },
    zoom:factor=>{if(cy)cy.zoom({level:Math.max(.2,Math.min(4,cy.zoom()*factor)),renderedPosition:{x:cy.width()/2,y:cy.height()/2}});},
    fit:()=>{if(cy){cy.elements().removeClass('muted active adjacent');fitGraph();updateLabels();}},
     resize:()=>cy?.resize(),destroy:()=>{observer.disconnect();cancelAnimationFrame(resizeFrame);cancelAnimationFrame(labelFrame);clearTimeout(enterTimer);cy?.destroy();}};
}
function shorten(text){text=String(text||'Untitled').replace(/\s+/g,' ');return text.length>48?text.slice(0,45)+'…':text;}
