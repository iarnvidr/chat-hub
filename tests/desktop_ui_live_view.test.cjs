const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const load = async () => import('data:text/javascript;base64,' + Buffer.from(fs.readFileSync(require.resolve('../desktop-ui/live-stream.js'))).toString('base64'));

const live = (kind, messageId, delta = '', conversationId = null) =>
  ({type: 'live', event: {kind, messageId, delta, conversationId}});
const run = (reduceLive, state, ...actions) => actions.reduce(reduceLive, state);

test('live view needs server mode with the live capability; provider view stays desktop-only', async () => {
  const {liveAvailable, providerViewAvailable} = await load();
  assert.equal(liveAvailable({transport: 'http', capabilities: {live: true}}), true);
  for (const input of [
    {transport: 'http', capabilities: {live: false}}, {transport: 'http', capabilities: {}}, {transport: 'http'},
    {transport: 'fixture', capabilities: {live: true}}, {native: true, transport: 'tauri', capabilities: {live: true}}, {},
  ]) assert.equal(liveAvailable(input), false);
  assert.equal(providerViewAvailable({native: true}), true);
  assert.equal(providerViewAvailable({capabilities: {provider_view: true}}), true);
  for (const input of [{capabilities: {provider_view: false, live: true}}, {capabilities: {}}, {}])
    assert.equal(providerViewAvailable(input), false);
});

test('stream deltas build the reply progressively and done requests a library reload', async () => {
  const {initialLive, reduceLive, needsLibraryReload, statusLine} = await load();
  let state = run(reduceLive, initialLive(), {type: 'sending', conversationId: null}, {type: 'sent'});
  assert.equal(statusLine(state), 'Sent. Waiting for the reply…');
  state = run(reduceLive, state, live('start', 'm1', '', 'c1'), live('delta', 'm1', 'Hel'), live('delta', 'm1', 'lo'));
  assert.deepEqual([state.phase, state.text, state.conversationId], ['streaming', 'Hello', 'c1']);
  assert.equal(needsLibraryReload(state), false);
  state = reduceLive(state, live('done', 'm1'));
  assert.equal(state.phase, 'done');
  assert.equal(state.text, 'Hello');
  assert.equal(needsLibraryReload(state), true);
  assert.equal(reduceLive(state, {type: 'settled', keepText: false}).text, '');
  assert.equal(reduceLive(state, {type: 'settled', keepText: true}).text, 'Hello');
  // A different message id starts a fresh reply.
  assert.equal(reduceLive(state, live('delta', 'm2', 'next')).text, 'next');
});

test('resync, interrupted streams, busy, sign-in and offline states have distinct handling', async () => {
  const {initialLive, reduceLive, needsLibraryReload, statusLine, rejectionCode} = await load();
  const base = run(reduceLive, initialLive(), live('delta', 'm1', 'x'));
  assert.equal(needsLibraryReload(reduceLive(base, {type: 'resync'})), true);
  const interrupted = reduceLive(base, live('error', 'm1'));
  assert.equal(interrupted.phase, 'interrupted');
  assert.equal(needsLibraryReload(interrupted), true);
  assert.equal(reduceLive(initialLive(), {type: 'rejected', code: 'busy'}).phase, 'busy');
  assert.match(statusLine(reduceLive(initialLive(), {type: 'rejected', code: 'busy'})), /busy/);
  const signIn = reduceLive(initialLive(), {type: 'status', state: 'needs_sign_in'});
  assert.match(statusLine(signIn), /sign in with the local helper/i);
  assert.match(statusLine(reduceLive(initialLive(), {type: 'status', state: 'offline'})), /offline/);
  assert.equal(reduceLive(initialLive(), {type: 'rejected', code: 'browser_unavailable'}).service, 'offline');
  assert.equal(rejectionCode({status: 409}), 'busy');
  assert.equal(rejectionCode({code: 'browser_unavailable'}), 'browser_unavailable');
  assert.equal(rejectionCode(new Error('x')), 'failed');
});

