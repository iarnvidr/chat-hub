const {test} = require('bun:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const {webcrypto} = require('node:crypto');

const workerSource = fs.readFileSync(require.resolve('../src-tauri/src/conversation_actions.js'), 'utf8');
const id = '123e4567-e89b-12d3-a456-426614174000';
const account = 'fixture-account';
const user = 'fixture-user';

async function scopeFor(accountId = account, userId = user) {
  const bytes = new TextEncoder().encode(JSON.stringify(['chatgpt-request-identity-v1', accountId, userId]));
  const hash = new Uint8Array(await webcrypto.subtle.digest('SHA-256', bytes));
  return `chatgpt:observed:${Array.from(hash, b => b.toString(16).padStart(2, '0')).join('')}`;
}

function sessionToken() {
  const payload = btoa(JSON.stringify({'https://api.openai.com/auth': {chatgpt_account_id: account, chatgpt_user_id: user}}))
    .replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
  return `header.${payload}.signature`;
}

function jsonResponse(value) {
  return new Response(JSON.stringify(value), {status: 200, headers: {'content-type': 'application/json'}});
}

async function invoke(action, desired, fetch, expectedScope) {
  const sandbox = {
    window: {}, location: {origin: 'https://chatgpt.com'}, fetch,
    URL, URLSearchParams, AbortController, TextEncoder, TextDecoder,
    Response, crypto: webcrypto, atob, btoa,
    setTimeout, clearTimeout,
  };
  const fn = vm.runInNewContext(`(${workerSource})`, sandbox);
  fn('action-test', 'nonce-test', expectedScope, id, action, desired);
  for (let i = 0; i < 100; i++) {
    if (sandbox.window['action-test']?.state !== 'running') break;
    await new Promise(resolve => setTimeout(resolve, 1));
  }
  return sandbox.window['action-test'];
}

test('pin and unpin use the documented routes and require exact list readback', async () => {
  const scope = await scopeFor();
  let pinned = false;
  const calls = [];
  const fetch = async (input, options = {}) => {
    const url = new URL(input);
    calls.push([url.pathname, options.method || 'GET', options.body, url.search]);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    if (url.pathname === '/backend-api/pins/conversation/' + id) {
      if (options.method === 'POST') pinned = true;
      if (options.method === 'DELETE') pinned = false;
      return new Response(null, {status: options.method ? 204 : 200});
    }
    if (url.pathname === '/backend-api/pins') return jsonResponse({items: pinned ? [{type: 'conversation', id}] : []});
    throw new Error(`unexpected ${url.pathname}`);
  };
  const result = await invoke('pin', true, fetch, scope);
  assert.equal(result.state, 'applied');
  assert.equal(result.pinned, true);
  assert.equal(result.verified, true);
  assert.deepEqual(calls.map(x => x.slice(0, 2)), [
    ['/api/auth/session', 'GET'], ['/backend-api/pins', 'GET'],
    [`/backend-api/pins/conversation/${id}`, 'POST'], ['/backend-api/pins', 'GET'],
  ]);
  assert.equal(calls[2][2], '{}');
  assert.ok(calls.filter(call=>call[0]==='/backend-api/pins').every(call=>call[3]==='?item_type=conversation'),'pin verification uses the server-accepted default query');
  const remove = await invoke('pin', false, fetch, scope);
  assert.equal(remove.state, 'applied');
  assert.equal(remove.pinned, false);
  assert.equal(pinned, false);
});

test('archive uses PATCH then verifies the exact conversation by GET', async () => {
  const scope = await scopeFor();
  let archived = false;
  const calls = [];
  const fetch = async (input, options = {}) => {
    const url = new URL(input);
    calls.push([url.pathname, options.method || 'GET', options.body]);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    if (url.pathname === `/backend-api/conversation/${id}` && options.method === 'PATCH') {
      archived = JSON.parse(options.body).is_archived;
      return new Response(null, {status: 204});
    }
    if (url.pathname === `/backend-api/conversation/${id}`) return jsonResponse({conversation_id: id, is_archived: archived});
    throw new Error(`unexpected ${url.pathname}`);
  };
  const result = await invoke('archive', true, fetch, scope);
  assert.equal(result.state, 'applied');
  assert.equal(result.archived, true);
  assert.deepEqual(calls.map(x => x.slice(0, 2)), [
    ['/api/auth/session', 'GET'], [`/backend-api/conversation/${id}`, 'GET'],
    [`/backend-api/conversation/${id}`, 'PATCH'], [`/backend-api/conversation/${id}`, 'GET'],
  ]);
  assert.equal(calls[2][2], JSON.stringify({is_archived: true}));
});

test('account mismatch fails before a mutation and token never enters published state', async () => {
  const expectedScope = await scopeFor('another-account');
  const token = sessionToken();
  let mutations = 0;
  const fetch = async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: token, user: {}});
    mutations++;
    return jsonResponse({items: []});
  };
  const result = await invoke('pin', true, fetch, expectedScope);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'account_changed');
  assert.equal(mutations, 0);
  assert.equal(JSON.stringify(result).includes(token), false);
});

test('pin mutation is rejected as unverified when the list does not contain the target', async () => {
  const scope = await scopeFor();
  const fetch = async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    if (url.pathname === '/backend-api/pins/conversation/' + id) return new Response(null, {status: 204});
    if (url.pathname === '/backend-api/pins') return jsonResponse({items: []});
    throw new Error(`unexpected ${url.pathname}`);
  };
  const result = await invoke('pin', true, fetch, scope);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'verification_failed');
  assert.equal(result.verified, undefined);
});

test('unknown pin rows fail closed and cannot verify unpinning', async () => {
  const scope = await scopeFor();
  let mutationCount = 0;
  const fetch = async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    if (url.pathname === '/backend-api/pins') return jsonResponse({items: [{}]});
    if (url.pathname.includes('/backend-api/pins/conversation/')) mutationCount++;
    return new Response(null, {status: 204});
  };
  const result = await invoke('pin', false, fetch, scope);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'verification_failed');
  assert.equal(mutationCount, 0);
});

test('an incomplete pin listing cannot prove the conversation is unpinned', async () => {
  const scope = await scopeFor();
  let mutationCount = 0;
  const fetch = async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    if (url.pathname === '/backend-api/pins') return jsonResponse({items: [], next_cursor: 'next-page'});
    if (url.pathname.includes('/backend-api/pins/conversation/')) mutationCount++;
    return new Response(null, {status: 204});
  };
  const result = await invoke('pin', false, fetch, scope);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'verification_failed');
  assert.equal(mutationCount, 0);
});

test('429 delays preserve long Retry-After values and use pause sentinel beyond one year', async () => {
  const scope = await scopeFor();
  const run = async retryAfter => invoke('pin', true, async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') return jsonResponse({accessToken: sessionToken(), user: {}});
    return new Response('rate limited', {status: 429, headers: {'retry-after': retryAfter}});
  }, scope);
  const long = await run('7200');
  assert.equal(long.error_code, 'rate_limited');
  assert.equal(long.retry_after, 7200);
  const exceedsSupported = await run('31536001');
  assert.equal(exceedsSupported.error_code, 'rate_limited');
  assert.equal(exceedsSupported.retry_after, 31536001);
});
