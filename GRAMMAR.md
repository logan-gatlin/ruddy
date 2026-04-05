# Halcyon Informal Grammar

This document defines concrete parsing syntax.
Semantic validation (name resolution, recursion legality, import graph checks, etc.) is out of scope unless called out explicitly.

## Conventions
- EBNF operators: `?` optional, `*` zero-or-more, `+` one-or-more.
- Newlines are whitespace, not syntax.
- Comma-delimited lists allow an optional trailing comma.
- Keywords are fully reserved and cannot be used as bare identifiers.

## Program Structure
```bnf
<bundle_root_file> ::= <bundle_declaration> <file>
<file> ::= <statement>*

<statement> ::= <module_statement>
              | <let_statement>
              | <do_statement>
              | <use_statement>
              | <type_statement>
              | <trait_statement>
              | <impl_statement>
              | <wasm_statement>

<bundle_declaration> ::= "bundle" <ident>

<module_statement>   ::= "module" <ident> "=" <statement>* "end"
              | "module" <ident> ("as" <string>)?

<let_statement> ::= "let" <pattern> "=" <expr>
                  | "let" "|" <ident> "=" (<ident> | <path>)
<do_statement>  ::= "do" <expr>
<use_statement> ::= "use" (<ident> | <path> | "bundle") ("as" <ident>)?

<type_statement>         ::= <nominal_type_statement> | <alias_type_statement>
<nominal_type_statement> ::= "type" <ident> <type_params>? "=" <type_def>
<alias_type_statement>   ::= "type" "~" <ident> <type_params>? "=" <type_expr>

<trait_statement>      ::= <trait_def_statement> | <trait_alias_statement>
<trait_def_statement>  ::= "trait" <ident> <type_params>? "=" <trait_item_decl>* "end"
<trait_alias_statement> ::= "trait" "~" <ident> "=" (<ident> | <path>)

<type_params> ::= ":" <ident>+

<impl_statement> ::= "impl" (<ident> | <path>) <type_expr> ("," <type_expr>)* ","? "=" <impl_item_def>* "end"

<trait_item_decl> ::= <trait_method_decl> | <trait_type_decl>
<impl_item_def>   ::= <impl_method_def> | <impl_type_def>

<trait_method_decl> ::= "let" <ident> ":" <type_expr>
<trait_type_decl>   ::= "type" <ident>

<impl_method_def> ::= "let" <ident> "=" <expr>
<impl_type_def>   ::= "type" <ident> "=" <type_expr>

<wasm_statement> ::= "wasm" "=>" (<wasm_declaration> | "(" <wasm_declaration>* ")")
```

- `bundle` are regular statements and can appear anywhere statements are allowed.
- Statements in `<file>` and `module ... end` are not separated by semicolons.
- Top-level statements are part of the bundle scope; a `module ... end` wrapper is optional.
- CLI bundle compilation requires the root file to start with `bundle <name>`.
- A bundle may only be declared once across the entire import graph.
- `compile_source` accepts files without a `bundle` declaration and uses implicit bundle name `_`.

## Type Definitions
```bnf
<type_def> ::= <record_type_def>
             | <sum_type_def>
             | <type_expr>

<record_type_def>  ::= "{" <struct_member_list>? "}"
<struct_member_list> ::= <struct_member> ("," <struct_member>)* ","?

<struct_member> ::= <field_decl>
                  | ".." <type_expr>

<field_decl> ::= <ident> ":" <type_expr>

<sum_type_def> ::= "|" <variant> ("|" <variant>)*
<variant>      ::= <ident> <type_expr>?
```

- `type Name ... = <type_def>` defines a nominal named type.
- `type ~Name ... = <type_expr>` defines a structural type alias.
- `trait Name ... = ... end` defines a trait.
- `trait ~Alias = <ident|path>` defines a trait alias.
- Recursive type aliases are rejected.
- Recursive nominal definitions are allowed only for sum types (`| ...`).

