// Diagnostic exports contain counts and enums only. Never include localStorage
// rows, upstream IDs, transcripts, cookies, bearer tokens or raw exceptions.
export function safeCaptureCommitted(health) {
  const counters=health?.capture?.counters;
  if(!counters || typeof counters!=='object')return 0;
  return ['committed_imported','committed_updated'].reduce((sum,key)=>{
    const n=counters[key];
    return sum+(Number.isSafeInteger(n)&&n>0?Math.min(n,1_000_000_000):0);
  },0);
}

export function safePendingSummary(summary) {
  const count=n=>Number.isSafeInteger(n)&&n>=0?Math.min(n,1000):0;
  return {
    pendingCount:count(summary?.pending_count),
    unsavedDraftCount:count(summary?.unsaved_draft_count),
    pendingStorageAvailable:summary?.storage_available===true && summary?.persistence_error!==true,
  };
}

if(typeof document!=='undefined'){
  const button=document.getElementById('export-diagnostics');
  const status=document.getElementById('diagnostics-export-status');
  button?.addEventListener('click',async()=>{
    button.disabled=true;
    status.textContent='Preparing metadata-only diagnostics…';
    try{
      if(typeof window.ChatHub?.invoke!=='function')throw Error('native_unavailable');
      const pending=safePendingSummary(window.ChatHub.pendingSummary?.());
      let captureSavedCount=0;
      try{captureSavedCount=safeCaptureCommitted(await window.ChatHub.invoke('service_health'));}
      catch{ /* Service may be unavailable: export history and pending state anyway. */ }
      const receipt=await window.ChatHub.invoke('export_diagnostics',{
        ...pending,captureSavedCount
      });
      status.textContent=receipt?.cancelled
        ? 'Diagnostic export cancelled.'
        : receipt?.published
          ? 'Metadata-only diagnostics saved. No messages or credentials included.'
          : 'Diagnostics were not saved.';
    }catch{
      // Do not print exception text: native errors can contain local paths.
      status.textContent='Diagnostic export failed or destination already exists. Try another filename.';
    }finally{button.disabled=false;}
  });
}
