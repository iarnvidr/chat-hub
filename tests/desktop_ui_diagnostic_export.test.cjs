const test=require('node:test'),assert=require('node:assert/strict');
const UUID='123e4567-e89b-12d3-a456-426614174000';

test('diagnostic summary exports only bounded numbers and storage availability',async()=>{
  const {safePendingSummary,safeCaptureCommitted}=await import('../desktop-ui/diagnostic-export.js');
  const metadata=safePendingSummary({pending_count:3,unsaved_draft_count:1,storage_available:false,persistence_error:true,
    source_id:UUID,account_scope:'private',transcript:'do not export'});
  assert.deepEqual(metadata,{pendingCount:3,unsavedDraftCount:1,pendingStorageAvailable:false});
  assert.doesNotMatch(JSON.stringify(metadata),/source_id|account_scope|transcript|123e4567/);
  assert.deepEqual(safePendingSummary({pending_count:-3,unsaved_draft_count:Infinity,storage_available:1}),
    {pendingCount:0,unsavedDraftCount:0,pendingStorageAvailable:false});
  assert.equal(safeCaptureCommitted({capture:{counters:{committed_imported:2,committed_updated:3,
    secret:'private token',failed_body:'secret transcript'}}}),5);
  assert.equal(safeCaptureCommitted(null),0);
});

test('diagnostic export uses native save rather than exporting localStorage or raw responses',async()=>{
  const fs=require('node:fs');
  const source=fs.readFileSync(require.resolve('../desktop-ui/diagnostic-export.js'),'utf8');
  assert.match(source,/export_diagnostics/);
  assert.doesNotMatch(source,/localStorage|getItem|response_body|cookies/i);
  const rust=fs.readFileSync(require.resolve('../src-tauri/src/main.rs'),'utf8');
  assert.match(rust,/create_new\(true\)/);
  assert.match(rust,/metadata_only_no_transcript_or_account_identifiers/);
  assert.doesNotMatch(rust.slice(rust.indexOf('async fn export_diagnostics('),rust.indexOf('async fn backup_library(')),/serde_json::to_value\(&status\)|status\.message/);
});
