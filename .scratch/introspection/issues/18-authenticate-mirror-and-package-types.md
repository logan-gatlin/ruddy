# 18 Check the semantic type of foreign mirror and package handles

Status: resolved
Type: task
Priority: P1

Reviewed baseline: `cd906cf`.

Spec: [Typed mirrors](../spec.md#1-typed-mirrors),
[Exact casts and dynamic reflection](../spec.md#exact-casts-and-dynamic-reflection),
and [Foreign conversion and invocation](../spec.md#4-foreign-conversion-and-invocation).

## Problem and evidence

Foreign conversion checks that a mirror or hidden-package handle is authentic,
but does not check that its semantic type equals the destination type. An
authentic `Mirror Nat` can therefore enter a value statically typed
`Mirror String`; comparing those mirrors then returns a witness claiming that
`Nat` and `String` are equal. A hidden package has the same defect: the visible
part of `hide 'a => { secret: 'a, visible: Nat }` can be read as `String` after a
foreign round trip.

The latest review reproduced both failures on JavaScript. The interpreter has
the corresponding unchecked branches; this part is supported by code inspection,
not a fresh interpreter execution during that review. The mirror failure was
also reported by the preceding implementation review.

Start in the `Mirror` and `Hidden` branches of
[`$convertType`](../../../src/backend/type-runtime.js) and
[`Conversion`](../../../interp/src/convert.rs). JavaScript's `$mirrors` membership
and `$packages` lookup establish authenticity only; the interpreter similarly
accepts any authenticated `Value::Type` or `Value::Package`.

## Minimal reproductions

Each program is standalone; use this `Ruddy.toml` and put one program at a time
in `main.rud`, build the library, then inspect its exported value:

```toml
name = "handle-type-repro"
version = "0.1.0"
kind = "library"
root = "main.rud"
target = "js"
platform = "node"
[dependencies]
std = false
```

Mirror confusion:

```ruddy
type Result 'a 'e = #Some 'a | #Error 'e
type Option 'a = #Some 'a | #None
type Error = { path: String, expected: String, message: String }
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern encode: 'a -> Result ForeignValue Error = "$ffiEncode"
@private extern decode: ForeignValue -> Result 'a Error = "$ffiDecode"
@private extern same: (Mirror 'a, Mirror 'b) -> Option {
  forward: 'a -> 'b, backward: 'b -> 'a,
} = "$sameMirror"
@private let nat: Mirror Nat = mirror ()
@private let wrong: Result (Mirror String) Error = match encode nat with
| #Some value => decode value
| #Error error => #Error error
end
let crossed: Bool = match wrong with
| #Some string_mirror => match same (nat, string_mirror) with
  | #Some _ => true
  | #None => false
  end
| #Error _ => false
end
```

Observed: `crossed` is `true`. Required: decoding reports `#Error`, and `crossed`
is `false`; no equality witness may be fabricated.

Hidden-package confusion:

```ruddy
type Result 'a 'e = #Some 'a | #Error 'e
type Error = { path: String, expected: String, message: String }
type Box 'v = hide 'a => { secret: 'a, visible: 'v }
@private extern encode: 'a -> Result ForeignValue Error = "$ffiEncode"
@private extern decode: ForeignValue -> Result 'a Error = "$ffiDecode"
@private let box: Box Nat = { secret: true, visible: 42n }
@private let wrong: Result (Box String) Error = match encode box with
| #Some value => decode value
| #Error error => #Error error
end
let value: String = match wrong with
| #Some package => match package with
  | hide 'a { secret: _, visible } => visible
  end
| #Error _ => "rejected"
end
```

Observed: the value declared `String` contains the number `42`. Required: the
decode returns `#Error`, so the exported value is `"rejected"`.

## Required change

- Bind each authentic handle to the complete semantic identity it represents,
  and check that identity against the expected type at ingress on both backends.
  A mirror checks the mirrored type; a hidden package checks its enclosing
  hidden type, without exposing the producer's hidden witness.
- Preserve exact structural equality, alpha-equivalent hidden binders, primitive
  kind distinctions, callable effect contracts, and free scope/owner identities.
  Shape compatibility, a matching hash, or authenticity alone is insufficient.
- Preserve valid opaque round trips and the associated scope dependencies. Do
  not fix this by admitting forged descriptions, unsealing hidden payloads to
  guess their type, or rejecting every valid mirror/package round trip.
- Account for handle reuse: a cache keyed only by payload object identity must
  not attach one static type's authority to another enclosing package type.

## Acceptance checks

- Both negative programs fail at the checked conversion boundary on JavaScript
  and the interpreter; neither yields a value under the wrong static type.
- Same-type mirrors and packages round-trip successfully, including nested
  records/arrays and structurally equal aliases. Alpha-renaming a hidden binder
  alone must not make a valid round trip fail.
- Reject mismatches in a package's visible field type and in a mirror's primitive
  kind or callable effect contract, even where runtime layouts coincide.
- Reject host-created lookalikes and forged descriptions. Preserve existing
  conservative rejection of unsupported scope-dependent evidence.
- Exercise the same programs through both backend runtimes and add focused
  regression coverage through the repository's `just test` runner.

## Answer

A handle is now held against the type the position reads it under, at ingress,
on both backends.

`$convertType` in `src/backend/type-runtime.js` and `Conversion::walk` in
`interp/src/convert.rs` check, at a `Mirror` node coming in, that the authentic
descriptor is the same type as the node's own mirrored type, by the exact
structural equality the rest of reflection uses. At a `Hidden` node, sealing
records the hidden type the value was sealed at alongside the value, and
unsealing accepts the handle only when that type is the one the position asks
for. A value sealed at more than one hidden type gets one handle per type, so
reusing a handle cannot lend one type's authority to another.

The check is on ingress alone. Going out, the program hands over its own
handle, and the plan it crosses under describes types under the host's
conversion policy rather than the exact policy a mirror carries, so comparing
the two there would reject valid exports. Where the type at the position is
not settled — a plan parameter nothing has supplied yet — ingress refuses
rather than letting the handle through unchecked.

Regression: `a_handle_is_read_only_under_the_type_it_was_made_at` in
`tests/src/interp.rs` runs the same program on JavaScript and the interpreter
and asserts they agree. It covers a `Mirror Nat` read as `Mirror String`
(refused, and no equality witness), the same mirror read as itself (accepted),
a `hide 'a => { secret: 'a, visible: Nat }` read as the `String` form
(refused) and as itself (accepted), the same package read through an
alpha-renamed binder and an alias of it (accepted), and a record of two
mirrors read with the two swapped (refused, naming the field).
