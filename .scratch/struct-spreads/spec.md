# Struct Literal Spreads

Status: ready-for-agent

## Problem Statement

Ruddy programmers cannot currently construct a struct by reusing the fields of
another struct. Updating one field or extending a struct therefore requires
writing every retained field again, even when the program already has the
complete value it wants to use as a base. That repetition is noisy, couples the
construction to fields it does not otherwise care about, and prevents concise
row-polymorphic helpers that preserve caller-chosen fields.

The language already uses `..` for array spreads, struct-pattern rests, and row
tails, but a `..` inside a value-level struct literal has no spread meaning.

## Solution

Add one optional spread to a value-level struct literal. A programmer can write
`{ a: 1, b: 2, ..c }` to construct a struct containing every field of `c`, with
the explicitly written `a` and `b` values replacing fields of the same name.

The spread must be the final item and a literal may contain at most one. Its
operand is a full expression that must have, or be inferable as having, a struct
type. Open struct rows are preserved, making a helper such as a field updater
retain fields chosen by its caller. Explicit replacements may change a field's
type and do not constrain the type of the discarded value.

Struct construction continues to evaluate source expressions once from left to
right. The result contains the complete union of the spread operand's fields and
the explicit fields; checking the result against an expected type never silently
drops fields.

## User Stories

1. As a Ruddy programmer, I want to spread an existing struct into a literal, so that I can reuse all of its fields without naming each one.
2. As a Ruddy programmer, I want to add fields while spreading a struct, so that I can extend an existing value concisely.
3. As a Ruddy programmer, I want explicitly written fields to replace fields with the same name from the spread operand, so that I can express immutable updates.
4. As a Ruddy programmer, I want replacement precedence to be independent of runtime field layout, so that an explicit field reliably wins.
5. As a Ruddy programmer, I want a replacement field to have a different type from the old field, so that an update can transform a struct's shape.
6. As a Ruddy programmer, I want the discarded field's old type not to constrain its replacement, so that unrelated old and new types do not produce a mismatch.
7. As a Ruddy programmer, I want a generic updater to accept a struct whether the replaced field is present or absent, so that one function works for both updates and extensions.
8. As a Ruddy programmer, I want a generic updater to preserve caller-chosen fields in an open row, so that abstraction does not erase structural information.
9. As a Ruddy programmer, I want spreading an unconstrained parameter to infer that it must be a struct, so that I do not need a redundant annotation.
10. As a Ruddy programmer, I want a known non-struct spread operand rejected at compile time, so that invalid construction cannot reach runtime.
11. As a Ruddy programmer, I want a spread operand to be any full expression, so that I can spread a function result without introducing a temporary binding.
12. As a Ruddy programmer, I want the spread expression evaluated exactly once, so that spreading an effectful computation does not duplicate its effects.
13. As a Ruddy programmer, I want explicit field expressions and the spread operand evaluated from left to right, so that effects follow visible source order.
14. As a Ruddy programmer, I want an overridden stored value omitted without being separately read or converted, so that discarded data has no behavior beyond evaluating the spread operand itself.
15. As a Ruddy programmer, I want `{ ..c }` to make a structurally identical value, so that copying does not require a special case.
16. As a Ruddy programmer, I want the empty struct and unit to be valid spread operands, so that the rule applies uniformly to every structural value.
17. As a Ruddy programmer, I want tuples to be valid spread operands because they are represented as structural records, so that semantic struct behavior does not depend on surface spelling.
18. As a Ruddy programmer, I want numeric explicit fields to replace tuple-backed fields, so that structural operations remain uniform for canonical tuple shapes.
19. As a Ruddy programmer, I want the resulting semantic type to print canonically as a tuple when its final closed fields form a tuple shape, so that existing type presentation remains consistent.
20. As a Ruddy programmer, I want a trailing comma after the final spread to be accepted, so that struct literals retain their existing trailing-comma convention.
21. As a Ruddy programmer, I want only one spread in a struct literal, so that inference remains predictable.
22. As a Ruddy programmer, I want the spread required at the end, so that the literal has one clear reading and evaluation order is visible.
23. As a Ruddy programmer, I want two spreads diagnosed specifically as a multiple-spread error, so that I know which restriction I violated.
24. As a Ruddy programmer, I want an explicit field after a spread diagnosed as a misplaced-spread error, so that I know to move the spread to the end.
25. As a Ruddy programmer, I want duplicate explicit fields to remain errors, so that allowing spread collisions does not hide likely mistakes in written fields.
26. As a Ruddy programmer, I want every non-overridden spread field retained even when an expected closed type omits it, so that type checking never acts as an implicit projection.
27. As a Ruddy programmer, I want an omitted spread field reported as an extra field when a closed expected type rejects it, so that the result's actual shape remains honest.
28. As a Ruddy programmer, I want quoted and bare labels compared by their decoded field identity, so that collision behavior matches all other struct operations.
29. As a Ruddy programmer, I want the language to treat struct fields as semantically unordered, so that spread does not create a new field-order guarantee.
30. As a Ruddy programmer, I want evaluation order preserved without exposing storage order, so that effects are predictable while representation remains private.
31. As a Ruddy programmer, I want compiler and debugger views to retain the written spread operation, so that inspecting a program does not pretend inferred fields were explicitly authored.
32. As a Ruddy programmer, I want printed spread literals to parse back to the same structure, so that canonical printing remains stable.
33. As a tooling user, I want editor parsing and highlighting to recognize struct spreads, so that source tooling agrees with the compiler.
34. As a compiler maintainer, I want the surface tree, normalized IR, inference, lowering, artifacts, and JavaScript backend to agree on spread semantics, so that no compiler stage invents a different result.
35. As a compiler maintainer, I want invalid spread diagnostics to identify the spread and relevant competing item, so that errors remain source-oriented and actionable.
36. As a compiler maintainer, I want value-level spreads distinguished from struct-pattern rests and type-row tails, so that this feature does not alter those established meanings of `..`.

