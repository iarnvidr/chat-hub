const {test} = require('bun:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const workerSource = fs.readFileSync(require.resolve('../src-tauri/src/browser_history.js'), 'utf8');

async function runRateLimited({body, headers}) {
  const claims = {
    sub: 'fixture-user',
    exp: Math.floor(Date.now() / 1000) + 3600,
    'https://api.openai.com/auth': {
      chatgpt_account_id: 'fixture-account',
      chatgpt_user_id: 'fixture-user',
    },
  };
  const payload = btoa(JSON.stringify(claims)).replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
  const token = `header.${payload}.signature`;
  const fetch = async input => {
    const url = new URL(input);
    if (url.pathname === '/api/auth/session') {
      return new Response(JSON.stringify({accessToken: token}), {
        status: 200, headers: {'content-type': 'application/json'},
      });
    }
    if (url.pathname.endsWith('/gizmos/snorlax/sidebar')) {
      return new Response(JSON.stringify({items: []}), {
        status: 200, headers: {'content-type': 'application/json'},
      });
    }
    if (url.pathname.endsWith('/conversations')) {
      return new Response(body, {status: 429, headers});
    }
    throw new Error(`unexpected fixture path ${url.pathname}`);
  };
  const sandbox = {
    window: {}, document: {querySelectorAll: () => []},
    location: {origin: 'https://chatgpt.com'}, fetch, Response, URL,
    AbortController, TextEncoder, TextDecoder, crypto, atob, btoa,
    setTimeout, clearTimeout, Date,
  };
  const start = vm.runInNewContext(`(${workerSource})`, sandbox);
  const state = start('history-test');
  await state.ready;
  return state;
}

test('429 challenge response is classified without retaining its HTML body', async () => {
  const state = await runRateLimited({
    body: '<html>fixture challenge secret</html>',
    headers: {'content-type': 'text/html; charset=utf-8', 'cf-mitigated': 'challenge', 'retry-after': '3600'},
  });
  assert.equal(state.status, 'waiting');
  assert.equal(state.error_code, 'rate_limited');
  assert.equal(state.http_status, 429);
  assert.equal(state.retry_after, 3600);
  assert.equal(state.retry_after_too_long, false);
  assert.equal(state.response_content_type, 'html');
  assert.equal(state.challenge_detected, true);
  assert.equal(state.provider_error_code, '');
  assert.equal(JSON.stringify(state).includes('fixture challenge secret'), false);
});

test('429 JSON error extracts only an allowlisted machine code and preserves long Retry-After', async () => {
  const state = await runRateLimited({
    body: JSON.stringify({error: {code: 'rate_limit_exceeded', message: 'private diagnostic text'}}),
    headers: {'content-type': 'application/json; charset=utf-8', 'retry-after': '7200'},
  });
  assert.equal(state.response_content_type, 'json');
  assert.equal(state.challenge_detected, false);
  assert.equal(state.provider_error_code, 'rate_limit_exceeded');
  assert.equal(state.retry_after, 7200);
  assert.equal(JSON.stringify(state).includes('private diagnostic text'), false);
});

test('429 error JSON inspection stops at the 4 KiB limit', async () => {
  let bytesRead = 0;
  let cancelled = false;
  const stream = new ReadableStream({
    pull(controller) {
      if (bytesRead >= 8192) { controller.close(); return; }
      bytesRead += 2048;
      controller.enqueue(new Uint8Array(2048));
    },
    cancel() { cancelled = true; },
  });
  const state = await runRateLimited({
    body: stream,
    headers: {'content-type': 'application/json', 'retry-after': '90'},
  });
  assert.equal(state.response_content_type, 'json');
  assert.equal(state.provider_error_code, '');
  assert.ok(bytesRead <= 8192, `reader stopped after bounded cancellation at ${bytesRead} bytes`);
  assert.equal(cancelled, true);
});

test('Retry-After beyond the supported range pauses automatic retries instead of capping the deadline', async () => {
  const state = await runRateLimited({
    body: '<html>long cooldown</html>',
    headers: {'content-type': 'text/html', 'retry-after': '31536001'},
  });
  assert.equal(state.retry_after, 0);
  assert.equal(state.retry_after_too_long, true);
  assert.equal(state.status, 'waiting');
});
