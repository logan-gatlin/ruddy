# Array Patterns, Array Spread, and an RRB Array Runtime

Status: ready-for-agent

## Problem Statement

Ruddy has immutable homogeneous arrays (`.scratch/array-types/spec.md`), but a
program can only take them apart through `array::get` and `array::len`. There is
no way to match on an array's shape, bind a prefix or suffix, or build one array
out of others. The parser reserves `[` in pattern position with a dedicated
"not yet" diagnostic, and `..` has no reading in expression position.

The current runtime is a fixed-width 32-way trie with a dense tail. It gives
O(1) length and O(log₃₂ n) `get`, `set`, and `push`, but has no way to drop a
prefix or join two arrays without rebuilding in O(n). Array patterns with a
rest binding and array spread both need those operations to be cheap, or the
natural recursive idioms become quadratic.

## Solution

Add array patterns and array spread to the surface language, and replace the
runtime trie with a relaxed-radix-balanced (RRB) tree so that slicing and
concatenation are O(log n).

```ruddy
let len = fn arr => match arr with
| [] => 0
| [_, ..rest] => 1 + (len rest)
end

let a = [1, 2, 3]
let b = [4, 5, 6]
let a_and_b = [..a, ..b]
```

An array pattern is a bracketed sequence of element patterns with at most one
rest, written `..` or `..name`, anywhere in the sequence. A spread is `..expr`
inside an array literal, any number of times, mixed freely with elements.

The standard library gains `concat`, `slice`, `prepend`, and `pop`, exposing
the same runtime helpers the compiler uses.

## User Stories

1. As a Ruddy programmer, I want to match an array against `[]`, `[x]`, and `[x, y]`, so that I can branch on its exact length and bind its elements.
2. As a Ruddy programmer, I want to write `[x, ..rest]`, so that I can bind the first element and the remainder in one pattern.
3. As a Ruddy programmer, I want to write `[..init, last]` and `[first, .., last]`, so that a rest can sit anywhere, not only at the end.
4. As a Ruddy programmer, I want to write `..` on its own to skip the remainder, so that I do not have to name what I discard.
5. As a Ruddy programmer, I want array element patterns to be any pattern, so that `[Some x, ..]` and `[[y, ..], ..]` work like every other nested pattern.
6. As a Ruddy programmer, I want a rest binding to be cheap, so that a recursive walk over an array is linear or better, not quadratic.
7. As a Ruddy programmer, I want an incomplete array match reported with a concrete missing shape, so that I can see which length I forgot.
8. As a Ruddy programmer, I want an unreachable array arm reported, so that a dead case is not left in my code silently.
9. As a Ruddy programmer, I want `let [..r] = arr` to be accepted and `let [x, ..r] = arr` to be rejected, so that a `let` never fails at runtime.
10. As a Ruddy programmer, I want to write `[..a, x, ..b]`, so that I can build one array from others without calling a library function.
11. As a Ruddy programmer, I want spreading a non-array rejected with a plain message, so that a mistaken operand is caught at compile time.
12. As a Ruddy programmer, I want `array::concat`, `array::slice`, `array::prepend`, and `array::pop`, so that the operations the compiler relies on are also mine to call.
13. As a Ruddy programmer, I want `concat` and `slice` to be O(log n), so that splitting and joining large arrays is practical.
14. As a Ruddy programmer, I want `get`, `set`, `push`, and `len` to keep their existing complexity, so that the runtime change is not a regression.
15. As a tooling author, I want the tree-sitter grammar to parse array patterns and spreads, so that highlighting and folding stay correct.
16. As a compiler maintainer, I want the three pattern matrices to agree on array patterns, so that checking and lowering never disagree about what is handled.

## Surface Syntax

