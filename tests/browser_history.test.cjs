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

function setup(handler, anchors = []) {
  const window = {getComputedStyle: () => ({display: 'block', visibility: 'visible', opacity: '1'})};
  const calls = [];
  const fetch = async (url, options) => {
    calls.push({url: String(url), options});
    return handler(String(url), options, calls);
  };
  const document = {querySelectorAll: selector => selector === 'a[href]' ? anchors : []};
  const context = vm.createContext({window, document, fetch, location: {origin: 'https://chatgpt.com'},
    crypto: webcrypto, TextEncoder, TextDecoder, AbortController, URL,
    atob: value => Buffer.from(value, 'base64').toString('binary'),
    setTimeout, clearTimeout, Date, Number, String, Array, Object, JSON, Math, Map, Set,
    Promise, Error, Boolean, RegExp});
  const factory = vm.runInContext(source, context);
  const state = factory(KEY);
  return {state, calls, window};
}

function projectAnchor(id, title, {url = `https://chatgpt.com/g/${id}/project`, visible = true, hidden = false} = {}) {
  return {href: url, innerText: title, getClientRects: () => visible ? [{}] : [],
    getAttribute: name => name === 'aria-hidden' && hidden ? 'true' : null};
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
  for (let sequence = 1; sequence <= 11; sequence++) {
    await until(() => job.state.chunk !== null && job.state.chunk.sequence === sequence);
    const remaining = 101 - (sequence - 1) * 10;
    assert.equal(job.state.chunk.conversations.length, Math.min(10, remaining));
    assert.equal(job.calls.filter(call => new URL(call.url).pathname === '/backend-api/conversations/batch').length, sequence,
      `detail batch ${sequence + 1} must wait for ACK ${sequence}`);
    if (sequence === 1) {
      assert.equal(job.state.chunk.conversations[0].mapping.assistant.message.content.parts[0], 'leaf');
    }
    job.state.ackSequence = sequence;
  }
  await job.state.ready;
  assert.equal(job.state.complete, true);
  assert.equal(job.state.conversations, 101);
});

test('acknowledges each successful detail batch before a later batch can rate limit', async () => {
  const graphs = Array.from({length: 20}, (_, i) => graph(`chat-${String(i).padStart(3, '0')}`));
  let batches = 0;
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') {
      const archived = new URL(url).searchParams.get('is_archived') === 'true';
      return archived ? response(200, {items: [], total: 0}) : response(200, {
        items: graphs.map(item => ({id: item.id})), total: graphs.length,
      });
    }
    if (path === '/backend-api/conversations/batch') {
      batches++;
      if (batches === 2) return response(429, {}, {'retry-after': '45'});
      const ids = JSON.parse(options.body).conversation_ids;
      return response(200, ids.map(id => graphs.find(item => item.id === id)).filter(Boolean));
    }
    throw new Error(`unexpected URL ${path}`);
  });
  await until(() => job.state.chunk !== null);
  assert.equal(job.state.chunk.conversations.length, 10);
  assert.equal(batches, 1);
  job.state.ackSequence = 1;
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.error_code, 'rate_limited');
  assert.equal(job.state.stage, 'detail');
  assert.equal(job.state.conversations, 10);
  assert.equal(batches, 2);
});

test('signed-out session fails without API history requests', async () => {
  const job = setup((url) => new URL(url).pathname === '/api/auth/session'
    ? response(200, {}) : response(500, {}));
  await job.state.ready;
  assert.equal(job.state.error_code, 'signed_out');
  assert.equal(job.calls.length, 1);
  assert.equal(job.state.chunk, null);
});

test('project labels and membership travel separately without refetching or changing raw graphs', async () => {
  const original = graph('chat-1'), normal = normalHandler([original]);
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(200, {
      items: [{gizmo: {gizmo: {id: 'project-a', display: {name: 'Research'}}}}], cursor: null});
    if (path === '/backend-api/gizmos/project-a/conversations') return response(200, {items: [{id: 'chat-1'}], cursor: null});
    return normal(url, options);
  });
  await until(() => job.state.chunk !== null);
  assert.equal(job.state.chunk.conversations.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.conversations[0])), original);
  job.state.ackSequence = 1;
  await until(() => job.state.chunk?.sequence === 2);
  assert.equal(job.state.chunk.conversations.length, 0);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.project_catalog)), [{id: 'project-a', title: 'Research'}]);
  job.state.ackSequence = 2;
  await until(() => job.state.chunk?.sequence === 3);
  assert.equal(job.state.chunk.conversations.length, 0);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.project_catalog)), [{id: 'project-a', title: 'Research'}]);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.conversation_projects)), [{conversation_id: 'chat-1', project_id: 'project-a'}]);
  job.state.ackSequence = 3;
  await job.state.ready;
  assert.equal(job.state.complete, true);
  assert.equal(job.calls.filter(call => new URL(call.url).pathname === '/backend-api/conversations/batch').length, 1);
});

test('active-list 429 stops immediately and publishes bounded retry timing', async () => {
  const job = setup((url) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    return response(429, {}, {'retry-after': '29'});
  });
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.error_code, 'rate_limited');
  assert.equal(job.state.stage, 'list');
  assert.equal(job.state.http_status, 429);
  assert.equal(job.state.retry_after, 29);
  assert.deepEqual(job.calls.map(call => new URL(call.url).pathname), [
    '/api/auth/session', '/backend-api/conversations',
  ]);
});