test('live text is bounded and malformed or oversized events are dropped', async () => {
  const {initialLive, reduceLive, parseLiveEvent, MAX_LIVE_CHARS} = await load();
  const long = 'a'.repeat(16_384);
  let state = initialLive();
  for (let i = 0; i < Math.ceil(MAX_LIVE_CHARS / long.length) + 2; i++) state = reduceLive(state, live('delta', 'm', long));
  assert.equal(state.text.length, MAX_LIVE_CHARS);
  const ok = {kind: 'delta', message_id: 'm', conversation_id: null, text_delta: 'hi'};
  assert.deepEqual({...parseLiveEvent(JSON.stringify(ok))}, {kind: 'delta', conversationId: null, messageId: 'm', delta: 'hi'});
  for (const bad of ['not json', 'null', '"x"', JSON.stringify({...ok, kind: 'other'}), JSON.stringify({...ok, message_id: 5}),
    JSON.stringify({...ok, text_delta: 'x'.repeat(16_385)}), JSON.stringify({...ok, conversation_id: 7})]) assert.equal(parseLiveEvent(bad), null);
});

test('prompt validation matches the server limits in bytes', async () => {
  const {promptProblem, MAX_PROMPT_BYTES} = await load();
  assert.equal(promptProblem('hello'), '');
  assert.notEqual(promptProblem('   '), '');
  assert.equal(promptProblem('a'.repeat(MAX_PROMPT_BYTES)), '');
  assert.notEqual(promptProblem('a'.repeat(MAX_PROMPT_BYTES + 1)), '');
  assert.notEqual(promptProblem('é'.repeat(MAX_PROMPT_BYTES / 2 + 1)), ''); // 2 bytes each
});

test('the stream connection forwards events, resyncs and reconnects with a reload', async () => {
  const {connectLiveStream} = await load();
  const sources = [];
  class FakeSource {
    constructor(url) { this.url = url; this.handlers = {}; this.closed = false; sources.push(this); }
    addEventListener(name, handler) { this.handlers[name] = handler; }
    close() { this.closed = true; }
  }
  const seen = [];
  const connection = connectLiveStream({EventSource: FakeSource, dispatch: action => seen.push(action)});
  const source = sources[0];
  assert.equal(source.url, '/api/live/stream');
  source.handlers.open();
  assert.deepEqual(seen, []); // first open is not a resync
  source.handlers.live({data: JSON.stringify({kind: 'delta', message_id: 'm', conversation_id: null, text_delta: 'a'})});
  source.handlers.live({data: 'garbage'});
  source.handlers.resync({data: '{}'});
  source.handlers.error();
  source.handlers.open();
  assert.deepEqual(seen.map(a => a.type === 'status' ? `status:${a.state}` : a.type),
    ['live', 'resync', 'status:offline', 'status:idle', 'resync']);
  connection.close();
  assert.equal(source.closed, true);
});

test('the web bridge sends prompts as CSRF-protected same-origin POSTs and reads status', async () => {
  const source = fs.readFileSync(require.resolve('../desktop-ui/data.js'), 'utf8')
    .replaceAll('export function', 'function').replace('export async function', 'async function')
    .replace('new URL(path, import.meta.url)', 'path') + '\nglobalThis.api={httpBridge};';
  const calls = [];
  const context = vm.createContext({
    document: {cookie: 'chat_hub_session=s; chat_hub_csrf=token123'},
    fetch: async (url, options) => { calls.push({url, options}); return {ok: true, status: 202, json: async () => ({accepted: true, job_id: 'j'})}; },
    URL,
  });
  vm.runInContext(source, context);
  const bridge = context.api.httpBridge({fetchImpl: context.fetch});
  assert.equal((await bridge.sendPrompt(null, 'hi')).accepted, true);
  await bridge.liveStatus();
  const [prompt, status] = calls;
  assert.equal(prompt.url, '/api/live/prompt');
  assert.equal(prompt.options.method, 'POST');
  assert.equal(prompt.options.credentials, 'same-origin');
  assert.equal(prompt.options.headers['X-CSRF-Token'], 'token123');
  assert.deepEqual(JSON.parse(prompt.options.body), {conversation_id: null, text: 'hi'});
  assert.equal(status.url, '/api/live/status');
  assert.equal(status.options.headers['X-CSRF-Token'], undefined);
});
