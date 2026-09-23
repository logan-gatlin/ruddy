# Inferring branch-shaped structural contracts

Research date: 2026-09-21. These are precedents and design constraints, not a proof for Ruddy.

Scope update, 2026-09-22: the user requires automatic inference, permits
potentially nonterminating checking, and wants conditional fields and tagged
variants only. General untagged unions and input-shape-dependent ordinary
result types are excluded. The broader type systems researched below are
comparisons, not Ruddy feature commitments. The current implementation policy
is recorded in [the structural-contract specification](../structural-contracts/spec.md).

## Main finding

Automatic inference, branch-sensitive polymorphic contracts, and termination are compatible. The contract language and its checking rules must be deliberately restricted. Inferring useful contracts, inferring a principal contract relative to specified typing rules, and inferring every semantically valid relationship are different promises. The papers below demonstrate different tradeoffs; none establishes all the previously sketched `Extend4`, `ReadChannel`, `SetChannel`, `CopyChannel`, and `Swizzle` semantics for Ruddy unchanged.

## Direct precedent: polymorphic branch inference

[Castagna, Laurent, and Nguyen, *Polymorphic Type Inference for Dynamic Languages*, POPL 2024](https://arxiv.org/pdf/2311.10426), §1, §4.4, Table 1:

- Infers polymorphic intersections of function types from unannotated code, preserving input/output relationships across branches.
- Infers logical-or's “return the first argument when truthy, otherwise the second” relationship; it also recovers this relationship when the condition calls a separately inferred `toBoolean` helper.
- Theorems 4.2 and 4.3 establish soundness and termination. Reconstruction is explicitly incomplete.
- Their recursive `map`, constructed through a fixed-point combinator, gets an additional empty-list branch which imposes no function requirement on its first argument.
- The completeness boundary is concrete: every fixed-length preservation contract for `map` can be checked, but their finite, nondependent type language has no principal type subsuming all such contracts. Higher-order intersection expansion is another deliberately omitted inference capability.

This is strong evidence that branch contracts can be inferred compositionally; it does not prove that arbitrary “type programs” can be synthesized.

## Principal inference with an intentionally limited algebra

[Parreaux and Chau, *MLstruct: Principal Type Inference in a Boolean Algebra of Structural Types*, OOPSLA 2022](https://lptk.github.io/files/%5Bv6.2%5D%20mlstruct.pdf), §2.2, §3.4, §5.4:

- Combines polymorphism, record subtyping, tag matching, Boolean type connectives, and equi-recursive types.
- Theorems 5.5, 5.7, and 5.8 state soundness, termination of constraining, and completeness of inference in its own type system.
- Termination uses normalized constraints and cached comparisons; cyclic type bounds do not require infinite unfolding.
- Its nonstandard rules deliberately approximate record unions and function intersections. It therefore does not provide ordinary set-theoretic overloaded arrows with unrestricted input/output correlation. Its principality theorem cannot simply justify that interpretation of a match-shaped signature.
- The paper identifies display simplification as substantial work and retains cyclic/shared bounds rather than always expanding them.

[Parreaux, *The Simple Essence of Algebraic Subtyping*, ICFP 2020](https://infoscience.epfl.ch/server/api/core/bitstreams/afe084e0-0050-4542-99c7-c499d2fe1620/content), §5, supplies an approachable MLsub-style foundation: principal inference with records, functions, let-polymorphism, and recursive types; Lemma 4 establishes constraining termination. This foundation alone does not supply branch-overloading semantics.

## Termination does not ensure cheap checking

[Chau and Parreaux, *The Simple Essence of Boolean-Algebraic Subtyping*, POPL 2026](https://cse.hkust.edu.hk/~parreaux/publication/popl26/) gives a new subtyping decision procedure but still reports exponential worst-case complexity. Shared graphs, lazy expansion, and practical budgets would matter even for a mathematically terminating design. A timeout is an engineering safeguard, not a completeness or termination proof.

## Type-function precedent and its limits

[GHC User's Guide, Type families, §6.4.10.2.6](https://ghc.gitlab.haskell.org/ghc/doc/users_guide/exts/type_families.html#decidability-of-type-synonym-instances) specifies syntactic decrease and occurrence conditions sufficient for termination of inference involving type families. It explicitly distinguishes those conditions from completeness in the presence of cyclic equalities. Disabling them with `UndecidableInstances` transfers termination responsibility to the programmer.

[Scala 3's match-types reference](https://docs.scala-lang.org/scala3/reference/new-types/match-types.html#match-type-reduction) leaves a match unreduced when it cannot prove a case matches or is disjoint; matching cannot instantiate variables already present in the input constraint. Its [termination section](https://docs.scala-lang.org/scala3/reference/new-types/match-types.html#termination) explicitly acknowledges infinite recursive reductions and translates stack overflows into type errors. Scala demonstrates useful surface syntax and symbolic residual matches, not a total-inference guarantee suitable for Ruddy to inherit.

## Design implications for Ruddy (inferences from the precedents)

1. A finite match notation can be a presentation of existing guarded constraints. That does not add computational power, provided the representation is faithful; readability is still a separate problem.
2. Contracts must retain the conditional shape relationships needed for composition. Under the updated scope, ordinary payload constraints remain shared wherever branches combine into the same result position; contracts must not reinterpret those positions as independently polymorphic branch overloads.
3. A promising restricted contract algebra has finite tag/tuple cases, field projection/update, ordinary type variables, shared composition, and an explicit treatment of recursive summaries. Unknown matches can remain symbolic rather than triggering speculative reduction or general equation solving.
4. Recursive value functions should initially keep the existing recursion discipline and a finite summary; they need not become recursively executed type functions.
5. Guaranteeing termination would require the entire algorithm to have a finite-state or well-founded argument, including composition, equality/subsumption, and recursive summaries. That guarantee is no longer mandatory for Ruddy; the current bounded implementation remains a practical choice. Relaxing termination does not remove the requirement for automatic inference or justify dropping checking obligations.
6. Preserve automatic inference for the intended conditional-shape language, including the image examples. A checked representation may remain symbolic, and resource exhaustion must be reported honestly. Required annotations are not an acceptable substitute for the requested inference behavior. This does not promise recovery of the shortest equivalent contract or every semantic property of arbitrary functions.
