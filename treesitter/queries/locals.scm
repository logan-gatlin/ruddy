; Scopes, definitions and references for ruddy.
;
; A `let` statement binds for the whole file, so the file is a scope; a `fn`,
; a `let ... in`, a match arm and a handler arm each bind for their own body,
; so each of those is one too.

(source_file) @local.scope
(function) @local.scope
(let_expression) @local.scope
(match_arm) @local.scope
(handler_arm) @local.scope

; ── what binds ───────────────────────────────────────────────────────────────

(let_definition pattern: (identifier) @local.definition.var)
(let_expression pattern: (identifier) @local.definition.var)

(function parameter: (identifier) @local.definition.parameter)
(handler_arm binder: (identifier) @local.definition.parameter)

; Every name inside a pattern binds, whichever shape took the value apart. A
; bare field name puns, binding the field to itself.
(match_arm pattern: (identifier) @local.definition.var)
(tag_pattern payload: (identifier) @local.definition.var)
(parenthesized_pattern (identifier) @local.definition.var)
(struct_pattern_field name: (identifier) @local.definition.var)
(struct_pattern_field pattern: (identifier) @local.definition.var)

; ── what a name may resolve to ───────────────────────────────────────────────

(type_definition name: (identifier) @local.definition.type)
(type_definition parameter: (identifier) @local.definition.type)

(identifier) @local.reference
