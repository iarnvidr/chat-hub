const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const {webcrypto} = require('node:crypto');

const source = fs.readFileSync(require.resolve('../src-tauri/src/browser_history.js'), 'utf8');
const KEY = '__chatHubHistoryTest';
const account = 'acct-a', user = 'user-a', subject = 'auth0|subject-a';
const b64url = value => Buffer.from(JSON.stringify(value)).toString('base64url');
const token = `header.${b64url({exp: Math.floor(Date.now() / 1000) + 3600, sub: subject,
  'https://api.openai.com/auth': {chatgpt_account_id: account, chatgpt_user_id: user}})}.sig`;

function response(status, value, headers = {}) {
  const bytes = Buffer.from(JSON.stringify(value));
  let index = 0;
  return {
    status,
    headers: {get: key => key.toLowerCase() === 'content-length' ? String(bytes.length) : headers[key.toLowerCase()] || null},
    body: {
      getReader: () => ({
        async read() { if (index) return {done: true}; index = 1; return {done: false, value: bytes}; },
        async cancel() { index = 1; }, releaseLock() {},
      }),
      async cancel() { index = 1; },
    },
  };
}

function setup(handler) {
  const window = {};
  const calls = [];
  const fetch = async (url, options) => {
    calls.push({url: String(url), options});
    return handler(String(url), options, calls);
  };
  const context = vm.createContext({window, fetch, location: {origin: 'https://chatgpt.com'},
    crypto: webcrypto, TextEncoder, TextDecoder, AbortController, URL,
    atob: value => Buffer.from(value, 'base64').toString('binary'),
    setTimeout, clearTimeout, Date, Number, String, Array, Object, JSON, Math, Map, Set,
    Promise, Error, Boolean, RegExp});
  const factory = vm.runInContext(source, context);
  const state = factory(KEY);
  return {state, calls, window};
}

const graph = (id, marker = 'leaf') => ({id, mapping: {
  root: {parent: null, children: ['user', 'assistant', 'assistant_alt']},
  user: {parent: 'root', children: [], message: {author: {role: 'user'}, content: {parts: ['question']}}},
  assistant: {parent: 'root', children: [], message: {author: {role: 'assistant'}, content: {parts: [marker]}}},
  assistant_alt: {parent: 'root', children: [], message: {author: {role: 'assistant'}, content: {parts: ['sibling branch']}}},
}, current_node: 'assistant_alt'});

function normalHandler(graphs) {
  return (url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') {
      const u = new URL(url), archived = u.searchParams.get('is_archived') === 'true';
      if (archived) return response(200, {items: [], total: 0});
      const offset = Number(u.searchParams.get('offset') || 0), limit = Number(u.searchParams.get('limit'));
      return response(200, {items: graphs.slice(offset, offset + limit).map(g => ({id: g.id, update_time: 'tag'})), total: graphs.length});
    }
    if (path === '/backend-api/conversations/batch') {
      const ids = JSON.parse(options.body).conversation_ids;
      return response(200, ids.map(id => graphs.find(g => g.id === id)).filter(Boolean));
    }
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(200, {items: []});
    throw new Error(`unexpected URL ${path}`);
  };
}

async function until(predicate, limit = 1000) {
  const deadline = Date.now() + limit;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('timed out waiting for worker state');
    await new Promise(resolve => setTimeout(resolve, 2));
  }
}

