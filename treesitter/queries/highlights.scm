; Highlights for ruddy.
;
; The classes the debugger paints from the token stream — keyword, punct,
; ident, number, tag, effect, variable — are the palette this file works in;
; see `debug/src/stage/tokens.rs`. Where a capture can say something the token
; stream cannot, because the tree knows what position a name is in, it does:
; a type is a type, a field is a field, and a parameter is a parameter.
;
; Later patterns win over earlier ones, so the catch-all names come first and
; every position that knows better comes after.

; ── names ────────────────────────────────────────────────────────────────────

(identifier) @variable

; ── types ────────────────────────────────────────────────────────────────────

(annotation type: (identifier) @type)
(function_type from: (identifier) @type)
(function_type to: (identifier) @type)
(type_application head: (identifier) @type)
(type_application argument: (identifier) @type)
(parenthesized_type (identifier) @type)
(struct_type_field type: (identifier) @type)
(sum_case payload: (identifier) @type)
(operation_declaration signature: (identifier) @type)

(type_definition name: (identifier) @type.definition)

; The one primitive there is; see `types::Prim::ALL`.
((identifier) @type.builtin
  (#eq? @type.builtin "Nat"))

; `'a` — a declaration's parameter, a variable of the annotation it is written
; in, and the presences a `where` clause is written about. One capture for all
; three, because they are one token and one idea: the thing a use site picks.
(type_variable) @variable.parameter

; ── labels ───────────────────────────────────────────────────────────────────

; `#Some` — a case of a sum.
(tag) @constructor

; `!Log` — an effect. Neither a name nor a tag: it resolves to a declaration
; like the first and wears a sigil like the second. Captured twice because the
; two editors spell the same idea differently: Neovim calls it `module` and
; Helix calls it `namespace`, and neither minds the name it does not know.
(effect_label) @module @namespace

(effect_definition name: (identifier) @module @namespace)
(operation_declaration name: (identifier) @function)
(operation name: (identifier) @function)

; ── definitions and parameters ───────────────────────────────────────────────

(let_definition
  pattern: (identifier) @function
  body: (function))

(let_expression
  pattern: (identifier) @function
  value: (function))

(function parameter: (identifier) @variable.parameter)
(handler_arm binder: (identifier) @variable.parameter)

; ── fields ───────────────────────────────────────────────────────────────────

(struct_field name: (identifier) @property)
(struct_type_field name: (identifier) @property)
(struct_pattern_field name: (identifier) @property)
(absent_field name: (identifier) @property)
(projection field: (identifier) @property)

; ── literals ─────────────────────────────────────────────────────────────────

(natural) @number

; ── keywords ─────────────────────────────────────────────────────────────────

[
  "let"
  "in"
  "type"
  "end"
  "with"
  "match"
  "fn"
  "effect"
  "handle"
] @keyword

"raise" @keyword.return

; Contextual: each of these is an ordinary name everywhere but the position
; that reads it, and the anonymous node only exists where it was read as one.
(when_clause "when" @keyword)
(where_clause "where" @keyword)
(clause_or "or" @keyword.operator)
(clause_and "and" @keyword.operator)
(clause_not "not" @keyword.operator)
(handler_arm "return" @keyword.return)

; ── the wildcard and the hole ────────────────────────────────────────────────

(wildcard) @variable.builtin
(hole) @type.builtin

; ── punctuation ──────────────────────────────────────────────────────────────

[
  "="
  "=>"
  "->"
  "!="
  "+"
  "|"
  "\\"
  ".."
] @operator

(rest_pattern) @operator

[
  ":"
  ","
  ";"
  "."
] @punctuation.delimiter

[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket
