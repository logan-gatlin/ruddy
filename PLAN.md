# Ruddy's type system — implementation plan

`src/types.rs` states the target in four lines:

- type inference
- structural subtyping
- higher kinded types (fully general type lambdas)
- a trait system (future work)

This document is how those get built. It is scoped to the type system; the
surrounding compiler (`token`, `parse`, `ir`, `symbol`, `tracking`) exists and is
only touched where the type system cannot proceed without it.

Every phase below is a working compiler with a new debugger tab. That is not a
courtesy to the tool — a constraint solver you cannot watch run is a solver you
cannot fix, and `CLAUDE.md` makes it a rule: if the debugger does not at least
compile, the feature is not done.

---

## 1. Where the compiler stands

The IR has `TypeKind::{Unit, Apply, Fn, Struct, Ident, Error}`. Four gaps decide
what has to happen before any inference can run.

**There is no function type.** `TypeKind::Fn` is a type *lambda* — a binder over
types, which is what "fully general type lambdas" asks for — not an arrow. There
is no `->` token and no arrow node anywhere. So today there is no type that a
term-level `fn` could be given.

**There is no field projection.** `Kind::Dot` is lexed and the parser never uses
it. Records can be built and never read, so nothing in the language *demands* a
record type, and structural width subtyping has nothing to prove itself against.
This is the gap that would quietly make the whole feature untestable.

**There is no ascription.** `let x : T = e` does not parse, so `type`
declarations are unreachable from terms and nothing can ever be checked against a
written type.

**There are no primitive types.** `Natural` literals have nothing to be.

Separately: `trait`, `impl`, `for`, `with`, `in` and `end` are all lexed and
unused. The trait system was anticipated in the token set; phase 7 collects on
that.

---

## 2. Decisions

Four choices shape everything else. Each is recorded with what it costs, because
the cost is the part that gets forgotten.

### 2.1 Inference core: biunification, with bidirectional checking later

Inference, structural subtyping and higher kinded types fight each other.
Unification-based Hindley–Milner breaks under subtyping — it produces `α = β`
where the program only required `α <: β`, and every use site over-constrains its
neighbours. Full higher-order unification, which general type lambdas invite, is
undecidable.

The engine is **algebraic subtyping**: Dolan's `MLsub` (*Algebraic Subtyping*,
2016), in the drastically simplified presentation of Parreaux's *The Simple
Essence of Algebraic Subtyping* (ICFP 2020). Each type variable carries a lower
and an upper bound list; `constrain(lhs, rhs)` walks the two types structurally
and, on reaching a variable, records a bound and propagates it to everything in
the opposite list.

```rust
fn constrain(&mut self, lhs: TypeId, rhs: TypeId) {
    if !self.cache.insert((lhs, rhs)) { return }
    match (self.get(lhs), self.get(rhs)) {
        (Arrow { from: a1, to: r1 }, Arrow { from: a2, to: r2 }) => {
            self.constrain(a2, a1);   // contravariant in the argument
            self.constrain(r1, r2);   // covariant in the result
        }
        (Var(v), _) => {
            self.upper(v).push(rhs);
            for lower in self.lower(v).clone() { self.constrain(lower, rhs) }
        }
        // ...
    }
}
```

This is the only realistic route to *inference together with structural
subtyping*: it yields principal types with no annotations at all, in roughly 500
to 700 lines.

What it costs: unions, intersections, `Top` and `Bot` have to exist in the
semantic type language whether or not they are ever written, and raw inferred
types are unreadable until the simplification pass of phase 5 exists.

A **bidirectional checking mode** is grafted on where ascription lands, not
instead of the above. Checking against a written type is where bounded
quantification and, later, trait constraints have somewhere to live. The ordering
matters: biunification is the part that cannot be retrofitted, and the checking
layer is purely additive.

### 2.2 Surface syntax: arrow, projection, and ascription

Phase 0 adds `->`, `e.f`, and `let x : T = e` — and stops there.

This is the minimum that makes structural subtyping *observable*. Without
projection, no term consumes a record, so width subtyping can be implemented and
never exercised:

```
let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x

let n = fst { x: 1, y: 2, z: 3 }        -- width subtyping, with a witness
```

### 2.3 Applications of an unknown constructor are invariant

`F A <: F B` requires `A ≡ B` unless `F` is a known constructor. Decomposing to
subtyping without knowing `F`'s variance is unsound, and this is what Scala does
in the absence of variance annotations.

Known constructors still decompose, because their variance is not in question:

```
{ x: A } <: { x: B }              =>  A <: B
A1 -> R1 <: A2 -> R2             =>  A2 <: A1  and  R1 <: R2
```

