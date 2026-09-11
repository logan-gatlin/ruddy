# Formal verification options for Ruddy

Investigated 2026-09-11 against checkout `f6fd295`. This is a research note, not an accepted design or an implementation. No proofs, implementation changes, or tests were run. Proposed priorities below are engineering judgments based on the repository and the cited primary sources.

## What would be verified?

Formal verification establishes a precisely stated property of a mathematical model or implementation, for every case within the statement's scope, using mathematical reasoning checked by a tool. The theorem must identify its assumptions. Writing formal syntax and evaluation rules is **formalization**; proving properties of those rules is **verification**. A model proof does not automatically cover separately written Rust code.

Different claims require different proofs:

| Target | Example claim | What it achieves |
| --- | --- | --- |
| Language rules | A closed, well-typed program cannot reach a specified kind of stuck state | Checks that the language's typing rules agree with its execution rules |
| Inference/checking algorithm | An accepted program satisfies the declarative typing rules | Connects the implementation's acceptance decisions to the language model |
| Individual algorithm | Formula simplification preserves its Boolean meaning | Removes a specific class of compiler mistakes |
| Compiler pass | Lowering preserves the specified observable behavior | Rules out miscompilations in that pass |
| User program | This function returns a sorted permutation of its input | Establishes an application-specific property, a separate task from verifying Ruddy |

