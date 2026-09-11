# Introspection implementation handoff

Reviewed baseline: `cd906cfae71235dc6df2fdfbe16737c64b330546` (`cd906cf`).
Every confirmed defect this handoff listed is fixed as of `a643494`, each with
a regression; what remains is the tracked spec work in the second table.

Implement the remaining requirements of [the agreed spec](spec.md). There is
substantial working implementation already: `hide`, mirrors and typed views,
library codecs, JSON and positional binary, JS adapters, ABI plans, and the
independent interpreter. The spec's original implementation-pending wording
predates that work. Tickets 01–11 describe completed milestones, not proof
that every spec acceptance criterion passes.

## Verified baseline

The review that produced this handoff reported **1,875 passed, 0 failed, 10
ignored**. After the fixes the suite reports **1,882 passed, 0 failed, 10
ignored** through `just test`, with `just fmt-check` and `just clippy` clean.

Earlier fixes to preserve: ABI fixtures use the current `Target` API;
Real-to-Int rounding and decimal Int parsing normalize integer zero; negative
nonzero Real underflow is rejected; effect-row order does not change mirror
identity; hidden arrow results retain the parentheses needed to keep effects
outside `hide`; revoked-proxy classification no longer throws. Debugger integer
settings survive load/save/cache/session paths, and their validation is now
shared with the command line.

Targeted review probes reproduced defects beyond the passing suite. Each
ticket keeps the source that recreated its case, beside the answer describing
what the behavior is now.

For probes using `std`, reuse the temporary project/build helpers in
[stdlib API tests](../../tests/src/stdlib_apis.rs) or
[JS adapter tests](../../tests/src/js_adapters.rs): they create a JS library
with a path dependency on this checkout and execute the generated module in
Node. Tickets 18, 19, and 25 also provide standalone manifests without `std`.

## Fix order and ownership

Every confirmed defect below is fixed at `a643494`. Each ticket records the
behavior it now has and the regression that holds it there.

| Order | Priority | Ticket | Outcome |
| --- | --- | --- | --- |
| 1 | P1 | [18 — Mirror and package identity](issues/18-authenticate-mirror-and-package-types.md) | Fixed. A handle is held against the type the position reads it under, at ingress, on both backends; sealing records the type a value was sealed at. |
| 2 | P1 | [19 — Recursive hidden descriptors](issues/19-terminate-recursive-hidden-descriptors.md) | Fixed. Re-entering a binder drops the occurrence it shadows, so the back edge closes; a node limit turns any remaining runaway into a diagnostic. |
| 3 | P1 | [20 — Debugger target validation](issues/20-validate-debugger-integer-targets.md) | Fixed. `Build::resolve` is the one validated construction; a refused configuration compiles nothing. |
| 4 | P1 | [21 — Host lifting effects](issues/21-make-host-lifting-effects-explicit.md) | Fixed. `js::lift` carries the observation effect; `js::read` is pure and accepts only a snapshot or a host primitive. |
| 5 | P1 | [22 — Scalar string replacement](issues/22-preserve-scalar-string-replacement.md) | Fixed. One literal, non-overlapping, scalar-boundary contract, the same on both backends. |
| 6 | P1 | [16 — Strict host text](issues/16-strict-host-text-ingress.md) | Fixed. Ingress refuses text that is not scalars; `js::text` is the explicit repair. |
| 7 | P2 | [14 — Hidden region safety](issues/14-regions-and-cells-in-descriptions.md) | Safety half fixed: packaging a value that carries a region is refused. The descriptive half stays open below. |
| 8 | P2 | [23 — Codec limits](issues/23-enforce-codec-resource-limits.md) | Fixed. Byte and member budgets are counted before the material is built, on both writers and both readers, in UTF-8 bytes. |
| 9 | P2 | [24 — Sequence completion](issues/24-check-declared-sequence-lengths.md) | Fixed. A frame keeps the declared length beside the completed count, in both handlers. |
| 10 | P2 | [25 — Intrinsic signatures](issues/25-validate-reflection-intrinsic-payloads.md) | Fixed. Recognition checks the whole resolved signature of `$describe` and `$shape`. |
| 11 | P2 | [26 — Observation errors](issues/26-contain-host-observation-failures.md) | Fixed. Describing a thrown value is total, and its text satisfies the scalar contract. |

Found and fixed while working through these, outside the list: the host
export adapter was compiled with the default integer domains, so a bundle
binding another could not link its own exports.

## Remaining spec work already tracked

These are open implementation requirements, not regressions. They are what is
left of the spec after the defects above.

