; Scopes, definitions and references for ruddy.
;
; A `let` statement binds for the whole file, so the file is a scope; a `fn`,
; a `do` block, a match arm and a handler arm each bind for their own body,
; so each of those is one too.

; A `type` declaration's parameters bind for its body and nowhere else, which
; makes the declaration a scope the way a `fn` is.
(source_file) @local.scope
(module_definition) @local.scope
(type_definition) @local.scope
(function) @local.scope
(do_block) @local.scope
(match_arm) @local.scope
(handler_arm) @local.scope

; Tree-sitter uses the bare definition capture; Helix takes the highlight
; class from its suffix. Keep both for the portable highlight tests. The
; Helix installer removes the bare captures, which Helix treats as discards.

; ── what binds ───────────────────────────────────────────────────────────────

(let_definition pattern: (identifier) @local.definition @local.definition.variable)
(extern_definition name: (identifier) @local.definition @local.definition.function)

(function parameter: (identifier) @local.definition @local.definition.variable.parameter)
(handler_arm binder: (identifier) @local.definition @local.definition.variable.parameter)

; Every name inside a pattern binds, whichever shape took the value apart. A
; bare identifier field puns, binding the field to itself; an explicit field's
; label does not bind. In particular, quoted fields are always explicit, so
; only the identifier in their subpattern is a definition.
(match_arm pattern: (identifier) @local.definition @local.definition.variable)
(tag_pattern payload: (identifier) @local.definition @local.definition.variable)
(parenthesized_pattern (identifier) @local.definition @local.definition.variable)
(tuple_pattern element: (identifier) @local.definition @local.definition.variable)
(struct_pattern_field
  name: (identifier) @local.definition @local.definition.variable
  !pattern)
(struct_pattern_field pattern: (identifier) @local.definition @local.definition.variable)
(array_pattern element: (identifier) @local.definition @local.definition.variable)
(rest_pattern name: (identifier) @local.definition @local.definition.variable)

; ── what a name may resolve to ───────────────────────────────────────────────

(type_definition parameter: (type_variable) @local.definition @local.definition.variable.parameter)

; A hidden type binds its variable for its body, and a hidden pattern binds
; its variable for the arm it opens in — the arm is already a scope, so only
; the type needs one of its own. The pattern's payload binds as a tag's does.
(hidden_type) @local.scope
(hidden_type variable: (type_variable) @local.definition @local.definition.variable.parameter)
(hidden_pattern variable: (type_variable) @local.definition @local.definition.variable.parameter)
(hidden_pattern pattern: (identifier) @local.definition @local.definition.variable)

; Only expression names resolve to value bindings. Field labels, qualified
; paths and type names can share a parameter's spelling without referring to
; it, so a catch-all identifier reference would give them its color too.
(function parameter: (identifier) @local.reference)
(handler_arm binder: (identifier) @local.reference)
(let_definition body: (identifier) @local.reference)
(function body: (identifier) @local.reference)
(match_arm body: (identifier) @local.reference)
(handler_arm body: (identifier) @local.reference)
(handle_expression body: (identifier) @local.reference)
(do_block value: (identifier) @local.reference)
(application function: (identifier) @local.reference)
(application argument: (identifier) @local.reference)
(projection base: (identifier) @local.reference)
(addition left: (identifier) @local.reference)
(addition right: (identifier) @local.reference)
(multiplication left: (identifier) @local.reference)
(multiplication right: (identifier) @local.reference)
(unary_expression value: (identifier) @local.reference)
(boolean_or left: (identifier) @local.reference)
(boolean_or right: (identifier) @local.reference)
(boolean_xor left: (identifier) @local.reference)
(boolean_xor right: (identifier) @local.reference)
(boolean_and left: (identifier) @local.reference)
(boolean_and right: (identifier) @local.reference)
(assignment target: (identifier) @local.reference)
(assignment value: (identifier) @local.reference)
(pipeline value: (identifier) @local.reference)
(pipeline function: (identifier) @local.reference)
(if_expression condition: (identifier) @local.reference)
(if_expression consequent: (identifier) @local.reference)
(if_expression alternative: (identifier) @local.reference)
(else_if_arm condition: (identifier) @local.reference)
(else_if_arm consequent: (identifier) @local.reference)
(match_expression scrutinee: (identifier) @local.reference)
(raise_expression (identifier) @local.reference)
(struct_field value: (identifier) @local.reference)
(tag_expression payload: (identifier) @local.reference)
(tuple_expression element: (identifier) @local.reference)
(array_expression element: (identifier) @local.reference)
(spread value: (identifier) @local.reference)
(parenthesized_expression (identifier) @local.reference)

; A parameter is written `'a` wherever it is used, so a variable is a reference
; the way a bare name is — including the `..'a` that tails a row.
(type_variable) @local.reference
