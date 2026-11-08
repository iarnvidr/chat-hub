const test=require('node:test');
const assert=require('node:assert/strict');
const {readFileSync}=require('node:fs');
const {resolve}=require('node:path');
const modulePromise=import('data:text/javascript;base64,'+readFileSync(resolve(__dirname,'../desktop-ui/capture-status.js')).toString('base64'));
test('capture details distinguish confirmed commits, active work and failures',async()=>{
  const {formatCaptureDetails}=await modulePromise;
  const text=formatCaptureDetails({status:'available',enabled:true,counters:{committed_imported:2,committed_updated:1,committed_unchanged:1,active:3,store_failed:1,dropped:2}});
  assert.match(text,/4 confirmed database outcomes/);
  assert.match(text,/3 active captures/);
  assert.match(text,/3 failed or dropped/);
  assert.match(text,/not saved transcripts/);
});
test('unknown health and failed metadata writes never claim a saved transcript',async()=>{
  const {formatCaptureDetails,formatPendingPersistence}=await modulePromise;
  assert.match(formatCaptureDetails(null),/unknown/);
  assert.match(formatPendingPersistence({metadata_persisted:false}),/could not be saved/);
  assert.match(formatPendingPersistence({code:'pending_storage_invalid'}),/preserved rather than overwritten/);
});
