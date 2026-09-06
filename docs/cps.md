# CPS execution and foreign completion

Ruddy functions use the same source types and calling convention whether they
complete immediately or suspend. There is no `async` keyword, `await` expression,
or `Async` effect. Evaluation within an invocation remains sequential. Domain
effect rows describe capabilities, independently of foreign completion timing.

## Foreign declarations

An unannotated extern uses immediate completion. This is a trusted foreign
contract: a returned Promise is data, and is not implicitly awaited. Mark a
binding that completes through a Promise explicitly:

```text
@ffi { completion: "promise" }
extern fetch_count : String -> Nat = "host.fetchCount"
```

`completion: "callback"` appends separate success and failure functions to the
host argument list. For example, the target of a unary binding is called as
`host.read(argument, resolve, reject)`. Registration may complete synchronously.
The first success or failure wins, including an exception during registration;
subsequent deliveries cannot resume the operation again. A handler exit remains
a distinct control transfer, rather than an ordinary failure value.

Use a marked `fn(...)` ABI when specifying nested function boundaries. Protocol
records mirror its parameters and result:

```text
@ffi { parameters: [{ callback: "promise" }] }
extern subscribe : fn(fn(Nat) -> Nat) -> () = "host.subscribe"
```

`parameters` has one record per marked parameter; `{}` retains the defaults.
`result` describes a returned function boundary. At host-function positions use
`completion`; at Ruddy callback positions use `callback`. Parameter positions
reverse direction, while results keep their enclosing direction. The compiler
rejects unknown keys, invalid protocols, arity mismatches, and protocols on
non-function values.

A callback has one fixed result contract:

| `callback` | Host observation |
| --- | --- |
| `"sync"` (default) | Returns a value or throws. This binding promises that the callback completes synchronously. Suspension violates that contract and fails the invocation. |
| `"promise"` | Always returns a Promise, including on immediate success or failure. |
| `"completion"` | Accepts success and failure functions after the visible arguments, delivers once, and returns `undefined`. |
| `"notification"` | Returns `undefined`; reports failures through the runtime reporter. |

Callbacks retain their closure, captures, evidence, and runtime as ordinary
JavaScript-owned values. Each call starts an independent invocation; repeated
and overlapping calls are supported. Captured values are shared according to
ordinary closure semantics. Promise and notification callbacks begin executing
immediately and run until completion or a real suspension.

Synchronous exceptions and asynchronous failures fail the invocation unless the
binding explicitly translates them. For example, a Promise target may use
`.catch(...)` to produce a normal Ruddy result representation. Host failures do
not implicitly become a Ruddy `raise`.

Embedders can install a notification error reporter before loading a module:

```js
globalThis[Symbol.for("ruddy.runtime")] = {
  onUnhandledError(error) { console.error(error); }
};
```

The default reports an uncaught host error. Notification adapters handle their
internal Promise failures; they do not depend on a caller observing a returned
Promise.

## Library exports and initialization

The compiler computes conservative suspension summaries while compiling each
bundle and persists them separately from source types. Proven synchronous
functions use synchronous JS exports. Potentially suspending functions use
Promise exports. Returned source functions follow the same rule at each curried
arrow. An unknown indirect call, handler implementation, or imported callable
summary is potentially suspending; an empty effect row alone is not a proof.

`@export "promise"` requests a Promise adapter even for a synchronous function.
`@export "sync"` requires a compile-time non-suspension proof. These attributes
apply to known function-valued `let` definitions and do not alter Ruddy-to-Ruddy
calls. A callback explicitly declared synchronous by an extern is a separate
trusted foreign contract, with a runtime guard against suspension.

Global initializers run through the same driver in dependency and declaration
order. JS module readiness waits for initialization to succeed. Executable
`main` runs once afterward, and Console output and Process exit retain their
platform behavior, including output draining.

## Handlers and stack bounds

An operation arm's ordinary return resumes the operation once. Normal completion
of the handled body runs its return arm. `raise` exits the matching handler and
bypasses that return arm. Suspension preserves the continuation and active
handler context; it does not grant source code access to continuations.

Ordinary captured evidence remains callable after registration. Exiting a
handler additionally requires a live identity in the current logical ancestry.
A retained callback cannot revive an expired handler or abort an unrelated
suspended invocation. An inline synchronous callback can borrow the enclosing
foreign call's ancestry. A callback may establish its own fresh handlers.

The driver bounds native stack use for Ruddy calls and continuation transfers,
including mutual, indirect, effectful and non-tail recursion. Tail calls reuse
their continuation; non-tail recursion can retain proportional heap state.
Unbounded recursive alternation through synchronous JS calls and callbacks is
outside this stack guarantee. Immediate transfers do not yield to the event
loop or allocate a Promise per call. Fair scheduling, cancellation, workers,
and explicit or multi-shot continuation operations remain future work.

## Compiler representation

Each bundle publishes CPS LIR immediately. A function owns flat parameterized
basic blocks; value instructions precede one explicit control terminator. Calls
carry continuations. A continuation names a function and block plus its explicit
captures, with the delivered result bound last. It is distinct from an ordinary
function closure. Handler entry, normal leave and abort are explicit transfers.

Private bundle-local construction may use structured control while determining
captures. This form is never persisted. Artifacts contain closed CPS blocks,
foreign completion operations, callback adapter operations, initializer function
references, and suspension summaries. The linker only concatenates artifacts
and relocates function IDs in calls, closures, continuations and initializers;
block IDs stay local. No transformation of the fully linked artifact is needed.

The executable artifact schema deliberately breaks compatibility with the old
nested LIR. Its `cps-lir` payload is canonical JSON embedded in the existing
artifact S-expression. Validation checks destinations, entry kinds, closed
block environments, argument and capture layouts, and required metadata before
the linker or backend consumes untrusted artifacts. Compiler stamps include the
JS driver source so changes invalidate cached artifacts.
