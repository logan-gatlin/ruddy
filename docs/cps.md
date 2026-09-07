# CPS execution and foreign completion

Ruddy functions use the same source types and calling convention whether they
complete immediately or suspend. The `@async` tag marks a foreign boundary;
ordinary functions need no annotation, `await` expression, or `Async` effect.
Evaluation within an invocation remains sequential. Domain effect rows describe
capabilities, independently of foreign completion timing.

## Foreign declarations

An unannotated extern uses immediate completion. This is a trusted foreign
contract: a returned Promise is data, and is not implicitly awaited. Mark a
binding that completes through a Promise explicitly:

```text
@async
extern fetch_count : String -> Nat = "host.fetchCount"
```

`@async` selects Promise completion; the return type describes the resolved
value. Without the tag, completion is immediate. Declaration-level `@async`
is shorthand for an annotation on the outermost function:

```text
extern fetch_count : @async fn(String) -> Nat = "host.fetchCount"
```

Use `fn(...)` to describe nested function boundaries, and put metadata directly
on the function it belongs to:

```text
-- JS receives a Ruddy callback that returns a Promise.
extern subscribe : fn(@async fn(Nat) -> Nat) -> () = "host.subscribe"

-- The factory returns immediately; its returned JS function returns a Promise.
extern make_reader : fn(String) -> @async fn(Nat) -> String = "host.makeReader"

-- JS receives a synchronous factory whose returned Ruddy callback is async.
extern use_factory : fn(fn() -> @async fn(Nat) -> String) -> () = "host.useFactory"
```

Annotations apply only to the function they decorate. Parameters reverse the
direction of the boundary; results keep it. In either direction, `@async` means
that a JS invocation returns a Promise. Ruddy awaits host Promises and supplies
Promise-returning adapters for its own annotated callbacks. An outer annotation
does not make any nested functions async. Parentheses can group an annotated
function, as in `(@async fn(Nat) -> Nat)`.

`@async` on an ordinary function type or a function alias selects its outermost
call boundary. Use nested `fn(...)` signatures when individual inner boundaries
need annotations. Metadata does not change the ordinary Ruddy type or effects.
Annotations are supported throughout nested function signatures; annotations
inside records, tuples, arrays, and ordinary type aliases are not supported.
`fn(Nat) -> @async String` is invalid: annotate the function, not its scalar
result. Unknown boundary attributes, non-tag `@async` values, and duplicate
attributes on one type occurrence are rejected. The boundary tree retains
metadata and source locations for future attributes.

A Ruddy callback has one fixed JS-facing result contract:

| Annotation | Host observation |
| --- | --- |
| None (default) | Returns a value or throws. Suspension violates this immediate contract and fails the invocation. |
| `@async` | Always returns a Promise, including on immediate success or failure. |

Extern completion also has only immediate and Promise modes. Adapt a
callback-based host API with an explicit JS Promise wrapper:

```text
@async
extern read : String -> String =
  "key => new Promise((resolve, reject) => host.read(key, resolve, reject))"
```

Callbacks retain their closure, captures, evidence, and runtime as ordinary
JavaScript-owned values. Each call starts an independent invocation; repeated
and overlapping calls are supported. Captured values are shared according to
ordinary closure semantics. Promise callbacks begin executing
immediately and run until completion or a real suspension.

Synchronous exceptions and asynchronous failures fail the invocation unless the
binding explicitly translates them. For example, a Promise target may use
`.catch(...)` to produce a normal Ruddy result representation. Host failures do
not implicitly become a Ruddy `raise`.

JavaScript callers handle failures by catching synchronous exceptions or
observing returned Promise rejections. There is no runtime notification reporter.

## Library exports and initialization

The compiler computes conservative suspension summaries while compiling each
bundle and persists them separately from source types, including the producer
proofs used by global reads. Artifact validation checks synchronous indirect
calls against those proofs; unknown parameters cannot certify themselves. Proven synchronous
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

Mutable cells use the same sequential CPS execution and survive suspension.
See [mutable cells](mutability.md) for region inference, the cell representation,
and the trusted foreign state contract.
