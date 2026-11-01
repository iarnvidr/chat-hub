const test=require('node:test');
const assert=require('node:assert/strict');

test('list preference remains until an explicit graph action, which restores the canvas mode',async()=>{
  const {normalizeOverviewMode,overviewModeForAction}=await import('../desktop-ui/overview-mode.js');
  assert.equal(normalizeOverviewMode('list'),'list');
  assert.equal(normalizeOverviewMode('unknown'),'graph');
  assert.equal(overviewModeForAction('unchanged','list'),'list');
  assert.equal(overviewModeForAction('graph-drilldown','list'),'graph');
  assert.equal(overviewModeForAction('show-list','graph'),'list');
});
