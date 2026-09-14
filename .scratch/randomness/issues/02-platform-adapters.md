# Default Node and web randomness handlers

Status: resolved
Blocked by: 01

Implement the default adapter described in [the spec](../spec.md), including
structural platform declarations, Immediate discipline, Node entry allowances,
and Node/web library exports. Assemble two host-generated 32-bit chunks exactly
as a Nat64. Do not initialize a global seeded generator.
Handle all three primitive operations through a private source. Verify local
scopes draw once from the host for their seed and never for subsequent draws.

Validate automatic handler installation, local override precedence, return
representation/ranges, and import purity. Stub host draws deterministically;
do not assert that successive ambient draws must differ. Use controlled async
completion orders to verify local handler state survives suspension and fixed
child seed assignment isolates per-child results from sibling interleaving.
Run Rust tests only
via `just test`.

## Answer

Installed structural Random handlers for Node entry points and Node/web
library exports. Controlled host tests pass for all primitives, exact chunk
assembly, import purity, explicit overrides, and one-word local seeding.
Opposite async completion schedules preserve each child stream with stable seed
assignment. See [review](../review.md) for validation.
