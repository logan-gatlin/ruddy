# 26 Contain failures while formatting host observation errors

Status: resolved
Type: task
Priority: P2

Review baseline: `cd906cf`. Reproduced in both reviews; still open.

Host observation promises recoverable `#Error` results. Its exception handler
currently observes the thrown value again while formatting its message; that
observation can throw a second exception outside the promised error boundary.

## Reproduction and impact

Compile this JavaScript-target module with the standard library dependency:

```ruddy
let field = std::js::field
```

Call the generated export from JavaScript:

```javascript
const thrown = {
  get message() { throw new Error("message lookup failed"); }
};
const input = {
  get boom() { throw thrown; }
};
const result = await (await app.field("boom"))(input);
```

Current behavior: the call throws/rejects with `message lookup failed` instead
of returning `#Error`. Expected: a recoverable error with a safe message and
the original observation's path/expected information. A thrown value whose
`toString` or `Symbol.toPrimitive` throws exposes the same formatting problem.

## Work

In [web-apis.js](../../../src/backend/web-apis.js), `$jsObserve` catches the
original observation, but `$jsFailure` invokes `$webMessage`, which performs
unguarded `error.message` lookup and `String(...)` coercion. Make formatting
total over arbitrary thrown host values, using a safe constant fallback when
inspection or coercion fails. Preserve useful ordinary error messages where
available. Check other users of the same formatter, including HTTP and URL
error conversion, so none assumes that formatting an unknown throw is pure
or infallible.

Coordinate with [strict host text ingress](16-strict-host-text-ingress.md):
error `message`, `path`, and `expected` strings must satisfy the scalar-text
contract. Invalid host error text must not leak into a Ruddy `String`; a safe
fallback must itself be valid scalar text.

The earlier revoked-proxy `kind` failure has already been fixed. Preserve its
`#Other` behavior; do not reopen it as unfinished work in this ticket.

## Acceptance

Extend [host failure tests](../../../tests/src/js_adapters.rs) with thrown
objects having a throwing `message` getter, throwing coercion, and a revoked
proxy; include malformed host message text in the coordinated ingress tests.
All observation operations retain recoverable failures, useful path metadata,
and valid scalar error text. A secondary formatting exception must never
escape. Run Rust tests only through `just test`.

## Answer

`$webMessage` in `src/backend/web-apis.js` reads the thrown value inside its
own `try`, so a `message` getter that throws, or a `Symbol.toPrimitive` that
refuses coercion, produces the constant "the host threw a value that cannot be
read" rather than a second exception escaping the boundary that promised a
recoverable error. The path and the expected description are unchanged, so a
caller still learns where the observation stopped.

The message is also brought into the scalar-text contract: whatever the host
wrote, each unpaired surrogate half becomes `U+FFFD`, so an error message is
always valid Ruddy text. Every user of the formatter gets this, including the
URL and HTTP error conversions, which share it.

Regression: `host_failures_arrive_as_errors_rather_than_exceptions` in
`tests/src/js_adapters.rs` throws a value whose `message` getter raises, one
whose coercion raises, and one whose message carries an unpaired half. Each
comes back as `#Error` with a readable path, and the last is repaired to
valid scalars with its length asserted. The revoked-proxy cases already there
still pass.
