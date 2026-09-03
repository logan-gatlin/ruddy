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
  each alias's `expanded` row over its own parameters) in one fixpoint. Effect label arguments feed the same facts a type application
  does (`Hands` for a bare parameter, `Tails` + descent otherwise). It runs twice: once before
  structural identities exist (lacks keyed by pending symbols, only senses used) and once after
  (final kinds, lacks and mixed-parameter errors).
- `row_arguments` also checks effect label arguments against the effect's parameter kinds
  (`NotARow`, `RepeatedRowField`), erasing an offending argument.
- Operation signatures may mention declared parameters anywhere (`'a` as a type, `..'r` as a
  field/case/effect tail). Anonymous tails, holes, `when`, undeclared variables and an outer
  effect row remain refused as before.
- Structural identity: the canonical interface node gains an `arity` edge and one `param:{i}`
  edge per parameter (to an atom naming its sense); operation types are built over `param:{i}`
  atoms for the parameter positions; effect label arguments become `arg:{i}` edges beside the
  label's identity and payload. Imported identities keep their published interface text. Row keys stay
  `EffectId::row_key()` (name + interface), so applications of one constructor share a label.
- `Program::effect_params: IndexMap<Symbol, Vec<ParamKind>>` carries every effect's ordered kinds
  (local and imported) for inference and export.

## Inference

- A row label's payload (`RowField::ty`) is the ordered argument tuple: `Ty::Struct` with
  positional labels `"0"`, `"1"`, … (`Ty::unit()` for a zero-arity effect, as today). Type
  arguments are plain types; fields arguments are `Ty::Struct(row)`; cases arguments are
  `Ty::Sum(row)`; effects arguments are `Ty::effects_argument(row)`, an arrow from unit to unit
  carrying the row, so that two of them unify as effect rows, carry lacks in effect nouns, and
  print as a row of effects. The empty effects argument prints as `(|)`, which the parser reads
  as the empty sum; the IR's argument check turns an empty closed sum at an effects-kinded
  parameter into the empty effects row.
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
- Text: `(effect "q" (params (param …)…) (identity …) (operations …))` or
  `(effect "q" (params …) (identity none) (alias (cases (case "q" <ty>…)…) (tail none|<n>)))`.
- Reader validates bound positions in operation signatures and alias arguments against the
  effect's parameter count, the tail index, alias arities against effects of the same artifact,
  and alias rings. Recovery drops a declaration that fails on its own.
- Inference publishes each alias's row semantically (`Semantics::effect_aliases`) for export.
- Import translates alias bodies back to syntactic `ir::Type` arguments (`imported_syntax`) so
  expansion is one implementation; operations import as generic semantic types.

## Alias lowering

- Alias bodies are lowered on first use (`Builder::alias_body`), in the alias's own module with its
  parameters in scope, so an operation signature above an alias may apply it. Every alias is
  forced after the effect declarations and expanded over its own parameters into
  `Alias::expanded`, the row the parameter fixpoint and the argument checks read.
- Cycles are detected on the expansion stack; the alias met again is reported (locally) or
  silently emptied (imported) and stands for nothing from then on.

## Erasure

- Lowering and the runtime key labels by `row_key()` and never read payloads, so arguments are
  erased without changes to LIR or JavaScript.

## UI

- `SemanticJob::Effect` renders the payload tuple after the label with application precedence.
- New IR codes: `effect-arity`, `alias-cycle`, `growing-alias-cycle`, `modified-open-alias`.
  New inference code: `effect-argument-mismatch`.