## Implementation Decisions

- A value-level struct literal gains one optional spread entry alongside its explicitly written fields. The spread is represented distinctly through the surface tree and normalized IR rather than being expanded into statically known fields during parsing.
- The spread entry is syntactically last. An optional trailing comma remains valid after it.
- The spread operand uses the full expression grammar, matching the operand treatment of array spreads.
- A struct literal accepts at most one spread. A second spread receives a focused diagnostic even though the first spread has already violated the final-position rule.
- A non-spread field following a spread receives a focused diagnostic that instructs the programmer to place the spread last.
- Explicitly written fields always take precedence over fields of the same decoded name in the spread operand. This precedence does not depend on source storage order or backend object order.
- Duplicate explicitly written fields continue to use the existing duplicate-field behavior. Spread collisions are the sole exception to duplicate rejection.
- Inference constrains an otherwise unknown spread operand to a struct. It reports the established non-struct failure only when the operand cannot be a struct.
- The spread operand may carry a closed or open struct row. Its row remainder is preserved in the result.
- For each explicit field, inference removes or shadows the corresponding possibility from the operand row and installs the explicit field as definitely present with its newly inferred type.
- A colliding operand field may be absent or present with any type. Its presence and type do not constrain the explicit replacement.
- The result type contains all non-overridden operand fields, all fields in the operand's open remainder, and all explicit fields. A contextual expected type may check this result but may not project fields away.
- Unit and tuples qualify as spread operands because the semantic type system represents them as structs. Existing canonical tuple recognition continues to apply to final closed result types.
- Field identity follows the compiler's existing decoded-label rules, including quoted and numeric labels.
- Evaluation is source ordered: every explicit field expression is evaluated once in written order, followed by the final spread expression once.
- Construction excludes colliding fields from the spread value and inserts the explicit values. It does not separately evaluate, read, or convert discarded field values.
- Struct field storage and display order remain outside the language contract. The implementation may choose a layout independently of expression evaluation order.
- Lowering must support copying or merging the operand's complete runtime field set, including fields represented only by an open row at the spread site. It then produces the result with explicit replacements.
- The artifact and backend representations must preserve enough information to perform the merge without making field order observable.
- Compiler and debugger printers render the spread after explicit fields and add grouping around its operand when required for an unambiguous round trip. They do not expand the spread into inferred fields.
- The tree-sitter grammar recognizes the same final optional spread and trailing comma as the compiler grammar. Highlighting treats `..` consistently with other rest and spread syntax.
- Existing meanings of `..` in struct types, struct patterns, sum rows, effect rows, and arrays are unchanged.

