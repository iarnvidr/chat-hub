const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const fixture=name=>JSON.parse(fs.readFileSync(path.resolve(__dirname,'fixtures/api-v1/'+name),'utf8'));const source=fs.readFileSync(require.resolve('../desktop-ui/render.js'),'utf8').replaceAll('export function','function')+'\nglobalThis.api={selectedPath,branchChoices,orderedMessages,messageText,hasUnsupportedContent,renderMarkdown};';const ctx=vm.createContext({URL,Map,Set,String,navigator:{clipboard:{writeText:async()=>{}}}});vm.runInContext(source,ctx);const {selectedPath,branchChoices,orderedMessages,messageText,hasUnsupportedContent,renderMarkdown}=ctx.api;

class TestNode {
 constructor(tagName='#text',text=''){this.tagName=tagName;this.nodeType=tagName==='#text'?3:1;this._text=text;this.children=[];this.dataset={};this.style={};this.attributes={};}
 append(...nodes){for(const node of nodes){node.parentNode=this;this.children.push(node);}}
 replaceChildren(...nodes){this.children=[];this._text='';this.append(...nodes);}
 set textContent(value){this._text=String(value);this.children=[];}
 get textContent(){return this.nodeType===3?this._text:this._text+this.children.map(node=>node.textContent).join('');}
 setAttribute(name,value){this.attributes[name]=String(value);}
}
const testDocument={createElement:name=>new TestNode(name),createTextNode:value=>new TestNode('#text',String(value))};
const walk=(node,predicate)=>[...(predicate(node)?[node]:[]),...node.children.flatMap(child=>walk(child,predicate))];
function markdown(sourceText){const root=new TestNode('div');renderMarkdown(sourceText,root,testDocument);return root;}
test('ordered split messages preserve IDs and null-message ancestry',()=>{const d={selection:{selected_node_id:'b'},nodes:[{id:'r',parent_id:null,message_id:'m1'},{id:'gap',parent_id:'r',message_id:null},{id:'b',parent_id:'gap',message_id:'m2'},{id:'sib',parent_id:'gap',message_id:'m3'}],messages:[{id:'m1',node_id:'r',content:'root'},{id:'m2',node_id:'b',content:'branch'},{id:'m3',node_id:'sib',content:'sibling'}],thread_node_ids:['r','gap','b']};assert.deepEqual(Array.from(orderedMessages(d),m=>m.content),['root','branch']);assert.deepEqual(Array.from(orderedMessages(d,'sib'),m=>m.content),['root','sibling']);});
test('missing IDs and graph cycles fail',()=>{assert.throws(()=>selectedPath({nodes:[{id:'a',parent_id:'gone',message_id:null}]},'a'),/missing parent/);assert.throws(()=>orderedMessages({selection:{selected_node_id:'a'},nodes:[{id:'a',parent_id:null,message_id:'bad'}],messages:[],thread_node_ids:['a']}),/missing message/);assert.throws(()=>selectedPath({nodes:[{id:'a',parent_id:'b'},{id:'b',parent_id:'a'}]},'a'),/cycle/);});
test('parts only expose text, unknown content neutral and raw ignored',()=>{const sentinel='RAW_SECRET_SENTINEL';assert.equal(messageText({content:'Visible',parts:[{kind:'unknown',raw:sentinel}]}),'Visible');assert.equal(messageText({content:'',parts:[{kind:'text',text:'Hello',raw:sentinel}]}),'Hello');assert.equal(messageText({content:'',parts:[{kind:'unknown',raw:sentinel}]}),'');assert.equal(hasUnsupportedContent({content:'Visible',parts:[{kind:'unknown',raw:sentinel}]}),true);assert.doesNotMatch(messageText({content:'',parts:[{kind:'unknown',raw:sentinel}]}),new RegExp(sentinel));});
test('official detail fixture preserves split-message ancestry and nullable anchors',()=>{const d=fixture('conversation-selected.json');assert.deepEqual(Array.from(orderedMessages(d),m=>m.id),['msg-u','msg-a']);assert.equal(d.nodes[0].message_id,null);assert.equal(d.messages[0].author.role,'user');assert.equal(d.messages[0].created_at_ms,1700000000125);});
test('branches use normative object leaf_node_id shape',()=>{assert.deepEqual(Array.from(branchChoices({nodes:[{id:'a'},{id:'b'}],branches:[{leaf_node_id:'b',selected:true}]}),n=>n.id),['b']);});
test('incomplete ancestry thread follows API-authoritative IDs only',()=>{const d={selection:{selected_node_id:'x',thread_complete:false},nodes:[{id:'x',parent_id:'missing',message_id:'m'}],messages:[{id:'m',content:'ok'}],thread_node_ids:['x']};assert.deepEqual(Array.from(orderedMessages(d),m=>m.content),['ok']);});

