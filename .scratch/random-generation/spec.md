# Inferred random generation

Status: implemented

Implement the agreed direct-return generator using constructive `Mirror` evidence.
`random::random ()` infers its result type and returns it directly under `Random`.
`random::generate budget mirror` accepts explicit evidence and a shared expansion
budget. Exhausted branches use the mirror's finite pure construction rather than
returning `None`. Unsupported types fail at compilation through the existing
construction requirement.

Support every constructive shape: primitives, records, realizable sum cases,
ordinary arrays, empty-only arrays, and recursive data. Use existing random
handlers, preserving seeded reproducibility and local handler isolation.
Keep `choose` and `nat_below` fallible for their existing invalid inputs.

Implementation policy: default budget 64 expansions; array and text lengths from
zero through eight; text uses printable ASCII; reals use the primitive's [0, 1)
contract; integers sample their exact domain. Sum cases retain positional uniform
sampling over realizable cases. These bounded structural distributions are not
uniform over all values of an arbitrary type. Rejection sampling retains the
existing source-liveness assumptions of `nat_below`.

The confirmed test seams are compiled public consumer programs, interpreter and
JavaScript agreement, and compile failures for unavailable evidence. Test direct
return types, all primitive domains, empty nested types, recursive termination
with exhausted budgets, deterministic seeded generation, and existing handlers.
Update public documentation. Run Rust tests only through `just test`.
