const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const crypto = require('node:crypto');
const {webcrypto} = crypto;

const source = fs.readFileSync(require.resolve('../src-tauri/src/project_assignment.js'), 'utf8');
const key = '__assignment_fixture';
const conversationId = 'a1a1a1a1-a1a1-4a1a-8a1a-a1a1a1a1a1a1';
const projectId = 'g-p-project_123';
const account = 'fixture-account', user = 'fixture-user';
const payload = Buffer.from(JSON.stringify({'https://api.openai.com/auth': {
  chatgpt_account_id: account, chatgpt_user_id: user,
}})).toString('base64url');
const token = `header.${payload}.signature`;
const scope = `chatgpt:observed:${crypto.createHash('sha256')
  .update(JSON.stringify(['chatgpt-request-identity-v1', account, user])).digest('hex')}`;

function response(status, value) {
  const bytes = Buffer.from(JSON.stringify(value));
  let read = false;
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: {get: () => null},
    body: {
      getReader: () => ({
        async read() { if (read) return {done: true}; read = true; return {done: false, value: bytes}; },
        async cancel() { read = true; },
        releaseLock() {},
      }),
      async cancel() { read = true; },
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
    crypto: webcrypto, TextEncoder, TextDecoder, AbortController, URL, URLSearchParams,
    atob: value => Buffer.from(value, 'base64').toString('binary'),
    setTimeout, clearTimeout, Date, Number, String, Array, Object, JSON, Math, Map, Set,
    Promise, Error, Boolean, RegExp});
  return {window, calls, run: vm.runInContext(`(${source})`, context)};
}

async function waitForResult(window) {
  for (let i = 0; i < 200; i++) {
    const result = window[key];
    if (result && result.state !== 'running') return result;
    await new Promise(resolve => setImmediate(resolve));
  }
  throw new Error('fixture assignment worker did not complete');
}

function authAndProjectFixture({projectExists = true} = {}) {
  let currentProject = null;
  return (url, options) => {
    const parsed = new URL(url);
    assert.equal(parsed.origin, 'https://chatgpt.com');
    if (parsed.pathname === '/api/auth/session') return response(200, {accessToken: token, user: {id: user}});
    if (parsed.pathname === `/backend-api/conversation/${conversationId}`) {
      if (options.method === 'PATCH') {
        currentProject = JSON.parse(options.body).gizmo_id || null;
        return response(200, {});
      }
      return response(200, {conversation_id: conversationId, gizmo_id: currentProject});
    }
    if (parsed.pathname === '/backend-api/gizmos/snorlax/sidebar') {
      const items = projectExists ? [{gizmo: {id: projectId, display: {name: 'Village research'}}}] : [];
      return response(200, {items, cursor: null});
    }
    throw new Error('unexpected fixture route');
  };
}

test('assignment worker changes a ChatGPT project and verifies the read-back before reporting success', async () => {
  const fixture = setup(authAndProjectFixture());
  fixture.run(key, 'fixture-nonce', scope, conversationId, projectId);
  const result = await waitForResult(fixture.window);
  assert.equal(result.state, 'applied');
  assert.equal(result.project_title, 'Village research');
  assert.deepEqual(fixture.calls.map(call => new URL(call.url).pathname), [
    '/api/auth/session',
    `/backend-api/conversation/${conversationId}`,
    '/backend-api/gizmos/snorlax/sidebar',
    `/backend-api/conversation/${conversationId}`,
    `/backend-api/conversation/${conversationId}`,
  ]);
  const patch = fixture.calls.find(call => call.options.method === 'PATCH');
  assert.deepEqual(JSON.parse(patch.options.body), {gizmo_id: projectId});
});

test('stale locally listed project is rejected before any ChatGPT conversation mutation', async () => {
  const fixture = setup(authAndProjectFixture({projectExists: false}));
  fixture.run(key, 'fixture-nonce', scope, conversationId, projectId);
  const result = await waitForResult(fixture.window);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'project_unavailable');
  assert.equal(fixture.calls.some(call => call.options.method === 'PATCH'), false);
});

test('a forbidden provider session returns a safe sign-in failure without a mutation', async () => {
  const fixture = setup(url => new URL(url).pathname === '/api/auth/session'
    ? response(403, {error: 'private fixture detail'})
    : response(500, {}));
  fixture.run(key, 'fixture-nonce', scope, conversationId, projectId);
  const result = await waitForResult(fixture.window);
  assert.equal(result.state, 'failed');
  assert.equal(result.error_code, 'signed_out');
  assert.equal(fixture.calls.length, 1);
});
