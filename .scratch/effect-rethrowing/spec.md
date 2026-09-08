# Set-like effects with rethrowing

Status: Implemented and reviewed; tests pass. See `implementation.md` for the remaining coverage-target gap.

## Scope

Keep Ruddy's set-like effect rows and allow an operation arm to perform an
effect that its own handler handles. That operation uses the surrounding
handler environment. This spec calls that behavior **rethrowing**; it also
applies to resumable operations such as logging and context lookup, not only
exceptions. It introduces no keyword or invocation syntax.

Masking is excluded. A computation cannot skip its nearest handler, select a
handler by depth, or capture an explicitly named effect instance through a new
language feature.

This spec supersedes the refusal of same-effect operations in handler arms in
`src/inference/constrain.rs` and
`tests/src/inference.rs::re_performing_a_discharged_effect_is_refused`.
It preserves the constructor identity, argument coherence, and written-row
uniqueness requirements in `../typed-effect-parameters/spec.md`. In particular,
it does not introduce rows such as `!Ask Nat + !Ask String`.

## Problem

Currently a handler checks its body in an ambient row extended with the
handled constructors. Row-lacks bookkeeping consequently prohibits those
constructors in the outer ambient, including the handler arms. A logging arm
that calls the surrounding logging handler is rejected as a repeated effect.

That rejection prevents ordinary handler decoration and forwarding even
though these behaviors need only one escaping entry per constructor. Handler
scope must distinguish local handling from outer handling without representing
that distinction as duplicate entries in a function's effect row.

## Required behavior

### Set-like rows

- A function's effect row contains at most one application of each structural
  effect constructor. Constructor identity continues to use existing rules,
  including equivalence across modules and alias expansion.
- Independently inferred compatible occurrences coalesce. Repeated calls and
  repeated rethrows do not increase the number of entries.
- Incompatible arguments of occurrences that must describe the same
  application remain errors. This feature does not add support for forwarding
  between different applications of one constructor or relaxing argument
  coherence across nested handlers.
- Explicit duplicate constructors remain errors, including duplicates exposed
  by alias expansion and duplicates with different type arguments.
- Rows remain upper bounds. An unused allowance in an annotation need not
  correspond to an operation executed at runtime.
- Existing explicit row-tail lacks, negative labels, conditional presence,
  and presence-formula semantics remain in force. The change removes the
  handler-induced prohibition on handled constructors in the outer ambient;
  it does not globally disable effect-row lacks checks.

### Handler scope and control flow

- The handled body uses the local handler for every constructor that handler
  completely implements. Unhandled constructors use the surrounding evidence.
- Operation arms execute using the surrounding handler environment. An arm's
  operation on a handled constructor therefore reaches the nearest surrounding
  handler for that constructor, not the arm's own handler.
- A rethrow may use a different payload or another operation of the same
  constructor, subject to the existing operation signatures and shared effect
  arguments.
- A surrounding resumable handler supplies the rethrown operation's result to
  the inner arm. The inner arm then continues normally. Its eventual result
  resumes the original operation according to existing handler semantics.
- Existing abort and `raise` behavior is preserved. Rethrowing does not mean
  `raise`, and does not implicitly abort the handled computation.
- Return arms use the surrounding environment as well. Their effects are not
  discharged by the handler whose return arm they belong to.
- Handler coverage and duplicate-operation-arm checks are unchanged. Partial
  handlers and two arms for different applications of one operation are not
  introduced.
- Nested handlers for the same coherent application are permitted. Their
  runtime scope does not create duplicate row entries. A local non-rethrowing
  handler can swallow an effect even inside a context that also uses it.

### Inference and checking

For the definitely present part of rows, the semantic relationship is:

```text
effects(handle body with arms and return-arm) =
  (effects(body) minus handled constructors)
  union effects(operation arms)
  union effects(return-arm)
```

The union is set-like and unifies compatible effect arguments. This equation
describes the effect relationship, not a requirement to use literal closed-set
operations in the solver. Open tails and conditional presences must retain
their existing polymorphism and constraints.

- Removing an effect from the body must not remove a same-effect operation
  performed by an arm. Conversely, an effect available in the surrounding
  environment is not automatically performed by a non-rethrowing handler.
- The residual effects of the body and the effects of the arms must be
  represented separately enough that a body-tail lacks constraint does not
  prohibit a legitimate arm rethrow.
- Rethrowing a handled application retains that application exactly once in
  the handler expression's escaping effects. Swallowing it leaves no entry
  attributable to the body's handled operations.
- An escaping rethrow must be allowed by its enclosing function's contract or
  handled by an enclosing handler. Pure annotations and top-level initializer
  purity cannot be bypassed. A missing outer allowance is an effect-boundary
  error, not a duplicate-row error.
- These rules apply through calls and effect-polymorphic callbacks, not only
  to a syntactically direct operation in an arm.
- Invoking a callback uses the handler environment at invocation. Defining,
  returning, or passing an effectful function does not itself perform its
  latent effects or bind it to the handler active at its definition.
