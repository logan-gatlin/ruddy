# Polymorphism with mutable cells

Status: effect-less-initializer restriction selected by the user.

Selected rule: generalize eligible variables only when evaluating a binding's
initializer is proven effect-less. Otherwise retain shared unknowns. Existing
environment/level restrictions still apply, and an unknown/open immediate effect
row is insufficient evidence of purity. A function's latent effects do not
count as effects of constructing it. Covariance-based relaxation and additional
state/effect dependency analysis are deferred.

`mut value` is proposed allocation syntax. Examples belong inside function
bodies when their evaluation performs mutation; effectful global initialization
is still forbidden.

## The failure to prevent

```text
let cell = mut (fn x => x)
let _ = cell := (fn n => nat::add n 1n)
let result = (~cell) true
```

This must fail: one cell cannot be instantiated independently at `Nat -> Nat`
for the write and `Boolean -> Boolean` for the read. Its initial element-type
unknown is shared and becomes fixed by uses. The allocation itself may initially
be unconstrained; this does not mean the cell must have an annotation or be
rejected immediately. It means uses share unknowns rather than instantiate
fresh ones.

An immutable binding to a mutable cell is still a reference to shared state.
Checking only whether the source binding uses `let` is not enough. Likewise,
checking only whether the result type is visibly `mut` misses closures that
capture cells and expose the shared type variable in their signatures.

## Factories are different

```text
let make = fn x => mut x
let numbers = make 0n
let flags = make true
```

`make` can have `'a -> mut 'r 'a + !mut 'r`: evaluating the lambda creates no
cell. Its calls create different cells and may independently choose element
types while sharing the enclosing region. The bindings `numbers` and `flags`
denote already allocated cells and must retain their own fixed types.

More generally, normal environment/level restrictions still apply. Merely
writing a lambda around access to an existing cell does not allow generalizing
the cell's region or shared element-type variables.

## Three policies to compare

1. **Syntactic value restriction.** Generalize only designated value forms
   such as lambdas, subject to existing environment restrictions. Allocations
   and calls do not qualify. This is simple but unnecessarily restricts calls
   known to be pure.
2. **Effect-based restriction.** Generalize eligible variables only when
   evaluating the initializer is proven effect-less. A lambda qualifies even
   if calling it will mutate. Calls of pure helpers qualify. Allocations and
   calls to mutable factories do not. Unknown/open immediate effects are not
   evidence of purity. This is the selected policy.
3. **A relaxed restriction using variance.** Permit some variables to generalize
   even in effectful initializers when their positions establish that sharing
   cannot lead to incompatible writes. This can recover useful polymorphism in
   immutable results but requires additional soundness work across Ruddy's
   structural rows, presence constraints, aliases, and higher-order types.
   Cell element types remain invariant.

For policy 2, purity of the initializer is separate from the latent effects in
its result type. Effect-less initializers still cannot generalize variables
owned by their environment. A pure helper whose mutation was legitimately
isolated at its function body can produce generalizable results. Purity of an
entire enclosing function cannot retroactively justify polymorphic mutable
bindings inside it.

The current compiler unconditionally generalizes local bindings after solving
their constraints; new policy needs to retain weak/shared variables at the
appropriate enclosing level so a later alias cannot generalize them. Presence
existentials remain producer-owned rather than becoming caller-chosen through
this change. [Binding solver](../../src/inference/solve.rs),
[existing generalization tests](../../tests/src/inference.rs)

The Koka calculus offers a primary-source example of effect-based let
generalization, requiring an empty initializer effect. It is precedent rather
than a proof for Ruddy's different rows and presence system.
[Leijen, Koka, section 2.5 and generalization rules](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/paper-20.pdf)

## Deferred: inferring safe generalization despite effects

This should be a per-variable decision. The proof obligation is that separate
instantiations cannot exchange incompatible values through shared state or
other effect-mediated communication. The selected initial policy does not
attempt this relaxation for effectful initializers.

One bounded extension is a relaxed restriction: after the existing scope and
dependency exclusions, permit ordinary type variables occurring only
covariantly in the result. For example, an effectful initializer returning an
immutable empty array can retain a polymorphic element type. A record can have
an invariant cell field whose variable remains shared and an independent
immutable field whose covariant variable generalizes. `mut r a` must make `a`
invariant even if its runtime representation is hidden.

This is the principle behind OCaml's relaxed value restriction. Applying it to
Ruddy requires checking effect arguments, row tails, presence formulas,
existential ownership, and aliases; the rule for ordinary type variables must
not automatically be applied to region or presence variables.
[OCaml manual, relaxed value restriction](https://ocaml.org/manual/4.14/polymorphism.html#s%3Arelaxed-value-restriction)

Ruddy already computes declared-type variance in `declaration_variances` and
`semantic_variances`, including recursive/imported aliases, and uses it for
presence classification. That provides reusable infrastructure, not an already
implemented relaxed generalization rule. New work would collect occurrences
of candidate inference variables, account for mutable invariance, and filter
the variables generalized while retaining the rest as shared unknowns.
[IR variance](../../src/ir.rs),
[semantic variance and generalization](../../src/inference/mod.rs)

Variance alone does not recognize every harmless effect. Consider:

```text
let id = do
  let _ = std::console::print "created"
  return fn x => x
end
```

The result type is `a -> a`, where `a` has both polarities. A variance-only
relaxation therefore cannot generalize it. A more precise rule can recognize
that the returned lambda captures no shared state involving `a`, while the
console operation does not exchange values of that unknown type. This requires
expression/dependency analysis or a suitably justified effect-specific rule,
not merely a variance walk of the result.

At opaque calls the same reasoning needs a trusted or verified summary; do not
inline all producers or decide from an effect's name. Higher-order callbacks
and handlers can connect result variables with existing state. Track the type
dependencies of allocation and captured mutable state where needed. In
particular, `a` being absent from `!mut r` proves nothing: that effect carries
only the region even when the result is `mut r a`.

The initial implementation is limited to the effect-less rule. Covariant
ordinary-variable generalization and more precise state/effect dependency
summaries remain possible future extensions.
