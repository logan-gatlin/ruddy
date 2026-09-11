// Native extern conversion snapshots arrays, records, and sums at the boundary.
const $webPair = (a, b) => $record([["0", a], ["1", b]]);
const $webMessage = error => String(error && error.message || error);
const $urlSnapshot = url => $record(["href", "origin", "protocol", "username", "password", "host", "hostname", "port", "pathname", "search", "hash"].map(key => [key, url[key]]));
const $urlCall = (input, make) => {
  try { return $sum("Some", $urlSnapshot(make())); }
  catch (error) { return $sum("Error", $record([["input", input], ["message", $webMessage(error)]])); }
};
const $url = {
  parse: input => $urlCall(input, () => new URL(input)),
  resolve: request => $urlCall(request.input, () => new URL(request.input, request.base)),
  parseQuery: text => Array.from(new URLSearchParams(text), ([key, value]) => $webPair(key, value)),
  stringifyQuery: pairs => new URLSearchParams(pairs.map(pair => [pair["0"], pair["1"]])).toString(),
  withQuery: request => $urlCall(request.href, () => { const url = new URL(request.href); url.search = request.search; return url; })
};
const $httpFailure = (kind, message) => Object.assign(new Error(message), { httpKind: kind });
const $httpError = (kind, url, error) => $sum("Error", $record([
  ["kind", $sum(kind, undefined)], ["url", url], ["message", $webMessage(error)]
]));
const $http = {
  text: bytes => new TextDecoder().decode(Uint8Array.from(bytes)),
  request: async request => {
    let timer, reader;
    const controller = new AbortController();
    try {
      if (typeof fetch !== "function") throw $httpFailure("Unsupported", "Fetch is unavailable");
      let input;
      try {
        const url = new URL(request.url);
        if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("Expected an HTTP or HTTPS URL");
        if (!Number.isSafeInteger(request.timeout_ms) || request.timeout_ms < 1 || request.timeout_ms > 2147483647) throw new Error("timeout_ms must be between 1 and 2147483647");
        if (!Number.isSafeInteger(request.max_bytes) || request.max_bytes < 0) throw new Error("max_bytes must be a safe natural number");
        const body = request.body.tag === "Empty" ? undefined : request.body.tag === "Text" ? request.body.value : Uint8Array.from(request.body.value);
        input = new Request(url, {
          method: request.method,
          headers: request.headers.map(pair => [pair["0"], pair["1"]]),
          body,
          redirect: request.redirect.tag.toLowerCase(),
          signal: controller.signal
        });
      } catch (error) { throw $httpFailure("InvalidRequest", $webMessage(error)); }
      timer = setTimeout(() => controller.abort(), request.timeout_ms);
      const response = await fetch(input);
      const chunks = [];
      let length = 0;
      if (response.body) {
        reader = response.body.getReader();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (value.length > request.max_bytes - length) throw $httpFailure("TooLarge", "Response exceeds max_bytes");
          chunks.push(value);
          length += value.length;
        }
      }
      const bytes = new Uint8Array(length);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      const headers = [];
      for (const [key, value] of response.headers) {
        if (key !== "set-cookie" || typeof response.headers.getSetCookie !== "function") headers.push($webPair(key, value));
      }
      if (typeof response.headers.getSetCookie === "function") for (const value of response.headers.getSetCookie()) headers.push($webPair("set-cookie", value));
      return $sum("Some", $record([
        ["status", response.status], ["status_text", response.statusText], ["headers", headers],
        ["body", Array.from(bytes)], ["url", response.url], ["redirected", response.redirected]
      ]));
    } catch (error) {
      const kind = error.httpKind || (controller.signal.aborted ? "Timeout" : "Network");
      controller.abort();
      if (reader) { try { await reader.cancel(); } catch (_) {} }
      return $httpError(kind, request.url, error);
    } finally {
      clearTimeout(timer);
      if (reader) reader.releaseLock();
    }
  }
};
