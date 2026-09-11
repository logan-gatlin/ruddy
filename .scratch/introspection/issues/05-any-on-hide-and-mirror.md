# 05 Define `Any` as a hidden package over a mirror

Status: open
Type: task
Blocked by: 03

Spec: "This lets the standard `Any` interface use the general package
machinery" and "These APIs have not shipped."

Replace the builtin `Any` type and the `$anyUpcast`/`$anyDowncast` intrinsics
with `std/any.rud` definitions: `type Any = hide 'a => { mirror: Mirror 'a,
value: 'a }`, `upcast` by contextual introduction, `downcast` by opening and
`reflect::same`. Keep exact-cast behaviour and forgery rejection; update
artifact, lowering, host conversion, and tests that relied on the builtin.

Seams: existing `Any` tests across `tests/src/`.
