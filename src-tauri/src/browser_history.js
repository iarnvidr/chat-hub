(function startChatHubBrowserHistory(key) {
  'use strict';
  const state = {
    status: 'starting', stage: 'auth', complete: false, cancelled: false,
    sequence: 0, ackSequence: 0, chunk: null, conversations: 0,
    listed: 0, exports: 0, error_code: '', retry_after: 0, retry_after_too_long: false,
    http_status: 0, response_content_type: 'missing', challenge_detected: false,
    provider_error_code: '',
  };
  window[key] = state;
  const API = 'https://chatgpt.com/backend-api/';
  const ORIGIN = 'https://chatgpt.com';
  const HARD_BODY = 60 * 1024 * 1024;
  const SOFT_EXPORT = 4 * 1024 * 1024;
  const SOFT_COUNT = 100;
  const MAX_ROWS = 100000;
  const MAX_PAGES = 1000;
  const MAX_RETRY_AFTER_SECONDS = 365 * 24 * 60 * 60;
  const MAX_ERROR_BODY = 4 * 1024;
  const PROVIDER_ERROR_CODES = new Set([
    'rate_limit_exceeded', 'rate_limit_reached', 'too_many_requests',
    'insufficient_quota', 'usage_limit_exceeded', 'model_rate_limit_exceeded',
  ]);
  const enc = new TextEncoder();
  let cancelled = false;
  let activeController = null;
  Object.defineProperty(state, 'cancelled', {
    enumerable: true, get: () => cancelled,
    set: value => { cancelled = Boolean(value); if (cancelled && activeController) activeController.abort(); },
  });
  let bearer = '';
  let accountId = '';
  let accountScope = '';
  let owner = '';
  const projectCatalog = new Map();

  function validId(value) {
    return typeof value === 'string' && value.length > 0 && value.length <= 256 && /^[A-Za-z0-9_:-]+$/.test(value);
  }
  function visibleSidebarProjectTitles() {
    const found = new Map();
    if (typeof document === 'undefined' || typeof document.querySelectorAll !== 'function') return found;
    let anchors;
    try { anchors = document.querySelectorAll('a[href]'); } catch (_) { return found; }
    let inspected = 0;
    for (const anchor of anchors) {
      if (++inspected > 2000) break;
      try {
        const url = new URL(anchor.href, ORIGIN);
        const match = url.origin === ORIGIN
          ? url.pathname.match(/^\/g\/(g-p-[A-Za-z0-9-]{1,240})\/project\/?$/)
          : null;
        if (!match || !anchor.getClientRects || anchor.getClientRects().length === 0 ||
            anchor.getAttribute?.('aria-hidden') === 'true') continue;
        const style = typeof window.getComputedStyle === 'function' ? window.getComputedStyle(anchor) : null;
        if (style && (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0')) continue;
        const title = typeof anchor.innerText === 'string'
          ? anchor.innerText.replace(/\s+/g, ' ').trim()
          : '';
        const lowerTitle = title.toLocaleLowerCase();
        if (!title || enc.encode(title).byteLength > 4096 ||
            ['project', 'projects', 'all projects', 'new project', 'create project'].includes(lowerTitle)) continue;
        if (!found.has(match[1])) found.set(match[1], title);
      } catch (_) {}
    }
    return found;
  }
  function contentTypeClass(response) {
    const raw = response.headers.get('content-type');
    if (typeof raw !== 'string' || !raw.trim()) return 'missing';
    const mime = raw.split(';', 1)[0].trim().toLowerCase();
    if (mime === 'text/html' || mime === 'application/xhtml+xml') return 'html';
    if (mime === 'application/json' || mime.endsWith('+json')) return 'json';
    return 'other';
  }
  function safeProviderErrorCode(text) {
    try {
      const value = JSON.parse(text);
      const candidates = [value?.code, value?.type, value?.error?.code,
        value?.error?.type, value?.error?.error?.code, value?.detail?.code];
      return candidates.find(code => typeof code === 'string' && PROVIDER_ERROR_CODES.has(code.toLowerCase()))?.toLowerCase() || '';
    } catch (_) { return ''; }
  }
  async function rateLimitDiagnostics(response) {
    const contentType = contentTypeClass(response);
    const challengeDetected = response.headers.get('cf-mitigated')?.trim().toLowerCase() === 'challenge';
    let providerErrorCode = '';
    if (contentType === 'json') {
      try { providerErrorCode = safeProviderErrorCode(await readBounded(response, MAX_ERROR_BODY)); }
      catch (_) { try { await response.body?.cancel(); } catch (_) {} }
    } else {
      try { await response.body?.cancel(); } catch (_) {}
    }
    return {contentType, challengeDetected, providerErrorCode};
  }
  function retryAfterSeconds(header) {
    const raw = typeof header === 'string' ? header.trim() : '';
    if (!raw) return {seconds: 60, tooLong: false};
    let seconds;
    if (/^\d+$/.test(raw)) {
      const parsed = Number(raw);
      if (!Number.isSafeInteger(parsed) || parsed > MAX_RETRY_AFTER_SECONDS) return {seconds: 0, tooLong: true};
      seconds = parsed;
    } else {
      const timestamp = Date.parse(raw);
      if (!Number.isFinite(timestamp)) return {seconds: 60, tooLong: false};
      seconds = Math.max(1, Math.ceil((timestamp - Date.now()) / 1000));
      if (seconds > MAX_RETRY_AFTER_SECONDS) return {seconds: 0, tooLong: true};
    }
    return {seconds: Math.max(1, seconds), tooLong: false};
  }
  function safeFail(code, stage, retryAfter, httpStatus, diagnostics = {}) {
    state.status = code === 'rate_limited' ? 'waiting' : 'failed';
    state.stage = stage;
    state.error_code = code;
    state.retry_after = Number.isFinite(retryAfter) ? Math.max(0, Math.floor(retryAfter)) : 0;
    state.retry_after_too_long = diagnostics.retryAfterTooLong === true;
    state.http_status = Number.isInteger(httpStatus) ? Math.max(0, Math.min(599, httpStatus)) : 0;
    state.response_content_type = ['json', 'html', 'other', 'missing'].includes(diagnostics.contentType)
      ? diagnostics.contentType : 'missing';
    state.challenge_detected = diagnostics.challengeDetected === true;
    state.provider_error_code = PROVIDER_ERROR_CODES.has(diagnostics.providerErrorCode)
      ? diagnostics.providerErrorCode : '';
    state.complete = false;
  }
  function checkCancelled() {
    if (state.cancelled) throw Object.assign(new Error('cancelled'), {code: 'cancelled'});
  }
  function decodeJwt(token) {
    try {
      const part = token.split('.')[1];
      if (!part || part.length > 16000) return null;
      const padded = part.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - part.length % 4) % 4);
      return JSON.parse(atob(padded));
    } catch (_) { return null; }
  }
  async function sha256(text) {
    const bytes = await crypto.subtle.digest('SHA-256', enc.encode(text));
    return Array.from(new Uint8Array(bytes), b => b.toString(16).padStart(2, '0')).join('');
  }
  async function readBounded(response, maximum) {
    const length = Number(response.headers.get('content-length') || 0);
    if (length > maximum) throw Object.assign(new Error(), {code: 'response_too_large'});
    if (!response.body || typeof response.body.getReader !== 'function') {
      const raw = await response.text();
      if (enc.encode(raw).byteLength > maximum) throw Object.assign(new Error(), {code: 'response_too_large'});
      return raw;
    }
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    try {
      for (;;) {
        checkCancelled();
        const next = await reader.read();
        if (next.done) break;
        total += next.value.byteLength;
        if (total > maximum) {
          try { await reader.cancel(); } catch (_) {}
          throw Object.assign(new Error(), {code: 'response_too_large'});
        }
        chunks.push(next.value);
      }
    } finally {
      try { reader.releaseLock(); } catch (_) {}
    }
    const all = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { all.set(chunk, offset); offset += chunk.byteLength; }
    return new TextDecoder().decode(all);
  }
  async function request(path, params, body, stage, maximum = HARD_BODY) {
    checkCancelled();
    if (location.origin !== ORIGIN || path.startsWith('/') || path.split('/').includes('..')) {
      throw Object.assign(new Error(), {code: 'invalid_request'});
    }
    const sessionPath = path === 'auth/session';
    const url = sessionPath ? new URL('/api/auth/session', ORIGIN) : new URL(API + path);
    if (url.origin !== ORIGIN || (!sessionPath && !url.pathname.startsWith('/backend-api/'))) {
      throw Object.assign(new Error(), {code: 'invalid_request'});
    }
    if (params) for (const [name, value] of Object.entries(params)) url.searchParams.set(name, String(value));
    const controller = new AbortController();
    activeController = controller;
    const timer = setTimeout(() => controller.abort(), 60000);
    let response;
    try {
      response = await fetch(url.href, {
        method: body === undefined ? 'GET' : 'POST',
        headers: {Accept: 'application/json',
          ...(!sessionPath && bearer ? {Authorization: 'Bearer ' + bearer} : {}),
          ...(!sessionPath && accountId ? {'ChatGPT-Account-Id': accountId} : {}),
          ...(body === undefined ? {} : {'Content-Type': 'application/json'})},
        body: body === undefined ? undefined : JSON.stringify(body),
        credentials: 'include', cache: 'no-store', redirect: 'error', signal: controller.signal,
      });
      if (response.status === 429) {
        const retry = retryAfterSeconds(response.headers.get('retry-after'));
        const diagnostics = await rateLimitDiagnostics(response);
        throw Object.assign(new Error(), {code: 'rate_limited', retryAfter: retry.seconds,
          retryAfterTooLong: retry.tooLong, httpStatus: response.status, stage,
          responseContentType: diagnostics.contentType,
          challengeDetected: diagnostics.challengeDetected,
          providerErrorCode: diagnostics.providerErrorCode});
      }
      if (response.status === 401) throw Object.assign(new Error(), {code: 'login_required', httpStatus: response.status, stage});
      if (response.status !== 200) throw Object.assign(new Error(), {code: 'http_error', httpStatus: response.status, stage});
      const text = await readBounded(response, maximum);
      try { return JSON.parse(text); }
      catch (_) { throw Object.assign(new Error(), {code: 'invalid_response', stage}); }
    } catch (error) {
      if (state.cancelled) throw Object.assign(new Error(), {code: 'cancelled', stage});
      if (error && error.name === 'AbortError') throw Object.assign(new Error(), {code: 'timeout', stage});
      if (error && error.code) throw error;
      throw Object.assign(new Error(), {code: 'network_error', stage});
    } finally {
      clearTimeout(timer);
      if (activeController === controller) activeController = null;
      try { await response?.body?.cancel(); } catch (_) {}
    }
  }
  async function pages(path, params, cursor, stage) {
    const seen = new Set();
    const output = [];
    let current = {...params};
    let offset = Number(current.offset || 0);
    for (let page = 0; page < MAX_PAGES; page++) {
      checkCancelled();
      const result = await request(path, current, undefined, stage, 4 * 1024 * 1024);
      if (!result || !Array.isArray(result.items)) throw Object.assign(new Error(), {code: 'invalid_response', stage});
      for (const item of result.items) {
        if (item && typeof item === 'object' && !Array.isArray(item)) {
          if (path === 'gizmos/snorlax/sidebar') {
            const wrapper = item.gizmo && typeof item.gizmo === 'object' ? item.gizmo : {};
            const gizmo = wrapper.gizmo && typeof wrapper.gizmo === 'object' ? wrapper.gizmo : wrapper;
            if (gizmo.id !== undefined) {
              if (!validId(gizmo.id)) throw Object.assign(new Error(), {code: 'invalid_response', stage});
              const title = gizmo.display?.name ?? gizmo.title ?? gizmo.name;
              const safeTitle = typeof title === 'string' && enc.encode(title).byteLength <= 4096 ? title : null;
              output.push({gizmo: {id: gizmo.id, title: safeTitle}});
            }
          } else {
            const id = providerId(item);
            if (!id) continue;
            const updateTime = item.update_time === undefined || item.update_time === null ? '' : item.update_time;
            const projectId = item.gizmo_id === undefined || item.gizmo_id === null ? '' : item.gizmo_id;
            if (!validId(id) || typeof updateTime !== 'string' || updateTime.length > 64 ||
                (projectId && !validId(projectId))) {
              throw Object.assign(new Error(), {code: 'invalid_response', stage});
            }
            output.push({id, update_time: updateTime, ...(projectId ? {gizmo_id: projectId} : {})});
          }
        }
        if (output.length > MAX_ROWS) throw Object.assign(new Error(), {code: 'result_limit', stage});
      }
      if (cursor) {
        const next = result.cursor;
        if (!next) return output;
        const marker = 'c:' + String(next);
        if (seen.has(marker)) throw Object.assign(new Error(), {code: 'invalid_pagination', stage});
        seen.add(marker);
        current = {...params, cursor: next};
      } else {
        if (result.items.length === 0) return output;
        offset += result.items.length;
        const total = result.total === undefined ? offset : Number(result.total);
        if (!Number.isFinite(total) || total < 0) throw Object.assign(new Error(), {code: 'invalid_response', stage});
        if (offset >= total) return output;
        const marker = 'o:' + offset;
        if (seen.has(marker)) throw Object.assign(new Error(), {code: 'invalid_pagination', stage});
        seen.add(marker);
        current = {...params, offset};
      }
    }
    throw Object.assign(new Error(), {code: 'page_limit', stage});
  }
  function providerId(row) {
    const id = row && (row.id || row.conversation_id);
    return typeof id === 'string' && id ? id : '';
  }
  async function sendChunk(graphs, projectForId = new Map()) {
    if (!graphs.length && !projectForId.size) return;
    checkCancelled();
    const sequence = ++state.sequence;
    state.chunk = {sequence, owner, account_scope: accountScope,
      format: 'chat-hub-history-v1', conversations: graphs};
    if (projectForId.size) {
      state.chunk.project_catalog = [...new Set(projectForId.values())].filter(Boolean)
        .map(id => ({id, title: projectCatalog.get(id) ?? null}));
      state.chunk.conversation_projects = [...projectForId].map(([conversation_id, project_id]) => ({conversation_id, project_id}));
    }
    state.exports++;
    while (state.ackSequence < sequence && state.chunk && state.chunk.sequence === sequence) {
      checkCancelled();
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    if (state.chunk && state.chunk.sequence === sequence) state.chunk = null;
  }
  async function sendProjectCatalog() {
    if (!projectCatalog.size) return;
    checkCancelled();
    const sequence = ++state.sequence;
    state.chunk = {sequence, owner, account_scope: accountScope,
      format: 'chat-hub-history-v1', conversations: [],
      project_catalog: [...projectCatalog].map(([id, title]) => ({id, title}))};
    state.exports++;
    while (state.ackSequence < sequence && state.chunk && state.chunk.sequence === sequence) {
      checkCancelled();
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    if (state.chunk && state.chunk.sequence === sequence) state.chunk = null;
  }
  async function deliverGraphs(rows, projectForId) {
    let pending = [];
    let pendingBytes = 0;
    let pendingProjects = new Map();
    const changed = [];
    for (const [id, row] of rows) changed.push({id, row});
    for (let start = 0; start < changed.length; start += 10) {
      checkCancelled();
      state.stage = 'detail';
      const selected = changed.slice(start, start + 10);
      const batch = await request('conversations/batch', null,
        {conversation_ids: selected.map(item => item.id)}, 'detail');
      if (!Array.isArray(batch)) throw Object.assign(new Error(), {code: 'invalid_response', stage: 'detail'});
      const fetched = new Map();
      for (const graph of batch) if (graph && typeof graph === 'object' && !Array.isArray(graph)) {
        const id = providerId(graph);
        if (id) fetched.set(id, graph);
      }
      for (const item of selected) {
        let graph = fetched.get(item.id);
        if (!graph) graph = await request('conversation/' + encodeURIComponent(item.id), null, undefined, 'detail');
        if (!graph || typeof graph !== 'object' || Array.isArray(graph) || providerId(graph) !== item.id ||
            !graph.mapping || typeof graph.mapping !== 'object' || Array.isArray(graph.mapping)) {
          throw Object.assign(new Error(), {code: 'invalid_response', stage: 'detail'});
        }
        const bytes = enc.encode(JSON.stringify(graph)).byteLength;
        if (bytes > HARD_BODY) throw Object.assign(new Error(), {code: 'conversation_too_large', stage: 'detail'});
        if (pending.length && (pending.length >= SOFT_COUNT || pendingBytes + bytes > SOFT_EXPORT)) {
          await sendChunk(pending, pendingProjects);
          pending = []; pendingBytes = 0; pendingProjects = new Map();
        }
        pending.push(graph); pendingBytes += bytes; state.conversations++;
        const projectId = projectForId.get(item.id) || item.row.gizmo_id || graph.gizmo_id;
        if (validId(projectId)) pendingProjects.set(item.id, projectId);
      }
      // ACK each successful detail batch before making another provider request.
      if (pending.length) {
        await sendChunk(pending, pendingProjects);
        pending = []; pendingBytes = 0; pendingProjects = new Map();
      }
    }
  }
  async function run() {
    try {
      if (location.origin !== ORIGIN) throw Object.assign(new Error(), {code: 'wrong_origin', stage: 'auth'});
      state.stage = 'auth';
      const session = await request('auth/session', null, undefined, 'auth', 1024 * 1024);
      if (!session || typeof session.accessToken !== 'string' || !session.accessToken) {
        throw Object.assign(new Error(), {code: 'signed_out', stage: 'auth'});
      }
      bearer = session.accessToken;
      const claims = decodeJwt(bearer);
      const auth = claims && claims['https://api.openai.com/auth'];
      if (!auth || typeof auth !== 'object' || Array.isArray(auth)) throw Object.assign(new Error(), {code: 'auth_invalid', stage: 'auth'});
      accountId = auth.chatgpt_account_id;
      let userId = auth.chatgpt_user_id;
      if (userId === undefined || userId === null) userId = auth.user_id;
      if (!validId(accountId) || !validId(userId) || typeof claims.sub !== 'string' ||
          !claims.sub || enc.encode(claims.sub).byteLength > 4096 ||
          !Number.isFinite(claims.exp) || claims.exp <= Date.now() / 1000) {
        throw Object.assign(new Error(), {code: 'auth_invalid', stage: 'auth'});
      }
      accountScope = 'chatgpt:observed:' + await sha256(JSON.stringify([
        'chatgpt-request-identity-v1', accountId, userId]));
      owner = (await sha256(accountId + ':' + claims.sub)).slice(0, 24);
      // Snapshot visible project labels now, but prioritize saving ordinary chats.
      // Acknowledge them before the first provider API request so a list 429
      // does not discard names already visible in the signed-in page.
      for (const [id, title] of visibleSidebarProjectTitles()) projectCatalog.set(id, title);
      await sendProjectCatalog();
      state.stage = 'list';
      const rows = new Map();
      const activeRows = await pages('conversations', {offset: 0, limit: 100, order: 'updated',
        is_archived: 'false', hide_snorlax: 'false'}, false, 'list');
      for (const row of activeRows) {
        const id = providerId(row);
        if (id) rows.set(id, row);
      }
      state.listed = rows.size;
      await deliverGraphs(rows, new Map());

      // Commit active chats before archive or project endpoints can rate limit.
      state.stage = 'list';
      const archivedRows = await pages('conversations', {offset: 0, limit: 100, order: 'updated',
        is_archived: 'true', hide_snorlax: 'false'}, false, 'list');
      const newArchivedRows = new Map();
      for (const row of archivedRows) {
        const id = providerId(row);
        if (id && !rows.has(id)) {
          rows.set(id, row);
          newArchivedRows.set(id, row);
        }
      }
      state.listed = rows.size;
      await deliverGraphs(newArchivedRows, new Map());

      // Sidebar names and project membership are enrichment; ordinary active and
      // archived conversation graphs have already been acknowledged locally.
      state.stage = 'project';
      const projects = await pages('gizmos/snorlax/sidebar', {
        conversations_per_gizmo: 0, limit: 20, owned_only: 'false'}, true, 'project');
      const catalogBeforeApi = JSON.stringify([...projectCatalog]);
      for (const item of projects) {
        const gizmo = item.gizmo || {};
        if (!gizmo || !validId(gizmo.id)) continue;
        if (gizmo.title) projectCatalog.set(gizmo.id, gizmo.title);
      }
      if (JSON.stringify([...projectCatalog]) !== catalogBeforeApi) await sendProjectCatalog();
      // Membership is supplemental. Sidebar titles were already acknowledged;
      // a later project-conversation 429 must not discard those names.
      state.stage = 'project';
      const projectRows = new Map();
      let memberships = new Map();
      const newProjectForId = new Map();
      for (const item of projects) {
        const wrapper = item.gizmo || {};
        const gizmo = wrapper.gizmo || wrapper;
        if (!gizmo || !validId(gizmo.id)) continue;
        const chats = await pages('gizmos/' + encodeURIComponent(gizmo.id) + '/conversations',
          {cursor: '0'}, true, 'project');
        for (const row of chats) {
          const id = providerId(row);
          if (!id) continue;
          if (rows.has(id)) memberships.set(id, gizmo.id);
          else { projectRows.set(id, row); newProjectForId.set(id, gizmo.id); }
          if (memberships.size >= 100) { await sendChunk([], memberships); memberships = new Map(); }
        }
      }
      if (memberships.size) await sendChunk([], memberships);
      if (projectRows.size) await deliverGraphs(projectRows, newProjectForId);
      state.stage = 'complete';
      state.status = 'complete';
      state.complete = true;
    } catch (error) {
      if (error && error.code === 'cancelled') {
        state.status = 'cancelled'; state.stage = 'cancelled'; state.complete = false;
      } else {
        safeFail(error && error.code || 'sync_failed', error && error.stage || state.stage,
          error && error.retryAfter, error && error.httpStatus, {
            retryAfterTooLong: error && error.retryAfterTooLong,
            contentType: error && error.responseContentType,
            challengeDetected: error && error.challengeDetected,
            providerErrorCode: error && error.providerErrorCode,
          });
      }
    } finally {
      bearer = '';
      accountId = '';
    }
  }
  state.ready = run();
  return state;
})