## Type Expressions
```bnf
<type_expr> ::= <type_forall_expr>
              | <type_fn_expr>

<type_forall_expr> ::= "for" <ident>+
                       "in" <type_expr>
                       ("where" <trait_constraint_list>)?

<trait_constraint_list> ::= <trait_constraint> ("," <trait_constraint>)* ","?
<trait_constraint>      ::= (<ident> | <path>) <type_atom>*

<type_fn_expr>    ::= <type_apply_expr> ("->" <type_fn_expr>)?
<type_apply_expr> ::= <type_atom> (<type_atom>)*

<type_atom> ::= <ident>
              | <path>
              | "(" ")"
              | "(" <type_expr> ")"
              | "(" <type_tuple_elems> ")"
              | "[" "]"

<type_tuple_elems> ::= <type_expr> "," (<type_expr> ("," <type_expr>)*)? ","?
```

- Type application is left-associative: `A B C` means `(A B) C`.
- `->` is right-associative: `A -> B -> C` means `A -> (B -> C)`.
- `(T)` is grouping.
- `(T,)` is a 1-tuple.

## Expressions
```bnf
<expr> ::= <let_expr>
         | <use_expr>
         | <fn_expr>
         | <if_expr>
         | <match_expr>
         | <seq_expr>

<let_expr>   ::= "let" <pattern> "=" <expr> "in" <expr>
<use_expr>   ::= "use" (<ident> | <path> | "bundle") ("as" <ident>)? "in" <expr>
<fn_expr>    ::= "fn" <parameter>* "=>" <expr>
               | "fn" <match_arm>+
<if_expr>    ::= "if" <expr> "then" <expr> "else" <expr>
<match_expr> ::= "match" <expr> "with" <match_arm>+

<parameter> ::= <ident>
              | "(" <ident> ":" <type_expr> ")"

<match_arm> ::= "|" <pattern> "=>" <expr>

<seq_expr>      ::= <and_expr> (";" <seq_expr>)?
<and_expr>      ::= <cmp_expr> ("and" <cmp_expr>)*
<cmp_expr>      ::= <or_pipe_expr> (<cmp_op> <or_pipe_expr>)?
<cmp_op>        ::= "==" | "!=" | "<" | "<=" | ">" | ">="
<or_pipe_expr>  ::= <xor_expr> (("or" | "|>" | "+>" | "*>") <xor_expr>)*
<xor_expr>      ::= <shift_expr> ("xor" <shift_expr>)*
<shift_expr>    ::= <add_expr> ((">>" | "<<") <add_expr>)*
<add_expr>      ::= <mul_expr> (("+" | "-") <mul_expr>)*
<mul_expr>      ::= <unary_expr> (("*" | "/" | "mod") <unary_expr>)*

<unary_expr> ::= ("not" | "-") <unary_expr>
               | <apply_expr>

<apply_expr> ::= <postfix_expr> (<postfix_expr>)*

<postfix_expr> ::= <atom_expr> ("." <ident>)*

<atom_expr> ::= <literal>
              | <ident>
              | <path>
              | <inline_wasm_expr>
              | "(" ")"
              | "(" <expr> ")"
              | "(" <tuple_expr_elems> ")"
              | "[" <array_elem_list>? "]"
              | "{" <field_def_list>? "}"

<inline_wasm_expr> ::= "(" "wasm" ":" <type_expr> ")" "=>" <sexpr>

<tuple_expr_elems> ::= <expr> "," (<expr> ("," <expr>)*)? ","?

<array_elem_list> ::= <array_elem> ("," <array_elem>)* ","?
<array_elem>      ::= <expr>
                    | ".." <expr>

<field_def_list> ::= <field_def> ("," <field_def>)* ","?
<field_def>      ::= <ident> ("=" | ":") <expr>
```

- Field access binds tighter than application: `f x.y` means `f (x.y)`.
- Application is left-associative: `f a b` means `(f a) b`.
- Unary operators are right-binding.
- Comparison operators are non-associative; chains like `a < b < c` are rejected.
- `;` is an expression operator only (not a statement terminator) and is right-associative.
- Special forms (`let ... in`, `use ... in`, `if ... then ... else`, `match ... with`, `fn ...`) are parsed as whole expressions and are right-greedy.
- Because `<apply_expr>` consumes only `<postfix_expr>`, special forms must be parenthesized when passed as function arguments.

