# Prior art and semantic boundaries for presence case signatures

Research date: 2026-09-21. This is a design note, not implemented syntax or a claim of global novelty.

## Main finding

Printing one function as several correlated argument/result contracts is established. The opportunity specific to Ruddy is generating those contracts from a presence-polymorphic scheme, making the notation writable, and ensuring that the presentation preserves the scheme's accepted calls and correlations. A readable collection of examples is easier than an equivalent annotation.

## Sources and useful distinctions

### CDuce: several contracts for one function

CDuce permits a function interface `(t1 -> s1; ...; tn -> sn)`. Its meaning is conjunctive: the function satisfies every arrow, corresponding to an intersection of function types. The tutorial distinguishes contracts describing separate branches from contracts expressing different behavior of shared code. It explicitly uses multiple arrows to retain input/output correlations that a single union-domain/union-result arrow loses. This directly supports the user's intuition about `extend4`, while showing that overload-like signatures themselves are not new. [CDuce overloading tutorial](https://www.cduce.org/tutorial_overloading.html)

The manual explains arrows as accepting at least their domain and returning their codomain when they terminate. Its example gives `(Int -> Int) & (Char -> Char)` as a subtype of `(Int | Char) -> (Int | Char)`. Thus accepting mixed-domain callers is part of the semantic-subtyping design, not an automatic consequence of drawing several arrows. [CDuce types and patterns](https://www.cduce.org/manual_types_patterns.html)

**Ruddy design implication:** choose explicitly between a new first-class intersection type system and a surface form that elaborates into existing presence schemes. The second is a smaller initial project. Calling the surface syntax `cases` avoids implying that arbitrary intersections already work, but its semantics still need definition.

### TypeScript: an instructive failure mode and an alternative

TypeScript's handbook shows that two overloads, one accepting a string and another accepting an array, do not accept an argument typed as string-or-array: ordinary call resolution must select one overload. This is an important counterexample to the assumption that a list of readable singleton cases automatically retains all existing callers. [TypeScript: more on functions](https://www.typescriptlang.org/docs/handbook/2/functions.html)

Conditional types compact repeated overload choices into a named type-level mapping. They can distribute over a union when their checked input is a bare type parameter; other spellings suppress that distribution. The same documentation says inference from an overloaded function uses its last signature. These are choices Ruddy should avoid inheriting accidentally. A presence-conditioned result could be useful, but arbitrary type-level conditionals introduce a separate inference and reduction problem. [TypeScript conditional types](https://www.typescriptlang.org/docs/handbook/2/conditional-types.html)

**Ruddy design implication:** favor conditions on existing Boolean presences over unconstrained tests on arbitrary whole types. Define behavior on symbolic conditions and multi-case sums, including callbacks, before choosing the compact syntax.

### MLsub / Simple-sub: simplification is separate from cases

MLsub's primary claim includes compact principal types obtained through simplification connected to regular-language algebra. It supplies useful precedent for preserving generality while changing presentation, not a ready-made answer to Ruddy's row-presence presentation. [Dolan and Mycroft, repository abstract](https://www.repository.cam.ac.uk/items/b7009a90-9bc6-4538-a388-90a2d5617de0)

Simple-sub's author explains simplification using variable co-occurrence analysis and shared structure. The same account stresses that MLsub's input/output polarity restrictions prevent freely using arbitrary union and intersection annotations. It also gives an example where repeated inline bounds are longer than a shared explicit constraint. This supports a hybrid printer: eliminate redundant variables, share repeated structure, then split only where splitting helps. It does not support promising that adding `&` to Ruddy would be a minor printer change. [Parreaux, Demystifying MLsub](https://lptk.github.io/programming/2020/03/26/demystifying-mlsub.html)

### Boolean representations: compact internals do not guarantee compact case lists

Bryant's foundational work represents Boolean functions as reduced graphs and notes exponential worst cases even though many practical functions have manageable representations. [Bryant, Graph-Based Algorithms for Boolean Function Manipulation, institutional record](https://doi.org/10.1184/r1/6605990)

**Ruddy design implication:** do not enumerate all satisfying assignments merely because the constraint solver can represent them. For `n` independent present/absent fields, a complete assignment table has `2^n` rows by direct counting. Group assignments into partial cases, retain residual constraints, and share repeated result structure. A printer should have a size budget and a readable fallback.

## Consequences for `../img`

The following deductions use [`src/main.rud`](../../../img/src/main.rud), Ruddy's [row chapter](../../docs/src/book/rows.md), and [grammar](../../docs/src/grammar.md). They describe contracts and pitfalls; they are not claims that each displayed contract is today's exact principal inferred type.

### `extend4`: the favorable case

The source has five closed tuple-shape branches. An arity table is immediately understandable:

```text
()                 -> (#None, #None, #None, #None)
('a,)              -> ('a,   #None, #None, #None)
('a, 'b)           -> ('a,   'b,    #None, #None)
('a, 'b, 'c)       -> ('a,   'b,    'c,    #None)
('a, 'b, 'c, 'd)   -> ('a,   'b,    'c,    'd)
```

These are behavioral specializations. To become a generated replacement annotation, the language must preserve any currently inferred open sum tails and correlations within the tuple slots. In particular, replacing an inferred `#None | ..'r` with closed `#None` may sharpen the result; it is not mere reformatting. The presence-aware compiler can continue to represent the arity choice symbolically, rather than introducing overload selection on every call.

### `get_channel`: singleton paths do not cover the whole type story

Four useful rows are `#Red -> { r: 'a, .. } -> 'a`, and the analogous green, blue, and alpha rows. But a selector statically typed `#Red | #Green` is also useful. As the row chapter explains, this requires both `r` and `g` with a common result-compatible payload type.

A sum-case presence means “this tag is admitted by the static type,” not “this runtime value currently carries this tag.” Both presences are true for a two-case sum. A table of singleton choices therefore needs a specified rule for multi-case inputs; a rule that exactly one row applies is insufficient.

There is a second trap: a shared result type can already force payload equalities in the current scheme. Splitting the table and independently generalizing every branch might suggest heterogeneous field support that inference does not provide today. Preserve global variable identity unless the implementation deliberately gains guarded payload equalities.

### `set_channel`: row preservation and replacement must remain visible

Each setter path preserves the rest of the input object. A red-specific row can be explained as “accept an image with optional old `r`; return the same remaining fields and a required new `r`.” If the annotation uses a shared row tail, that tail must exclude explicitly mentioned `r`. If a general `set_channel` scheme shares payload variables across branches, allowing each path its own old/new field types could again promise more than current inference.

### `try_get_set` and `swizzle`: splitting needs a budget

`try_get_set` has two identity paths (`get` is `#None`, or `set` is `#None`) and a copy path. The identity paths overlap at `(#None, #None)`; an unordered contract collection and an ordered decision tree must not silently get different meanings. The copy path combines read requirements with a row update.

`swizzle` composes four such updates. A whole-function overload expansion duplicates combinations across input tuple length, output tuple length, and the possible source/target tags in each position. Better candidates are a shared intermediate relation, a sequential row-transform signature, or a partially split signature retaining symbolic presences. Calling it a simultaneous field permutation would be incorrect: each later copy observes the image produced by earlier copies.

## Proposed evaluation criteria

1. **Round trip:** parse a printed signature, check the original definition against it, and compare semantic acceptance/generalization with the unannotated scheme.
2. **Symbolic callers:** accept callers whose presences are unresolved, and selectors whose sum types admit multiple tags.
3. **Shared identities:** preserve type variables, row tails, effects, quantifier scope, and residual constraints across displayed cases.
4. **Overlap:** define whether all applicable contracts hold or whether cases form a disjoint ordered partition. Do not borrow runtime pattern precedence accidentally.
5. **Bounded output:** prefer a few informative splits over total enumeration; retain a writable formula when splitting increases size.
6. **Stable output:** alpha-renaming or irrelevant implementation changes should not reorder or radically restructure the printed contract.

The most promising initial design is a constrained presence-case presentation, with explicit shared binders and a rule for elaborating back into the existing scheme. Full intersection types and arbitrary conditional types are valuable comparison points, but substantially larger language changes.
