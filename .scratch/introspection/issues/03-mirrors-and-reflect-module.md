# 03 Typed mirrors and the `reflect` module

Status: open
Type: task
Blocked by: 02

Spec sections: "1. Typed mirrors", "Exact casts and dynamic reflection",
compiler work items 1 and 5.

- Builtin opaque invariant type constructor `Mirror 'a` with authenticated
  runtime values (descriptor graphs) on the JS backend.
- `std/reflect.rud`: `mirror : () -> Mirror 'a`, `type_of : 'a -> Mirror 'a`,
  `describe : Mirror 'a -> Description`, `same : Mirror 'a -> Mirror 'b ->
  Option { forward: 'a -> 'b, backward: 'b -> 'a }`, plus the `Description`
  data type (numeric kinds with effective domains, arrays, record and sum fields
  with presence, recursive references, function argument/result/effect rows,
  abstract positions).
- Evidence: the mirror intrinsics demand runtime type information for their
  type parameter through the existing per-arrow demand machinery. An authentic
  `Mirror 'a` bound by a successful `hide` pattern (through explicit
  record/tuple/option/sum payload patterns) supplies evidence for the opened
  rigid within that arm, installed before arm closure construction; ignored
  mirrors, whole-payload bindings, and unopened nested packages do not.
- Exact equality follows structural equivalence including effect contracts and
  alpha-equivalent hidden binders; rigid opened types never compare equal to
  anything but themselves.
- Debugger: reification tab shows mirror demands and scoped evidence.

Seams: `tests/src/inference.rs`, `tests/src/compile.rs`, generated JS execution
(`tests/src/stdlib.rs` / bundles), `tests/src/artifact.rs`.