- Generalization and public signatures must preserve the relationship between
  a callback's residual effects and the enclosing function's escaping effects.
  Inference must not close away an unknown escaping effect or expose duplicate
  layers in printed types.

## Examples

These examples specify intended behavior after implementation.

```ruddy
effect Log = { write: String -> () }

let work : () -> () + !Log = fn _ => do
  let _ = !Log.write "hello"
  let _ = !Log.write "again"
  return ()
end

let forward :
  (() -> 'a + !Log + ..'e) -> 'a + !Log + ..'e
  = fn action =>
    handle action () with
      | !Log.write message => !Log.write message
    end

let silence :
  (() -> 'a + !Log + ..'e) -> 'a + ..'e
  = fn action =>
    handle action () with
      | !Log.write _ => ()
    end

let forwarded : () -> () + !Log = fn _ => forward work

let quiet : () -> () = fn _ =>
  silence (fn _ => forward work)
```

`forwarded` exposes exactly one `Log`. `quiet` exposes none: both messages
reach the outer swallowing handler. Neither function needs a duplicate row.
The explicit `..'e` tails in these signatures retain ordinary constructor-lacks
rules; they describe the remaining constructors rather than another `Log`.
The corresponding unannotated definitions must infer equivalent relationships.

Rethrowing can return a value and resume the original computation:

```ruddy
effect Ask 'a = { get: () -> 'a }

let answer : () -> Nat = fn _ =>
  handle
    handle !Ask.get () with
      | !Ask.get _ => !Ask.get ()
    end
  with
    | !Ask.get _ => 42n
  end
```

`answer ()` returns `42n`. Both handlers agree on `Ask Nat`; the inner arm
uses the outer handler rather than recursively invoking itself.

The following declarations remain invalid:

```ruddy
let duplicate : () -> () + !Log + !Log = fn _ => ()

let incompatible : () -> () + !Ask Nat + !Ask String = fn _ => ()

let falsely_pure : () -> () = fn _ => forward work
```

The first two fail existing written-row uniqueness checks. The last fails
because `Log` escapes a function promising purity.

## Implementation boundaries

- Update handler constraint generation and solving without changing the row
  algebra for structs or variants. Merely deleting the handler's lacks call
  is insufficient if it leaves repeated constructors in intermediate rows or
  loses relationships involving open tails.
- Preserve surrounding evidence while building operation and return arms;
  supply local evidence only to the handled computation. Verify evidence
  adaptation through higher-order functions and open effect bundles.
- Preserve one effect entry per constructor in semantic publication, printed
  signatures, and artifacts. Imported functions must rethrow like local ones.
- Keep causal diagnostics for incompatible arguments and missing effect
  allowances. Remove explanations that treat valid rethrowing itself as a
  repeated row label; ordinary explicit duplicate diagnostics remain valid.
- No runtime type inspection, instance-selection syntax, or new user-visible
  row constraints are introduced by this spec. Solver representation changes
  are implementation choices subject to the required public behavior.

## Validation

Use source-level compilation and execution tests for observable behavior:

1. Infer one escaping `Log` for direct rethrowing and repeated rethrows; infer
   no `Log` after an enclosing swallowing handler.
2. Execute nested rethrowing handlers and verify ordering, payloads, and exactly
   one outer dispatch per rethrow. Verify that the inner arm does not invoke
   itself and that returned operation results resume the original computation.
3. Verify modified payloads, same-constructor multi-operation forwarding, and
   existing abort behavior through nested handlers.
4. Verify effectful return arms and arm effects on unrelated constructors.
   Neither is accidentally discharged as a body effect.
5. Verify same-constructor nested swallowing and that an unrelated outer use
   of the constructor remains visible. Do not infer a phantom escaping effect
   just because an outer handler is available.
6. Compile annotated and inferred `forward` and `silence` equivalents with
   pure callbacks, callbacks performing `Log`, and callbacks with additional
   effects. Additional effects survive and remain connected to the callback.
7. Verify callback invocation-site behavior, rethrowing through helpers and
   stored operation values, and open-row evidence adaptation at runtime.
8. Retain rejection tests for explicit duplicates, alias overlap, incompatible
   arguments, invalid row-tail substitutions, and incomplete handler coverage.
9. Verify that negative and conditional effect annotations and their formulas
   retain existing behavior, including constraints in generalized signatures.
10. Reject unhandled rethrows at pure function and initializer boundaries with
    effect-boundary diagnostics. Do not report valid rethrowing as duplication.
11. Round-trip public signatures and execute a dependency-imported forwarding
    function under a consumer's handler, including structural effect aliases.
12. Retain mutation isolation, value generalization, and record/variant row
    regression coverage. Rethrowing does not relax those guarantees.

Run the Rust test suite only through `just test`, never `cargo test`.

## Deferred features

- Masking, handler-depth selection, and explicit instance handles.
- Duplicate or scoped-list effect rows.
- Multiple applications of one constructor in a row, such as
  `!Ask Nat + !Ask String`, or selection by expected type.
- New permission to use incompatible applications across handler scopes or
  rethrow between those applications.
- Multiple mutation regions, changes to builtin mutation handling/isolation,
  and changes to handler control-flow syntax or partial coverage.