| Ticket | Remaining work and scope |
| --- | --- |
| [12 — Streaming](issues/12-streaming-codecs.md) | Incremental sources/sinks, declared I/O effects, nested session isolation, owned output, and failure ending a streaming session. Include propagation of custom codec effects; current `Encoder`/`Decoder` types admit only `!Write`/`!Read`. |
| [13 — Conditional presence](issues/13-conditional-presence-in-mirrors.md) | Presence evidence in descriptions and calling conventions, joint builder checks, and injectors available only when case admission is proved. |
| [14 — Regions and cells](issues/14-regions-and-cells-in-descriptions.md) | Descriptive half only: safe opaque descriptions for cells and regions, and preservation of transitive dependencies. Conservative rejection of unsafe packaging is done. |
| [15 — Codec policies and schemas](issues/15-codec-combinators-and-schema-evolution.md) | Projection/validation/record/sum/rename/default/version combinators, source-schema decoding, cycle rejection, schema/canonical work limits, and typed field lookup. Complete the shared wire vocabulary for bytes, maps, tuples, and extensions. |
| [17 — Artifact identity and binding](issues/17-symbolic-artifacts-and-host-identities.md) | Symbolic target primitives bound once, incompatible specialization rejection, and alpha-equivalent hidden type identity across bundles. Distinct native/Wasm host identities belong with those future integrations; their production runtimes are not required now. |

[16](issues/16-strict-host-text-ingress.md) is closed: the checked-Unicode
defect is fixed for payloads, keys, and metadata, and the lossy conversion is
explicit.

The shared protocol currently cannot directly express several of the explicit
wire policies promised by the spec: there are no byte-string/map/extension
operations, or a JSON-null operation suitable for a nullable codec. A handwritten
format-specific bypass is not a replacement for reusable portable codec
behavior. The follow-up comments in 12 and 15 make these interface gaps explicit.

Closed-policy codec caching is an optional optimization. Correctness,
termination, bounded work, and the separation of custom policy environments
remain requirements whether or not caching is implemented. Do not make a cache
or production native/Wasm integration a prerequisite for closing unrelated
correctness tickets.

## Preserve the agreed design

- `hide` is globally reserved. Introduction is contextual, opening is explicit
  in match arms, and authentic mirrors explicitly bound by patterns supply
  evidence equally for user and compiler packages. Preserve abstraction and
  exact scoped identity.
- Every admitted `Mirror` has complete identity. `reflect::same` and `Any`
  downcasts return `Option`; unsupported evidence is not inequality. Opaque
  shapes and readable descriptions do not authorize casts or construction.
- `Nat` and `Int` have target-selected fixed precision **in bits**, with
  signedness and exact bounds separate. JS may use Number over its 53-bit safe
  domains. Fixed-width types retain their own identity and bounds.
- Invalid ordinary numeric operations/conversions remain undefined, with no
  new `Result` signatures. Checked codec/FFI boundaries must still reject bad
  external data. Preserve saturating/wrapping operations that already have
  defined contracts and the single integer zero.
- `Real` remains binary64. Default decimal decoding is correctly rounded;
  strict JSON rejects overflow, non-finite values, and nonzero underflow to
  zero, while preserving Real negative zero. Integer parsing has no floating
  intermediate; document JSON preserves number token spelling and duplicates.
- String operations use Unicode scalars; storage layout must not change their
  behavior. Keep strict ingress distinct from explicit lossy conversion.
- Keep derivation an ordinary recoverably fallible library operation, not a
  compile-time serializer/instance constraint. Keep format policies explicit,
  Any reconstruction registry-controlled, and exact casts distinct from
  conversion.
- Preserve the [selected foreign trust policy](../region-mutability/ffi.md).
  Fix checked data boundaries and effect signatures without introducing
  mandatory lifetime/thread guards or callback revocation. Production native
  and Wasm integrations remain later work.
- None of these APIs has shipped. No migration documentation or compatibility
  wrappers are required for their experimental forms. Future wire-schema
  evolution is still part of the codec design.

## Validation and completion

Use [AGENTS.md](../../AGENTS.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md).
All Rust tests run **only through `just test`**, including focused tests. Never
invoke `cargo test` directly. Tests belong in `tests/`; compiler changes need
the corresponding debugger support, and grammar changes need Tree-sitter and
highlight updates.

Add a meaningful regression for each ticket. Compare the same source programs
on JS and the independent interpreter where the behavior is portable, and use
real JS boundary tests for host-specific effects and failures. Assert only
defined numeric behavior. Keep recursive-descriptor regressions bounded: the
unfixed reproduction previously exhausted a 500 MiB memory cap.

Run focused tests while working, then the required repository checks. Compiler
coverage is governed by `just cov`; a passing normal suite does not establish
that requirement. Formatting, link checking, or parity on ordinary inputs alone
does not close a type-safety or malformed-input ticket.

When a ticket is fixed, record its concrete behavior and validation in that
ticket, update its status, and keep this handoff's remaining-work index current.
For accepted conservative limitations, state exactly which cases are rejected.
Do not mark the whole spec complete while any required acceptance behavior
above remains open.