test('visible sidebar titles are committed after active chats and before sidebar API rate limiting', async () => {
  const job = setup((url) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') return response(200, {items: [], total: 0});
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(429, {}, {'retry-after': '31'});
    throw new Error(`unexpected URL ${path}`);
  }, [
    projectAnchor('g-p-real-project', 'Visible project name'),
    projectAnchor('g-p-hidden-project', 'Hidden name', {visible: false}),
    projectAnchor('g-p-aria-hidden', 'Hidden aria name', {hidden: true}),
    projectAnchor('g-p-generic', 'Projects'),
    projectAnchor('g-p-empty', '  '),
    projectAnchor('g-p-external', 'External name', {url: 'https://example.com/g/g-p-external/project'}),
    projectAnchor('g-p-chat', 'Chat title', {url: 'https://chatgpt.com/c/g-p-chat/project'}),
  ]);
  await until(() => job.state.chunk !== null);
  assert.equal(job.state.chunk.sequence, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.project_catalog)), [
    {id: 'g-p-real-project', title: 'Visible project name'},
  ]);
  assert.deepEqual(job.calls.map(call => new URL(call.url).pathname), ['/api/auth/session'],
    'visible DOM names are acknowledged before the first ChatGPT API request');
  job.state.ackSequence = 1;
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.stage, 'project');
  assert.equal(job.state.http_status, 429);
  assert.deepEqual(job.calls.map(call => new URL(call.url).pathname), [
    '/api/auth/session', '/backend-api/conversations', '/backend-api/conversations',
    '/backend-api/gizmos/snorlax/sidebar',
  ]);
});

test('active chat is acknowledged before an archived-list 429', async () => {
  const original = graph('active-chat');
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') {
      const archived = new URL(url).searchParams.get('is_archived') === 'true';
      return archived
        ? response(429, {}, {'retry-after': '45'})
        : response(200, {items: [{id: original.id}], total: 1});
    }
    if (path === '/backend-api/conversations/batch') return response(200, [original]);
    throw new Error(`unexpected URL ${path}`);
  });
  await until(() => job.state.chunk !== null);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.conversations)), [original]);
  job.state.ackSequence = 1;
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.error_code, 'rate_limited');
  assert.equal(job.state.stage, 'list');
  assert.equal(job.state.conversations, 1);
  assert.deepEqual(job.calls.map(call => new URL(call.url).pathname), [
    '/api/auth/session', '/backend-api/conversations',
    '/backend-api/conversations/batch', '/backend-api/conversations',
  ]);
});

test('active chat is acknowledged before a project-sidebar 429', async () => {
  const original = graph('active-chat');
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/conversations') {
      const archived = new URL(url).searchParams.get('is_archived') === 'true';
      return response(200, archived ? {items: [], total: 0} : {items: [{id: original.id}], total: 1});
    }
    if (path === '/backend-api/conversations/batch') return response(200, [original]);
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(429, {}, {'retry-after': '45'});
    throw new Error(`unexpected URL ${path}`);
  });
  await until(() => job.state.chunk !== null);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.conversations)), [original]);
  job.state.ackSequence = 1;
  await job.state.ready;
  assert.equal(job.state.status, 'waiting');
  assert.equal(job.state.error_code, 'rate_limited');
  assert.equal(job.state.stage, 'project');
  assert.equal(job.state.conversations, 1);
  assert.deepEqual(job.calls.map(call => new URL(call.url).pathname), [
    '/api/auth/session', '/backend-api/conversations',
    '/backend-api/conversations/batch', '/backend-api/conversations',
    '/backend-api/gizmos/snorlax/sidebar',
  ]);
});

test('authenticated sidebar API title supersedes the rendered fallback', async () => {
  const id = 'g-p-real-project';
  const job = setup((url) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(200, {
      items: [{gizmo: {id, display: {name: 'Canonical API name'}}}], cursor: null});
    if (path === `/backend-api/gizmos/${id}/conversations`) return response(200, {items: [], cursor: null});
    if (path === '/backend-api/conversations') return response(200, {items: [], total: 0});
    throw new Error(`unexpected URL ${path}`);
  }, [projectAnchor(id, 'Rendered fallback name')]);
  await until(() => job.state.chunk !== null);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.project_catalog)), [
    {id, title: 'Rendered fallback name'},
  ]);
  job.state.ackSequence = 1;
  await until(() => job.state.chunk?.sequence === 2);
  assert.deepEqual(JSON.parse(JSON.stringify(job.state.chunk.project_catalog)), [
    {id, title: 'Canonical API name'},
  ]);
  job.state.ackSequence = 2;
  await job.state.ready;
  assert.equal(job.state.complete, true);
});

test('malformed graph is rejected before it can be acknowledged', async () => {
  const job = setup((url, options) => {
    const path = new URL(url).pathname;
    if (path === '/api/auth/session') return response(200, {accessToken: token});
    if (path === '/backend-api/gizmos/snorlax/sidebar') return response(200, {items: []});
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
