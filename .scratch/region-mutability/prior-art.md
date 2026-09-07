# Region-based mutability: prior art

Investigation notes, 2026-09-06. This is not an approved language specification. Primary sources were read on that date; the 2014 Koka calculus and current language documentation are distinguished below.

## Haskell ST: isolate observable state using a quantified identity

`runST :: (forall s. ST s a) -> a` requires the supplied computation to work for an arbitrary state identity `s`, while the result `a` is outside that quantifier. `ST` operations retain the identity; different `runST` invocations cannot share usable state through the safe API. This is a static abstraction boundary, not a promise that the implementation allocates a physical arena. The documented guarantee is that internal state is inaccessible outside the computation. [GHC library documentation](https://downloads.haskell.org/~ghc/8.4.2/docs/html/libraries/base-4.11.1.0/Control-Monad-ST-Safe.html)

Design inference: an intrinsic region binder can provide the analogous fresh, rigid identity without making general rank-2 annotations part of Ruddy's surface language. Checking only whether the returned value is literally a reference is insufficient: a returned function that reads it carries the dependency in its computation type. Conversely, an existentially hidden identity with no usable operation is not the same as an observable escaping cell; do not overstate `runST` as prohibiting every inert hidden reference.

## Koka: heap effects and lexical variables are different mechanisms

Current Koka has lexical mutable `var` bindings and first-class heap cells `ref<h,a>`. A `var` cannot outlive its lexical scope, including through a returned closure. Its state handler supplies semantics for multiple resumptions. Heap cells can be passed as values; reads, writes, and allocation are indexed by heap `h`. Koka can infer a pure result for an algorithm using internal cells by applying `run` when the heap identity is polymorphic and unobservable outside. Consequently, a proposal for “Koka-like state” must specify which mechanism it means. [Koka book, §§3.2.4–3.2.5](https://koka-lang.github.io/koka/doc/book.html#sec-local-mutable-variables)

The 2014 calculus gives the essential elimination side condition: the heap variable must not occur free in the environment, result type, or remaining effect (`h ∉ ftv(Γ, τ, ε)`). The environment condition prevents hiding access to pre-existing state; the residual-effect condition prevents losing dependencies elsewhere. Its let generalization requires a total initializer: reference allocation therefore cannot produce one cell independently instantiated at incompatible element types. This is an effect-based restriction, rather than unrestricted generalization or a syntactic value restriction. The paper also identifies Landin's knot: storing a function that reads its own cell can produce divergence without syntactic recursion. [Leijen, 2014, §§2.5–2.7, rule RUN, §5.2](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/paper-20.pdf)

Higher-order inference also matters: the 2017 Koka asynchrony paper explicitly inserts a state effect around incoming actions so that their existing effect variable does not unify directly with the internal heap effect. Otherwise the private identity appears in the argument types and cannot be eliminated. This is a concrete example of why simply subtracting `!mut r` after typechecking a block is not a complete inference algorithm. [Leijen, 2017, §3.3.1](https://www.microsoft.com/en-us/research/wp-content/uploads/2017/05/asynceffects-msr-tr-2017-21.pdf)

## Effekt: explicit regions and captured capabilities

Effekt permits `region r { var x in r = ... }`. A boxed closure using `x` has a type mentioning capture `{r}`; it can only be used while that region remains in scope. Region choice also determines continuation behavior: state outside a nondeterminism handler is shared across its resumptions, whereas state inside the captured continuation backtracks. Heap references offer a separately managed lifetime that permits escaping closures. [Effekt regions tour](https://effekt-lang.org/tour/regions)

Effekt separately tracks effects (capabilities the caller must provide) and captures (specific capabilities already fixed by lexical scope). Its functions/objects are second-class by default, with explicit boxing to make captures visible in first-class value types. [Effekt captures tour](https://effekt-lang.org/tour/captures)

Design inference: adopting explicit capability captures would affect Ruddy's function model much more broadly than adding a private heap index to existing effects. It becomes attractive if the objective includes general resource lifetimes and escaping function values, beyond locally pure mutation.

## Questions to test against a Ruddy sketch

These are proposed design probes, not claims that any option has been selected:

- Fresh identity: two region evaluations must not accidentally share a usable identity, including recursive/repeated evaluation of one syntactic binder.
- Escape paths: return a cell directly, return a reader closure, hide either in a data constructor, or write it into an outer cell. Any rejected dependency must survive substitution and nested function types.
- Generalization: allocating an initially empty list cell must not create a polymorphic cell usable as both integer and Boolean storage. A polymorphic cell factory is a different case.
- Heterogeneous cells: `mut r Int` and `mut r Bool` should coexist. Keep the cell element type in `mut`; an effect `!mut r a` would incorrectly force all cells handled together to share `a`.
- Higher-order use: an ordinary traversal should accept a callback mutating the active region and preserve that effect until the enclosing boundary discharges it.
- Multiple regions: if the existing row representation unifies parameters of repeated effect constructors, `mut r1` and `mut r2` need an explicit policy; adding a region parameter alone does not solve simultaneous access.
- Continuations: resuming twice must have a defined shared-versus-restored state result. Physical cell storage and scope alone do not settle this semantic question.
- Export: returning a copied immutable snapshot is straightforward; zero-copy freezing needs a separate account of surviving aliases and any cells reachable through the result.
