# 16 Strict host text ingress

Status: open
Type: task
Blocked by: 07

Spec: "Strict host ingress rejects malformed text unless an explicitly lossy
conversion was selected. This applies to record keys and reflection/error
metadata as well as payload strings."

The boundary check in `src/backend/type-runtime.js` accepts any JavaScript
string, so a lone surrogate enters as a `String`. What is missing is the check
itself, the lossy conversion a caller opts into instead, and the same rule for
record keys and for the text inside reflection and error metadata.

## Comments

Implementer handoff at `cd906cf`: current probes observe `#Some` from a typed
`js::lift` of the JavaScript string `"\ud800"`, and from `js::keys` on
`{ "\ud800": 1 }`. Both admit lone surrogates as Ruddy strings. Check nested
payloads, keys, and error/description text, not just root string values.

Acceptance covers lone high and low surrogates being rejected, valid surrogate
pairs and other valid scalar text being preserved, and explicit lossy
conversion having a documented replacement policy. Coordinate arbitrary-host
observation effects with [21](21-make-host-lifting-effects-explicit.md) and
safe error text with [26](26-contain-host-observation-failures.md). The reference
interpreter's valid Rust strings are not evidence that the JS ingress checks
are present; exercise real JS host values.
