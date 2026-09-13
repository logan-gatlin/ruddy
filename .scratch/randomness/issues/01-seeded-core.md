# Primitive Random operations and parent-seeded local handlers

Status: resolved

Implement the public interface and deterministic semantics in
[the spec](../spec.md). Add the std module export. Keep SplitMix64 state private;
use existing arithmetic unless exact conversion requires a narrow primitive.
Expose word64, boolean, and real as actual effect operations. Implement `local`
by eagerly drawing one outer word before entering `with_seed`. All primitive
handler arms share private state and never forward their draws to the parent.

Validate through the public interface using independent reference vectors,
scripted rejection cases, numeric endpoints, generic array choices, nested
handlers, exact parent seed consumption (including empty and aborting bodies),
effect forwarding, and interpreter/JavaScript agreement. This ticket
must work without the ambient host adapter. Run Rust tests only via `just test`.

## Answer

Implemented `std::random`, SplitMix64 scopes, primitive operations, rejection
sampling, and generic array choice. Reference vectors, numeric endpoints,
parent seed counts, nested/aborting scopes, effect forwarding, and supported
backend/domain combinations pass focused tests. Compiler prerequisites and
validation are recorded in [the spec](../spec.md) and [review](../review.md).