- Array patterns: `[]`, `[p]`, `[p, q]`, `[p, q,]`. Elements are arbitrary patterns. A trailing comma is optional, matching literals.
- Rest: `..` or `..name`, allowed at any position, at most once. `[..]` and `[..r]` match every array. `[p, ..]`, `[.., p]`, and `[p, .., q]` are all valid.
- `.._` is a parse error. `..` on its own is the only way to discard the rest.
- Two rests in one array pattern is a parse error.
- Spread: `..expr` as an element of an array literal. Any number of spreads, at any position, mixed with plain elements. `[..a]` is a fresh array equal to `a`.
- `..` keeps its existing rejection in every other expression position. Inside `[` it is unambiguous.
- Array types are unchanged: `[T]`. Length is still not part of the type.

## Semantics

- An array pattern with no rest matches arrays of exactly its element count. An array pattern with a rest matches arrays of at least its fixed element count. Elements before the rest bind from the front; elements after the rest bind from the back; the rest binds the middle.
- A rest binding has type `[T]` where `T` is the column's element type.
- Spread operands must have the array type `[T]` where `T` is the literal's element type. There is no implicit conversion from lists, strings, or anything else.
- A spread literal evaluates its pieces left to right and produces a fresh array. Elements of the result are shared with the spread operands by structure but never by identity that the program can observe.

## Checking

- Irrefutability (rule R3 in `src/ir.rs`): an array pattern is irrefutable only when it consists of exactly one rest and no fixed elements, and the rest is `..` or a plain binding. Every other array pattern is refutable, because length is a runtime property.
- Exhaustiveness and reachability treat an array column as a constructor with variable arity, keyed by exact length for rest-free patterns and minimum length for rest patterns. This is the standard Maranget list-constructor specialisation; both `src/patterns.rs` and the inference column rule in `src/ir.rs` (`Matrix::handled` / `Matrix::open`) must implement it, and `src/lir.rs` must agree.
- Array columns do not participate in the presence SAT store. They qualify for the same path as tags and naturals.
- Witnesses for unhandled cases print as array patterns: `[]`, `[_]`, `[_, _, ..]`. The checker reports the shortest missing length, adding `..` when every longer length is also missing.
- Existing unreachable-arm reporting covers array arms with no new diagnostic kind.

## Lowering

- A match column of array type switches once on length, with "exactly n" cases for rest-free patterns and "at least n" cases for rest patterns, then projects fixed elements by index from the front or from the back, then slices the rest.
- The exact shape of the new lowering ops (length switch, element at index from front or back, slice) is left to implementation, but the decision-tree strategy above is the design.
- A spread literal lowers to a single runtime concat over its pieces, where each plain element is a one-element array. Consecutive plain elements may be grouped into one literal piece before concatenation.
- Lowering remains infallible and relies on the checker's proof of exhaustiveness.

## Runtime Representation

- Arrays use a private relaxed-radix-balanced tree following Bagwell and Rompf (2011): 32-way branching, cumulative size tables on relaxed nodes, and concatenation that rebalances to the "at most two extra search steps" invariant so that indexed access stays O(log n).
- Keep the dense tail of at most 32 elements for `push`, and keep the private transient builder for literals.
- The representation is not observable or guaranteed. Observable guarantees are immutability, structural sharing, and the performance contract below.
- Arrays remain unsupported in user-authored extern signatures. The new helpers join the existing intrinsic allowlist.

## Performance Contract

| operation | cost |
|---|---|
| `len` | O(1) |
| `get`, `set` | O(log n) |
| `push`, `prepend`, `pop` | O(log n), structural sharing |
| `slice` | O(log n) |
| `concat` | O(log n) |
| literal construction | O(n) |
| spread literal | O(k log n) for k pieces plus O(m) for m plain elements |
| rest binding in a pattern | O(log n) |
| array pattern match | O(k log n) for k fixed elements |

## Standard API

All functions are array-first and use `Nat` indices. `#None` is returned for an
out-of-range index, matching `get` and `set`.

```ruddy
array::concat  : ['a] -> ['a] -> ['a]
array::slice   : ['a] -> Nat -> Nat -> Option ['a]
array::prepend : ['a] -> 'a -> ['a]
array::pop     : ['a] -> Option ('a, ['a])
```

