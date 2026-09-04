# Definition Metadata

Status: ready-for-agent

## Problem Statement

A Ruddy programmer has no way to say anything about a top-level definition
other than its name, its type, and its value. There is nowhere to record that a
function is deprecated, which host feature an extern needs, which test group a
value belongs to, or anything else a tool outside the compiler might want to
know about a definition. Comments can hold the words, but a comment is thrown
away by the lexer, so nothing downstream — the artifact a dependent bundle
reads, the debugger, an editor, a documentation generator — can see it.

The language has no attribute or annotation syntax at all, and `@` begins no
token, so today `@` is reported as a character the language does not use.

## Solution

Let zero or more attributes precede any top-level definition. An attribute is
`@key` or `@key <literal>`: a key spelled like an identifier, and an optional
literal value. A definition's metadata is the struct built from its attributes,
one field per key:

```
@deprecated "use nat::add instead"
@since 2n
@stability #Experimental
@js { module: "host/math", export: "add" }
extern add : fn(Nat, Nat) -> Nat = "(a, b) => a + b"

@test
let adds = add 1n 2n
```

The value is data, not code: a string, a number, a boolean, a tag with an
optional literal payload, or a tuple, array, or struct of those. A bare `@key`
carries unit, exactly as a bare `#Tag` does. The compiler gives no key any
meaning: it checks the shape, refuses a repeated key, carries the metadata
through every phase unchanged, shows it in the debugger, and publishes it in
the bundle's artifact for every value, type, effect, and module, so that tools
and dependents can read it. Nothing about a definition's type, its grouping,
its inferred scheme, or its generated JavaScript depends on its metadata.

## User Stories