Variance inference for declared aliases arrives in phase 6 and upgrades them to
co- and contravariant decomposition without any annotation burden.

### 2.4 Recursive types are rejected, not folded

Biunification naturally produces recursive types — `fn x => x x` forces
`α <: α -> β`. The usual answer is an equirecursive `μ` binder, which is also
what makes `constrain` total. Ruddy rejects the cycle instead and reports it.

This changes the shape of phase 4. Without `μ` in the type language, structural
types stay finite trees, so a cycle can only ever run through a variable's bound
list. An **occurs-check across the bound graph, performed as each bound is
added**, is therefore sufficient — and it reports at the constraint that closed
the loop, which is the only place a user could act on:

```
let f = fn x => x x
                ^^^
error: recursive type
  'a would have to satisfy 'a <: 'a -> 'b
```

The seen-pair cache stays regardless: it is a large performance win, and a
backstop against divergence.

---

## 3. Architecture

### 3.1 Two type representations

- **`ir::TypeKind`** — syntactic. What was written, spans and all. Already exists.
- **`types::Type`** — semantic. What inference manipulates: variables, unions,
  intersections, `Top`, `Bot`. Never written by a user, never parsed.

Elaboration — kind-check, then normalize — is the bridge between them. Keeping
them apart is what stops the surface syntax from being bent to suit the solver,
the same way `SPEC.md` keeps serialization out of the compiler crate.

### 3.2 Storage follows the `Mint` pattern

The codebase already has the shape this wants: `Symbol` is an index into a `Mint`
that owns every fact about it. Types work the same way — an arena with
`TypeId(u32)` indices rather than `Rc<RefCell<_>>`. Variables live in a
`Vec<Var>` indexed by `TyVar(u32)`, so bounds are plain `Vec<TypeId>` with no
interior mutability, and the hash-consing of phase 5 becomes an `IndexSet`
lookup rather than a redesign.

```rust
pub struct Var { level: u32, lower: Vec<TypeId>, upper: Vec<TypeId> }

pub enum Type {
    Prim(Prim),                             // Nat, Unit
    Arrow { from: TypeId, to: TypeId },
    Struct(IndexMap<String, TypeId>),
    Apply { func: TypeId, arg: TypeId },    // neutral: rigid or flexible head
    Lambda { arg: Symbol, body: TypeId },
    Ident(Symbol),                          // rigid: a bound type parameter
    Var(TyVar),
    Union(TypeId, TypeId),                  // phase 4
    Inter(TypeId, TypeId),                  // phase 4
    Top,
    Bot,
    Error,                                  // absorbing; mirrors TermKind::Error
}
```

`Type::Error` is absorbing for the same reason `TermKind::Error` is: one bad name
must produce one diagnostic, not a cascade from a dropped definition.

---

## 4. Phases

| # | Phase | Files | Debugger tab |
|---|-------|-------|--------------|
| 0 | Surface prerequisites | `token.rs`, `parse.rs`, `ir.rs` | free in `AST` / `IR` |
| 1 | Kinds | `types/kind.rs` | `Kinds` |
| 2 | Normalization | `types/normal.rs` | `Normal` |
| 3 | Inference skeleton | `types/infer.rs` | `Types` + annotates `ir` |
| 4 | Biunification | `types/constrain.rs` | `Constraints` |
| 5 | Simplification | `types/simplify.rs` | `Simplify` |
| 6 | Higher kinded integration | extends 1 and 4 | extends `Kinds`, `Constraints` |
| 7 | Traits | future work | `Traits` |

### Phase 0 — Surface prerequisites

No inference yet; this is what the rest stands on.

- `token.rs` — `Kind::Arrow` for `->`. `-` is currently not lexed at all, so this
  is a clean new two-character token rather than a disambiguation.
- `parse.rs` — `TypeKind::Arrow`, right-associative and binding looser than type
  application. `ExprKind::Project { base, field }`, postfix `.name`, binding
  *tighter* than application so that `f p.x` parses as `f (p.x)`.
  `StmtKind::Let` gains `ty: Option<Type>`.
- `ir.rs` — mirrors all three; `Decl` carries the optional annotation. The
  projected field name stays a `String`, for the reason `Field` keys already do:
  it is a label scoped to its own struct, not a path anything can refer to.
- Primitives — `TypeKind::Prim(Prim)`, with `Nat` seeded into the type namespace
  by the builder so that a literal has something to be.
- Printer round-trip tests for each addition, following the existing
  `display_program` discipline: print, re-lower, print again, compare.

**Debugger.** The `AST` and `IR` stages have to learn `Project`, `Arrow` and the
annotation child, or `ruddy-debug` will not compile.