- `slice arr start end` returns elements `start` up to but excluding `end`, like `str::slice`. It is `#None` when `start > end` or `end > len arr`. `slice arr i i` is `#Some []`.
- `pop arr` returns the last element and the array without it, or `#None` on empty.
- `concat` is total.

## Diagnostics

All new messages are plain English per the project's diagnostic rule.

- Two rests: "an array pattern can only have one `..`".
- `.._`: "write `..` on its own to skip the rest".
- Spread of a non-array: "only an array can be spread into an array".
- Refutable `let` on an array pattern reuses the existing refutable-binding diagnostic.
- The reserved `ArrayPattern` parse error is removed.

## Implementation Decisions

- `..` is reused for rest and spread inside brackets. No new token.
- The array pattern is a new surface `PatternKind` and a new IR pattern carrying fixed prefix, optional rest, and fixed suffix.
- Spread is a new array literal element kind; `TermKind::Array` becomes a sequence of pieces, each an element or a spread.
- The inference column rule demands `[T]` for the column and `T` for every element pattern and rest-free element; a rest binds at `[T]`.
- The runtime helpers for slice and concat are shared between the standard library intrinsics and the lowering of patterns and spread.
- `push`, `set`, `get`, and `len` keep their observable behaviour. Their implementations change only as needed for relaxed nodes.
- Tree-sitter gains `array_pattern` and a `spread` element inside `array_expression`. The existing `rest_pattern` node is reused with an optional name. Highlights, folds, locals, and corpus are updated; `just grammar` must pass.
- Debugger printers render array patterns and spreads in bracket syntax in the AST, IR, and LIR views.
- README and the canonical source printer round-trip the new syntax.

## Testing Decisions

- Parser tests: every valid rest position, bare `..`, trailing commas, nested element patterns, spread positions, and the three new parse errors. Canonical printer round-trips.
- Inference tests: rest binds at `[T]`, element patterns constrain `T`, spread of a non-array is a type error, `[]` with spread stays generalised.
- Pattern tests in `tests/src/patterns.rs`: exhaustive and non-exhaustive array matches, witness shapes `[]`, `[_]`, `[_, ..]`, `[_, _, ..]`, unreachable arms after `[..]`, nested array-in-array and tag-in-array cases, and mixed columns with tuples and structs.
- Irrefutability tests: `let [..r]`, `let [..]`, and `let [[..], ..]` are rejected or accepted according to the rule; `let [x]` and `let [x, ..r]` are rejected.
- Execution tests in `tests/bundles`: the `len` example, `[..init, last]`, `[first, .., last]`, spread literals with zero, one, and many spreads, and the four new standard functions.
- Runtime differential test in `tests/bundles/runtime`: a seeded pseudo-random sequence of `push`, `set`, `get`, `slice`, `concat`, `prepend`, and `pop` checked against a plain JavaScript array, with sizes crossing the 32 and 1024 boundaries and repeated concatenation of unbalanced pieces to exercise rebalancing. The seed is fixed.
- UI tests pin the new diagnostic prose and spans.
- Tree-sitter corpus gains cases for every syntax form above.
- 100% coverage per `just cov` for the compiler crate. All Rust tests run only through `just test`.

## Out of Scope

- Named struct rest `{x, ..r}`. It stays a parse error and remains deferred.
- Spread into tuples, structs, or function calls.
- Spread of strings or lists into arrays, or any implicit conversion.
- Length-indexed array types or any static knowledge of length.
- Or-patterns, guards, and `as` bindings.
- The Stucki et al. (2015) display/focus optimisation for sequential access.
- Arrays in user-authored extern signatures.
- Computed-index syntax, mapping, folding, equality, and conversions.

## Further Notes

- The array-types spec is left as history. Its representation section and its
  "spread and patterns are not included" line are superseded by this document.
- `[..a, ..b]` is only ever expression syntax. As a pattern it would be
  ambiguous and is rejected by the one-rest rule.
- A rest at the front or middle is what forces the runtime choice. A window
  offset over the old trie would have made a trailing rest O(1) but left
  concatenation at O(n); the RRB makes both O(log n) and was chosen for that
  reason.
