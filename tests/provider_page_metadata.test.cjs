const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const ROOT = path.resolve(__dirname, '..');
const UUID = '123e4567-e89b-12d3-a456-426614174000';
const probe = fs.readFileSync(path.join(ROOT, 'src-tauri/src/provider_page_metadata.js'), 'utf8');

function run(href, nonce = 'test-nonce') {
  const context = vm.createContext({location: {href}, URL});
  return JSON.parse(vm.runInContext(`(${probe})(${JSON.stringify(nonce)})`, context));
}

test('reads only href from browser Location and recognizes ChatGPT root and UUID pages', () => {
  const root = run('https://chatgpt.com/');
  assert.deepEqual(root, {nonce: 'test-nonce', exact_origin: true, conversation_id: null});
  const conversation = run(`https://chatgpt.com/c/${UUID}`, 'n-2');
  assert.deepEqual(conversation, {nonce: 'n-2', exact_origin: true, conversation_id: UUID});
});

test('rejects userinfo and other origins, and does not accept query or fragment routes', () => {
  for (const href of [
    `https://user:pass@chatgpt.com/c/${UUID}`,
    `http://chatgpt.com/c/${UUID}`,
    `https://not-chatgpt.com/c/${UUID}`,
  ]) {
    const result = run(href);
    assert.equal(result.exact_origin, false);
    assert.equal(result.conversation_id, null);
  }
  for (const suffix of ['?x=1', '#section']) {
    const result = run(`https://chatgpt.com/c/${UUID}${suffix}`);
    assert.equal(result.exact_origin, true);
    assert.equal(result.conversation_id, null);
  }
});

test('returns only the nonce-bound metadata fields', () => {
  const result = run(`https://chatgpt.com/c/${UUID}`);
  assert.deepEqual(Object.keys(result).sort(), ['conversation_id', 'exact_origin', 'nonce']);
  assert.equal(Object.hasOwn(result, 'href'), false);
  assert.equal(Object.hasOwn(result, 'username'), false);
});
