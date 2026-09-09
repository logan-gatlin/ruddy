// Ruddy calls and continuations are data. Only this driver executes transfers.
const $closureMark = Symbol("Ruddy closure");
const $abortMark = Symbol("Ruddy abort");
const $wrapped = new WeakMap();
const $exported = new WeakMap();
const $hostFunctions = new WeakSet();
const $promiseCallbacks = new WeakSet();
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
// Internal waiters observe terminal state without assimilating result data.
function $settled(state) {
  if (!state.settled) state.settled = new Promise(resolve => {
    state.settle = resolve;
    if (state.done) resolve();
  });
  return state.settled;
}
function $promise(state) {
  if (!state.promise) state.promise = $settled(state).then(() => {
    if (state.failed) throw state.error;
    return $export(state.value);
  });
  return state.promise;
}
// Each foreign request has one settlement. A synchronous throw is consumed by
// the running driver; Promise settlement schedules a fresh turn.
function $foreign(state, s) {
  const args = s.a.map(value => value && value[$closureMark] ? $callback(value, "Sync") : value);
  if (s.completion === "Immediate") {
    state.next = $resume(s.k, s.c(...args));
    return;
  }
  const token = { settled: false, calling: true };
  const apply = () => {
    if (state.done) return;
    state.pending = false;
    if (token.failed) $finish(state, true, token.value);
    else state.next = $resume(s.k, token.value);
  };
  const deliver = (failed, value) => {
    if (token.settled || state.done) return;
    token.settled = true; token.failed = failed; token.value = value;
    if (!token.calling) queueMicrotask(() => { apply(); if (!state.done) $drive(state); });
  };
  try {
    if (s.completion === "Promise") {
      Promise.resolve(s.c(...args)).then(value => deliver(false, value), error => deliver(true, error));
    } else throw new Error("invalid foreign completion protocol");
  } catch (error) {
    if (error && error[$abortMark]) throw error;
    deliver(true, error);
  }
  token.calling = false;
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
          if (c.nativeType) {
            let n = c.nativeType;
            if (n.slots && n.slots.length) {
              const supplied = n.descriptor.arguments.slice();
              n.slots.forEach((slot, index) => {
                if (s.a[index] === undefined && supplied[slot] === undefined) throw new TypeError("missing runtime type information for native invocation");
                if (s.a[index] !== undefined) supplied[slot] = s.a[index];
              });
              n = { ...n, descriptor: $nativePlan(n.descriptor.template, supplied) };
            }
            const argument = $convertType(n.descriptor, s.a[s.a.length - 1], true, n.from);
            state.next = $raw(n.function, [argument], { conversion: n, parent: s.k }, s.h, "Immediate");
            break;
          }
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
          if (s.k.conversion) {
            const n = s.k.conversion;
            state.next = $resume(s.k.parent, $convertType(n.descriptor, s.value, false, n.to));
          } else if (s.k.root) $finish(state, false, s.value);
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
  const count = c.nativeType ? 1 : $f[c.f].arity - c.a.length;
  if (foreign) {
    args = args.slice(0, count);
    while (args.length < count) args.push(undefined);
  } else if (count) {
    // Source closures are curried: captured values precede evidence, then
    // their one visible argument. JS supplies no domain handler evidence.
    args = [...Array(count - 1).fill(null).map(() => $record([])), args[0]];
  } else args = [];
  args = args.map(value => {
    if (value && value[$closureMark]) $hostFunctions.add(value);
    return value;
  });
  state.next = $call(c, args, { root: true }, parent);
  return $drive(state);
}
function $callback(value, mode, foreign = true) {
  value = $fromHost(value);
  let modes = $exported.get(value);
  if (!modes) { modes = new Map(); $exported.set(value, modes); }
  const key = mode + (foreign ? ":foreign" : ":source");
  if (modes.has(key)) return modes.get(key);
  const callback = (...args) => $invoke(value, mode, args, foreign);
  if (mode === "Promise") $promiseCallbacks.add(callback);
  $wrapped.set(callback, value); modes.set(key, callback);
  return callback;
}
function $invoke(value, mode, args, foreign = false) {
    if (mode === "Sync") {
      const state = $start(value, args, $active ? $active.h : null, false, foreign);
      if (state.failed) throw state.error;
      return $export(state.value);
    }
    if (mode === "Promise") return $promise($start(value, args, null, true, foreign));
    throw new Error("invalid callback protocol");
}
function $export(value) { return value; }
function $initialize(initializers) {
  let index = 0;
  function advance() {
    while (index < initializers.length) {
      const [name, f] = initializers[index++];
      const state = $start($closure(f, []), [], null);
      if (state.failed) throw state.error;
      if (state.pending) return $settled(state).then(() => {
        if (state.failed) throw state.error;
        $g[name] = state.value; return advance();
      });
      $g[name] = state.value;
    }
    return null;
  }
  return advance();
}
