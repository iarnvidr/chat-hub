const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const load = async () => import('data:text/javascript;base64,' + Buffer.from(fs.readFileSync(require.resolve('../desktop-ui/continuation.js'))).toString('base64'));

test('continuation uses ChatGPT identity from raw and Paseo history without trusting arbitrary URLs', async () => {
  const {chatgptConversationId: id} = await load();
  const uuid = '123e4567-e89b-12d3-a456-426614174000';
  assert.equal(id({provider: 'chatgpt', source_id: uuid}), uuid);
  assert.equal(id({provider: 'paseo-chatgpt', source_id: `conversation:${uuid}`}), uuid);
  for (const c of [null, {}, {provider: 'paseo-codex_local', source_id: uuid},
    {provider: 'chatgpt', source_id: '../settings', url: `https://chatgpt.com/c/${uuid}`},
    {provider: 'paseo-chatgpt', source_id: `https://chatgpt.com/c/${uuid}`},
    {provider: 'paseo-chatgpt', source_id: 'conversation:not-a-uuid'}]) assert.equal(id(c), null);
});