A common type-safety proof establishes **progress** (a well-typed closed term is a value or can step) and **preservation** (a step preserves typing). Their combination excludes stuck states in the modeled language. It does not imply termination: a program may keep stepping forever. Effects and other observable exits require a correspondingly richer statement. [Software Foundations: STLC properties](https://softwarefoundations.cis.upenn.edu/plf-current/StlcProp.html)

Checker **soundness** means acceptance implies a valid typing derivation. **Completeness** means typable terms are accepted, under the theorem's stated restrictions. These are distinct from the language's type-safety theorem. A checker that rejects every program is sound but useless. [Software Foundations: a typechecker](https://softwarefoundations.cis.upenn.edu/plf-current/Typechecking.html)

For compilation, define the behavior of both source and target, then prove a suitable preservation/refinement relation. Results alone are insufficient when effects, divergence, or failures matter. CompCert supplies a practical example: it composes correctness proofs for separate passes and explicitly identifies boundaries outside the verified pipeline. Even a substantial verified compiler has assumptions about surrounding tools. [CompCert manual, sections 1.2–1.3](https://compcert.org/man/manual001.html)

## Best first target: presence formulas

Ruddy documents Hindley–Milner unification and `let` generalization, plus propositional constraints on row-label presence. Its types are equated rather than ordered by subtyping. The natural first proof is therefore about presence formulas, not a new semantic-subtyping system. [Inference overview](../../src/inference/mod.rs), [formula definitions and evaluation](../../src/types.rs)

The smallest useful sequence is:

1. Define Boolean meaning independently for `True`, `False`, atoms, `Not`, `And`, `Or`, `Iff`, and `Xor`; `Owned` is logically transparent metadata. Prove the smart constructors/simplifications preserve this meaning.
2. Specify the Tseitin encoder: a valuation satisfies the input exactly when it extends to a satisfying assignment of the generated clauses **with the root literal asserted**. For model extraction and incremental queries, strengthen this to preserve assignments of the original atoms and handle guard/assumption behavior.
3. Establish the contracts used by `model` and `entails`. Correct encoding alone does not prove BatSat correct: initially state its SAT/model answers as an explicit assumption. Verifying the solver or checking independently verifiable certificates would be additional work.
4. Prove successful projection computes existential elimination, then show its minimization/rebuilding stages preserve that result. [Current SAT integration and projection](../../src/inference/sat.rs)

The concrete projection theorem should be:

```text
project(F, keep, budget) = Ok(G)
  implies, for every valuation k of the kept atoms:
    eval(G, k) = true
      iff there exists an assignment d of the dropped atoms
          such that eval(F, k combined with d) = true.
```

Also show that `G` mentions only kept atoms. This is a proposed specification, not an established theorem. It captures the practical requirement that generalization must neither invent nor discard legal combinations of visible fields/cases when it forgets internal presence variables. For example, projecting `a = b` onto `a` should allow either value of `a`; projecting `(a = b) and b` onto `a` should require `a`.

`project` can return `TermLimitExceeded`; the default product-term budget is 256. Its conditional correctness theorem must permit that explicit error. Do not promise an absolutely smallest formula: the implementation documents exact minimization only within a minterm limit and a less expensive fallback above it. [Projection budgets and minimization](../../src/inference/sat.rs)

This project is small enough to teach the workflow while addressing Ruddy-specific behavior. A proof of a copied mathematical implementation still leaves a Rust correspondence obligation. Ways to close it include verifying the production function directly, proving its refinement of the model, or using a proved checker to validate results. Differential/property tests help find discrepancies but do not discharge that obligation.

## A route toward language-wide guarantees

Build a small executable calculus containing constants, functions/application, `let`, closed records, sums, and matching. Define its terms, values, evaluation, and typing independently of implementation shortcuts; establish type safety. Connect an executable checker to those typing rules. Then extend deliberately through rows/presences, effects, and region/state features, revisiting the theorems at every extension. This is a suggested order, not a claim that these extensions are routine.

Inference termination is a particularly relevant later theorem because it is an explicit repository requirement. Document the decreasing measures for unification/occurs checking, alias traversal, constraint generation, and projection, including assumptions about SAT termination. Termination means finite accepted/rejected/error outcomes; it is distinct from accepting every typable program and from termination of programs written in Ruddy. The contribution guide's separate aspiration that inference be “total” needs a precise interpretation before choosing a theorem. [Contributor requirements](../../CONTRIBUTING.md), [inference staging argument](../../src/inference/mod.rs)

A particularly valuable later theorem would connect Ruddy's effect annotations to observable behavior: under trusted primitive/foreign contracts, effect-free evaluation cannot access externally observable mutable state, and isolated state cannot escape through a usable returned value or closure. This requires a state/effect or noninterference argument beyond ordinary type safety, with careful treatment of polymorphism and hidden packages. The region spec explicitly limits generalization for effectful initializers and treats foreign declarations/callers as trusted. These are proposed verification targets, not properties proved by this investigation. [Region rules and FFI assumptions](../region-mutability/spec.md), [solver](../../src/inference/solve.rs)

For compiler correctness, an eventual target could relate source-core evaluation to linked LIR evaluation, followed by LIR to JavaScript behavior. The existing reference interpreter runs the same lowered, linked LIR used by the JavaScript backend. Their agreement can reveal backend/runtime discrepancies but cannot reveal a lowering error shared by both paths; the interpreter also rejects asynchronous foreign completion. An independently specified source evaluator would address the shared-lowering gap in testing; formal simulation would prove the relationship. [Reference interpreter](../../interp/src/machine.rs), [LIR](../../src/lir.rs), [JavaScript backend](../../src/backend/js.rs)

## Tools and a first learning milestone

**Rocq (formerly Coq)** is a direct learning route: start with Software Foundations' [Logical Foundations](https://softwarefoundations.cis.upenn.edu/lf-current/index.html), then its [Programming Language Foundations](https://softwarefoundations.cis.upenn.edu/plf-current/index.html) treatment of evaluation, typing, and checkers. Implement Ruddy's small formula language and one preservation theorem after learning structural induction. **Lean** is a reasonable alternative for the same mathematical-model project. In either system, audit proof placeholders and assumptions; Lean's documentation explains how axioms affect trust and how `#print axioms` reports dependencies. [Lean axioms reference](https://lean-lang.org/doc/reference/latest/Axioms/)

**Kani** is worth a separate, small feasibility experiment on pure existing Rust formula/cube routines. It can reason over all inputs represented by a proof harness; practical loop/input bounds must remain explicit, and unwinding checks must succeed. A proof about formulas up to a chosen size is not a theorem about arbitrary formulas. Its experimental loop contracts can support unbounded reasoning, but this is additional proof work. Do not assume that the entire `Arc`/collection/BatSat integration will verify without adaptation. [Kani bounds tutorial](https://model-checking.github.io/kani/tutorial-loop-unwinding.html), [loop contracts](https://model-checking.github.io/kani/reference/experimental/loop-contracts.html)

**Verus** is an option if direct Rust functional verification becomes the priority: it uses specifications, proof annotations, and SMT verification. It supports a subset of Rust/libraries and does not verify its own verifier or the Rust/LLVM compiler. It need not be introduced alongside the first theorem-prover project. [Verus overview](https://verus-lang.github.io/verus/guide/)

The first meaningful deliverable would be a checked simplification or encoding theorem, an explicit map to the corresponding Rust function, and a short list of remaining assumptions. The payoff is a precise contract that survives refactors and exposes design mistakes. A whole-language claim ultimately requires those contracts to compose; it would still cover the named properties and model boundaries, not every conceivable defect or user-program requirement.
