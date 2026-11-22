// Server-mode composer and live conversation view. Prompts go to the web
// server (which drives the headless browser); the reply is drawn from the
// observed stream deltas, then replaced by the saved conversation.
import {renderMarkdown} from './render.js';
import {chatgptConversationId} from './continuation.js';
import {
  connectLiveStream, initialLive, liveAvailable, needsLibraryReload, promptProblem, reduceLive, rejectionCode, statusLine,
} from './live-stream.js';

const $ = id => document.getElementById(id);
const UUID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;
let state = initialLive();
let mode = null; // null | 'new' | 'continue'
let target = null; // ChatGPT conversation id, or null for a new chat
let current = null; // last saved conversation shown in the reader
let continueLocalId = null;
let drawTimer = null;
let reloadTimers = [];
let started = false;

const available = () => liveAvailable({
  native: typeof window.__TAURI_INTERNALS__?.invoke === 'function',
  transport: window.ChatHub?.transport,
  capabilities: window.ChatHub?.capabilities,
});
const busy = () => ['sending', 'waiting', 'streaming'].includes(state.phase);

function drawOutput() {
  drawTimer = null;
  const body = $('live-output-body');
  body.replaceChildren();
  if (state.text) renderMarkdown(state.text, body);
  $('live-output').hidden = !state.text;
}

function render() {
  $('live-status').textContent = statusLine(state);
  $('live-send').disabled = busy();
  $('live-target').textContent = target ? 'Continuing this chat' : 'New chat';
  if (!drawTimer) drawTimer = setTimeout(drawOutput, 120); // throttle markdown re-rendering
}

function reloadLibrary() {
  reloadTimers.forEach(clearTimeout);
  // The capture writer saves the reply shortly after the stream ends.
  reloadTimers = [
    setTimeout(() => {
      if (mode === 'continue') window.dispatchEvent(new Event('chat-hub-capture-updated'));
      else $('search').dispatchEvent(new Event('input', {bubbles: true}));
    }, 1500),
    setTimeout(() => dispatch({type: 'settled', keepText: mode !== 'continue'}), 6000),
  ];
}

function dispatch(action) {
  state = reduceLive(state, action);
  // A new chat becomes a continuing one as soon as ChatGPT names it.
  if (mode === 'new' && !target && UUID.test(state.conversationId || '')) target = state.conversationId;
  render();
  if (action.type !== 'settled' && needsLibraryReload(state)) reloadLibrary();
}

function openPanel(nextMode, nextTarget) {
  mode = nextMode;
  target = nextTarget;
  continueLocalId = nextMode === 'continue' ? current?.id ?? null : null;
  state = {...initialLive(), service: state.service};
  if (nextMode === 'new') {
    $('reader-empty').hidden = true;
    $('reader').hidden = true;
  }
  $('live-panel').hidden = false;
  render();
  refreshStatus();
  $('live-text').focus();
}

function closePanel() {
  mode = null;
  target = null;
  $('live-panel').hidden = true;
  if ($('reader').hidden) $('reader-empty').hidden = false;
}

async function refreshStatus() {
  try {
    const result = await window.ChatHub.session.liveStatus();
    dispatch({type: 'status', state: typeof result?.state === 'string' ? result.state : 'error'});
  } catch {
    dispatch({type: 'status', state: 'offline'});
  }
}

async function send(event) {
  event.preventDefault();
  if (busy()) return;
  const text = $('live-text').value;
  const problem = promptProblem(text);
  if (problem) { $('live-status').textContent = problem; return; }
  dispatch({type: 'sending', conversationId: target});
  try {
    await window.ChatHub.session.sendPrompt(target, text);
    $('live-text').value = '';
    dispatch({type: 'sent'});
  } catch (error) {
    dispatch({type: 'rejected', code: rejectionCode(error)});
  }
}

function showControls() {
  document.querySelectorAll('[data-live-new-chat]').forEach(button => { button.hidden = false; });
  const id = chatgptConversationId(current);
  $('live-continue-chat').hidden = !(available() && id);
}

function start() {
  if (started || !available()) return;
  started = true;
  showControls();
  connectLiveStream({EventSource: window.EventSource, dispatch});
  refreshStatus();
  setInterval(() => { if (!$('live-panel').hidden && !document.hidden) refreshStatus(); }, 20_000);
}

if (typeof document !== 'undefined' && $('live-panel')) {
  document.querySelectorAll('[data-live-new-chat]').forEach(button =>
    button.addEventListener('click', () => openPanel('new', null)));
  $('live-continue-chat').addEventListener('click', () => {
    const id = chatgptConversationId(current);
    if (id) openPanel('continue', id);
  });
  $('live-close').addEventListener('click', closePanel);
  $('live-composer').addEventListener('submit', send);
  window.addEventListener('chat-hub:authenticated', start);
  window.addEventListener('chat-hub:conversation-loaded', event => {
    current = event.detail;
    if (started) showControls();
  });
  window.addEventListener('chat-hub:conversation-loading', event => {
    // Reloading the conversation being continued must not close its composer.
    if (mode && !(mode === 'continue' && event.detail?.id === continueLocalId)) closePanel();
  });
}
