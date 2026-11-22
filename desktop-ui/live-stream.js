// Pure live-view logic: capability gating, stream event parsing and the state
// machine. No DOM and no imports, so it can be tested directly. The live text
// comes only from stream deltas the server observed, never from pixels.

export const MAX_LIVE_CHARS = 500_000;
export const MAX_PROMPT_BYTES = 32 * 1024;

// The server-mode live view. Desktop uses its own provider view instead.
export const liveAvailable = ({native = false, transport, capabilities} = {}) =>
  !native && transport === 'http' && capabilities?.live === true;

// Desktop-only provider controls (open ChatGPT in the app, continue there).
export const providerViewAvailable = ({native = false, capabilities} = {}) =>
  native === true || capabilities?.provider_view === true;

export const promptBytes = text => new TextEncoder().encode(String(text ?? '')).length;
export const promptProblem = text => {
  if (!String(text ?? '').trim()) return 'Type a message first.';
  if (promptBytes(text) > MAX_PROMPT_BYTES) return 'That message is too long to send.';
  return '';
};

const KINDS = new Set(['start', 'delta', 'done', 'error']);

export function parseLiveEvent(data) {
  let value;
  try { value = JSON.parse(data); } catch { return null; }
  if (!value || typeof value !== 'object' || !KINDS.has(value.kind)) return null;
  const {conversation_id: conversationId = null, message_id: messageId, text_delta: delta = ''} = value;
  if (typeof messageId !== 'string' || messageId.length > 128 || typeof delta !== 'string' || delta.length > 16_384) return null;
  if (conversationId !== null && (typeof conversationId !== 'string' || conversationId.length > 128)) return null;
  return {kind: value.kind, conversationId, messageId, delta};
}

export const initialLive = () => ({phase: 'idle', service: 'unknown', messageId: null, conversationId: null, text: ''});

// phase: idle | sending | waiting | streaming | done | interrupted | resync | busy | error
// service: unknown | idle | busy | needs_sign_in | error | offline
export function reduceLive(state, action) {
  switch (action.type) {
    case 'sending': return {...state, phase: 'sending', text: '', messageId: null, conversationId: action.conversationId ?? null};
    case 'sent': return {...state, phase: 'waiting'};
    case 'rejected':
      if (action.code === 'busy') return {...state, phase: 'busy'};
      return {...state, phase: 'error', service: action.code === 'browser_unavailable' ? 'offline' : state.service};
    case 'status': return {...state, service: action.state};
    case 'resync': return {...state, phase: 'resync'};
    case 'settled': return {...state, phase: 'idle', text: action.keepText ? state.text : ''};
    case 'live': {
      const e = action.event;
      if (e.kind === 'done') return {...state, phase: 'done', conversationId: e.conversationId ?? state.conversationId};
      if (e.kind === 'error') return {...state, phase: 'interrupted', conversationId: e.conversationId ?? state.conversationId};
      const fresh = e.messageId !== state.messageId;
      const text = ((fresh ? '' : state.text) + e.delta).slice(0, MAX_LIVE_CHARS);
      return {...state, phase: 'streaming', messageId: e.messageId, text, conversationId: e.conversationId ?? state.conversationId};
    }
    default: return state;
  }
}

// One line for the status area; the sign-in instruction is deliberately plain text.
export function statusLine(state) {
  if (state.service === 'needs_sign_in') return 'ChatGPT needs a sign-in. Sign in with the local helper on the server, then try again.';
  if (state.service === 'offline') return 'The browser service is offline. Saved chats still work.';
  switch (state.phase) {
    case 'sending': return 'Sending…';
    case 'waiting': return 'Sent. Waiting for the reply…';
    case 'streaming': return 'ChatGPT is replying…';
    case 'done': return 'Reply received. Saving to your library…';
    case 'interrupted': return 'The reply stream was interrupted. Reloading the saved copy…';
    case 'resync': return 'Live connection resumed. Reloading the saved copy…';
    case 'busy': return 'ChatGPT is busy with another message. Try again in a moment.';
    case 'error': return 'The message could not be sent.';
    default: return state.service === 'busy' ? 'ChatGPT is busy with another message.' : '';
  }
}

// Map a failed send (HTTP error from the web bridge) to a reducer `rejected` code.
export const rejectionCode = error =>
  error?.status === 409 || error?.code === 'busy' ? 'busy'
    : error?.code === 'browser_unavailable' ? 'browser_unavailable' : 'failed';

// Phases after which the saved conversation should be reloaded from the library.
export const needsLibraryReload = state => ['done', 'interrupted', 'resync'].includes(state.phase);

// EventSource keeps reconnecting by itself; a reconnect means events may have
// been missed, so it reports `resync` instead of silently continuing.
export function connectLiveStream({EventSource, url = '/api/live/stream', dispatch}) {
  const source = new EventSource(url);
  let interrupted = false;
  source.addEventListener('open', () => {
    if (!interrupted) return;
    interrupted = false;
    dispatch({type: 'status', state: 'idle'});
    dispatch({type: 'resync'});
  });
  source.addEventListener('live', message => {
    const event = parseLiveEvent(message.data);
    if (event) dispatch({type: 'live', event});
  });
  source.addEventListener('resync', () => dispatch({type: 'resync'}));
  source.addEventListener('error', () => {
    interrupted = true;
    dispatch({type: 'status', state: 'offline'});
  });
  return {close: () => source.close()};
}