test('GFM tables parse aligned cells, escaped pipes, code pipes, and nested inline formatting',()=>{
 const root=markdown('| Name | Formula | Note |\n|:---|---:|:---:|\n| **Ada *Lovelace*** | ``a|b`` | use \\| safely |');
 const tables=walk(root,node=>node.tagName==='table'),headers=walk(root,node=>node.tagName==='th'),cells=walk(root,node=>node.tagName==='td');
 assert.equal(tables.length,1);assert.equal(headers.length,3);assert.equal(cells.length,3);
 assert.deepEqual(headers.map(cell=>cell.style.textAlign),['left','right','center']);
 assert.equal(cells[0].textContent,'Ada Lovelace');
 assert.equal(walk(cells[0],node=>node.tagName==='strong').length,1);
 assert.equal(walk(cells[0],node=>node.tagName==='em').length,1);
 assert.equal(walk(cells[1],node=>node.tagName==='code')[0].textContent,'a|b');
 assert.equal(cells[2].textContent,'use | safely');
});

test('lists, blockquotes, rules, fenced code, and nested inline formatting create semantic elements',()=>{
 const root=markdown('- first\n- **second**\n  - nested\n\n1. ordered\n2. next\n\n> quoted *text*\n>\n> second line\n\n---\n\n```js\nconst value = "<b>";\n```\n\n***bold and italic*** with ~~strike~~');
 assert.equal(walk(root,node=>node.tagName==='ul').length,2);
 assert.equal(walk(root,node=>node.tagName==='ol').length,1);
 assert.equal(walk(root,node=>node.tagName==='li').length,5);
 assert.equal(walk(root,node=>node.tagName==='blockquote').length,1);
 assert.equal(walk(root,node=>node.tagName==='hr').length,1);
 assert.equal(walk(root,node=>node.tagName==='pre').length,1);
 assert.equal(walk(root,node=>node.tagName==='code')[0].textContent,'const value = "<b>";');
 assert.equal(walk(root,node=>node.tagName==='del').length,1);
 assert.equal(walk(root,node=>node.tagName==='strong').length,2);
 assert.equal(walk(root,node=>node.tagName==='em').length,2);
});

test('HTML stays inert and links retain the HTTPS-only policy',()=>{
 const secret='window.xss = true';
 const root=markdown(`<img src=x onerror="${secret}"> <script>${secret}</script> [safe](https://example.com/path) [blocked](javascript:alert(1))`);
 assert.equal(walk(root,node=>node.tagName==='img'||node.tagName==='script').length,0);
 assert.match(root.textContent,/\<img src=x onerror=/);
 assert.match(root.textContent,/\<script>/);
 assert.match(root.textContent,/window\.xss = true/); // Markup and its contents stay visible text, not executable nodes.
 const links=walk(root,node=>node.tagName==='a');
 assert.equal(links.length,1);assert.equal(links[0].href,'https://example.com/path');
 assert.equal(links[0].target,'_blank');assert.equal(links[0].rel,'noopener noreferrer');
 assert.match(root.textContent,/\[blocked\]\(javascript:alert\(1\)\)/);
 const identifier=markdown('request_id_value_ stays literal');
 assert.equal(walk(identifier,node=>node.tagName==='em').length,0);
 assert.match(identifier.textContent,/request_id_value_/);
});
