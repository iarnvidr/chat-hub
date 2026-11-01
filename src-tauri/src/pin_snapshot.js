function (key, nonce, expectedScope) {
  const origin = "https://chatgpt.com";
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const state = { nonce, state: "running", cancelled: false };
  window[key] = state;
  let active = null;
  const fail = code => Object.assign(new Error(code), { safe: code });
  const plain = value => value && typeof value === "object" && !Array.isArray(value);
  const finish = (name, fields = {}) => {
    if (window[key] === state) Object.assign(state, { state: name }, fields);
  };
  const run = async () => {
    if (location.origin !== origin || typeof expectedScope !== "string" ||
        !/^chatgpt:observed:[a-f0-9]{64}$/.test(expectedScope)) {
      finish("failed", { error_code: "invalid_request" }); return;
    }
    const request = async (path, options = {}) => {
      if (state.cancelled) throw fail("cancelled");
      const url = new URL(path, origin);
      if (url.origin !== origin) throw fail("invalid_request");
      active = new AbortController();
      const timeout = setTimeout(() => active?.abort(), 20000);
      let response;
      try {
        response = await fetch(url.href, {
          ...options, credentials: "include", cache: "no-store", redirect: "error", signal: active.signal
        });
        if (response.status === 429) {
          const raw = response.headers.get("retry-after");
          let retry = 60;
          if (typeof raw === "string" && /^\d+$/.test(raw.trim())) {
            const parsed = Number(raw.trim());
            retry = Number.isSafeInteger(parsed) ? parsed : 365 * 24 * 60 * 60 + 1;
          } else if (typeof raw === "string" && raw.trim()) {
            const timestamp = Date.parse(raw);
            if (Number.isFinite(timestamp)) retry = Math.max(1, Math.ceil((timestamp - Date.now()) / 1000));
          }
          throw Object.assign(fail("rate_limited"), { retry_after: Math.min(365 * 24 * 60 * 60 + 1, retry) });
        }
        if (response.status === 401 || response.status === 403) throw fail("signed_out");
        if (!response.ok) throw fail("request_failed");
        if (!/\bjson\b/i.test(response.headers.get("content-type") || "")) throw fail("invalid_pin_list");
        if (!response.body || !response.body.getReader) throw fail("invalid_pin_list");
        const reader = response.body.getReader();
        const chunks = [];
        let total = 0;
        try {
          while (true) {
            const part = await reader.read();
            if (part.done) break;
            total += part.value.byteLength;
            if (total > 4 * 1024 * 1024) throw fail("pin_list_too_large");
            chunks.push(part.value);
          }
        } finally { try { reader.releaseLock(); } catch (_) {} }
        const bytes = new Uint8Array(total);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        try { return JSON.parse(new TextDecoder().decode(bytes)); }
        catch (_) { throw fail("invalid_pin_list"); }
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
      const actualScope = "chatgpt:observed:" + Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
      if (actualScope !== expectedScope) throw fail("account_changed");
      const headers = { Authorization: `Bearer ${token}`, "ChatGPT-Account-Id": account };
      const payload = await request("/backend-api/pins?item_type=conversation&limit=1000", { headers });
      let rows;
      if (Array.isArray(payload)) rows = payload;
      else if (plain(payload)) {
        for (const key of ["total", "total_count", "count"]) {
          if (Object.prototype.hasOwnProperty.call(payload, key) &&
              (!Number.isSafeInteger(payload[key]) || payload[key] < 0)) throw fail("incomplete_pin_list");
        }
        if (payload.has_more === true ||
            (payload.next_cursor != null && payload.next_cursor !== "") ||
            (payload.next != null && payload.next !== "")) throw fail("incomplete_pin_list");
        const shapes = [payload.items, payload.pins, payload.data].filter(Array.isArray);
        if (shapes.length !== 1) throw fail("invalid_pin_list");
        rows = shapes[0];
        for (const key of ["total", "total_count", "count"]) {
          if (Object.prototype.hasOwnProperty.call(payload, key) && payload[key] !== rows.length) throw fail("incomplete_pin_list");
        }
      } else throw fail("invalid_pin_list");
      if (rows.length >= 1000) throw fail("incomplete_pin_list");
      const pinned = new Set();
      for (const row of rows) {
        if (!plain(row)) throw fail("invalid_pin_list");
        const item = plain(row.item) ? row.item : plain(row.resource) ? row.resource : null;
        const kind = row.item_type || row.type || (item && item.type);
        const itemId = row.item_id || row.conversation_id || (kind === "conversation" ? row.id : null) || (item && item.id);
        if (kind === "conversation" || (kind == null && typeof row.conversation_id === "string")) {
          if (typeof itemId !== "string" || !uuid.test(itemId) || pinned.has(itemId.toLowerCase())) throw fail("invalid_pin_list");
          pinned.add(itemId.toLowerCase());
        } else {
          throw fail("invalid_pin_list");
        }
      }
      if (state.cancelled) throw fail("cancelled");
      finish("complete", {
        complete: true,
        account_scope: expectedScope,
        pinned_ids: Array.from(pinned).sort()
      });
    } catch (error) {
      finish("failed", {
        error_code: error && typeof error.safe === "string" ? error.safe : "request_failed",
        ...(error && Number.isSafeInteger(error.retry_after) ? { retry_after: error.retry_after } : {})
      });
    } finally {
      active?.abort();
    }
  };
  void run();
}