## Patterns
```bnf
<pattern> ::= <path> <pattern_arg>
            | <annot_pattern>

<pattern_arg> ::= <annot_pattern>

<annot_pattern> ::= <pattern_atom> (":" <type_expr>)?

<pattern_atom> ::= <ident>
                 | <path>
                 | <literal>
                 | "(" <pattern> ")"
                 | "(" <tuple_pattern_elems> ")"
                 | "[" <pat_array_elem_list>? "]"
                 | "{" <pat_field_list>? "}"

<tuple_pattern_elems> ::= <pattern> "," (<pattern> ("," <pattern>)*)? ","?

<pat_array_elem_list> ::= <pat_array_elem> ("," <pat_array_elem>)* ","?
<pat_array_elem>      ::= <pattern>
                        | ".." <ident>?

<pat_field_list> ::= <pat_field> ("," <pat_field>)* ("," "..")? ","?
                   | ".."
<pat_field>      ::= <ident> ("=" <pattern>)?
```

- Constructor patterns take exactly one direct argument (`<path> <pattern_arg>`).
- Chained constructor patterns require parentheses (`A (B C)`), so bare `A B C` is invalid.
- Type annotation binds tighter than constructor application: `Ctor x : T` means `Ctor (x : T)`.
- At most one `: <type_expr>` annotation is allowed per pattern node.
- Rest elements (`..name?`) are syntactically allowed anywhere in array patterns; positional validation is a later semantic pass.
- Record patterns are closed by default (`{x}`), and become open only with an explicit rest marker (`{x, ..}`).

## S-Expressions (inline wasm)
```bnf
<sexpr> ::= "(" <sexpr_item>* ")"

<sexpr_item> ::= <sexpr>
               | <sexpr_path>
               | <sexpr_ident>
               | <string>
               | <integer>
               | <natural>
               | <real>
               | "true"
               | "false"

<sexpr_path>  ::= "$" <ident> "::" <ident> ("::" <ident>)*
<sexpr_ident> ::= "$"? <ident> ("." <ident>)*

<sexpr_symbol_ident> ::= "$" <ident>
```

### Inline WASM forms
```bnf
<wasm_declaration> ::= "(" "type" <sexpr_symbol_ident> <wasm_type> ")"
                     | "(" "global" <sexpr_symbol_ident> <wasm_type> ")"
                     | "(" "func" <sexpr_symbol_ident> <func_section>* <instruction>* ")"
                     | "(" "memory" <sexpr_symbol_ident> <integer> <integer>? ")"

<func_section> ::= "(" "param" (<sexpr_symbol_ident> <wasm_type>)+ ")"
                 | "(" "result" <wasm_type>+ ")"
                 | "(" "local" (<sexpr_symbol_ident> <wasm_type>)+ ")"

<inline_wasm_body> ::= "(" <local_decl>* <instruction>* ")"
<local_decl>       ::= "(" "local" (<sexpr_symbol_ident> <wasm_type>)+ ")"

<instruction> ::= <sexpr_ident> <sexpr_item>*

<wasm_type> ::= "any" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
              | <sexpr_ident>
              | "(" "struct" <wasm_type>* ")"
              | "(" "array" <wasm_type> ")"
              | "(" "func" ("(" "param" <wasm_type>* ")")* ("(" "result" <wasm_type>* ")")* ")"
```

- Instruction streams are flat (token-by-token), not nested WAT-style instruction trees.
- In `(wasm : ...) => (...)` expressions, only `(local ...)` declarations are valid before instructions.
- `wasm =>` accepts either a single `<wasm_declaration>` or a parenthesized list of declarations, and the list form may be empty (`wasm => ()`).
- Memory limits use 32-bit page counts; when a maximum is present, it must be `>=` the initial size.
- The first token of each instruction is an opcode identifier (for example `get`, `call`, `struct.new`, `i32.add`).

## Lexical Elements

### Identifiers and paths
```bnf
<ident> ::= <bare_ident> | <bracketed_ident>

<bare_ident> ::= <xid_start> <xid_continue>* ("-" <xid_start> <xid_continue>*)*
<bracketed_ident> ::= "[" <bracketed_ident_text> "]"

<path> ::= "root" "::" <path_segment> ("::" <path_segment>)*
         | "bundle" "::" <path_segment> ("::" <path_segment>)*
         | <path_segment> "::" <path_segment> ("::" <path_segment>)*

<path_segment> ::= <ident>
```

