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
