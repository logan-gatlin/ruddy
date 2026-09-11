# 07 Portable primitive contracts

Status: claimed
Type: task

Spec section: "Portable primitive contracts" and acceptance bullets on target
domains, scalar text, and literals.

- Target integer-domain binding: `Nat`/`Int` precision, signedness, exact
  bounds as compiler target metadata (JS: 53-bit safe domains); out-of-domain
  literal diagnostics when the domain is bound; domain metadata reported by
  mirrors/descriptions; artifact/cache stamps include the bound domains.
- One integer zero; saturating/wrapping contracts unchanged.
- `std/str.rud`: scalar-based length, char_at, slicing, index_of, reverse,
  padding, splitting; strict host text ingress rejecting malformed UTF-16.

Seams: `tests/src/stdlib_apis.rs`, `tests/src/fixed_integers.rs`, generated JS.
