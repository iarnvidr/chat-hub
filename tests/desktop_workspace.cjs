const {chromium} = require(process.env.PLAYWRIGHT_MODULE || '/home/coder/.cache/chat-hub-f4-playwright/node_modules/playwright');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

const root = path.resolve(__dirname, '..');
const mime = {'.html':'text/html', '.js':'text/javascript', '.css':'text/css', '.json':'application/json'};
const server = http.createServer((request, response) => {
  const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  const file = path.resolve(root, `.${pathname}`);
  if (!file.startsWith(root + path.sep)) { response.writeHead(403).end(); return; }
  fs.readFile(file, (error, bytes) => {
    if (error) { response.writeHead(404).end(); return; }
    response.setHeader('Content-Type', mime[path.extname(file)] || 'application/octet-stream');
    response.end(bytes);
  });
});

(async () => {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage({viewport: {width: 1280, height: 900}});
    const pageErrors = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    await page.addInitScript(() => {
      window.__calls = [];
      window.__TAURI_INTERNALS__ = {invoke: async (command, args) => {
        window.__calls.push([command, args]);
        if (command === 'list_conversations') return {items: [{id: 'fixture-chat', title: 'Fixture chat', provider: 'chatgpt'}], next_cursor: null, library_revision: 1};
        if (command === 'get_conversation') return {conversation: {id: args.id, title: 'Fixture chat', provider: 'chatgpt'}, messages: [], selection: {}};
        if (command === 'service_health') return {healthy: true};
        if (command === 'get_capture_preferences') return {enabled: false, restart_required: false};
        if (command === 'update_status') return {version: 'test', message: 'Ready'};
        if (command === 'service_info') return {};
        if (command === 'chatgpt_history_status') return {state: 'idle'};
        if (command === 'get_chatgpt_connection') return {state: 'signed_out'};
        if (command === 'set_provider_view') return {state: 'ready', conversation_id: null};
        return {};
      }};
    });
    await page.goto(`http://127.0.0.1:${server.address().port}/desktop-ui/index.html`);
    await page.locator('button.conversation-item').first().waitFor();
    const geometry = await page.evaluate(() => {
      const workspace = document.querySelector('.workspace');
      const pane = document.querySelector('.list-pane');
      const bar = document.querySelector('.workspace-bar');
      return {
        workspaceChildren: [...workspace.children].map(node => node.className || node.id),
        listWidth: pane.getBoundingClientRect().width,
        tabsHeight: bar.getBoundingClientRect().height,
        heroCount: document.querySelectorAll('.hero, .outer-sidebar').length,
      };
    });
    assert.deepEqual(geometry.workspaceChildren, ['list-pane', 'library-shell']);
    assert.equal(geometry.listWidth, 272);
    assert.equal(geometry.tabsHeight, 42);
    assert.equal(geometry.heroCount, 0);
    assert.equal(await page.locator('#settings-page').isVisible(), false);

    await page.locator('#show-chatgpt').click();
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'set_provider_view' && args.visible));
    await page.locator('#show-settings').click();
    await page.locator('#settings-page').waitFor({state: 'visible'});
    assert.equal(await page.locator('body').evaluate(node => node.classList.contains('settings-open')), true);
    assert.equal(await page.locator('[data-workspace-content]').evaluateAll(nodes => nodes.every(node => getComputedStyle(node).display === 'none')), true);
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'set_provider_view' && !args.visible));

    await page.locator('#settings-back').click();
    await page.locator('#settings-page').waitFor({state: 'hidden'});
    assert.equal(await page.locator('body').evaluate(node => node.classList.contains('settings-open')), false);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'show-settings');
    assert.equal(await page.locator('[data-workspace-content]').evaluateAll(nodes => nodes.every(node => getComputedStyle(node).display !== 'none')), true);
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'set_provider_view' && args.visible));

    await page.locator('#show-settings').click();
    await page.locator('#settings-page').waitFor({state: 'visible'});
    await page.keyboard.press('Escape');
    await page.locator('#settings-page').waitFor({state: 'hidden'});
    assert.equal(await page.locator('body').evaluate(node => node.classList.contains('settings-open')), false);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'show-settings');
    await page.waitForFunction(() => window.__calls.filter(([name]) => name === 'set_provider_view').at(-1)?.[1]?.visible === true);
    await page.locator('#show-library').click();
    await page.waitForFunction(() => window.__calls.filter(([name]) => name === 'set_provider_view').at(-1)?.[1]?.visible === false);
    assert.deepEqual(pageErrors, []);
    console.log('PASS compact workspace geometry, settings back/Escape/focus, native provider show/hide');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error.stack || error.message); process.exitCode = 1; });
