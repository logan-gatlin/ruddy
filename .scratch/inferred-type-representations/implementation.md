# Implementation work in progress

Status: in-progress

Worktree: `/var/home/dev/code/hc/ruddy-inferred-type-representations`
Branch: `inferred-type-representations`
Base/spec commit: `42f6558`

The spec is not implemented yet. Compiler changes are uncommitted and must not
be presented as a finished feature. The current scheme-factory convention has
an observable semantic bug. Keep the new failing execution tests until the
underlying callable architecture is corrected; do not weaken their assertions.

## Existing work

- Compiler-known `Any` and `JsValue`, intrinsic declarations in `std::any` and
  `std::js`, distinct lowered descriptor/boxed/host representations.
- Inferred scheme-level descriptor demand sets and forwarding through generic
  bindings, published interfaces, hover/debug output, and artifacts.
- Lowered descriptor, reflection, and conversion operations, with CPS/JS support.
- Authentic opaque Any packages, structural graph equality, native array
  snapshots, structural conversion, checked JsValue decoding, Promise/callback
  adapter integration, and concrete JS root interface checking.
- Behavioral tests for generic boxing/externs, recursive aliases and calls,
  independent parameters, rows, imports, opaque transport, decoding, root
  exports, and standard-library modules.

These pieces need integration with the corrected callable representation below.
The passing tests do not establish that the complete feature is sound.

## Required callable rewrite

`src/lir/lower.rs::reified_value` currently moves the entire initializer into a
factory called at each instantiation. A computation preceding a lambda, or the
argument of a generalized partial application, consequently executes repeatedly
instead of once. Unused generalized initializers can disappear entirely.

Both execution regressions now compile and fail for the actual semantic reason:

- `reification_generalized_initializers_preserve_eager_allocation_identity`
- `reification_generalized_partial_application_evaluates_its_argument_once`

They allocate an opaque Any token during initialization and compare tokens
captured by two instantiations through an ordinary JS identity comparison. Both
comparisons must be true; both are currently false.

Requirements currently exist only as binding/scheme parameter sets. `Ty::Arrow`
has no latent requirement information, so every curried argument descriptor is
required when its outer binding is referenced. This violates the spec. A local
hoisting special case or rejecting generalized partial applications would not
implement the promised behavior.

A semantic sidecar is a possible implementation organization, but it needs to
be part of every semantic value occurrence and scheme, not a map from symbols
to arrow depths. It must distinguish evaluation requirements, latent call
requirements, and captured evidence. Its finite type-shape graph needs arrow
requirements, argument/result shapes, aggregate members, recursive edges, and
shape variables for ordinary type variables. Erased identity must preserve the
shape of a callable (or aggregate of callables) through its shared input/output
type variable without receiving descriptors itself.

A second pass after ordinary inference must retain the actual binder and scheme
instantiation links. Merely matching equal solved type trees does not recover
all independent higher-order demand variables. Higher-order `apply` must
quantify and propagate its callback's demands. Generalization must preserve
ordinary value restriction and presence ownership; recursive groups need a
finite joint constraint solution.

Lowering can reuse `lambda`, `indirect`, known calls, `held`/`holding`,
`fits`/`fitted`, capture threading, and curry wrappers. Initializer evaluation
must stay in `define`/`Let`; only lambda invocation receives hidden descriptor
parameters. Fit callable conventions at higher-order and aggregate boundaries.

Generic native adapters must also defer demands needed only by returned
functions. Obtaining the raw result of `extern make: () -> ('a -> 'a)` does not
yet require a descriptor for `'a`; invoking that result does. Boxed callable
payloads need a canonical convention (for example, capturing supplied evidence
when boxing) so an exact successful downcast cannot expose a different hidden
argument layout.

Publish the corrected metadata in schemes/artifacts and interface invalidation,
and replace the current artifact initializer-factory-layout validator.

## Review fixes made

The standards/spec reviews also found finite-synthesis and descriptor-validation
issues. These have received focused fixes:

- Runtime descriptor synthesis now uses the existing IR `RegularType` graph
  builder's argument interning, forwarding-alias normalization, and structural
  growth detection, with packages retained for identity-policy inspection.
- Native parameter discovery traverses the finite graph. Requirement
  substitution memoizes structural alias identities and stops malformed growing
  graph paths instead of repeatedly expanding finite syntax.
- Artifact validation follows Alias and Extend dependencies to reject unguarded
  cycles, primitive/mixed-kind extensions, and duplicate explicit row fields.
- JS normalizes concrete Extend templates even when there are no parameters.
- Added public artifact/import and generated-execution tests for these cases.

Remaining review concerns include the separate native graph builder in
`src/backend/host.rs` and completeness of conversion/evidence validation. The
new shared graph helper currently lives in `src/ir.rs`; consider extracting the
common finite graph module when integrating native policy.

## Compatibility failures to resolve

The first full `just test` run reported 1662 passed, 49 failed, 9 ignored in the
main test crate. Its log is `/tmp/ruddy-reification-full.log`. That result predates
some fixes and the new regression tests; it is not a current clean-suite result.

Failures include:

- Native shape rejection added to raw inference rejects existing hidden/optional
  presence package tests. Preserve ownership and existing supported ABI contracts;
  do not simply relabel these tests as obsolete. A native conversion shape and an
  exact structural identity descriptor have distinct policy needs.
- Existing generic JS root fixtures require deliberate concrete interfaces or
  JsValue annotations where that change is intended by the spec. Do not default
  their types automatically.
- Native sums/arrays change host-facing test expectations that currently inspect
  private symbols/persistent array storage. Update only intentional ABI changes.
- Existing callable/effect adapter, primitive/large-Nat, thenable-data, runtime,
  CLI, debug/snapshot and standard-library regressions need investigation.
- Two new primitives require updating the primitive spelling/count assertion.

Some old tests were already adjusted in the working diff. Review those edits
against the original test intent; do not use fixture changes to conceal a
regression. The presence-package JS fixture changes deserve particular review.

## Validation and next actions

Use `CARGO_TARGET_DIR=/var/home/dev/code/hc/ruddy/target` to reuse build artifacts.
All Rust tests must run via `just test`, never directly via Cargo.

Latest completed checks during this work:

- `cargo check --workspace --all-targets`: passed.
- `just test reification_`: 14 passed, the two eager-initialization tests failed.
- `just test reification_concrete_artifact`: passed (added after that focused run).
- The expanded malformed descriptor artifact test passed.
- Imported forwarding-recursion/growing-alias descriptor compilation test passed.

`cargo fmt --all` and `git diff --check` passed. `just test ir::` matches both
IR and LIR test names: 405 passed and the same two known LIR regressions failed
(`callback_evidence_joins_definite_then_conditional_occurrences` and
`existential_packages_are_transparent_to_container_lowering`). The affected IR
graph tests passed, including the finite rotation longer than 256 states and
small-stack imported-type tests. See `/tmp/ruddy-ir-test.log`.

After the callable rewrite and compatibility fixes: rerun the relevant tests,
full `just test`, required formatting/type/lint checks and `just cov`, repeat the
spec/standards review, then commit the completed implementation on this branch.
No implementation commit has been made yet.
