@ffi { completion: "promise" }
extern delayed : Real -> Real = "globalThis.host.delayed"
effect Ask = { get: Real -> Real }
let read = fn n => handle (1.0 + !Ask.get n) with
  | !Ask.get value => delayed value
end

@ffi { parameters: [{ callback: "promise" }] }
extern remember : fn(fn(Real) -> Real) -> () = "globalThis.host.remember"
let save = fn offset => remember (fn n => delayed (n + offset))

let abort = fn n => handle !Ask.get n with
  | !Ask.get value => do let result = delayed value return raise result end
  | return value => value + 100.0
end
let normal = fn n => handle !Ask.get n with
  | !Ask.get value => delayed value
  | return value => value + 100.0
end

@ffi { completion: "callback" }
extern completed : Real -> Real = "globalThis.host.completed"
let via_callback = fn n => 1.0 + completed n

@ffi { parameters: [{ callback: "notification" }] }
extern watch : fn(fn(Real) -> Real) -> () = "globalThis.host.watch"
let notify = fn offset => watch (fn n => delayed (n + offset))

@ffi { parameters: [{ callback: "completion" }] }
extern keep_completed : fn(fn(Real) -> Real) -> () = "globalThis.host.keepCompleted"
let save_completed = fn offset => keep_completed (fn n => delayed (n + offset))

effect Exit = { stop: Real -> Real }
@ffi { parameters: [{ callback: "promise" }] }
extern keep_exit : fn(fn(Real) -> Real + !Exit) -> () + !Exit = "globalThis.host.keepExit"
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
@ffi { parameters: [{ callback: "promise" }] }
extern keep_reader : fn(fn(Real) -> Real + !Reader) -> () + !Reader = "globalThis.host.keepReader"
let ordinary_evidence = fn offset => handle keep_reader (fn n => !Reader.get n) with
  | !Reader.get n => n + offset
end

let record = { run: fn n => delayed n }

let fresh = fn offset => remember (fn n => handle !Ask.get (n + offset) with
  | !Ask.get value => do let result = delayed value return raise result end
  | return value => value + 100.0
end)

@ffi { parameters: [{ callback: "sync", result: { callback: "promise" } }] }
extern keep_factory : fn(fn(Real) -> fn(Real) -> Real) -> () = "globalThis.host.keepFactory"
let factory = fn offset => keep_factory (fn n => fn m => delayed (offset + n + m))

extern keep_sync : fn(fn(Real) -> Real) -> () = "globalThis.host.keepSync"
extern observed : Real -> Real = "globalThis.host.observed"
let unsafe_sync = fn _ => keep_sync (fn n => observed (delayed n))

extern failure : Real -> Real = "n => { throw new Error('immediate failure'); }"
let notify_failure = fn _ => watch (fn n => failure n)

@ffi { completion: "promise" }
extern translated : Real -> Real = "n => Promise.reject(new Error('translated')).catch(() => n + 10)"
let translation = fn n => translated n

extern data : () -> { "then": Real } = "() => ({ get then() { globalThis.dataInspections++; throw new Error('data was inspected'); } })"
@export "sync"
let immediate_data = fn _ => data ()
@export "promise"
let fixed_promise = fn n => n

let registrations = fn n => match n with | 0.0 => 0.0 | _ => do
  let _ = completed 0.0
  return registrations (n - 1.0)
end
end

effect Other = Real -> Real
let nested_handlers = fn n => handle (handle do
  let a = !Ask.get n
  return !Other a
end with | !Ask.get value => delayed value end) with
  | !Other value => delayed (value + 10.0)
end

@ffi { parameters: [{ callback: "completion" }] }
extern keep_data : fn(fn(Real) -> { "then": Real }) -> () = "globalThis.host.keepData"
let completion_data = fn _ => keep_data (fn n => do
  let _ = delayed n
  return data ()
end)
