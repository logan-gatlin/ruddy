# 25 Validate complete reflection intrinsic signatures

Status: open
Type: task
Priority: P2

Reviewed baseline: `cd906cf`.

Spec: [Typed mirrors](../spec.md#1-typed-mirrors),
[Heterogeneous fields](../spec.md#heterogeneous-fields-without-erasing-their-types),
and [Compiler implementation and termination](../spec.md#5-compiler-implementation-and-termination).

## Problem and evidence

The compiler recognizes reserved reflection intrinsic declarations from partial
signature checks. `$describe` accepts a record merely because it has required
fields called `root` and `nodes`; their types are not checked. `$shape` checks
the outer case names and type argument, but not case payloads. Generated runtime
values can consequently violate the program's checked types.

The latest review reproduced the `$describe` failure below: compilation succeeds
and the generated JavaScript fails during module initialization because the
declared `String` result actually contains a number. The `$shape` defect is
established by code inspection of its recognition branch, not a fresh runtime
reproduction in that review.

Start in [`Intrinsic::recognize`](../../../src/reification.rs), the declarations
in [`std/reflect.rud`](../../../std/reflect.rud), and the signature tests in
[`tests/src/inference.rs`](../../../tests/src/inference.rs). The current accepted
test fixture itself supplies only a partial `Description.Node` sum; tightening
recognition requires replacing that fixture with a valid complete contract.

## Minimal reproduction

Compile and load this standalone library:

```ruddy
@private extern mirror: () -> Mirror 'a = "$mirror"
@private extern describe: Mirror 'a -> {
  root: String, nodes: String,
} = "$describe"
@private let nat: Mirror Nat = mirror ()
let value: String = (describe nat).root
```

```toml
name = "intrinsic-signature-repro"
version = "0.1.0"
kind = "library"
root = "main.rud"
target = "js"
platform = "node"
[dependencies]
std = false
```

Required: a compiler diagnostic on the invalid `$describe` declaration, before
runtime code is produced.

For a focused `$shape` regression, take the valid `Shape` declarations from
`std/reflect.rud` and change only `#Nat`'s payload from
`{ read: 'a -> Nat, make: Nat -> 'a }` to
`{ read: 'a -> String, make: String -> 'a }`. Keep every case name and the
declaration `extern shape: Mirror 'a -> Shape 'a = "$shape"`. The present
recognition code cannot distinguish that invalid contract from the valid one.

## Required change

- Validate the full resolved structural signature of compiler-owned reflection
  intrinsics: primitive payloads, recursive description nodes, typed view and
  builder records, hidden binder relationships, required presences, closed rows,
  and callable argument/result/effect contracts.
- Preserve structural typing: equivalent aliases and renamed source type names
  are valid. Matching a particular module path or source type spelling is not
  proof that a declaration has the right contract.
- Check the rest of the reserved reflection family for the same omission,
  including required field presences in equality-witness functions. Invalid
  reserved declarations must report a compiler error, not silently become
  unchecked ordinary externs.
- Keep ordinary foreign declarations under the existing trusted FFI policy.
  This task tightens compiler-owned intrinsic contracts; it does not require
  proving arbitrary user extern implementations or changing callback policy.

## Acceptance checks

- Reject the `$describe` reproduction and malformed nested node payloads,
  missing cases, wrong field types/presences, and unsupported open rows.
- Reject `$shape` declarations that preserve every case name but change a
  primitive accessor, array element relationship, field binding, builder result,
  hidden witness, or callable effect contract.
- Accept the real standard-library declarations and structurally equivalent
  aliases. Check successful describe/shape use on both JavaScript and the
  interpreter after adding these stricter compiler checks.
- Preserve trusted ordinary extern behavior and existing valid exact-cast
  declarations. Run the regression suite only through `just test`.
