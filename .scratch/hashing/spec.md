# Structural hashing

Agreed in the design interview: implement reflection-based hashing for ordinary
maps and sets, following the explicit/inferred mirror pattern of `std::order`.

- Pure `std::hash` API: `hash`, `hash_seeded`, and `hash_with` (explicit mirror
  and seed). Seeds and outputs are `Nat64`; the default seed is fixed.
- Equal supported values at their shared static type must hash equally with
  the same seed. No persistence or hostile-collision guarantee.
- Traverse supported primitives, arrays, alphabetically ordered record fields,
  and active sum payloads, including finite recursive data. Frame input with
  tags, lengths, field names, and case names.
- Fail only on encountered unsupported values (functions, cells/foreign values,
  hidden packages, mirrors) or NaNs. Empty containers and inactive unsupported
  cases remain hashable. Normalize signed zero.
- Return structured errors with root-to-leaf field/case/index paths.
- Use a private immutable FNV-1a 64 accumulator; add a narrow integer XOR
  primitive for interpreter and JavaScript. No public hashing effect or
  pluggable algorithm interface. No map/set implementation.
- Document the public contract and tradeoffs. Test the public hashing API for
  equality compatibility, seeds, nested failures, recursive data, and agreement
  between interpreter and JavaScript. Run Rust tests only through `just test`.
- Review the change against repository standards and this spec, then commit
  on the current branch.
