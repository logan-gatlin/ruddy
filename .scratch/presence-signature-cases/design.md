# Readable presence signatures: cases and templates

Design exploration, 2026-09-21. The proposed syntax below is not implemented. No compiler or `img` source was changed. The examples are candidate source annotations, not a separate display language.

The central recommendation is a hybrid: simplify Boolean relationships first, split a few mutually exclusive shapes into cases, and factor repeated independent choices into reusable templates. `extend4` benefits from cases; `swizzle` benefits from factoring.

Evidence: [captured signatures](current-signatures.md), [compiler probes](probes.md), [Boolean equivalence verifier](verify-extend4.py), and [primary-source prior art](prior-art.md). The source is [`img/src/main.rud`](../../../img/src/main.rud). Signatures were captured using existing CLI binaries from isolated copies with private visibility removed; `std = false` avoids unrelated dependency work. Compiler sources have existing uncommitted changes, so this is evidence about the available binaries, supported by inspection of the working-tree inference and printer regression tests, rather than a freshly rebuilt compiler validation.

## What cases must preserve

Ruddy currently stores a generalized structural type and Boolean presence formula, not arbitrary intersection or conditional whole types. See [`Scheme` and `Ty`](../../src/types.rs), [presence projection](../../src/inference/sat.rs), and [row documentation](../../docs/src/book/rows.md).

A sum presence means a tag is admitted by the static type. `#Red | #Green` admits both, even though a value has only one runtime tag. Both requirements must apply. A table that requires selecting one singleton overload loses useful existing callers.

There are also shared payload constraints. All named image fields `r`, `g`, `b`, and `a` share one payload type in these inferred functions, including an unselected field. Arbitrary extra fields in the row tail can have unrelated types. The identity branches of `try_get_set` still retain this skeleton. These restrictions must not disappear when printing cases.

## 1. Simplification available in the existing language

`extend4` currently prints 25 clauses. Keeping its original type body, its formula is exactly equivalent to:

```ruddy
where
  'slot3 <= 'slot2 <= 'slot1 <= 'slot0;
  'out_none0 = (not 'slot0 or 'in_none0);
  'out_none1 = (not 'slot1 or 'in_none1);
  'out_none2 = (not 'slot2 or 'in_none2);
  'out_none3 = (not 'slot3 or 'in_none3)
```

The variable names here are shortened from the captured signature. An output admits `#None` if the corresponding input slot is missing, or its input sum admits `#None`.

The verifier checks all 4,096 assignments to the twelve presences: both formulas admit exactly the same 80 assignments. An annotation using the five equations and the original type body also checks against the original definition. This is a real improvement before adding notation.

A further possible language extension is allowing Boolean expressions in presence markers: `#None (when (not 'slot0 or 'in_none0))`. Elaboration introduces a fresh presence and equates it to the expression. This removes the four named output presences, while retaining the input prefix constraint. It is smaller in scope than arbitrary conditional types.

## 2. Shape cases for `extend4`

The helpers below use existing alias syntax. `cases` is proposed source syntax:

```ruddy
type Item 'none 'row = #None (when 'none) | ..'row
type Pad 'row = #None | ..'row

let extend4: cases
| () ->
    (Pad 'a, Pad 'b, Pad 'c, Pad 'd)
| (Item 'p 'a,) ->
    (Item 'p 'a, Pad 'b, Pad 'c, Pad 'd)
| (Item 'p 'a, Item 'q 'b) ->
    (Item 'p 'a, Item 'q 'b, Pad 'c, Pad 'd)
| (Item 'p 'a, Item 'q 'b, Item 'r 'c) ->
    (Item 'p 'a, Item 'q 'b, Item 'r 'c, Pad 'd)
| (Item 'p 'a, Item 'q 'b, Item 'r 'c, Item 's 'd) ->
    (Item 'p 'a, Item 'q 'b, Item 'r 'c, Item 's 'd)
end
```

The same function must support all five families of calls. The blocks describe type instantiations, not runtime dispatch or an ordered overload search. Their semantics should retain the existing symbolic presence behavior under partial application, generalization, and higher-order use.

An initial implementation can require that a block reconstruct one existing presence scheme: align the common structural skeleton, retain payload and row identities, and translate shape alternatives into presence constraints. A block requiring incompatible scalar skeletons must fail with a clear diagnostic. Arbitrary `Nat -> Nat` and `String -> String` intersections are a separate feature.

