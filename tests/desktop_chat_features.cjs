const {chromium} = require(process.env.PLAYWRIGHT_MODULE || '/home/coder/.cache/chat-hub-f4-playwright/node_modules/playwright');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

const root = path.resolve(__dirname, '..');
const UUID = '123e4567-e89b-12d3-a456-426614174000';
const SCOPE = 'chatgpt:observed:' + 'a'.repeat(64);
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
  const browser = await chromium.launch({headless: true, ...(process.env.CHROMIUM_PATH ? {executablePath: process.env.CHROMIUM_PATH, args: ['--no-sandbox']} : {})});
  try {
    const page = await browser.newPage({viewport: {width: 1280, height: 900}});
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(({conversationId, scope}) => {
      const summary = {id: `chatgpt:${conversationId}`, provider: 'chatgpt', account_scope: scope,
        source_id: conversationId, title: 'Amber project notes', message_count: 2,
        project: {title: 'Chat Project'}};
      const detail = {
        conversation: {...summary}, selection: {selected_node_id: 'answer', thread_complete: true},
        nodes: [
          {id: 'root', parent_id: null, message_id: null},
          {id: 'question', parent_id: 'root', message_id: 'm1'},
          {id: 'answer', parent_id: 'question', message_id: 'm2'},
        ],
        messages: [
          {id: 'm1', node_id: 'question', role: 'user', content: 'How does amber work?', parts: [{kind: 'text', text: 'How does amber work?'}]},
          {id: 'm2', node_id: 'answer', role: 'assistant', content: 'Amber records remain local.', parts: [{kind: 'text', text: 'Amber records remain local.'}]},
        ],
        branches: [{leaf_node_id: 'answer', selected: true}], thread_node_ids: ['root', 'question', 'answer'],
      };
      const queryHits = [
        {conversation: summary, node_id: 'question', leaf_node_id: 'question', message_id: 'm1', snippet: 'Question about amber records.', score: 0.9},
        {conversation: summary, node_id: 'answer', leaf_node_id: 'answer', message_id: 'm2', snippet: 'Amber records remain local.', score: 0.8},
      ];
      window.__calls = [];
      window.__TAURI_INTERNALS__ = {invoke: async (command, args = {}) => {
        window.__calls.push([command, args]);
        if (command === 'list_conversations') {
          if (args.query) return {items: queryHits, next_cursor: null, library_revision: 3};
          const isCodex = args.source === 'codex';
          const item = {...summary, id: isCodex ? 'codex:fixture' : summary.id,
            provider: isCodex ? 'codex_local' : 'chatgpt', source_id: isCodex ? 'thread-fixture' : conversationId,
            title: isCodex ? 'Codex fixture' : summary.title,
            project: {title: isCodex ? 'Codex Project' : 'Chat Project'}};
          return {items: [item], next_cursor: null, library_revision: 3};
        }
        if (command === 'get_conversation') return detail;
        if (command === 'list_projects') return {projects: args.source === 'codex'
          ? [{project_key: 'codex-project:key', title: 'Codex Project', conversation_count: 2}]
          : [{project_key: 'chat-project:key', title: 'Chat Project', conversation_count: 1}]};
        if (command === 'service_health') return {healthy: true};
        if (command === 'get_capture_preferences') return {enabled: false, restart_required: false};
        if (command === 'update_status') return {version: 'test', message: 'Ready'};
        if (command === 'service_info') return {};
        if (command === 'chatgpt_history_status') return {state: 'idle'};
        if (command === 'get_chatgpt_connection') return {state: 'connected', account_scope: scope};
        if (command === 'set_provider_view') return {state: 'ready', conversation_id: conversationId};
        if (command === 'open_provider_conversation') return {state: 'ready'};
        return {};
      }};
    }, {conversationId: UUID, scope: SCOPE});

    await page.goto(`http://127.0.0.1:${server.address().port}/desktop-ui/index.html`);
    await page.locator('button.conversation-item').first().waitFor();
    assert.equal(await page.locator('#source-chat').getAttribute('aria-pressed'), 'true');
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_conversations' && args.source === 'chatgpt'));
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_projects' && args.source === 'chatgpt'));
    await page.locator('#project-filter').selectOption('chat-project:key');
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_conversations' && args.projectKey === 'chat-project:key'));
    assert.equal(await page.locator('#project-filter option:checked').innerText(), 'Chat Project (1)');
    assert.equal(await page.locator('.conversation-item .project-label').innerText(), 'Chat Project');

    await page.locator('#source-codex').click();
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_conversations' && args.source === 'codex'));
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_projects' && args.source === 'codex'));
    assert.equal(await page.locator('#project-filter option').nth(1).innerText(), 'Codex Project (2)');
    await page.locator('#project-filter').selectOption('codex-project:key');
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_conversations' && args.projectKey === 'codex-project:key'));
    assert.equal(await page.locator('#source-codex').getAttribute('aria-pressed'), 'true');

    await page.locator('#source-chat').click();
    await page.waitForFunction(() => window.__calls.some(([name, args]) => name === 'list_conversations' && args.source === 'chatgpt' && args.projectKey === null));
    await page.locator('button.conversation-item').first().click();
    await page.locator('#continue-chatgpt').waitFor({state: 'visible'});
    assert.equal(await page.locator('#continue-chatgpt').isVisible(), true);
    await page.locator('#continue-chatgpt').click();
    await page.waitForFunction(id => window.__calls.some(([name, args]) => name === 'open_provider_conversation' && args.conversationId === id), UUID);
    const navigation = await page.evaluate(() => window.__calls.filter(([name]) => name === 'open_provider_conversation').map(([, args]) => args.conversationId));
    assert.deepEqual(navigation, [UUID]);

    await page.locator('#show-library').click();
    await page.locator('#search').fill('amber');
    await page.waitForFunction(() => document.querySelectorAll('button.conversation-item').length === 1 &&
      document.querySelector('.search-match-count')?.textContent === '2 matching messages');
    assert.equal(await page.locator('button.conversation-item').count(), 1);
    assert.equal(await page.locator('.search-match-count').innerText(), '2 matching messages');
    assert.equal(await page.locator('button.conversation-item strong mark').count(), 1);
    assert.equal(await page.locator('button.conversation-item .search-snippet mark').count(), 1);
    assert.deepEqual(errors, []);
    console.log('PASS chat/codex sources, project filter, direct continuation, grouped search and highlights');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error.stack || error.message); process.exitCode = 1; });
