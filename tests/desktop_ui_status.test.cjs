const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require.resolve('../desktop-ui/status.js'), 'utf8').replace('export async function', 'async function') + '\nglobalThis.updateStatus=updateStatus;';
const context = vm.createContext({ setTimeout, Promise }); vm.runInContext(source, context); const updateStatus = context.updateStatus;
function harness(invoke) { const values = {}; return { values, run: () => updateStatus({ invoke, setVersion:v=>values.version=v, setMessage:v=>values.message=v, setHealth:v=>values.health=v, setRetryVisible:v=>values.retry=v, delay:async()=>{} }) }; }
test('transient health failure retries and reaches ready', async()=>{let count=0;const h=harness(async n=>{if(n==='update_status')return {version:'1.2',message:'Current'};if(n==='service_info')return {};count++;if(count===1)throw Error('transient');return {healthy:true};});await h.run();assert.equal(count,2);assert.equal(h.values.health,'Local service READY.');assert.equal(h.values.retry,false);});
test('failed bounded attempts expose retry and button wiring',async()=>{let count=0;const h=harness(async n=>{count++;if(n==='update_status')return {version:'1.2',message:'Current'};if(n==='service_info')return {};return {healthy:false};});await h.run();assert.equal(count,5);assert.equal(h.values.retry,true);assert.match(fs.readFileSync(require.resolve('../desktop-ui/index.html'),'utf8'),/retry-health/);assert.match(fs.readFileSync(require.resolve('../desktop-ui/app.js'),'utf8'),/retry-health'\)\.addEventListener\('click',status\)/);});
test('missing bridge is visibly retryable',async()=>{const h=harness(null);await h.run();assert.match(h.values.message,/bridge unavailable/);assert.equal(h.values.retry,true);});
test('metadata success with health failure is not ready',async()=>{const h=harness(async n=>{if(n==='update_status')return {version:'1.2',message:'Current'};if(n==='service_info')return {};throw Error('offline');});await h.run();assert.notEqual(h.values.health,'Local service READY.');assert.equal(h.values.retry,true);});
