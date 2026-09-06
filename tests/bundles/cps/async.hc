@async
extern delayed : Real -> Real = "globalThis.host.delayed"
effect Ask = { get: Real -> Real }
let read = fn n => handle (1.0 + !Ask.get n) with
  | !Ask.get value => delayed value
end

extern remember : fn(@async fn(Real) -> Real) -> () = "globalThis.host.remember"
let save = fn offset => remember (fn n => delayed (n + offset))

let abort = fn n => handle !Ask.get n with
  | !Ask.get value => do let result = delayed value return raise result end
  | return value => value + 100.0
end
let normal = fn n => handle !Ask.get n with
  | !Ask.get value => delayed value
  | return value => value + 100.0
end

@async
extern completed : Real -> Real = "n => new Promise((resolve, reject) => globalThis.host.completed(n, resolve, reject))"
let via_wrapped_callback = fn n => 1.0 + completed n

effect Exit = { stop: Real -> Real }
extern keep_exit : fn(@async fn(Real) -> Real + !Exit) -> () + !Exit = "globalThis.host.keepExit"
extern run_exit : fn(fn(Real) -> Real + !Exit) -> Real + !Exit = "globalThis.host.runExit"
let expired = fn _ => handle do
  let _ = keep_exit (fn n => !Exit.stop n)
  return 99.0
end with | !Exit.stop n => raise n end
let live = fn _ => handle do
  let _ = keep_exit (fn n => !Exit.stop n)
  return delayed 1.0
end with | !Exit.stop n => raise n end
let inline_exit = fn _ => handle run_exit (fn n => !Exit.stop n) with
  | !Exit.stop n => raise n
  | return n => n + 100.0
end

effect Reader = { get: Real -> Real }
extern keep_reader : fn(@async fn(Real) -> Real + !Reader) -> () + !Reader = "globalThis.host.keepReader"
let ordinary_evidence = fn offset => handle keep_reader (fn n => !Reader.get n) with
  | !Reader.get n => n + offset
end

let record = { run: fn n => delayed n }

let fresh = fn offset => remember (fn n => handle !Ask.get (n + offset) with
  | !Ask.get value => do let result = delayed value return raise result end
  | return value => value + 100.0
end)

extern keep_factory : fn(fn(Real) -> @async fn(Real) -> Real) -> () = "globalThis.host.keepFactory"
let factory = fn offset => keep_factory (fn n => fn m => delayed (offset + n + m))

extern keep_sync : fn(fn(Real) -> Real) -> () = "globalThis.host.keepSync"
extern observed : Real -> Real = "globalThis.host.observed"
let unsafe_sync = fn _ => keep_sync (fn n => observed (delayed n))

extern failure : Real -> Real = "n => { throw new Error('immediate failure'); }"
let save_failure = fn _ => remember (fn n => failure n)

@async
extern translated : Real -> Real = "n => Promise.reject(new Error('translated')).catch(() => n + 10)"
let translation = fn n => translated n

extern data : () -> { "then": Real } = "() => ({ get then() { globalThis.dataInspections++; throw new Error('data was inspected'); } })"
@export "sync"
let immediate_data = fn _ => data ()
@export "promise"
let fixed_promise = fn n => n

let settled_promises = fn n => match n with | 0.0 => 0.0 | _ => do
  let _ = completed 0.0
  return settled_promises (n - 1.0)
end
end

effect Other = Real -> Real
let nested_handlers = fn n => handle (handle do
  let a = !Ask.get n
  return !Other a
end with | !Ask.get value => delayed value end) with
  | !Other value => delayed (value + 10.0)
end

extern keep_data : fn(fn(Real) -> { "then": Real }) -> () = "globalThis.host.keepData"
let save_data = fn _ => keep_data (fn _ => data ())

-- A returned host function has its own Promise contract.
extern make_reader : fn(Real) -> @async fn(Real) -> Real = "offset => n => Promise.resolve(offset + n)"
let returned_host = fn offset => make_reader offset

-- Direction reverses at every parameter boundary. The host passes an async
-- function to Ruddy's async callback, which returns another async callback.
extern use_nested : fn(@async fn(@async fn(Real) -> Real) -> @async fn(Real) -> Real) -> () = "globalThis.host.useNested"
let nested_callbacks = fn offset => use_nested (fn host => fn n => host (offset + n))

-- The outer @async does not change the returned function's immediate contract.
extern make_sync : @async fn(Real) -> fn(Real) -> Real = "offset => Promise.resolve(n => offset + n)"
let returned_sync = fn offset => make_sync offset
