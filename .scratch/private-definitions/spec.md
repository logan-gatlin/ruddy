# Bundle-private definitions

## Agreed contract

- A top-level `let`, `extern`, `type`, `effect`, or `module` marked `@private` is accessible throughout its declaring bundle but its name is not exported to dependent bundles.
- A private module hides all descendants, for both inline and file-backed modules. Unmarked declarations remain public unless inside a private module. A pattern binding applies privacy to every bound name.
- The marker accepts only unit, including the equivalent `@private ()` and empty struct spelling. Other payloads produce a diagnostic.
- Public values, types, and effects may alias private definitions. Explicit and inferred public signatures may reference private types and effects, including effect operation signatures. Structural type/effect information must remain usable across artifact round trips and dependencies without exposing private names to source lookup.
- Private implementation code remains available to execute public definitions and initialization; it is omitted from JavaScript exports.
- An executable's root `main` must be public. A library's `main` remains an ordinary definition.

## Validation and review

Confirmed test boundaries: public compiler/artifact APIs for producer/consumer bundles, and CLI/JavaScript behavior for exports and executable entry validation. Rust tests run only through `just test`.

Review baseline: `6415db4c43ae083feb10ab76828697faf6c6d00b`.

## Implementation decisions

- Preserve private type/effect declarations as semantic support in artifact headers, with an explicit export flag distinct from source metadata. Dependency admission loads their semantics but excludes their names from source resolution. This supports recursive aliases and structural effects without forcing lossy expansion or changing the written metadata of descendants.
- Omit private values and modules from header exports; retain their implementation in LIR. Existing JavaScript export construction and executable entry lookup then enforce the same boundary.
- Compute visibility from the declaration and all enclosing modules at artifact construction. Source lookup inside the declaring bundle is unchanged.
