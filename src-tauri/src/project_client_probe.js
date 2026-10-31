function (key, nonce) {
  const exactOrigin = "https://chatgpt.com";
  if (location.origin !== exactOrigin) return false;
  window[key] = { nonce, state: "running" };

  const allowed = (raw, base = location.href) => {
    try {
      const url = new URL(raw, base);
      if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) return null;
      if (url.hostname !== "chatgpt.com" && url.hostname !== "cdn.oaistatic.com") return null;
      if (!/\.(?:js|mjs)$/.test(url.pathname) || url.pathname.length > 2048) return null;
      return url.origin + url.pathname;
    } catch (_) {
      return null;
    }
  };

  const scriptUrls = new Set();
  for (const script of Array.from(document.scripts).slice(0, 512)) {
    if (script.src) {
      const url = allowed(script.src);
      if (url) scriptUrls.add(url);
    }
  }
  for (const link of Array.from(document.querySelectorAll('link[rel="modulepreload"],link[rel="preload"][as="script"]')).slice(0, 512)) {
    if (link.href) {
      const url = allowed(link.href);
      if (url) scriptUrls.add(url);
    }
  }
  for (const entry of performance.getEntriesByType("resource").slice(-2000)) {
    if (entry.initiatorType === "script") {
      const url = allowed(entry.name);
      if (url) scriptUrls.add(url);
    }
  }

  const routePattern = /["'`]((?:https:\/\/chatgpt\.com)?\/backend-api\/[A-Za-z0-9_/${}.-]{1,180}|\/(?:gizmos?|projects?|conversation|conversations)(?:\/[A-Za-z0-9_/${}.-]{0,180})?)["'`]/g;
  const interesting = /gizmo|project|rename|update/i;
  let contextBudget = 12 * 1024;
  const scrub = (value, preserveRoute) => value
    .replace(/(?:Bearer\s+)[A-Za-z0-9._~-]+/gi, "Bearer [redacted]")
    .replace(/(["'`])((?:\\.|.)*?)\1/g, (whole, quote, content) => {
      if (content === preserveRoute || /^(?:PATCH|PUT|POST|GET|DELETE)$/i.test(content)) return whole;
      return "[string]";
    })
    .replace(/[A-Za-z0-9_-]{80,}/g, "[redacted]")
    .replace(/[\r\n\t]+/g, " ")
    .slice(0, 1200);

  (async () => {
    const findings = [];
    let bytes = 0;
    let fetched = 0;
    let interestingTokens = 0;
    const deadline = Date.now() + 24_000;
    const queue = Array.from(scriptUrls);
    const queued = new Set(queue);
    while (queue.length && fetched < 64) {
      const url = queue.shift();
      if (location.origin !== exactOrigin || bytes >= 16 * 1024 * 1024 || Date.now() >= deadline) break;
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), Math.min(5000, deadline - Date.now()));
      try {
        const response = await fetch(url, {
          method: "GET", credentials: "omit", cache: "force-cache", redirect: "error",
          signal: controller.signal,
        });
        if (!response.ok || !/^application\/(?:javascript|x-javascript|ecmascript)|text\/javascript/i.test(response.headers.get("content-type") || "")) continue;
        const declared = Number(response.headers.get("content-length") || 0);
        if (declared > 2 * 1024 * 1024) continue;
        const reader = response.body && response.body.getReader();
        if (!reader) continue;
        const chunks = [];
        let size = 0;
        while (size <= 2 * 1024 * 1024 && bytes + size <= 16 * 1024 * 1024) {
          const part = await reader.read();
          if (part.done) break;
          size += part.value.byteLength;
          if (size <= 2 * 1024 * 1024) chunks.push(part.value);
        }
        if (size > 2 * 1024 * 1024 || bytes + size > 16 * 1024 * 1024) {
          await reader.cancel();
          continue;
        }
        bytes += size;
        const source = await new Blob(chunks).text();
        fetched += 1;
        const tokenMatches = source.match(/gizmo_id|gizmoId|project_id|projectId|display_name|displayName|renameProject|updateProject|rename_project|update_project/gi) || [];
        interestingTokens += tokenMatches.length;
        for (const importMatch of source.matchAll(/["'`]([^"'`]{1,240}\.(?:js|mjs))(?:\?[^"'`]*)?["'`]/g)) {
          const imported = allowed(importMatch[1], url);
          if (imported && !queued.has(imported) && queued.size < 512) {
            queued.add(imported);
            queue.push(imported);
          }
        }
        const routes = [];
        for (const match of source.matchAll(routePattern)) {
          const route = match[1];
          const at = match.index || 0;
          const start = Math.max(0, at - 600);
          const end = Math.min(source.length, at + match[0].length + 600);
          const near = source.slice(start, end);
          if (!interesting.test(route) && !interesting.test(near)) continue;
          const methods = Array.from(new Set([
            ...Array.from(near.matchAll(/\bmethod\s*:\s*["'](PATCH|PUT|POST|GET|DELETE)["']/gi), m => m[1].toUpperCase()),
            ...Array.from(near.matchAll(/\.(patch|put|post|get|delete)\s*\(/gi), m => m[1].toUpperCase()),
          ])).slice(0, 5);
          const bodyFields = Array.from(new Set(Array.from(near.matchAll(/(?:["']?)(gizmo_id|project_id|projectId|display|display_name|displayName|name|title)(?:["']?)\s*:/g), m => m[1]))).slice(0, 12);
          const cleanContext = scrub(near, route);
          if (contextBudget > 0) {
            const snippet = cleanContext.slice(0, Math.min(cleanContext.length, contextBudget));
            contextBudget -= snippet.length;
            routes.push({ path: scrub(route, route), base_uncertain: !route.startsWith('/backend-api/'), methods, body_fields: bodyFields, context: snippet });
          } else {
            routes.push({ path: scrub(route, route), base_uncertain: !route.startsWith('/backend-api/'), methods, body_fields: bodyFields });
          }
          if (routes.length >= 30) break;
        }
        if (routes.length) {
          findings.push({ url: new URL(url).pathname, routes });
        }
      } catch (_) {
        // This probe reports only findings from files that were fetched and parsed successfully.
      } finally {
        clearTimeout(timer);
      }
    }
    if (window[key] && window[key].nonce === nonce) {
      window[key] = { nonce, state: "complete", module_count: queued.size, scanned_path_count: fetched, fetched_count: fetched, interesting_token_count: interestingTokens, bytes, findings };
    }
  })();
  return true;
}
