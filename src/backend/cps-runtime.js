// Ruddy calls and continuations are data. Only this driver executes transfers.
const $closureMark = Symbol("Ruddy closure");
const $abortMark = Symbol("Ruddy abort");
const $wrapped = new WeakMap();
const $exported = new WeakMap();
const $hostFunctions = new WeakSet();
let $active = null;
const $closure = (f, a) => {
  const value = (...args) => $invoke(value, $f[f].suspends ? "Promise" : "Sync", args);
  value[$closureMark] = true; value.f = f; value.a = a;
  return value;
};
const $continuation = (f, b, a, h) => ({ f, b, a, h });
const $step = (f, b, a, h) => ({ kind: "step", f, b, a, h });
const $call = (c, a, k, h) => ({ kind: "call", c, a, k, h });
const $raw = (c, a, k, h, completion) => ({ kind: "raw", c, a, k, h, completion });
const $resume = (k, value) => ({ kind: "resume", k, value });
const $enter = (tag, k, next) => ({ kind: "enter", tag, k, next });
const $leave = (tag, value, h) => ({ kind: "leave", tag, value, h });
const $inside = (h, tag) => { for (; h; h = h.parent) if (h === tag) return true; return false; };
const $fromHost = value => typeof value === "function" && $wrapped.has(value) ? $wrapped.get(value) : value;
const $expire = (h, stop) => {
  for (; h && h !== stop; h = h.parent) {
    h.active = false;
    h.k = null;
    h.owner = null;
  }
};
function $finish(state, failed, value) {
  if (state.done) return;
  $expire(state.h, state.base);
  state.done = true; state.pending = false; state.failed = failed;
  if (failed) state.error = value; else state.value = value;
  state.next = null; state.h = state.base;
  if (state.settle) state.settle();
}
function $promise(state) {
  if (!state.promise) state.promise = new Promise((resolve, reject) => {
    state.settle = () => state.failed ? reject(state.error) : resolve($export(state.value));
    if (state.done) state.settle();
  });
  return state.promise;
}
// Each foreign request has one settlement. A completion during registration is
// consumed by the running driver; a later completion schedules a fresh turn.
function $foreign(state, s) {
  const args = s.a.map(value => value && value[$closureMark] ? $callback(value, "Sync") : value);
  if (s.completion === "Immediate") {
    state.next = $resume(s.k, $fromHost(s.c(...args)));
    return;
  }
  const token = { settled: false, registering: true };
  const apply = () => {
    if (state.done) return;
    state.pending = false;
    if (token.failed) $finish(state, true, token.value);
    else state.next = $resume(s.k, $fromHost(token.value));
  };
  const deliver = (failed, value) => {
    if (token.settled || state.done) return;
    token.settled = true; token.failed = failed; token.value = value;
    if (!token.registering) queueMicrotask(() => { apply(); if (!state.done) $drive(state); });
  };
  try {
    if (s.completion === "Promise") {
      Promise.resolve(s.c(...args)).then(value => deliver(false, value), error => deliver(true, error));
    } else if (s.completion === "Callback") {
      s.c(...args, value => deliver(false, value), error => deliver(true, error));
    } else throw new Error("invalid foreign completion protocol");
  } catch (error) {
    if (error && error[$abortMark]) throw error;
    deliver(true, error);
  }
  token.registering = false;
  if (token.settled) apply();
  else if (!state.allowSuspend) $finish(state, true, new Error("synchronous callback attempted to suspend"));
  else state.pending = true;
}
function $drive(state) {
  const previous = $active;
  $active = state;
  try {
    while (!state.done && !state.pending) {
      const s = state.next;
      if (s.h !== undefined) state.h = s.h;
      switch (s.kind) {
        case "step": state.next = $f[s.f].blocks[s.b](s.h, ...s.a); break;
        case "call": {
          const c = $fromHost(s.c);
          if (!c || !c[$closureMark]) throw new TypeError("not a Ruddy function");
          let args = s.a;
          const count = $f[c.f].arity - c.a.length;
          // A source closure supplied through JS can implement a narrower
          // evidence convention. Its visible unary argument remains last.
          if ($hostFunctions.has(c) && args.length !== count) {
            args = count ? args.slice(-count) : [];
            while (args.length < count) args.unshift($record([]));
          }
          state.next = $f[c.f].entry(s.h, ...c.a, ...args, s.k);
          break;
        }
        case "raw": {
          try { $foreign(state, s); }
          catch (error) {
            if (!error || !error[$abortMark]) throw error;
            state.next = $leave(error.tag, error.value, state.h);
          }
          break;
        }
        case "resume":
          if (s.k.root) $finish(state, false, s.value);
          else state.next = $step(s.k.f, s.k.b, [...s.k.a, s.value], s.k.h);
          break;
        case "enter":
          s.tag.active = true; s.tag.parent = s.next.h; s.tag.k = s.k;
          s.tag.owner = state;
          state.next = { ...s.next, h: s.tag };
          break;
        case "leave": {
          if (!s.tag.active || !$inside(s.h, s.tag)) throw new Error("invalid handler exit");
          if (s.tag.owner !== state) throw { [$abortMark]: true, tag: s.tag, value: s.value };
          const k = s.tag.k;
          $expire(s.h, s.tag.parent);
          state.h = s.tag.parent;
          state.next = $resume(k, s.value);
          break;
        }
        default: throw new Error("invalid Ruddy transfer");
      }
    }
  } catch (error) { $finish(state, true, error); }
  finally { $active = previous; }
  return state;
}
function $start(c, args, parent, allowSuspend = true, foreign = false) {
  const state = { done: false, pending: false, failed: false, base: parent, h: parent, allowSuspend };
  c = $fromHost(c);
  const count = $f[c.f].arity - c.a.length;
  if (foreign) {
    args = args.slice(0, count);
    while (args.length < count) args.push(undefined);
  } else if (count) {
    // Source closures are curried: captured values precede evidence, then
    // their one visible argument. JS supplies no domain handler evidence.
    args = [...Array(count - 1).fill(null).map(() => $record([])), args[0]];
  } else args = [];
  args = args.map(value => {
    value = $fromHost(value);
    if (value && value[$closureMark]) $hostFunctions.add(value);
    return value;
  });
  state.next = $call(c, args, { root: true }, parent);
  return $drive(state);
}
function $report(error) {
  const configuration = globalThis[Symbol.for("ruddy.runtime")];
  if (configuration && typeof configuration.onUnhandledError === "function") {
    try { configuration.onUnhandledError(error); }
    catch (failure) { queueMicrotask(() => { throw failure; }); }
  }
  else queueMicrotask(() => { throw error; });
}
function $callback(value, mode, foreign = true) {
  value = $fromHost(value);
  let modes = $exported.get(value);
  if (!modes) { modes = new Map(); $exported.set(value, modes); }
  const key = mode + (foreign ? ":foreign" : ":source");
  if (modes.has(key)) return modes.get(key);
  const callback = (...args) => $invoke(value, mode, args, foreign);
  $wrapped.set(callback, value); modes.set(key, callback);
  return callback;
}
function $invoke(value, mode, args, foreign = false) {
    if (mode === "Sync") {
      const state = $start(value, args, $active ? $active.h : null, false, foreign);
      if (state.failed) throw state.error;
      return $export(state.value);
    }
    let success, failure;
    if (mode === "Completion") {
      failure = args.pop(); success = args.pop();
      if (typeof success !== "function" || typeof failure !== "function") throw new TypeError("completion callback requires success and failure functions");
    }
    const state = $start(value, args, null, true, foreign);
    if (mode === "Promise") return $promise(state);
    const done = () => {
      if (mode === "Completion") { if (state.failed) failure(state.error); else success($export(state.value)); }
      else if (state.failed) $report(state.error);
    };
    if (state.done) done(); else $promise(state).then(done, done).catch($report);
    return undefined;
}
function $export(value) { return value; }
function $initialize(initializers) {
  let index = 0;
  function advance() {
    while (index < initializers.length) {
      const [name, f] = initializers[index++];
      const state = $start($closure(f, []), [], null);
      if (state.failed) throw state.error;
      if (state.pending) return $promise(state).then(() => { $g[name] = state.value; return advance(); });
      $g[name] = state.value;
    }
    return null;
  }
  return advance();
}