`Item` is necessary for fidelity. `extend4 (1n,)` is rejected today because every input element unifies structurally with a padded sum. Even an arbitrary sum alias is too broad: `extend4 (#None 1n,)` is rejected because `#None` has unit payload. `Pad` preserves the inferred open output sums; replacing it with closed `#None` sharpens the type.

The case table follows from the verified equations: a present slot keeps exactly its input sum, and an absent slot yields an open sum containing `#None`. No implementation-path guessing is needed.

## 3. Templates for independent channel choices

Introduce a proposed `template` declaration with hygienic expansion into an ordinary annotation. It differs from a current `type` alias by permitting private inferred presence parameters that are fresh at each application.

Two useful source forms within that facility:

- `when <= 'r` creates a fresh presence `'selected` with the constraint `'selected -> 'r`. It means the case may be admitted only if the supplied capacity is present.
- `prefix (T0, T1, T2, T3)` means any closed tuple prefix of those four positions, including `()`. It creates four fresh slot presences with the usual prefix chain. It is a tuple type, not an array type or variadic runtime representation.

```ruddy
type Image 'r 'g 'b 'a 'value 'rest = {
  r when 'r: 'value,
  g when 'g: 'value,
  b when 'b: 'value,
  a when 'a: 'value,
  ..'rest
}

template ChoiceFor 'none 'r 'g 'b 'a =
  | #None (when 'none)
  | #Red   (when <= 'r)
  | #Green (when <= 'g)
  | #Blue  (when <= 'b)
  | #Alpha (when <= 'a)

template ChannelsFor 'r 'g 'b 'a = prefix (
  ChoiceFor _ 'r 'g 'b 'a,
  ChoiceFor _ 'r 'g 'b 'a,
  ChoiceFor _ 'r 'g 'b 'a,
  ChoiceFor _ 'r 'g 'b 'a
)
```

Each `ChoiceFor` occurrence independently chooses its admitted tags, subject to the same image field capacities. For example, red being available does not force every tuple element to admit red. Repeated use of a template shares only explicit arguments, not its private presences.

Generated signatures could then be:

```ruddy
let get_channel:
  ChoiceFor false 'r 'g 'b 'a
  -> Image 'r 'g 'b 'a 'v { ..'rest }
  -> 'v

let try_get_set:
  ChoiceFor true 'r 'g 'b 'a
  -> ChoiceFor true 'r 'g 'b 'a
  -> Image 'r 'g 'b 'a 'v { ..'rest }
  -> Image 'r 'g 'b 'a 'v { ..'rest }

let swizzle:
  ChannelsFor 'r 'g 'b 'a
  -> ChannelsFor 'r 'g 'b 'a
  -> Image 'r 'g 'b 'a 'v { ..'rest }
  -> Image 'r 'g 'b 'a 'v { ..'rest }
```

These are complete annotation fragments: attach the original implementation after `=`. The proposed declarations would be printed or placed in scope with the signatures.

The intended expansion exactly matches the captured relationships. For `get_channel`, no `#None` case is admitted. For `try_get_set`, `#None` is admitted in both selector arguments; every admitted color from either selector requires its image field. For `swizzle`, each optional tuple slot has an independent optional `#None` case and four bounded color cases. Conjoining individual implications `red_i -> r` is equivalent to the printed `(red_0 or ... or red_7) -> r`. The two prefix expansions give the two existing arity chains. The same image appears in argument and result.

This preserves current conservative behavior: `swizzle (#Red,) (#Green,) { r: 1n }` is rejected. Its inferred contract requires the destination field already to exist. It also does not claim a simultaneous permutation: each implementation step reads the image updated by the preceding step.

Current ordinary aliases reject anonymous presences and `_` arguments in their definitions; see [`declared_type_variables_require_explicit_scoped_parameters`](../../tests/src/inference.rs) and tuple-presence alias rejection tests nearby. Therefore this proposal needs a genuine template or implicit-parameter language feature. Replacing `template` with `type` in these examples would not work today.

A plausible implementation expands templates before ordinary annotation lowering, freshens internal variables, lifts generated constraints into the containing annotation, then runs the existing inference/generalization and ownership rules. It should not introduce existential runtime packages or polymorphic record fields. Start with acyclic templates. Parsing, diagnostics, formatting, editor grammar, export representation, and correct freshness are real implementation work even though the solver theory can remain unchanged.

