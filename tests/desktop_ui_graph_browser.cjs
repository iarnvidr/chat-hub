const {chromium}=require(process.env.PLAYWRIGHT_MODULE || '/home/coder/.cache/chat-hub-f4-playwright/node_modules/playwright');
const http=require('node:http'),fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict');

const root=path.resolve(__dirname,'..'),uiRoot=path.join(root,'desktop-ui');
const chats=Array.from({length:12},(_,i)=>({
  id:`chat:local-${i}`,kind:'chat',label:`Design research conversation ${i+1}`,
  conversation_id:`local-${i}`,source_id:`source-${i}`,provider:'chatgpt',account_scope:'chatgpt:observed:fixture',
  updated_at_ms:1_700_000_000_000-i*1000,message_count:20,branch_count:2,cached_archive:false,
}));
const focus=chats[0], neighbor=chats[1];
const defaultGraph={focus_id:null,truncated:true,nodes:chats,edges:[
  {source:chats[0].id,target:chats[2].id,basis:'project',reason:'Same project: Design research',evidence:['Design research'],score:1_000_000},
  {source:chats[3].id,target:chats[4].id,basis:'project',reason:'Same project: Design research',evidence:['Design research'],score:1_000_000},
]};
const focusedGraph={focus_id:focus.id,truncated:false,nodes:[focus,neighbor],edges:[
  {source:focus.id,target:neighbor.id,basis:'shared_keyword',reason:'Shares 2 indexed keywords',evidence:['migration','workflow'],score:3420},
]};
async function canvasMetrics(page){
  return page.locator('#graph-canvas canvas').evaluateAll(canvases=>{
    const results=[];
    for(const canvas of canvases){
      const rect=canvas.getBoundingClientRect(),ctx=canvas.getContext('2d');
      if(!ctx||!canvas.width||!canvas.height)continue;
      const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
      let painted=0,bright=0;
      for(let i=0;i<pixels.length;i+=4){if(pixels[i+3]){painted++;if(pixels[i]>205&&pixels[i+1]>205&&pixels[i+2]>205)bright++;}}
      results.push({width:rect.width,height:rect.height,painted,bright});
    }
    return results;
  });
}
function assertPaintedCanvas(metrics,viewport){
  assert.ok(metrics.length>0,`${viewport}: Cytoscape created drawing canvases`);
  assert.ok(metrics.every(m=>Number.isFinite(m.width)&&Number.isFinite(m.height)&&m.width>100&&m.height>100),`${viewport}: canvas has finite viewport geometry`);
  assert.ok(metrics.some(m=>m.painted>500&&m.bright>20),`${viewport}: chat labels and graph shapes were painted`);
}

const mime={'.html':'text/html','.js':'text/javascript','.mjs':'text/javascript','.css':'text/css','.svg':'image/svg+xml','.json':'application/json'};
const server=http.createServer((req,res)=>{
  const pathname=decodeURIComponent(new URL(req.url,'http://localhost').pathname);
  const file=path.resolve(root,pathname.replace(/^\/+/,''));
  if(!file.startsWith(uiRoot+path.sep) && file!==path.join(uiRoot,'index.html')){res.writeHead(403).end();return;}
  fs.readFile(file,(err,data)=>{if(err){res.writeHead(404).end();return;}res.setHeader('Content-Type',mime[path.extname(file)]||'application/octet-stream');res.end(data);});
});

(async()=>{
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const browser=await chromium.launch({headless:true,...(process.env.CHROMIUM_PATH?{executablePath:process.env.CHROMIUM_PATH,args:['--no-sandbox']}: {})});
  try{
    const page=await browser.newPage({viewport:{width:1280,height:900},deviceScaleFactor:1});
    const pageErrors=[];page.on('pageerror',error=>pageErrors.push(error.message));
    await page.addInitScript(({chats,defaultGraph,focusedGraph})=>{
      window.__calls=[];
      window.__TAURI_INTERNALS__={invoke:async(command,args={})=>{
        window.__calls.push({command,args});
        if(command==='list_conversations')return {items:chats.map(chat=>({id:chat.conversation_id,title:chat.label,provider:'chatgpt',message_count:chat.message_count})),next_cursor:null,library_revision:1};
        if(command==='catalog_overview')return {index:{stale:false,index_version:2},stats:{chats:40,projects:3,messages:820,branch_points:56}};
        if(command==='catalog_graph')return args.focusId?focusedGraph:defaultGraph;
        if(command==='list_projects')return {projects:[{project_key:'project:design',title:'Design research',conversation_count:8}]};
        if(command==='update_status')return {version:'fixture',message:'Ready'};
        if(command==='service_info')return {};
        if(command==='service_health')return {healthy:true};
        if(command==='get_capture_preferences')return {enabled:false};
        if(command==='chatgpt_history_status')return {state:'idle'};
        if(command==='get_chatgpt_connection')return {state:'signed_out'};
        if(command==='check_for_updates')return {status:'current',message:'Current'};
        if(command==='rebuild_catalog_index')return {stale:false,index_version:2};
        throw new Error(`unexpected native command: ${command}`);
      }};
    },{chats,defaultGraph,focusedGraph});

    await page.goto(`http://127.0.0.1:${server.address().port}/desktop-ui/index.html`);
    await page.locator('#library-overview').waitFor({state:'visible'});
    await page.waitForFunction(()=>window.__calls.some(call=>call.command==='catalog_graph'&&call.args.focusId==null));
    await page.locator('#graph-canvas canvas').first().waitFor({state:'visible'});
    assertPaintedCanvas(await canvasMetrics(page),'1280x900');
    for(const viewport of [{width:1100,height:760},{width:1700,height:1000}]){
      await page.setViewportSize(viewport);await page.waitForTimeout(100);
      assertPaintedCanvas(await canvasMetrics(page),`${viewport.width}x${viewport.height}`);
    }
    await page.setViewportSize({width:1280,height:900});await page.waitForTimeout(100);
    assert.equal(await page.locator('#graph-canvas').getAttribute('role'),'img');
    assert.match(await page.locator('#graph-canvas').getAttribute('aria-label'),/Labelled chat connections/);
    assert.equal(await page.locator('#overview-recent .overview-chat').count(),12);

    await page.locator('#overview-recent .overview-chat').first().click();
    await page.waitForFunction(()=>window.__calls.some(call=>call.command==='catalog_graph'&&call.args.focusId==='local-0'));
    await page.waitForFunction(()=>document.querySelector('#graph-selection').innerText.includes('Shares 2 indexed keywords'));
    assert.match(await page.locator('#graph-selection').innerText(),/Shares 2 indexed keywords/);
    assert.match(await page.locator('#graph-selection').innerText(),/Design research conversation 2/);
    const focusedCanvas=await page.locator('#graph-canvas canvas').evaluateAll(canvases=>canvases.some(canvas=>canvas.width>0&&canvas.height>0));
    assert.equal(focusedCanvas,true,'focused neighborhood remains rendered');

    await page.locator('#overview-back').click();
    await page.waitForFunction(()=>window.__calls.some(call=>call.command==='catalog_graph'&&call.args.focusId==null));
    await page.waitForFunction(()=>document.querySelectorAll('#overview-recent .overview-chat').length===12);
    assert.equal(await page.locator('#overview-title').innerText(),'Chat overview');
    assert.deepEqual(pageErrors,[]);
    console.log('PASS Cytoscape canvas paints labelled chats; focus requests exact local chat ID, renders relationship reason, and Back restores recent overview');
  }finally{await browser.close();server.close();}
})().catch(error=>{console.error(error);server.close();process.exitCode=1;});