1. As a Ruddy programmer, I want to write `@key <literal>` in front of a top-level definition, so that I can attach a named piece of data to it.
2. As a Ruddy programmer, I want to write several attributes in front of one definition, so that a definition can carry more than one piece of metadata.
3. As a Ruddy programmer, I want a bare `@key` to carry unit, so that a flag such as `@test` needs no value, the way a bare `#None` needs none.
4. As a Ruddy programmer, I want `@key ()` and `@key` to mean the same thing, so that the rule for an absent value has no special case.
5. As a Ruddy programmer, I want an attribute's value to be a string, natural, integer, real, or boolean literal, so that the ordinary scalar data of the language is available as metadata.
6. As a Ruddy programmer, I want a raw `\\` string, including one spanning several lines, accepted as an attribute's value, so that a long description such as a doc string is written the way any other multi-line string is.
7. As a Ruddy programmer, I want an attribute's value to be a tag with an optional literal payload, so that metadata can name a choice such as `#Experimental` or `#Removed 3n`.
8. As a Ruddy programmer, I want an attribute's value to be a tuple, array, or struct whose parts are themselves literal values, so that metadata can be structured and nested.
9. As a Ruddy programmer, I want struct and array values in metadata to accept a trailing comma, so that they follow the convention every other struct and array literal follows.
10. As a Ruddy programmer, I want struct fields in a metadata value to accept bare, quoted, and numeric labels, so that a metadata struct is written like any other struct.
11. As a Ruddy programmer, I want attributes on a `let`, an `extern`, a `type`, an `effect`, and a `module`, so that every kind of top-level definition can carry metadata.
12. As a Ruddy programmer, I want attributes on a definition inside an inline `module ... end` body, so that a module's definitions are as describable as the file's.
13. As a Ruddy programmer, I want attributes on a bare `module name` whose body is another file, so that the module itself can carry metadata.
14. As a Ruddy programmer, I want the metadata of a pattern `let` such as `let (a, b) = ...` carried by every name the pattern binds, so that a definition's metadata is never lost by taking its value apart.
15. As a Ruddy programmer, I want metadata to be inert: no key changes what a definition means, how it is typed, how it is grouped, or what JavaScript it compiles to, so that adding metadata can never break a program.
16. As a Ruddy programmer, I want any key accepted, so that the metadata I attach is mine and the compiler does not tell me which words I may use.
17. As a Ruddy programmer, I want two attributes with the same key on one definition refused, with the message pointing at the second and naming the first, so that a definition's metadata is a struct with one value per key.
18. As a Ruddy programmer, I want duplicate fields inside a metadata struct value refused the way duplicate fields in any struct literal are, so that the same rule holds at every depth.
19. As a Ruddy programmer, I want a value that is not a literal, such as `@since add 1n 2n` or `@id -1i`, refused in plain English that says a metadata value must be a literal and lists what a literal may be, so that I know the value is data and not code.
20. As a Ruddy programmer, I want an attribute followed by nothing that begins a definition, such as an attribute at the end of a file or before `end`, refused at the attribute in plain English as metadata with no definition to describe, so that the complaint points at what I wrote rather than at what I did not.
21. As a Ruddy programmer, I want an attribute on a `let` inside a `do` block refused in plain English as metadata that belongs to a top-level definition, so that I am not left guessing whether local definitions can carry it.
22. As a Ruddy programmer, I want an attribute where an expression is expected, such as `let x = @key 1n`, refused in plain English as metadata that goes in front of a definition, so that the fix is named.
23. As a Ruddy programmer, I want `@` with nothing identifier-shaped after it reported as a malformed attribute, in the way `#` and `!` alone are reported, so that `@` is understood as the start of a key rather than as a character the language does not know.
24. As a Ruddy programmer, I want the debugger to show each definition's metadata in its parse, IR, and artifact views, so that I can confirm what the compiler carried.
25. As a Ruddy programmer, I want the debugger's token view to paint `@key` in its own colour, so that attributes are distinguishable from tags and effects.
26. As a bundle author, I want every exported value's metadata published in the bundle's artifact, so that a dependent bundle or a tool can read it without the source.
27. As a bundle author, I want every declared type's and effect's metadata published in the artifact, so that publication is not limited to values.
28. As a bundle author, I want every module's metadata published in the artifact, so that a module-level annotation is not the one kind that is lost.
29. As a bundle author, I want a hidden definition such as `let _ = ...` to publish nothing, so that metadata publication follows the artifact's existing rule about which definitions are exported.
30. As a bundle author, I want metadata to survive the artifact's write and read exactly: strings with their escapes, naturals, integers, and reals kept distinct, tags with their payloads, and nested values in order, so that a tool reads what I wrote.
31. As a tooling author, I want the artifact's metadata for a definition to be an empty struct when none was written, so that a reader never has to ask whether the field is there.
32. As a tooling user, I want the editor grammar to parse attributes and to highlight `@key` as an attribute, so that source tooling agrees with the compiler.
33. As a compiler maintainer, I want a metadata value held as a literal data tree rather than as a term, so that inference, pattern checking, and lowering never see one and cannot be asked what it computes.
34. As a compiler maintainer, I want the debugger's surface and normalized printers to emit attributes that reparse to the same tree, so that the fixed-point printing tests keep holding.
35. As a compiler maintainer, I want the demo program to carry an example of every attribute form, so that the debugger's default document shows the syntax.

## Implementation Decisions

