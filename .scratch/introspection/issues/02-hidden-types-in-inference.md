# 02 Hidden types: introduction, opening, and escape checks

Status: resolved
Type: task
Blocked by: 01

Spec sections: "Source syntax: `hide`", "Heterogeneous fields without erasing
their types", compiler work item 2, acceptance bullets 2 and 5 (skolem escapes).

- Semantic `Ty::Hidden(body)` binding one type variable over its body with a
  de Bruijn-style bound occurrence; alpha-equivalent hidden types are equal in
  `same_finite_syntax`, unification, hashing, printing, artifacts, and imports.
- IR: `hide 'a => T` lowers with `'a` scoped to the body; `hide 'a p` in a match
  arm scopes `'a` over the arm's payload pattern and body, and local annotations
  in the arm refer to that scoped type. A hidden pattern outside a match arm is
  rejected.
- Contextual introduction: checking an expression against a known hidden type
  mints a witness, checks the expression against the opened body, and packages.
  Propagates through record/array literal children, function-literal results,
  individual match branches (which may choose different witnesses), and the
  arguments of a known function parameter type. No introduction for
  unconstrained expressions; an unresolved witness (one only the checked
  expression's own fresh variables could fix) is a diagnostic. No implicit
  mapping of existing arrays/functions, no implicit opening to coerce.
- Opening: the scrutinee position must already be known to be a hidden type.
  The arm gets a fresh rigid type; the payload pattern is checked against the
  opened body; the rigid may not appear in the arm's outward type or escape into
  outer variables (skolem escape diagnostic).
- Patterns phase treats a hidden pattern as its payload pattern; lowering is
  representation-transparent; the debugger's type views print hidden types.

Seams: `tests/src/inference.rs`, `tests/src/patterns.rs`, `tests/src/ir.rs`,
`tests/src/artifact.rs`, diagnostics fixtures, `tests/src/types.rs`.