- `<xid_start>` and `<xid_continue>` use Unicode XID character classes.
- Bare identifiers support kebab-style segments (`foo-bar-baz`); leading/trailing `-` remain separate operator tokens.
- `root` and `bundle` are reserved keywords; `bundle` is also allowed as a standalone `use` target.
- Bracketed identifiers may contain operator-like text (examples: `[+]`, `[ + ]`, `[not]`).

### Literals
```bnf
<integer> ::= <decimal_integer>
            | <binary_integer>
            | <octal_integer>
            | <hex_integer>

<natural> ::= <decimal_integer> "n"
            | <binary_integer> "n"
            | <octal_integer> "n"
            | <hex_integer> "n"

<decimal_integer> ::= <dec_digit> ("_"? <dec_digit>)*
<binary_integer>  ::= "0b" <bin_digit> ("_"? <bin_digit>)*
<octal_integer>   ::= "0o" <oct_digit> ("_"? <oct_digit>)*
<hex_integer>     ::= "0x" <hex_digit> ("_"? <hex_digit>)*

<real> ::= <decimal_integer> "." <decimal_integer> <exponent_part>?
         | <decimal_integer> <exponent_part>

<exponent_part> ::= ("e" | "E") ("+" | "-")? <decimal_integer>

<string>        ::= "\"" <string_char>* "\""
<glyph>         ::= "'" <glyph_char> "'"
<format_string> ::= "`" <format_item>* "`"

<format_item> ::= <format_text>
                | "{}"
                | "{{"
                | "}}"
```

- Strings and glyphs support escapes: `\\n`, `\\t`, `\\r`, `\\\\`, `\\\"`, `\\'`, `\\u{HEX+}`.
- A glyph literal must decode to exactly one Unicode scalar value.
- Numeric literals are unsigned; leading sign is tokenized as an operator (`-1` is `-` + `1`).
- Natural literals are integer literals with an immediate `n` suffix (examples: `0n`, `0b101n`, `0o755n`, `0xFFn`).
- Format strings are dedicated literals with placeholder tokens only (`{}`); they do not embed expressions.

### Comments and whitespace
- Line comments: `-- ...` to end of line.
- Block comments: `(* ... *)`, nestable.
- Newlines are ordinary whitespace outside literals/comments.

### Keywords and operators
- Reserved keywords:
  - `bundle`, `module`, `end`
  - `let`, `do`, `use`, `as`, `in`
  - `type`, `trait`, `impl`, `for`, `where`
  - `fn`, `if`, `then`, `else`, `match`, `with`
  - `wasm`, `root`, `true`, `false`
  - `mod`, `xor`, `or`, `and`, `not`
- Fixed operator set (no additional user-defined infix operators):
  - `+`, `-`, `*`, `/`, `mod`
  - `>>`, `<<`
  - `xor`
  - `or`, `|>`, `+>`, `*>`
  - `==`, `!=`, `<`, `<=`, `>`, `>=`
  - `and`
  - `;`

### Expression operator precedence (highest to lowest)
1. Field access (`.`)
2. Function application
3. Unary prefix (`not`, unary `-`)
4. `*`, `/`, `mod`
5. `+`, `-`
6. `>>`, `<<`
7. `xor`
8. `or`, `|>`, `+>`, `*>`
9. `==`, `!=`, `<`, `<=`, `>`, `>=` (non-associative)
10. `and`
11. `;`

Associativity:
- Unary prefix operators are right-binding.
- Binary levels are left-associative unless stated otherwise.
- Comparison operators are non-associative.
- `;` is right-associative.

### Name resolution notes (semantic)
- Internally, resolved paths use `major::minor`, where `major` is the bundle name and `minor` is the declaration path inside that bundle.
- `root::...` is fully qualified.
- `bundle::...` is bundle-qualified and anchored at the current bundle root.
- `use bundle` is shorthand for `use root::<current_bundle_name>`.
- Free paths (no prefix) resolve relative to current module scope first, then through `use`, then as absolute `<bundle>::...`.
- Module-level `use` applies to following statements in the same module.
- Expression-level `use ... in ...` applies only in its `in` body.
- `use M` opens `M` into the current scope.
- `use M as X` adds alias `X` for module path lookups (`X::name`) without opening contents.
- `as` alias collisions are errors.
- If multiple opened modules provide the same symbol, usage is ambiguous and reported as an error.
