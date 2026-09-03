# Typed effect parameters: design

Companion to `spec.md`. Records the representation decisions the implementation
follows so every phase agrees.

## Surface syntax

- `effect Ask 'a = { get: () -> 'a }`, `effect Log 'a = 'a -> ()`, `effect Nil 'a`,
  `effect Both 'a 'e = !Ask 'a + !Log + ..'e` (alias with a spliced tail).
- Parameters use the declaration parameter syntax (`'a`), in order, after the name.
- An applied effect in a row uses type-argument grammar: `!Ask Nat`, `!State { x: Nat, ..'r }`,
  `!Run (!Log + !IO)`, `\!Ask Nat + ..'e`, `!Ask Nat (when 'p)`. Arguments are type atoms;
  `(when` after a label is the presence clause, never an argument.
- Alias bodies allow labels with arguments and one optional `..'e` tail; no `\` or `when`.
- Term syntax is unchanged (`!Ask.get ()`, handler arms).

## IR

- `Decl<Effect>::params` is populated for effects exactly as for types (`declare_params`).
- `EffectLabel::Written { args: Vec<Type>, .. }` / `Absent { args, .. }`. Annotation rows are
  expanded: every label names a concrete effect and carries fully substituted arguments.
- `Effect::Alias(AliasBody)`: `AliasBody { span, cases: Vec<AliasCase { name_span, symbol, args }>, tail: Option<Tail> }`,
  stored unexpanded. Written duplicates inside one alias body are reported by symbol.
- Arity is checked where an effect is applied (`ErrorKind::EffectArity { name, expected, found }`),
  at the whole label span; a wrong count drops the label.
- Alias expansion (`Builder::expand_alias`) substitutes the alias's parameters into each case's
  arguments (`TypeKind::Param { index }` replaced by the supplied `Type`), expands alias cases
  recursively with a visiting stack, and splices the tail parameter's argument: a written row
  contributes its labels and tail, a parameter of the enclosing declaration becomes that
  parameter's tail, anything else is `NotARow { sense: Effects }`. A cycle is
  `AliasCycle { name, growing }` (`alias-cycle` / `growing-alias-cycle`); the alias on the cycle
  expands to nothing. A negative or conditional application of an alias whose expansion keeps an
  open tail is `ModifiedOpenAlias { name }`; otherwise the modifier distributes to every label.
- Kinds: `kinds` runs over type declarations and effect declarations (operation signatures and
  alias bodies) in one fixpoint. Effect label arguments feed the same facts a type application
  does (`Hands` for a bare parameter, `Tails` + descent otherwise). It runs twice: once before
  structural identities exist (lacks keyed by pending symbols, only senses used) and once after
  (final kinds, lacks and mixed-parameter errors).
- `row_arguments` also checks effect label arguments against the effect's parameter kinds
  (`NotARow`, `RepeatedRowField`), erasing an offending argument.
- Operation signatures may mention declared parameters anywhere (`'a` as a type, `..'r` as a
  field/case/effect tail). Anonymous tails, holes, `when`, undeclared variables and an outer
  effect row remain refused as before.
- Structural identity: the canonical interface node gains `arity`, one `param:{i}:{sense}` edge
  per parameter, and operations whose parameter references are `param:{i}` atoms; effect label
  arguments become `arg:{i}` edges beside the label's interface. Row keys stay
  `EffectId::row_key()` (name + interface), so applications of one constructor share a label.
- `Program::effect_params: IndexMap<Symbol, Vec<ParamKind>>` carries every effect's ordered kinds
  (local and imported) for inference and export.

## Inference

- A row label's payload (`RowField::ty`) is the ordered argument tuple: `Ty::Struct` with
  positional labels `"0"`, `"1"`, … (`Ty::unit()` for a zero-arity effect, as today). Type
  arguments are plain types; fields arguments are `Ty::Struct(row)`; cases and effects arguments
  are `Ty::Sum(row)` (the same encoding a declaration's row argument already uses).
- Operations are stored generically (`Ty::Bound(i)` / `Rest::Bound(i)`). An operation reference
  mints fresh arguments per parameter kind (type variable, or a fresh row variable wrapped in
  `Ty::Struct`/`Ty::Sum`), opens `from`/`to` with them, notes lacks, and puts the tuple in the
  label payload. A handler mints one fresh tuple per discharged identity and opens every arm of
  that identity with it. Annotation rows lower each written argument into the tuple.
- Two labels of one constructor meeting in row unification unify their payloads position by
  position through `Solve::effect_arguments`; a failure there is
  `ErrorKind::EffectArgument { effect, position, cause: Box<ErrorKind> }` (code
  `effect-argument-mismatch`) with the underlying failure kept as causal detail.
- Generalization, close_effects, printing and substitution walk payloads already.

## Artifact (breaking schema)

- `DeclaredEffect { name, params: Vec<Parameter>, identity: Option<EffectIdentity>, kind }`.
- `EffectKind::Operations(Vec<Operation>)` with generic `from`/`to`, or
  `EffectKind::Alias(AliasRow { cases: Vec<AliasCase { name, args: Vec<Type> }>, tail: Option<u32> })`.
- Text: `(effect "q" (params (param …)…) (identity …) (operations …|alias (cases (case "q" (args <ty>…))…) (tail closed|<n>)))`.
- Reader validates arity of arguments against the named effect's parameter count when the effect is
  in the same artifact, bound positions, tail index, and alias cycles.
- Import translates alias bodies back to syntactic `ir::Type` arguments (`imported_syntax`) so
  expansion is one implementation; operations import as generic semantic types.

## Erasure

- Lowering and the runtime key labels by `row_key()` and never read payloads, so arguments are
  erased without changes to LIR or JavaScript.

## UI

- `SemanticJob::Effect` renders the payload tuple after the label with application precedence.
- New IR codes: `effect-arity`, `alias-cycle`, `growing-alias-cycle`, `modified-open-alias`.
  New inference code: `effect-argument-mismatch`.
