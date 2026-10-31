function (key, nonce, expectedScope, projectId, requestedName) {
  const origin = "https://chatgpt.com";
  const state = { nonce, state: "running", cancelled: false };
  window[key] = state;
  let active = null;
  const finish = (next, extra = {}) => {
    if (window[key] === state) Object.assign(state, { state: next }, extra);
  };
  const fail = code => Object.assign(new Error(code), { safe: code });
  const plain = value => value && typeof value === "object" && !Array.isArray(value);
  const projectIdValid = value => typeof value === "string" && /^g-p-[A-Za-z0-9_-]{1,128}$/.test(value);
  const readJson = async response => {
    if (!response.body || !response.body.getReader) throw fail("request_failed");
    const reader = response.body.getReader(), chunks = [];
    let total = 0;
    try {
      while (true) {
        const part = await reader.read();
        if (part.done) break;
        total += part.value.byteLength;
        if (total > 16 * 1024 * 1024) throw fail("request_failed");
        chunks.push(part.value);
      }
    } finally { try { reader.releaseLock(); } catch (_) {} }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    try { return JSON.parse(new TextDecoder().decode(bytes)); } catch (_) { throw fail("request_failed"); }
  };
  const run = async () => {
    if (location.origin !== origin || !projectIdValid(projectId) ||
        typeof requestedName !== "string" || !requestedName.trim() || requestedName.trim().length > 120 ||
        /[\u0000-\u001f\u007f]/.test(requestedName)) {
      finish("failed", { error_code: "invalid_request" }); return;
    }
    const request = async (path, options = {}, parse = true) => {
      if (state.cancelled) throw fail("cancelled");
      const url = new URL(path, origin);
      if (url.origin !== origin) throw fail("invalid_request");
      active = new AbortController();
      let response;
      try {
        response = await fetch(url.href, {
          ...options, credentials: "include", cache: "no-store", redirect: "error", signal: active.signal,
        });
        if (response.status === 429) {
          let retry = Number(response.headers.get("retry-after"));
          if (!Number.isFinite(retry) || retry < 1) retry = 60;
          throw Object.assign(fail("rate_limited"), { retry_after: Math.min(1800, Math.ceil(retry)) });
        }
        if (response.status === 401 || response.status === 403) throw fail("signed_out");
        if (!response.ok) throw fail("request_failed");
        if (!parse) {
          try { await response.body?.cancel(); } catch (_) {}
          return null;
        }
        return await readJson(response);
      } finally {
        active = null;
        if (response?.body && !response.body.locked) {
          try { await response.body.cancel(); } catch (_) {}
        }
      }
    };
    try {
      const session = await request("/api/auth/session");
      const token = session && session.accessToken;
      if (typeof token !== "string" || !session.user) throw fail("signed_out");
      const encoded = token.split(".")[1];
      if (!encoded || encoded.length > 16000) throw fail("account_unavailable");
      let claims;
      try {
        claims = JSON.parse(atob(encoded.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - encoded.length % 4) % 4)));
      } catch (_) { throw fail("account_unavailable"); }
      const auth = claims && claims["https://api.openai.com/auth"];
      const account = auth && auth.chatgpt_account_id;
      const user = auth && (auth.chatgpt_user_id || auth.user_id);
      const valid = value => typeof value === "string" && value.length > 0 && value.length <= 256 && /^[A-Za-z0-9_:-]+$/.test(value);
      if (!valid(account) || !valid(user)) throw fail("account_unavailable");
      const identity = new TextEncoder().encode(JSON.stringify(["chatgpt-request-identity-v1", account, user]));
      const digest = await crypto.subtle.digest("SHA-256", identity);
      const scope = "chatgpt:observed:" + Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
      if (scope !== expectedScope) throw fail("account_changed");
      const headers = { Authorization: `Bearer ${token}`, "ChatGPT-Account-Id": account };
      const path = `/backend-api/gizmos/${encodeURIComponent(projectId)}`;
      const readProject = async () => {
        const data = await request(path, { headers });
        const gizmo = plain(data) && (plain(data.gizmo) ? data.gizmo : data);
        const display = plain(gizmo) && gizmo.display;
        if (!plain(gizmo) || gizmo.id !== projectId || !plain(display) ||
            typeof display.name !== "string" || !display.name.trim() || display.name.length > 120 ||
            typeof gizmo.instructions !== "string" || gizmo.instructions.length > 1024 * 1024) {
          throw fail("project_shape_unavailable");
        }
        const emoji = display.emoji == null ? null : display.emoji;
        const theme = display.theme == null ? null : display.theme;
        if ((emoji !== null && (typeof emoji !== "string" || emoji.length > 128)) ||
            (theme !== null && (typeof theme !== "string" || theme.length > 128))) throw fail("project_shape_unavailable");
        return { name: display.name, instructions: gizmo.instructions, emoji, theme };
      };
      const before = await readProject();
      const body = { name: requestedName.trim(), instructions: before.instructions, emoji: before.emoji, theme: before.theme };
      await request(`/backend-api/projects/${encodeURIComponent(projectId)}`, {
        method: "PATCH", headers: { ...headers, "Content-Type": "application/json" }, body: JSON.stringify(body),
      }, false);
      const after = await readProject();
      if (after.name !== body.name || after.instructions !== before.instructions ||
          after.emoji !== before.emoji || after.theme !== before.theme) throw fail("verification_failed");
      finish("applied", { verified: true });
    } catch (error) {
      const code = error && error.safe;
      const allowed = new Set(["cancelled", "invalid_request", "signed_out", "account_unavailable", "account_changed", "project_shape_unavailable", "request_failed", "rate_limited", "verification_failed"]);
      finish("failed", { error_code: allowed.has(code) ? code : "request_failed", ...(Number.isSafeInteger(error?.retry_after) ? { retry_after: error.retry_after } : {}) });
    }
  };
  run();
}
