# 16 Strict host text ingress

Status: resolved
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

## Answer

Checked ingress refuses a host string that is not Unicode scalar values.
`$convertType` in `src/backend/type-runtime.js` checks every code unit pairs
at a `String` node coming in, so a lone surrogate never becomes a Ruddy
`String` — as a payload, as a record field, or as one of the strings an
observation answers with, since all three cross at that node. Going out is
unchanged: a Ruddy string is scalars already. The interpreter needs no check,
because a Rust string is scalars by construction, and its branch says so.

The repair is the explicit conversion the spec asks for rather than something
strict reading does quietly: `std::js::text` reads a host string with each
unpaired half replaced by `U+FFFD`, and refuses a value that is not a string
at all. An error message is the one place text must exist whatever the host
wrote, so the message formatter repairs rather than refuses; see
[26](26-contain-host-observation-failures.md).

Regressions: `host_failures_arrive_as_errors_rather_than_exceptions` in
`tests/src/js_adapters.rs` lifts a lone surrogate strictly (refused, naming
`String`), lifts an astral character (accepted), reads the same lone
surrogate through `std::js::text` (repaired, three scalars), and reads a
non-string through it (refused). `whole_file_errors_are_results_and_invalid_text_does_not_overwrite_files`
in `tests/src/stdlib_fs.rs` now asserts the boundary refuses the lone
surrogate, which is why there is no such text to write to a file.

Still open from this ticket: nothing for strict ingress. The lossy
conversion is provided for JavaScript host values; a target with its own text
representation supplies its own.