test('uses the current WebView session and keeps account scope, owner and raw graph private and faithful', async () => {
  const item = graph('chat-1', 'alternate branch text');
  const job = setup(normalHandler([item]));
  await until(() => job.state.chunk !== null);
  const chunk = job.state.chunk;
  assert.equal(job.state.chunk.conversations[0].mapping.assistant.message.content.parts[0], 'alternate branch text');
  assert.equal(job.state.chunk.conversations[0].mapping.root.children.length, 3);
  assert.equal(job.state.chunk.conversations[0].current_node, 'assistant_alt');
  assert.equal(job.state.chunk.conversations[0].mapping.assistant_alt.message.content.parts[0], 'sibling branch');
  assert.match(chunk.account_scope, /^chatgpt:observed:[0-9a-f]{64}$/);
  const expected = Buffer.from(JSON.stringify(['chatgpt-request-identity-v1', account, user]));
  assert.equal(chunk.account_scope, 'chatgpt:observed:' + require('node:crypto').createHash('sha256').update(expected).digest('hex'));
  assert.match(chunk.owner, /^[0-9a-f]{24}$/);
  const ownerHash = require('node:crypto').createHash('sha256').update(account + ':' + subject).digest('hex').slice(0, 24);
  assert.equal(chunk.owner, ownerHash);
  job.state.ackSequence = 1;
  await job.state.ready;
  assert.equal(job.state.complete, true);
  assert.equal(job.state.status, 'complete');
  const sessionCall = job.calls.find(call => new URL(call.url).pathname === '/api/auth/session');
  assert.equal(sessionCall.options.credentials, 'include');
  assert.equal(sessionCall.options.redirect, 'error');
  assert.equal(sessionCall.options.headers.Authorization, undefined);
  assert.equal(JSON.stringify(job.state).includes(token), false);
  assert.equal(JSON.stringify(job.state).includes('alternate branch text'), false);
  const apiCall = job.calls.find(call => new URL(call.url).pathname === '/backend-api/conversations/batch');
  assert.equal(apiCall.options.headers['ChatGPT-Account-Id'], account);
  assert.equal(job.state.conversations, 1);
});

test('waits for native ACK between bounded raw graph chunks', async () => {
  const graphs = Array.from({length: 101}, (_, i) => graph(`chat-${String(i).padStart(3, '0')}`));
  const job = setup(normalHandler(graphs));
  await until(() => job.state.chunk !== null);
  assert.equal(job.state.chunk.sequence, 1);
  assert.equal(job.state.chunk.conversations.length, 100);
  assert.equal(job.calls.filter(call => new URL(call.url).pathname === '/backend-api/conversations/batch').length, 10,
    'worker must pause before fetching the eleventh detail batch');
  assert.equal(job.state.chunk.conversations[0].mapping.assistant.message.content.parts[0], 'leaf');
  job.state.ackSequence = 1;
  await until(() => job.state.chunk !== null && job.state.chunk.sequence === 2);
  assert.equal(job.state.chunk.conversations.length, 1);
  job.state.ackSequence = 2;
  await job.state.ready;
  assert.equal(job.state.complete, true);
  assert.equal(job.state.conversations, 101);
});

test('signed-out session fails without API history requests', async () => {
  const job = setup((url) => new URL(url).pathname === '/api/auth/session'
    ? response(200, {}) : response(500, {}));
  await job.state.ready;
  assert.equal(job.state.error_code, 'signed_out');
  assert.equal(job.calls.length, 1);
  assert.equal(job.state.chunk, null);
});

test('429 stops immediately and publishes bounded retry timing', async () => {
  const job = setup((url) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    return response(429, {}, {'retry-after': '29'});
  });
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.error_code, 'rate_limited');
  assert.equal(job.state.stage, 'list');
  assert.equal(job.state.retry_after, 29);
  assert.equal(job.calls.length, 2);
});

test('malformed graph is rejected before it can be acknowledged', async () => {
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') return response(200, {items: [{id: 'chat-1'}], total: 1});
    if (path === '/backend-api/conversations/batch') return response(200, [{id: 'chat-1', title: 'bad'}]);
    throw new Error(path);
  });
  await job.state.ready;
  assert.equal(job.state.error_code, 'invalid_response');
  assert.equal(job.state.stage, 'detail');
  assert.equal(job.state.chunk, null);
});

test('cancellation aborts the active request and exposes no token', async () => {
  const job = setup((url, options) => new Promise((resolve, reject) => {
    options.signal.addEventListener('abort', () => {
      const error = new Error('aborted'); error.name = 'AbortError'; reject(error);
    }, {once: true});
  }));
  await until(() => job.calls.length === 1);
  job.state.cancelled = true;
  await job.state.ready;
  assert.equal(job.state.status, 'cancelled');
  assert.equal(job.state.stage, 'cancelled');
  assert.equal(JSON.stringify(job.state).includes(token), false);
});
