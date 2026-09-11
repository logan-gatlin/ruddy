# 07 Portable primitive contracts

Status: resolved
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

Resolution: `types::Domains` binds `Nat` and `Int` to 53-, 32-, or 64-bit
domains per compilation (the manifest's `integers`, the debugger request), a
literal outside them is `literal-outside-domain` in terms and patterns,
artifact headers record the domains and the linker refuses a mix, generated
JavaScript checks the boundary against `$domains` and keeps one integer
zero, and mirrors describe exact integer domains. Strings count scalars and
JSON escapes pair surrogates.
