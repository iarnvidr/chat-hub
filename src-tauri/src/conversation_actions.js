function (key, nonce, expectedScope, conversationId, action, desired) {
  const origin = "https://chatgpt.com";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const state = { nonce, state: "running", cancelled: false };
  window[key] = state;
  const MAX_RETRY_AFTER_SECONDS = 365 * 24 * 60 * 60;
  let active = null;
  const finish = (name, extra = {}) => {
    if (window[key] === state) Object.assign(state, { state: name }, extra);
  };
  const fail = code => Object.assign(new Error(code), { safe: code });
  const plain = value => value && typeof value === "object" && !Array.isArray(value);
  const run = async () => {
    if (location.origin !== origin || !uuid.test(conversationId) ||
        typeof expectedScope !== "string" || !/^chatgpt:observed:[a-f0-9]{64}$/.test(expectedScope) ||
        !["archive", "pin"].includes(action) || typeof desired !== "boolean") {
      finish("failed", { error_code: "invalid_request" }); return;
    }
    const request = async (path, options = {}, parse = true) => {
      if (state.cancelled) throw fail("cancelled");
      const url = new URL(path, origin);
      if (url.origin !== origin) throw fail("invalid_request");
      active = new AbortController();
      const timeout = setTimeout(() => active?.abort(), 20000);
      let response;
      try {
        response = await fetch(url.href, { ...options, credentials: "include", cache: "no-store", redirect: "error", signal: active.signal });
        if (response.status === 429) {
          const raw = response.headers.get("retry-after");
          let retry = 60;
          if (typeof raw === "string" && /^\d+$/.test(raw.trim())) {
            const parsed = Number(raw.trim());
            retry = Number.isSafeInteger(parsed) ? parsed : MAX_RETRY_AFTER_SECONDS + 1;
          } else if (typeof raw === "string" && raw.trim()) {
            const timestamp = Date.parse(raw);
            if (Number.isFinite(timestamp)) retry = Math.max(1, Math.ceil((timestamp - Date.now()) / 1000));
          }
          if (retry > MAX_RETRY_AFTER_SECONDS) retry = MAX_RETRY_AFTER_SECONDS + 1;
          throw Object.assign(fail("rate_limited"), { retry_after: Math.max(1, retry) });
        }
        if (response.status === 401 || response.status === 403) throw fail("signed_out");
        if (!response.ok) throw fail("request_failed");
        if (!parse) return null;
        if (!response.body || !response.body.getReader) throw fail("request_failed");
        const reader = response.body.getReader(), chunks = [];
        let total = 0;
        try {
          while (true) {
            const part = await reader.read();
            if (part.done) break;
            total += part.value.byteLength;
            if (total > 4 * 1024 * 1024) throw fail("request_failed");
            chunks.push(part.value);
          }
        } finally { try { reader.releaseLock(); } catch (_) {} }
        const bytes = new Uint8Array(total);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        try { return JSON.parse(new TextDecoder().decode(bytes)); } catch (_) { throw fail("request_failed"); }
      } finally {
        clearTimeout(timeout);
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
      try { claims = JSON.parse(atob(encoded.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - encoded.length % 4) % 4))); }
      catch (_) { throw fail("account_unavailable"); }
      const auth = claims && claims["https://api.openai.com/auth"];
      const account = auth && auth.chatgpt_account_id;
      const user = auth && (auth.chatgpt_user_id || auth.user_id);
      const validClaim = value => typeof value === "string" && value.length > 0 && value.length <= 256 && /^[A-Za-z0-9_:-]+$/.test(value);
      if (!validClaim(account) || !validClaim(user)) throw fail("account_unavailable");
      const identity = new TextEncoder().encode(JSON.stringify(["chatgpt-request-identity-v1", account, user]));
      const digest = await crypto.subtle.digest("SHA-256", identity);
      const observedScope = "chatgpt:observed:" + Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
      if (observedScope !== expectedScope) throw fail("account_changed");
      const headers = { Authorization: `Bearer ${token}`, "ChatGPT-Account-Id": account };
      const detailPath = `/backend-api/conversation/${encodeURIComponent(conversationId)}`;
      const readConversation = async () => {
        const detail = await request(detailPath, { headers });
        if (!plain(detail) || (detail.conversation_id || detail.id) !== conversationId) throw fail("conversation_unavailable");
        return detail;
      };
      const isPinned = (value, desiredState) => {
        const rows = Array.isArray(value) ? value : plain(value) &&
          [value.items, value.pins, value.data].find(Array.isArray);
        if (!rows) throw fail("verification_failed");
        if (rows.length > 1000) throw fail("verification_failed");
        let found = false;
        for (const row of rows) {
          if (!plain(row)) throw fail("verification_failed");
          const item = plain(row.item) ? row.item : plain(row.resource) ? row.resource : null;
          const kind = row.item_type || row.type || item?.type;
          const itemId = row.item_id || row.conversation_id || (kind === "conversation" ? row.id : null) || item?.id;
          if (kind === "conversation" || (kind === undefined && itemId === conversationId)) {
            if (typeof itemId !== "string" || !uuid.test(itemId)) throw fail("verification_failed");
            if (itemId === conversationId) found = true;
          } else if (typeof kind !== "string" || !kind.trim() || kind.length > 64) {
            throw fail("verification_failed");
          }
        }
        const partial = plain(value) && (value.has_more === true ||
          (typeof value.next_cursor === "string" && value.next_cursor.length > 0) ||
          (typeof value.next === "string" && value.next.length > 0));
        if (!desiredState && !found && partial) throw fail("verification_failed");
        return found;
      };
      const readPinState = async () => {
        const value = await request("/backend-api/pins?item_type=conversation", { headers });
        return value;
      };
      const before = action === "archive" ? await readConversation() : null;
      const beforeState = action === "archive" ? before.is_archived : isPinned(await readPinState(), desired);
      if (typeof beforeState === "boolean" && beforeState === desired) {
        finish("applied", { verified: true, conversation_id: conversationId, [action === "archive" ? "archived" : "pinned"]: desired });
        return;
      }
      if (action === "archive") {
        await request(detailPath, { method: "PATCH", headers: { ...headers, "Content-Type": "application/json" }, body: JSON.stringify({ is_archived: desired }) }, false);
        const after = await readConversation();
        let verified = typeof after.is_archived === "boolean" && after.is_archived === desired;
        if (typeof after.is_archived !== "boolean") {
          const params = new URLSearchParams({ offset: "0", limit: "100", order: "updated", is_archived: String(desired), hide_snorlax: "false" });
          const listed = await request(`/backend-api/conversations?${params.toString()}`, { headers });
          verified = plain(listed) && Array.isArray(listed.items) && listed.items.some(item => plain(item) && item.id === conversationId && (typeof item.is_archived !== "boolean" || item.is_archived === desired));
        }
        if (!verified) throw fail("verification_failed");
      } else {
        const pinPath = `/backend-api/pins/conversation/${encodeURIComponent(conversationId)}`;
        await request(pinPath, { method: desired ? "POST" : "DELETE", headers: desired ? { ...headers, "Content-Type": "application/json" } : headers, ...(desired ? { body: "{}" } : {}) }, false);
        if (isPinned(await readPinState(), desired) !== desired) throw fail("verification_failed");
      }
      finish("applied", { verified: true, conversation_id: conversationId, [action === "archive" ? "archived" : "pinned"]: desired });
    } catch (error) {
      const code = error && error.safe;
      const allowed = new Set(["cancelled", "invalid_request", "signed_out", "account_unavailable", "account_changed", "conversation_unavailable", "request_failed", "rate_limited", "verification_failed"]);
      finish("failed", { error_code: allowed.has(code) ? code : "request_failed", ...(Number.isSafeInteger(error?.retry_after) ? { retry_after: error.retry_after } : {}) });
    } finally { active = null; }
  };
  state.cancel = () => { state.cancelled = true; if (active) active.abort(); };
  run();
}
