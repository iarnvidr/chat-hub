const test = require('node:test');
const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const modulePromise = import('data:text/javascript;base64,' + readFileSync(resolve(__dirname, '../desktop-ui/build-identity.js')).toString('base64'));

test('native source identity distinguishes equal base app versions', async () => {
  const {describeBuildIdentity} = await modulePromise;
  const build = {schema: 1, source_commit: 'e'.repeat(40), source_tree: 'a'.repeat(40)};
  assert.match(describeBuildIdentity(build), /eeeeeeeeeeee/);
  assert.notEqual(describeBuildIdentity(build), describeBuildIdentity({...build, source_commit: 'b'.repeat(40)}));
});

test('unknown identity never invents a stable or preview channel', async () => {
  const {describeBuildIdentity} = await modulePromise;
  for (const input of [null, {}, {schema: 1, source_commit: '<script>', source_tree: 'a'.repeat(40)}]) {
    assert.equal(describeBuildIdentity(input), 'Source build identity unavailable');
  }
});