### Phase 1 — Kinds

`κ ::= * | κ → κ`. Plain Hindley–Milner unification over a tiny language with
mutable kind variables; free kind variables default to `*` at the end. There is
deliberately no kind polymorphism — it buys nothing here and complicates phase 6.

Kind-checks every `type` declaration and infers each type lambda's parameter
kinds. Catches `Nat Nat`, arity mismatches, and `{ x: List }`.

This is the smallest self-contained win in the plan and it is what unblocks
higher kinded types, so it goes first.

**Tab `Kinds`** (list) — one row per type declaration and per binder:
`Pair :: * -> * -> *`.

### Phase 2 — Normalization

β-reduction for type lambdas plus alias unfolding, so that
`(fn t => { value: t }) Nat` and `{ value: Nat }` compare equal. Normalization by
evaluation with a `Symbol`-keyed environment.

Alias cycles — `type T = T` — get an SCC pass over the declaration graph and a
diagnostic. A compiler that hangs on a two-word program is worse than one that
rejects it.

**Tab `Normal`** (tree) — declared form beside normal form, per declaration.

### Phase 3 — Inference skeleton

All of the plumbing, using plain unification so that it can be got right in
isolation before subtyping complicates it: type variables, Rémy levels, `Scheme`,
generalize and instantiate, the term walk, and `Error` absorption.

**Tab `Types`** (tree) — each top-level binding's scheme. Plus a second `Stage`
with `annotates: "ir"`, painting each IR row's type as an inline badge through
the hook `SPEC.md` §7.2 already specifies, with no frontend change:

```
▾ let compose   demo::compose        (b -> c) -> (a -> b) -> a -> c
```

### Phase 4 — Biunification

Replace unification with `constrain`. Bounds propagate through the opposite list;
arrows are contravariant in the argument and covariant in the result; records get
width *and* depth subtyping; `extrude` handles a type escaping to a shallower
level. `Union`, `Inter`, `Top` and `Bot` join the type language. The
occurs-check of §2.4 reports `recursive-type` at the closing constraint.

**Tab `Constraints`** (tree) — the ordered constraint stream, each entry with the
span that produced it, and every variable's live lower and upper bounds. This tab
is the reason phase 4 is tractable at all: biunification is opaque when it
misbehaves, and "why did this end up `⊤`?" should be a five-second read rather
than an afternoon with `dbg!`.

### Phase 5 — Simplification

Coalesce bounds into union and intersection form, drop variables occurring in
only one polarity, merge variables by co-occurrence, hash-cons the result.
Without this, inferred types are correct and unreadable — which in a language
whose types are inferred means the feature is not finished.

**Tab `Simplify`** — raw scheme beside simplified scheme, with the rule that
fired.

### Phase 6 — Higher kinded integration

Extends phases 1 and 4 rather than adding a file.

Neutral applications with matching rigid heads decompose invariantly per §2.3;
mismatched rigid heads fail with a message naming both. A flexible head goes to
Miller pattern unification — the higher-order-pattern fragment that Agda, Idris
and Lean's elaborators all restrict themselves to, and the reason undecidability
is not reached. Variance inference for declared aliases upgrades known
constructors to co- and contravariant decomposition.

### Phase 7 — Traits

Future work. `Scheme` gains a constraint set and resolution runs against `impl`
declarations. The `trait`, `impl`, `for` and `with` tokens are already lexed.

---

## 5. Testing

Following the discipline already in `ir.rs` and `snapshot.rs`:

- **Round-trip printing** — every new syntactic form prints, re-lowers, and
  prints identically. This is what makes the printer's parentheses trustworthy,
  and phase 0 adds three forms that need it.
- **Span hygiene** — `debug/src/snapshot.rs` already asserts that every span
  every stage emits lies inside the source. New stages inherit that check the
  moment they are registered, which turns bad `merge` arithmetic into a failing
  test rather than a mangled highlight.
- **Principality** — a scheme inferred for a definition must be at least as
  general as any type that definition checks against.
- **Termination** — the occurs-check and the constraint cache each get cases that
  would otherwise diverge: `fn x => x x`, `type T = T`, and mutually recursive
  aliases.

---

## 6. Non-goals

- **Recursive types.** Rejected by decision §2.4, not deferred.
- **Kind polymorphism.** Free kind variables default to `*`.
- **Row polymorphism.** Structural subtyping through unions and intersections
  covers what the feature list asks for; rows are a different design, not a later
  increment of this one.
- **Recursive value bindings.** `ir::build` lowers a body before binding its name,
  so a definition cannot see itself. Nothing here changes that.
