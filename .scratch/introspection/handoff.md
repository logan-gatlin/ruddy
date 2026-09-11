# Introspection implementation handoff

Reviewed baseline: `cd906cfae71235dc6df2fdfbe16737c64b330546` (`cd906cf`).
The working tree was clean at the end of the review. This handoff adds issue
documentation only.

Implement the remaining requirements of [the agreed spec](spec.md) and fix the
confirmed defects below. There is substantial working implementation already:
`hide`, mirrors and typed views, library codecs, JSON and positional binary,
JS adapters, ABI plans, and the independent interpreter. The spec's original
implementation-pending wording predates that work. Tickets 01–11 describe
completed milestones, not proof that every spec acceptance criterion passes.

## Verified baseline

The latest review ran `just test -q` successfully. The main `ruddy-tests` crate
reported **1,875 passed, 0 failed, 10 ignored**; the workspace command completed
successfully. Compiler coverage was not measured in that review.

Confirmed fixes to preserve: ABI fixtures use the current `Target` API;
Real-to-Int rounding and decimal Int parsing normalize integer zero; negative
nonzero Real underflow is rejected; effect-row order does not change mirror
identity; hidden arrow results retain the parentheses needed to keep effects
outside `hide`; revoked-proxy classification no longer throws. Debugger integer
settings now survive load/save/cache/session paths, but their validation still
has the defect in ticket 20.

Targeted review probes reproduced defects beyond the passing suite. Each new
ticket includes enough source/input to recreate its case. Do not rely on the
reviewer's temporary `/tmp` projects being present on another machine.

For probes using `std`, reuse the temporary project/build helpers in
[stdlib API tests](../../tests/src/stdlib_apis.rs) or
[JS adapter tests](../../tests/src/js_adapters.rs): they create a JS library
with a path dependency on this checkout and execute the generated module in
Node. Tickets 18, 19, and 25 also provide standalone manifests without `std`.

## Fix order and ownership

P1 denotes a type/effect/representation invariant violation, nontermination,
or silent miscompilation. P2 denotes other confirmed correctness defects.
Each row links to the implementation instructions and acceptance checks.

| Order | Priority | Ticket | Required outcome |
| --- | --- | --- | --- |
| 1 | P1 | [18 — Mirror and package identity](issues/18-authenticate-mirror-and-package-types.md) | Checked conversion cannot change the type of an authentic mirror or hidden package, on either backend. |
| 2 | P1 | [19 — Recursive hidden descriptors](issues/19-terminate-recursive-hidden-descriptors.md) | Recursive types crossing `hide` produce a finite graph or a supported diagnostic, never unbounded expansion. |
| 3 | P1 | [20 — Debugger target validation](issues/20-validate-debugger-integer-targets.md) | Debugger compilation rejects JS with 64-bit target integers, matching the CLI. |
| 4 | P1 | [21 — Host lifting effects](issues/21-make-host-lifting-effects-explicit.md) | Arbitrary getter/proxy observation is effectful; pure lifting accepts only justified inert input. |
| 5 | P1 | [22 — Scalar string replacement](issues/22-preserve-scalar-string-replacement.md) | Replacement preserves scalar validity and has identical defined behavior on JS and the interpreter. |
| 6 | P1 | [16 — Strict host text](issues/16-strict-host-text-ingress.md) | Reject malformed host strings, keys, and metadata at checked ingress. |
| 7 | P2 | [14 — Hidden region safety](issues/14-regions-and-cells-in-descriptions.md) | Reject packaging that drops untracked region/presence dependencies; complete the descriptive work separately. |
| 8 | P2 | [23 — Codec limits](issues/23-enforce-codec-resource-limits.md) | Enforce advertised input/output/member/work limits before allocation. |
| 9 | P2 | [24 — Sequence completion](issues/24-check-declared-sequence-lengths.md) | A writer cannot report success after emitting fewer or more elements than it declared. |
| 10 | P2 | [25 — Intrinsic signatures](issues/25-validate-reflection-intrinsic-payloads.md) | Validate complete reflection result types, not just field/case names. |
| 11 | P2 | [26 — Observation errors](issues/26-contain-host-observation-failures.md) | Even hostile thrown values become structured host-observation errors. |

Work can proceed in parallel: compiler/evidence work (18, 19, 25 and the safety
part of 14), host/text work (21, 22, 16, 26), codec work (23, 24), and debugger
validation (20). Coordinate 18 with exact descriptor equality and scope rules;
coordinate 21/16/26 at every host boundary. Address these defects before
expanding the codec surface.

Two especially useful end-to-end regressions are:

- A correctly typed `ffi::encode`/`ffi::decode` round trip must not turn
  `Mirror Nat` into `Mirror String` and thereby supply a `Nat -> String`
  identity cast. The same check is needed for hidden packages; authentic does
  not imply authentic for the requested type.
- In the debugger, selecting `target: "js", integers: 64` currently lets
  `9007199254740993n` match the literal `9007199254740992n` without diagnostics.
  The emitted runtime bounds are clamped to 53 bits, but the two internal
  literals still collapse to the same Number. Validate the build configuration
  before compilation; clamping an admitted domain does not implement it.

## Remaining spec work already tracked

These are open implementation requirements, not new regressions introduced by
the latest fix commit. Keep them visible after the immediate defects are fixed.

| Ticket | Remaining work and scope |
| --- | --- |
| [12 — Streaming](issues/12-streaming-codecs.md) | Incremental sources/sinks, declared I/O effects, nested session isolation, owned output, and failure ending a streaming session. Include propagation of custom codec effects; current `Encoder`/`Decoder` types admit only `!Write`/`!Read`. |
| [13 — Conditional presence](issues/13-conditional-presence-in-mirrors.md) | Presence evidence in descriptions and calling conventions, joint builder checks, and injectors available only when case admission is proved. |
| [14 — Regions and cells](issues/14-regions-and-cells-in-descriptions.md) | Safe opaque descriptions for cells/regions and preservation of transitive dependencies. Conservative rejection of unsafe packaging is an immediate requirement; full scoped reflection must not precede its dependency tracking. |
| [15 — Codec policies and schemas](issues/15-codec-combinators-and-schema-evolution.md) | Projection/validation/record/sum/rename/default/version combinators, source-schema decoding, cycle rejection, schema/canonical work limits, and typed field lookup. Complete the shared wire vocabulary for bytes, maps, tuples, and extensions. |
| [16 — Text ingress](issues/16-strict-host-text-ingress.md) | The immediate checked-Unicode defect, including keys and metadata, and explicit opt-in lossy conversion. |
| [17 — Artifact identity and binding](issues/17-symbolic-artifacts-and-host-identities.md) | Symbolic target primitives bound once, incompatible specialization rejection, and alpha-equivalent hidden type identity across bundles. Distinct native/Wasm host identities belong with those future integrations; their production runtimes are not required now. |

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
