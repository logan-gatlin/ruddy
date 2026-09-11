// Native extern conversion snapshots arrays, records, and sums at the boundary.
const $webPair = (a, b) => $record([["0", a], ["1", b]]);
// A host string may hold an unpaired surrogate half, which is no Unicode
// scalar value and so is not Ruddy text. Where text is taken in strictly that
// is a refusal; where it is taken in lossily, and in a message that has to
// exist either way, each unpaired half becomes the replacement character.
const $webScalars = value => {
  let out = "";
  for (let at = 0; at < value.length; at++) {
    const code = value.charCodeAt(at);
    if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(at + 1);
      if (low >= 0xdc00 && low <= 0xdfff) { out += value[at] + value[at + 1]; at += 1; continue; }
      out += "\ufffd";
      continue;
    }
    if (code >= 0xdc00 && code <= 0xdfff) { out += "\ufffd"; continue; }
    out += value[at];
  }
  return out;
};
const $webValid = value => {
  for (let at = 0; at < value.length; at++) {
    const code = value.charCodeAt(at);
    if (code >= 0xdc00 && code <= 0xdfff) return false;
    if (code >= 0xd800 && code <= 0xdbff) {
      const low = value.charCodeAt(at + 1);
      if (!(low >= 0xdc00 && low <= 0xdfff)) return false;
      at += 1;
    }
  }
  return true;
};
// Reading a thrown value to describe it observes it, and that observation can
// throw again. A value that cannot be read is the constant below rather than
// a second failure escaping the boundary that promised a recoverable one.
const $webMessage = error => {
  try {
    const written = (error !== null && error !== undefined && error.message) || error;
    return $webScalars(typeof written === "string" ? written : String(written));
  } catch (unreadable) { return "the host threw a value that cannot be read"; }
};
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
// Host observation. Reading a property can run a getter or a proxy trap, so
// each of these is an effect operation whose failure is data: a host throw
// becomes `#Error { path, expected, message }` carrying the host's own words,
// and nothing thrown here crosses back into Ruddy as an exception.
const $jsFailure = (path, expected, error) => $sum("Error", $record([
  ["path", path], ["expected", expected], ["message", $webMessage(error)]
]));
const $jsObserve = (path, expected, observe) => {
  try { return $sum("Some", observe()); }
  catch (error) { return $jsFailure(path, expected, error); }
};
const $jsPresent = (value, what) => {
  if (value === null || value === undefined) throw new TypeError("Cannot read " + what + " of " + String(value));
  return value;
};
// The copies this program has taken. Membership is the authority: a snapshot
// is data nobody else holds, so reading one runs no host code, and nothing
// outside these operations can put a value in here.
const $snapshots = new WeakSet();
// An active path, not a set of everything seen: a repeated sibling is copied,
// and only a cycle is refused, which is what conversion does as well.
const $jsSnapshot = (value, path, active) => {
  if (value === null) return null;
  const kind = typeof value;
  if (kind === "undefined" || kind === "boolean" || kind === "number" || kind === "string") return value;
  if (kind !== "object") throw new TypeError("A " + kind + " at " + path + " is not inert data");
  if (active.has(value)) throw new TypeError("The value at " + path + " refers to itself");
  active.add(value);
  let copy;
  if (Array.isArray(value)) {
    copy = [];
    for (let index = 0; index < value.length; index++) copy.push($jsSnapshot(value[index], path + "[" + index + "]", active));
  } else {
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) throw new TypeError("The value at " + path + " is a host object, not plain data");
    copy = Object.create(null);
    for (const key of Object.keys(value)) copy[key] = $jsSnapshot(value[key], path + "." + key, active);
  }
  active.delete(value);
  $snapshots.add(copy);
  return copy;
};
// `Array.isArray` runs on the host and can refuse — a revoked proxy answers
// `typeof` but nothing else — so a value that cannot report what it is is
// `#Other`, which is what this operation promises instead of failing.
const $jsKind = value => {
  if (value === null) return "Null";
  const kind = typeof value;
  if (kind === "undefined") return "Undefined";
  if (kind === "boolean") return "Bool";
  if (kind === "number") return "Number";
  if (kind === "string") return "String";
  if (kind === "function") return "Function";
  if (kind !== "object") return "Other";
  try { return Array.isArray(value) ? "Array" : "Object"; }
  catch (error) { return "Other"; }
};
const $js = {
  kind: value => $sum($jsKind(value), undefined),
  field: request => $jsObserve("$." + request.name, "a readable property", () =>
    $jsPresent(request.of, request.name)[request.name]),
  element: request => $jsObserve("$[" + request.at + "]", "a readable element", () =>
    $jsPresent(request.of, "element " + request.at)[request.at]),
  length: value => $jsObserve("$.length", "a finite non-negative integer length", () => {
    const length = $jsPresent(value, "length").length;
    if (!Number.isInteger(length) || length < 0 || length > $domains.nat.high) throw new TypeError("Its length is not a natural number this target can hold");
    return length;
  }),
  keys: value => $jsObserve("$", "an object", () => {
    if (value === null || (typeof value !== "object" && typeof value !== "function")) throw new TypeError("A " + (value === null ? "null" : typeof value) + " has no keys");
    return Object.keys(value);
  }),
  snapshot: value => $jsObserve("$", "inert data", () => $jsSnapshot(value, "$", new Set())),
  // Whether reading this value runs no host code: a primitive, or a copy this
  // program took. A live host object is not one, whatever its shape.
  inert: value => value === null
    || (typeof value !== "object" && typeof value !== "function")
    || $snapshots.has(value),
  // The explicit lossy reading. Strict conversion refuses a host string that
  // is not scalars; this one says what to do about it instead.
  text: value => $jsObserve("$", "a string", () => {
    if (typeof value !== "string") throw new TypeError("This value is not a string");
    return $webScalars(value);
  }),
  apply: request => $jsObserve("$", "a callable value", () => {
    if (typeof request.of !== "function") throw new TypeError("This value is not callable");
    return request.of(...request.arguments);
  })
};
