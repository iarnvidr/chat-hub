function (key, nonce, expectedScope, conversationId, targetProjectId) {
  const origin = "https://chatgpt.com";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  const projectId = value => typeof value === "string" && /^g-p-[A-Za-z0-9_-]{1,128}$/.test(value);
  const state = { nonce, state: "running", cancelled: false };
  window[key] = state;
  let active = null;
  const finish = (name, extra = {}) => {
    if (window[key] === state) Object.assign(state, { state: name }, extra);
  };
  const run = async () => {
    if (location.origin !== origin || !uuid.test(conversationId) ||
        (targetProjectId !== null && !projectId(targetProjectId)) ||
        typeof expectedScope !== "string" || !/^chatgpt:observed:[a-f0-9]{64}$/.test(expectedScope)) {
      finish("failed", { error_code: "invalid_context" }); return;
    }
    const request = async (path, options = {}, parseJson = true) => {
      if (state.cancelled) throw Object.assign(new Error("cancelled"), { safe: "cancelled" });
      active = new AbortController();
      const response = await fetch(new URL(path, origin).href, { ...options, credentials: "include", cache: "no-store", redirect: "error", signal: active.signal });
      if (response.status === 429) {
        let retry = Number(response.headers.get("retry-after"));
        if (!Number.isFinite(retry) || retry < 1) retry = 60;
        throw Object.assign(new Error("rate_limited"), { safe: "rate_limited", retry_after: Math.min(1800, Math.ceil(retry)) });
      }
      if (response.status === 401 || response.status === 403) throw Object.assign(new Error("signed_out"), { safe: "signed_out" });
      if (!response.ok) throw Object.assign(new Error("request_failed"), { safe: "request_failed" });
      if (!parseJson) { try { await response.body?.cancel(); } catch (_) {} return null; }
      const reader = response.body && response.body.getReader ? response.body.getReader() : null;
      if (!reader) throw Object.assign(new Error("request_failed"), { safe: "request_failed" });
      const chunks = [];
      let total = 0;
      try {
        while (true) {
          const part = await reader.read();
          if (part.done) break;
          total += part.value.byteLength;
          if (total > 16 * 1024 * 1024) throw Object.assign(new Error("request_failed"), { safe: "request_failed" });
          chunks.push(part.value);
        }
      } finally { try { reader.releaseLock(); } catch (_) {} }
      const bytes = new Uint8Array(total);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      return JSON.parse(new TextDecoder().decode(bytes));
    };
    try {
      const session = await request("/api/auth/session");
      const token = session && session.accessToken;
      if (typeof token !== "string" || !session.user) throw Object.assign(new Error("signed_out"), { safe: "signed_out" });
      const part = token.split(".")[1];
      if (!part || part.length > 16000) throw Object.assign(new Error("account_unavailable"), { safe: "account_unavailable" });
      const claims = JSON.parse(atob(part.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - part.length % 4) % 4)));
      const auth = claims["https://api.openai.com/auth"];
      const account = auth && auth.chatgpt_account_id;
      const user = auth && (auth.chatgpt_user_id || auth.user_id);
      const valid = value => typeof value === "string" && value.length > 0 && value.length <= 256 && /^[A-Za-z0-9_:-]+$/.test(value);
      if (!valid(account) || !valid(user)) throw Object.assign(new Error("account_unavailable"), { safe: "account_unavailable" });
      const bytes = new TextEncoder().encode(JSON.stringify(["chatgpt-request-identity-v1", account, user]));
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const scope = "chatgpt:observed:" + Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
      if (scope !== expectedScope) throw Object.assign(new Error("account_changed"), { safe: "account_changed" });
      const headers = { Authorization: `Bearer ${token}`, "ChatGPT-Account-Id": account };
      const readConversation = async () => {
        const detail = await request(`/backend-api/conversation/${encodeURIComponent(conversationId)}`, { headers });
        const actualId = detail && (detail.conversation_id || detail.id);
        if (!detail || actualId !== conversationId) throw Object.assign(new Error("conversation_unavailable"), { safe: "conversation_unavailable" });
        return detail;
      };
      const before = await readConversation();
      let title = null;
      if (targetProjectId !== null) {
        let found = null;
        const seen = new Set();
        let cursor = null;
        for (let page = 0; page < 20; page++) {
          const params = new URLSearchParams({ conversations_per_gizmo: "0", limit: "20", owned_only: "false" });
          if (cursor) params.set("cursor", cursor);
          const catalog = await request(`/backend-api/gizmos/snorlax/sidebar?${params}`, { headers });
          const items = catalog && catalog.items;
          if (!Array.isArray(items)) throw Object.assign(new Error("project_catalog_unavailable"), { safe: "project_catalog_unavailable" });
          for (const item of items) {
            const gizmo = item && item.gizmo && (item.gizmo.gizmo || item.gizmo);
            if (gizmo && gizmo.id === targetProjectId) {
              found = gizmo;
              break;
            }
          }
          if (found) break;
          cursor = typeof catalog.cursor === "string" && catalog.cursor ? catalog.cursor : null;
          if (!cursor || seen.has(cursor)) break;
          seen.add(cursor);
        }
        if (!found) throw Object.assign(new Error("project_unavailable"), { safe: "project_unavailable" });
        title = [found.display && found.display.name, found.title, found.name].find(v => typeof v === "string" && v.trim());
        if (title != null && (typeof title !== "string" || new TextEncoder().encode(title).length > 4096)) throw Object.assign(new Error("project_name_unavailable"), { safe: "project_name_unavailable" });
      }
      await request(`/backend-api/conversation/${encodeURIComponent(conversationId)}`, {
        method: "PATCH", headers: { ...headers, "Content-Type": "application/json" },
        body: JSON.stringify({ gizmo_id: targetProjectId || "" })
      }, false);
      const after = await readConversation();
      const actual = typeof after.gizmo_id === "string" && after.gizmo_id ? after.gizmo_id : null;
      if (actual !== targetProjectId) throw Object.assign(new Error("verification_failed"), { safe: "verification_failed" });
      finish("applied", { project_title: targetProjectId === null ? null : title });
    } catch (error) {
      if (state.cancelled || (error && error.name === "AbortError")) finish("cancelled");
      else finish("failed", {
        error_code: error && typeof error.safe === "string" ? error.safe : "request_failed",
        ...(error && Number.isInteger(error.retry_after) ? { retry_after: error.retry_after } : {})
      });
    } finally {
      active = null;
    }
  };
  state.cancel = () => { state.cancelled = true; if (active) active.abort(); };
  run();
}
