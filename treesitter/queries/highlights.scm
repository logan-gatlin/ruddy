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

; ── comments ─────────────────────────────────────────────────────────────────

(line_comment) @comment
(block_comment) @comment

; ── names ────────────────────────────────────────────────────────────────────

(identifier) @variable

; ── types ────────────────────────────────────────────────────────────────────

(annotation type: (identifier) @type)
(function_type from: (identifier) @type)
(function_type to: (identifier) @type)
(type_application head: (identifier) @type)
(type_application argument: (identifier) @type)
(parenthesized_type (identifier) @type)
(tuple_type element: (identifier) @type)
(struct_type_field type: (identifier) @type)
(sum_case payload: (identifier) @type)
(effect_operation_field signature: (function_type from: (identifier) @type))
(effect_operation_field signature: (function_type to: (identifier) @type))
(parenthesized_effect_operation_signature signature: (function_type from: (identifier) @type))
(parenthesized_effect_operation_signature signature: (function_type to: (identifier) @type))
(extern_definition type: (extern_annotation type: (identifier) @type))
(extern_function_type parameter: (identifier) @type)
(extern_function_type result: (identifier) @type)
(annotated_extern_type type: (identifier) @type)
(effect_case argument: (identifier) @type)
(absent_effect argument: (identifier) @type)
(effect_alias argument: (identifier) @type)

; A path written where a type goes: the name at the end of it is the type, and
; the modules in front of it are painted with every other path's, below.
(annotation type: (path name: (identifier) @type))
(function_type from: (path name: (identifier) @type))
(function_type to: (path name: (identifier) @type))
(type_application head: (path name: (identifier) @type))
(type_application argument: (path name: (identifier) @type))
(parenthesized_type (path name: (identifier) @type))
(tuple_type element: (path name: (identifier) @type))
(struct_type_field type: (path name: (identifier) @type))
(sum_case payload: (path name: (identifier) @type))
(effect_operation_field signature: (function_type from: (path name: (identifier) @type)))
(effect_operation_field signature: (function_type to: (path name: (identifier) @type)))
(parenthesized_effect_operation_signature signature: (function_type from: (path name: (identifier) @type)))
(parenthesized_effect_operation_signature signature: (function_type to: (path name: (identifier) @type)))
(extern_definition type: (extern_annotation type: (path name: (identifier) @type)))
(extern_function_type parameter: (path name: (identifier) @type))
(extern_function_type result: (path name: (identifier) @type))
(annotated_extern_type type: (path name: (identifier) @type))
(effect_case argument: (path name: (identifier) @type))
(absent_effect argument: (path name: (identifier) @type))
(effect_alias argument: (path name: (identifier) @type))

(type_definition name: (identifier) @type.definition)

; The primitive types; see `types::Prim::ALL`.
((identifier) @type.builtin
  (#any-of? @type.builtin "Nat" "Int" "Nat8" "Nat16" "Nat32" "Nat64" "Int8" "Int16" "Int32" "Int64" "Real" "String" "Boolean"))

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

; `@deprecated` — an attribute's key: metadata about the definition after it.
(attribute_key) @attribute

(effect_definition name: (identifier) @module @namespace)
(effect_operation_field name: (identifier) @function)
(operation name: (identifier) @function)
(extern_definition name: (identifier) @function)

; ── modules ──────────────────────────────────────────────────────────────────

; `Math::` — the modules a path is reached through, whatever sort of name sits
; at the end of it. Captured twice for the reason an effect is: the two editors
; spell this one idea differently, and a module is exactly what each of them
; means by its word.
(path module: (identifier) @module @namespace)
(effect_path module: (identifier) @module @namespace)

(module_definition name: (identifier) @module @namespace)

; ── definitions and parameters ───────────────────────────────────────────────

(let_definition
  pattern: (identifier) @function
  body: [(function) (match_function)])

(do_block
  statement: (let_definition
    pattern: (identifier) @function
    body: [(function) (match_function)]))

(function parameter: (identifier) @variable.parameter)
(handler_arm binder: (identifier) @variable.parameter)

; ── fields ───────────────────────────────────────────────────────────────────

(struct_field name: (identifier) @property)
(data_field name: (identifier) @property)
(struct_type_field name: (identifier) @property)
(struct_pattern_field name: (identifier) @property)
(absent_field name: (identifier) @property)
(projection field: (identifier) @property)
(projection field: (numeric_field) @property)
(struct_field name: (numeric_field) @property)
(data_field name: (numeric_field) @property)
(struct_type_field name: (numeric_field) @property)
(struct_pattern_field name: (numeric_field) @property)
(absent_field name: (numeric_field) @property)

; ── literals ─────────────────────────────────────────────────────────────────

(natural) @number
(string) @string
(boolean) @boolean

; A quoted field is lexically a string, but its syntactic role is a property.
; These come after the catch-all string capture so the contextual class wins.
(struct_field name: (string) @property)
(data_field name: (string) @property)
(struct_type_field name: (string) @property)
(struct_pattern_field name: (string) @property)
(absent_field name: (string) @property)
(projection field: (string) @property)

; ── keywords ─────────────────────────────────────────────────────────────────

[
  "mut"
  "let"
  "extern"
  "do"
  "if"
  "then"
  "else"
  "type"
  "end"
  "with"
  "match"
  "fn"
  "effect"
  "handle"
  "module"
] @keyword

[
  "raise"
  "return"
] @keyword.return

; Contextual: each of these is an ordinary name everywhere but the position
; that reads it, and the anonymous node only exists where it was read as one.
(when_clause "when" @keyword)
(where_clause "where" @keyword)
(boolean_or "or" @keyword.operator)
(boolean_xor "xor" @keyword.operator)
(boolean_and "and" @keyword.operator)
(unary_expression "not" @keyword.operator)
(clause_or "or" @keyword.operator)
(clause_and "and" @keyword.operator)
(clause_not "not" @keyword.operator)

; ── the wildcard and the hole ────────────────────────────────────────────────

(wildcard) @variable.builtin
(hole) @type.builtin

; ── punctuation ──────────────────────────────────────────────────────────────

[
  ":="
  "~"
  "="
  "=>"
  "->"
  "!="
  "+"
  "-"
  "*"
  "/"
  "|"
  "\\"
  ".."
] @operator

(rest_pattern "..") @operator
(rest_pattern name: (identifier) @variable)

[
  ":"
  "::"
  ","
  ";"
  "."
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(mut_type element: (identifier) @type)