## Testing Decisions

- Prefer external behavior over internal representation. Tests should assert accepted source, inferred public schemes, emitted diagnostics, canonical printed source, and executed JavaScript results rather than particular AST fields, constraint sequences, lowering instruction counts, or object layouts.
- The primary seam is one checked-in end-to-end bundle compiled through the normal CLI path and executed with Node. This covers parsing through backend execution with the same path a user invokes.
- The end-to-end bundle covers a plain copy, extension with a new field, replacement of an existing field, replacement with a different type, several explicit replacements, a full-expression operand, unit, tuple-backed structs, and access to every retained result field.
- The end-to-end bundle proves left-to-right, exactly-once evaluation with observable effects or another existing source-level sequencing mechanism. It separately proves that explicit values win despite being evaluated before the spread operand.
- The public compilation seam tests inferred schemes for a closed operand, an open-row generic updater, an operand where the replacement is absent, and an operand where the replacement has an unrelated old type.
- The public compilation seam verifies that an unconstrained operand is inferred as a struct and that its caller-chosen row remainder survives in the result.
- Rejected compilation tests cover a known non-struct operand, a closed expected result missing a retained spread field, duplicate explicit fields, multiple spreads, and explicit fields after a spread.
- Structured diagnostic tests pin stable error codes, primary spans, related spans where two items are involved, concise titles, and actionable help. Diagnostic prose remains in the project's user-facing vocabulary rather than exposing row-solver implementation terms.
- Public parser tests cover a spread-only literal, explicit fields plus a spread, a full-expression operand, nested struct spreads, quoted and numeric labels, and optional trailing commas.
- Canonical printer tests exercise both surface and normalized trees, operands that require grouping, and fixed-point reparsing. They verify that spread syntax remains visible rather than expanding into inferred fields.
- Semantic type-printing tests verify that a closed numeric result canonicalizes as a tuple where the existing tuple rules require it, without requiring the expression printer to erase the spread syntax.
- Tree-sitter corpus tests mirror every accepted and rejected surface form, and highlighting tests cover the `..` marker. This is a separate tooling seam because the editor grammar does not execute through the compiler.
- Artifact validation and JavaScript backend coverage exercise spread through the end-to-end bundle. Focused lower-level tests are added only for invariants that cannot be observed at that seam, such as malformed unchecked artifacts.
- Prior art is the repository's array-spread bundle for full compilation and execution, public compilation tests for inferred schemes and errors, printer fixed-point tests for surface/IR agreement, structured diagnostic audits, and tree-sitter corpus tests for grammar changes.
- All Rust tests are run through `just test`. Grammar generation and corpus tests are run through `just grammar`.

## Out of Scope

- More than one spread in a single struct literal.
- A spread placed before or between explicit struct fields.
- Order-dependent precedence between fields and spreads.
- Replacing an explicit field with a later explicit field.
- Silently dropping spread fields to satisfy a closed expected type.
- Any language-level promise about struct storage order, display order, JavaScript property order, memory layout, or ABI order.
- Changes to struct-pattern rest syntax or semantics.
- Changes to struct type rows, sum rows, effect rows, or array spread semantics.
- A new mutation operation; construction produces a new immutable value and does not modify the operand.
- Implicit conversion from arrays, sums, primitives, functions, or foreign values into structs.
- Performance guarantees for struct spreading beyond evaluating each source expression once.

## Further Notes

- The single-spread restriction is a deliberate inference boundary. Supporting multiple spreads would require a separate design for overlapping unknown rows and precedence between them.
- “Explicit fields win” is semantic precedence, not evaluation precedence. Because the spread is last, explicit expressions evaluate first and the spread expression evaluates last, while construction still retains the explicit values on collisions.
- Open-row replacement is the central inference case: the operand's field may exist with an unrelated type or may be absent, but the result field is definitely present with the explicit value's type.
- Struct fields are identified by decoded labels. Surface spelling differences do not create distinct fields.
- The repository currently has no applicable domain glossary or ADR that changes this design.
