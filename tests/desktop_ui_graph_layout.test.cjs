const test=require('node:test');
const assert=require('node:assert/strict');

test('category graph layout settles synchronously with finite spread, independent of animation frames',async()=>{
  const [{runFiniteGraphLayout},{default:cytoscape}]=await Promise.all([
    import('../desktop-ui/graph-view.js'),
    import('../desktop-ui/vendor/cytoscape.esm.min.mjs'),
  ]);
  const nodes=Array.from({length:100},(_,index)=>({data:{id:`chat:${index}`}}));
  const edges=Array.from({length:155},(_,index)=>({data:{id:`edge:${index}`,source:`chat:${index%100}`,target:`chat:${(index*7+13)%100}`}}));
  const cy=cytoscape({headless:true,elements:[...nodes,...edges]});
  let finished=false;
  runFiniteGraphLayout(cy,false,()=>{finished=true;});
  try{
    const positions=cy.nodes().map(node=>node.position());
    assert.equal(finished,true,'layout completion must not wait for requestAnimationFrame');
    assert.equal(positions.length,100);
    assert.ok(positions.every(point=>Number.isFinite(point.x)&&Number.isFinite(point.y)));
    assert.ok(Math.max(...positions.map(point=>point.x))-Math.min(...positions.map(point=>point.x))>100);
    assert.ok(Math.max(...positions.map(point=>point.y))-Math.min(...positions.map(point=>point.y))>100);
  }finally{cy.destroy();}
});