The human names `Image`, `ChoiceFor`, and `ChannelsFor` are illustrative. A generator can reuse user-supplied named templates by verified structural matching, or emit neutral fresh helper names. It should not invent semantic names without evidence.

## 4. `set_channel`: simultaneous obligations matter

A useful red-specific specialization, using the ordinary `Image` alias, is:

```ruddy
let set_red:
  'v -> Image 'r 'g 'b 'a 'v { ..'rest }
  -> Image true 'g 'b 'a 'v { ..'rest }
  = set_channel #Red
```

This allows the red field to be added; other named fields and the tail are preserved. It still requires the old red payload and all present named color fields to have type `'v`, as current inference does.

The full function should retain simultaneous obligations for mixed selector types. In current source notation, its constraints can be stated as four guarded groups:

```ruddy
type Selection 'red 'green 'blue 'alpha =
  | #Red (when 'red) | #Green (when 'green)
  | #Blue (when 'blue) | #Alpha (when 'alpha)

let set_channel:
  Selection 'red 'green 'blue 'alpha -> 'v
  -> Image 'r 'g 'b 'a 'v { ..'rest }
  -> Image 'rr 'gg 'bb 'aa 'v { ..'rest }
where
  'red -> 'rr and ('gg = 'g) and ('bb = 'b) and ('aa = 'a);
  'green -> 'gg and ('rr = 'r) and ('bb = 'b) and ('aa = 'a);
  'blue -> 'bb and ('rr = 'r) and ('gg = 'g) and ('aa = 'a);
  'alpha -> 'aa and ('rr = 'r) and ('gg = 'g) and ('bb = 'b)
```

The six additional pairwise clauses in the current printer follow from these groups. For example, if both red and green are admitted, red forces output red present and green preserves input red into output red; therefore input red must already exist, and symmetrically input green must exist. The four-clause annotation and `set_red` annotation both checked against the existing CLI in `/tmp/ruddy-setter-design-x98a69bp`; exhaustive evaluation of all 4,096 assignments to the twelve setter presences also confirmed the pairwise clauses are redundant. A hypothetical named-type-position notation could make these groups read `channel.#Red -> output.r and output.g = input.g ...`, without globally generated variable names. This is another presentation option; it need not be implemented alongside cases/templates initially.

## Other designs and limits

| Design | Good fit | Main cost |
| --- | --- | --- |
| Boolean factoring and equations | All examples; immediate win for extend4 | Need bounded simplification and equivalence checks |
| Restricted case families | Few exclusive tuple or record shapes | Shared skeleton reconstruction and symbolic callers |
| Boolean expressions in presence markers | Derived output presences | Source grammar and fresh-equation elaboration |
| Templates with bounded presences and prefixes | Repeated channel requirements, especially swizzle | Freshness, constraint lifting, declaration semantics |
| Named type positions in constraints | Setter input/output relations | Stable names, paths, and binding rules |
| General intersections and conditional whole types | Truly unrelated scalar argument/result types | A substantial inference and annotation-system project |

Overload contracts themselves have established precedent in [CDuce](https://www.cduce.org/tutorial_overloading.html). [TypeScript's overload example](https://www.typescriptlang.org/docs/handbook/2/functions.html) illustrates why choosing a single overload can reject union-typed arguments. The interesting opportunity here is finding a compact, exact source representation of Ruddy's existing presence algebra, not claiming overload syntax is novel.

## Printer selection and acceptance criteria

1. Simplify constraints and eliminate derived variables within a budget; check Boolean equivalence before accepting a rewrite.
2. Find independent constraint factors and repeated structural fragments.
3. Propose a small number of useful shape splits, such as the five tuple arities. Leave unrelated presences symbolic.
4. Compare complete output cost, including helper declarations. Penalize duplicated structures and deeply nested choices, not just character count.
5. Emit cases only if their elaboration preserves the whole scheme. Keep a complete ordinary annotation fallback.

Validation must compare more than successful checking of the original function against the proposed annotation: a specialization may also check. Verify equivalence of normalized skeletons, payload and row identities, lacks constraints, presence formula, and quantifier/ownership information. Exercise absent and present fields, mixed selectors, symbolic arities, partial application, higher-order callbacks, effects, and the negative probes in the evidence file.

For swizzle, enumerating full paths multiplies two arity choices and many independent selector subsets. Factoring retains the independently quantified choices without printing their Cartesian product. Cases and templates should be competing source presentations selected for the type at hand, rather than forcing every type into one visual normal form.