- The lexer reads `@` followed by an identifier-shaped name as one attribute token carrying the key without its sigil, through the same reader that produces tag and effect-label tokens. `@` followed by anything else is a malformed-attribute lexer error, replacing the current "invalid character" report for `@`. The `@` is not part of the key, exactly as `#` and `!` are not part of theirs.
- The grammar of a statement becomes `<attribute>* <definition>`. An attribute is the attribute token followed by an optional literal value. The value is present exactly when the next token can begin a literal; nothing that begins a definition, another attribute, or a block's `end` can begin a literal, so the reading is unambiguous.
- A literal is: a string, in either the quoted or the raw `\\` line form, since both lex to the one string token; a natural, integer, or real; a boolean; `()`; a tuple of literals in parentheses, read as parentheses are in an expression, so that a lone parenthesized literal groups and a trailing comma makes it a tuple; an array of literals in brackets; a struct of literals in braces with bare, quoted, or numeric labels; or a tag with an optional literal payload. Struct and array literals accept a trailing comma. A struct or array literal may contain no spread, and a literal contains no name, path, application, operator, `fn`, `if`, `match`, `do`, `handle`, `raise`, or effect operation.
- A tag's payload is one literal, read greedily, as a tag's payload is in an expression. A tag written without a payload carries unit, and a bare attribute carries unit, so `@key`, `@key ()`, `#Flag`, and `#Flag ()` all denote the same value in the same way.
- The surface tree carries a metadata value as its own literal tree, distinct from the expression tree. No phase converts one into a term.
- The parser attaches the attributes read to the statement they precede. Each attribute keeps the span of its key and the span of its value; the statement's own span is unchanged, so existing diagnostics that point at a definition keep pointing where they did.
- The statement reader used for `do` blocks reads attributes too, so that an attribute before a block-level `let` reaches the block's own refusal: the attribute is reported at its span as metadata belonging to a top-level definition, in the way a `type` inside a block is reported, and reading continues past the statement.
- Attributes with no definition after them are reported at the span from the first attribute to the last, as metadata with nothing to describe. Recovery then skips to the next definition as it does after any broken statement, so the token that stopped the attributes costs no second complaint.
- A token that can begin an expression atom but not a literal, met where a literal value is expected — at the top of an attribute value, inside a tuple, array, struct field, or tag payload — is reported as a metadata value needing to be a literal. The help lists the literal forms. Anything else met there falls to the usual unexpected-token complaints.
- An attribute token met where an expression atom is expected is reported as metadata that goes in front of a definition, at the token's span.
- Lowering refuses a key repeated across one definition's attributes, at the second attribute's key with the first as related, and refuses a duplicate field inside a metadata struct with the existing duplicate-field diagnostic. Keys are compared as decoded names, the way struct labels are.
- Metadata may nest at most a fixed number of levels, shared by lowering and the artifact reader, so that a reader can hold a file's metadata in an ordinary recursive tree without trusting the file. Lowering reports a value below the limit in plain English as nested too deeply and lets unit stand in for it; the artifact reader refuses text nested below it. The limit is one compiler constant used on both sides, so an artifact the compiler writes is one the reader admits.
- Every declaration in the IR — extern, term, type, effect, and module — carries its metadata as a map from key to literal value, in written order. A pattern `let` gives each name it binds the whole metadata of the statement; a definition that binds no name keeps its metadata in the surface tree and nowhere else.
- Inference, pattern checking, the effect system, grouping, generalization, extern review, and the JavaScript backend do not read metadata. Two programs that differ only in metadata produce identical generated JavaScript.
- The artifact header gains a metadata entry on every value, declared type, and declared effect, and gains a list of the bundle's modules in declaration order, each with its qualified name and metadata. The entry is a struct of literal data and is empty when no attribute was written. The literal representation distinguishes naturals, integers, and reals, keeps strings as decoded text, and keeps tags with their payloads. The artifact text writer and parser round-trip it exactly. This is a schema change; the artifact format is internal and no compatibility with earlier artifacts is required.
- Dependency import does not read metadata into the IR. A dependent bundle's compiler never needs it; tools read it from the artifact.
- The debugger's token stage names the attribute token kind and paints it with a class of its own. Its parse, IR, and artifact stages show each definition's metadata beside the definition.
- The debugger's surface printer prints each attribute in front of the definition it belongs to, key first, then the value when it is not unit, separated by spaces as the printers write everything else, using the existing string, field-label, and tag-label writers. The normalized printer prints a declaration's metadata the same way. A unit value prints as the bare attribute, and so does a tag's unit payload.
- The tree-sitter grammar adds an attribute rule with a `key` field and an optional `value` field, and a literal rule family mirroring the compiler's. The statement rule becomes attributes followed by a definition. The highlights query captures the attribute token as `@attribute`. The reserved word list is unchanged.
- The demo program gains one definition per attribute form: a bare attribute, a scalar value, a tag with a payload, a nested struct, an array, a tuple, and several attributes on one definition. Its existing `let bad = @` line stays as an error example and now reports a malformed attribute.
- The standard library is unchanged. No key is reserved or interpreted; a future feature that gives a key meaning does so in its own spec.
- The README gains a paragraph describing the syntax beside the other language feature notes.

## Testing Decisions

- Prefer external behavior over internal representation: accepted source, diagnostics, printed source, the artifact's published text, and executed JavaScript, rather than fields of the surface tree.
- One checked-in end-to-end bundle compiled through the CLI and executed with Node has definitions carrying every attribute form on a `let`, an `extern`, a `type`, an `effect`, and an inline and a file-backed `module`, and proves the program computes exactly what it would without them.
- Artifact tests build the bundle, then assert the published metadata for a value, a type, an effect, and a module; that a `let _` publishes nothing; that each name of a pattern `let` carries the statement's metadata; that a definition with no attributes publishes an empty struct; and that writing the artifact to text and parsing it back yields an equal artifact, with naturals, integers, and reals still distinct and a string's escapes intact.
- Public compilation tests assert that a definition's inferred scheme is the same with and without metadata, and that generated JavaScript is byte-identical for two sources that differ only in metadata.
- Rejected compilation tests cover a repeated key, a duplicate field inside a metadata struct, a non-literal value at the top of a value and inside a struct field, a value nested below the depth limit, an attribute at end of file, an attribute before `end`, an attribute on a block-level `let`, an attribute in expression position, and `@` alone.
- Structured diagnostic tests pin codes, primary spans, related spans, titles, and help for every new message. Prose stays in plain English per the project's diagnostic style.
- Public parser tests cover a bare attribute, a value of each literal kind, a tag with and without a payload, nesting, trailing commas, quoted and numeric struct labels, several attributes on one definition, attributes on each of the five definition kinds, attributes inside an inline module body, and the equivalence of `@key` and `@key ()`.
- Printer fixed-point tests cover attributes on each definition kind, every literal form, and the bare printing of a unit value, in both the surface and normalized trees.
- Parser and printer tests include a raw multi-line string as an attribute value: it is accepted, its decoded text has the newlines the lexer joined its lines with, and the printer's quoted spelling of it reparses to the same value.
- Lexer tests cover the attribute token's key and span, `@` alone, `@` before a digit, and `@` before punctuation.
- Debugger snapshot tests cover the token class and the metadata shown in the parse, IR, and artifact stages.
- Tree-sitter corpus tests mirror every accepted and rejected surface form, and a highlighting test covers the attribute capture. Run through `just grammar`.
- Prior art is the do-blocks bundle and its parser, diagnostic, and printer tests; the struct-spread artifact and printer tests; and the existing token tests for `#` and `!` sigils.
- All Rust tests run through `just test`. Coverage stays at the required level through `just cov`.

## Out of Scope

- Any compiler-interpreted key: deprecation warnings, test discovery, inlining, export renaming, or documentation extraction. Each is a feature of its own that would build on this one.
- Reading metadata from Ruddy code at compile time or run time.
- Attributes on anything that is not a top-level definition: function parameters, struct fields, sum cases, effect operations, match arms, handler arms, or `let` statements inside `do` blocks.
- Negative numeric literals in a metadata value. `-1i` is a unary-minus expression, and a literal grammar for it is a small follow-up if it is wanted.
- Quoted keys such as `@"my key"`, path-shaped keys such as `@js::export`, and any key namespacing beyond a nested struct value.
- Merging or inheriting metadata between definitions, modules, or bundles.
- Importing a dependency's metadata into the IR of the bundle that depends on it.
- Doc comments or any relationship between comments and metadata.
- A stable, versioned artifact format.

## Further Notes

- The attribute is the fourth sigilled token after `#Tag`, `!Effect`, and `'var`, and it borrows the tag's two rules: sigil and name are one token, and an absent payload is unit. A reader who knows `#None` knows `@test`.
- Restricting values to literals is what keeps metadata inert. A value that could compute would have to be typed, and a value that is typed would give a key the power to fail a build. Data cannot.
- Metadata rides on the IR's existing per-declaration record rather than on a side table keyed by symbol, so a phase that already has a declaration has its metadata and nothing has to be looked up twice.
- Publishing modules in the artifact is new. Values, types, and effects were already listed; modules were only ever visible as segments of qualified names. Listing them is the smallest change that gives module metadata somewhere to live.
- The repository has no domain glossary or ADR that bears on this design.
