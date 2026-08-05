//! Tests for [`ruddy::ir`].

use indexmap::IndexMap;
use ruddy::{
    ir::{ErrorKind, Field, Output, Term, TermKind, TypeField, TypeKind, build},
    parse,
    symbol::{Bundle, Mint, Namespace, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::{ParamKind, Prim},
};
use ruddy_debug::print;

/// A row parameter that may not stand for any of `labels` — the shape
/// [`ParamKind::Row`] carries, written the way a test wants to read it.
fn row(labels: &[&str]) -> ParamKind {
    ParamKind::Row(labels.iter().map(|label| label.to_string()).collect())
}

/// A mint for the builder to mint into. Fresh per build, so one test's
/// symbols cannot show up in another's.
fn dummy_mint() -> Mint {
    Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
}

fn build_src(src: &str) -> (Mint, Output) {
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(
        parsed.errors.is_empty(),
        "unexpected parse errors: {:#?}",
        parsed.errors
    );
    let mut mint = dummy_mint();
    let out = build(&mut mint, parsed.stmts);
    (mint, out)
}

fn built(src: &str) -> (Mint, Output) {
    let (mint, out) = build_src(src);
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    (mint, out)
}

/// Find a top-level term by the name it was defined under. Nothing outside
/// the builder maps names to symbols any more, so this walks the program.
fn term_symbol(mint: &Mint, out: &Output, name: &str) -> Symbol {
    *out.program
        .terms
        .keys()
        .find(|symbol| mint.name(**symbol) == name)
        .unwrap_or_else(|| panic!("no term named {name}"))
}

fn term_value<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a TermKind {
    &out.program.terms[&term_symbol(mint, out, name)].value.kind
}

/// The struct a definition evaluates to, looking through any lambdas
/// wrapped around it — a test needs those to bind the names its fields
/// refer to, but they are not what it is asserting about.
fn term_fields<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a IndexMap<String, Field<Term>> {
    let mut node = term_value(mint, out, name);
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    match node {
        TermKind::Struct(fields) => fields,
        other => panic!("expected a struct term, got {other:?}"),
    }
}

fn type_symbol(mint: &Mint, out: &Output, name: &str) -> Symbol {
    *out.program
        .types
        .keys()
        .find(|symbol| mint.name(**symbol) == name)
        .unwrap_or_else(|| panic!("no type named {name}"))
}

fn type_fields<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a IndexMap<String, TypeField> {
    let node = &out.program.types[&type_symbol(mint, out, name)]
        .value
        .tracked;
    match node {
        TypeKind::Struct { fields, .. } => fields,
        other => panic!("expected a struct type, got {other:?}"),
    }
}

/// Lower a program and render it back. The IR prints as surface syntax, so
/// the rendering is re-lowered to confirm it parses and describes the same
/// program — which is what makes the printer's parentheses trustworthy.
fn display_program(src: &str) -> String {
    let (mint, out) = built(src);
    let printed = print::ir::program(&out.program, &mint).to_string();

    let (remint, relowered) = built(&printed);
    assert_eq!(
        print::ir::program(&relowered.program, &remint).to_string(),
        printed,
        "printing {src:?} did not round-trip"
    );
    printed
}

#[test]
fn displays_curried_functions() {
    // The surface form binds both arguments at one `fn`; the IR does not,
    // and the printer shows the currying rather than hiding it.
    assert_eq!(
        display_program("let k = fn f a b => f a b"),
        "let k = fn f => fn a => fn b => f a b"
    );
}

#[test]
fn displays_application_grouping() {
    assert_eq!(
        display_program("let a = fn f g x => f (g x)"),
        "let a = fn f => fn g => fn x => f (g x)"
    );
    // Redundant grouping is gone; necessary grouping is reconstructed.
    assert_eq!(
        display_program("let b = fn f g x => (f g) x"),
        "let b = fn f => fn g => fn x => f g x"
    );
    assert_eq!(
        display_program("let c = fn map => map fn x => x"),
        "let c = fn map => map (fn x => x)"
    );
}

#[test]
fn displays_structs_and_unit() {
    assert_eq!(
        display_program("let p = fn a b => { x: a, y: b }"),
        "let p = fn a => fn b => { x: a, y: b }"
    );
    // `()` the value and `()` the type both lower to the struct with no
    // fields, so both read back as `{}` rather than as what was written.
    assert_eq!(display_program("let u = ()"), "let u = {}");
    assert_eq!(
        display_program("type T = { items: Nat, next: () }"),
        "type T = { items: Nat, next: {} }"
    );
}

#[test]
fn displays_naturals() {
    assert_eq!(display_program("let n = 0"), "let n = 0");
    assert_eq!(
        display_program("let p = fn f => f 1 (f 2 3)"),
        "let p = fn f => f 1 (f 2 3)"
    );
    assert_eq!(
        display_program("let s = { width: 3, height: 4 }"),
        "let s = { width: 3, height: 4 }"
    );
}

#[test]
fn a_natural_lowers_to_its_value_and_span() {
    let (mint, out) = built("let n = 4096");

    let TermKind::Natural(value) = term_value(&mint, &out, "n") else {
        panic!("expected a natural");
    };
    assert_eq!(*value, 4096);

    let span = out.program.terms[&term_symbol(&mint, &out, "n")].value.span;
    assert_eq!(span.start, 8);
    assert_eq!(span.width, 4);
}

#[test]
fn a_natural_names_nothing() {
    // A literal is not a name, so lowering one mints no symbol and looks
    // none up: only the `let` itself is minted...
    let (mint, _) = built("let n = 7");
    assert_eq!(mint.symbols().count(), 1);

    // ...and an undefined name beside a literal is still the one error.
    let (_, out) = build_src("let m = f 7");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn displays_arrows() {
    // The arrow is right-associative, so nesting to the right needs no
    // parentheses and the printer drops the ones that were written.
    assert_eq!(
        display_program("type F = Nat -> Nat -> Nat"),
        "type F = Nat -> Nat -> Nat"
    );
    assert_eq!(
        display_program("type G = Nat -> (Nat -> ())"),
        "type G = Nat -> Nat -> {}"
    );
    // The one grouping the printer has to reconstruct: an arrow on the left
    // of an arrow, which would otherwise re-parse as the right half of one.
    assert_eq!(
        display_program("type H = (Nat -> Nat) -> ()"),
        "type H = (Nat -> Nat) -> {}"
    );
    // A struct is atomic on either side, and its fields are types in their
    // own right.
    assert_eq!(
        display_program("type K = { f: Nat -> Nat, g: () -> Nat }"),
        "type K = { f: Nat -> Nat, g: {} -> Nat }"
    );
    // A declaration reads the same as a primitive wherever a type may stand.
    assert_eq!(
        display_program("type A = ()  type B = A -> A"),
        "type A = {}\ntype B = A -> A"
    );
}

#[test]
fn displays_projections() {
    assert_eq!(
        display_program("let a = fn p => p.x"),
        "let a = fn p => p.x"
    );
    assert_eq!(
        display_program("let b = fn p => p.x.y"),
        "let b = fn p => p.x.y"
    );
    // Redundant grouping is gone; necessary grouping is reconstructed.
    assert_eq!(
        display_program("let c = fn f p => f p.x"),
        "let c = fn f => fn p => f p.x"
    );
    assert_eq!(
        display_program("let d = fn f p => (f p).x"),
        "let d = fn f => fn p => (f p).x"
    );
    assert_eq!(
        display_program("let e = fn a => { x: a }.x"),
        "let e = fn a => { x: a }.x"
    );
}

#[test]
fn a_projected_field_is_a_label_and_not_a_name() {
    // The field resolves to nothing, so an undefined name inside a
    // projection can only ever be the base.
    let (mint, out) = built("let a = fn p => p.x");

    let mut node = term_value(&mint, &out, "a");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Project { base, field } = node else {
        panic!("expected a projection, got {node:?}");
    };
    assert_eq!(field.tracked, "x");
    // Written where it was written, so a diagnostic can point at the label
    // rather than at the whole projection.
    assert_eq!(field.span.start, 18);
    assert_eq!(field.span.width, 1);
    assert!(matches!(base.kind, TermKind::Ident(s) if mint.name(s) == "p"));

    // A field name never has to resolve; only the base does.
    let (_, out) = build_src("let b = q.x");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
}

#[test]
fn displays_ascriptions() {
    assert_eq!(display_program("let x : () = ()"), "let x : {} = {}");
    assert_eq!(
        display_program("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"),
        "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"
    );
    // The annotation resolves in the type namespace, and a declaration is
    // in scope for it exactly as it would be in a `type` body.
    assert_eq!(
        display_program("type T = ()  let u : T = ()"),
        "type T = {}\nlet u : T = {}"
    );
    // Printing hoists types the way lowering does, so an interleaved program
    // comes back in the order the builder saw it — and re-lowering the
    // rendering, which `display_program` does, lands on the same program.
    assert_eq!(
        display_program("let u : T = ()  type T = ()"),
        "type T = {}\nlet u : T = {}"
    );
    assert_eq!(
        display_program("type A = ()  let x : B = ()  type B = A  let y : A = ()"),
        "type A = {}\ntype B = A\nlet x : B = {}\nlet y : A = {}"
    );
}

#[test]
fn an_ascription_resolves_in_the_type_namespace() {
    // A term of the same name is not what `: T` refers to.
    let (_, out) = build_src("let T = ()  let u : T = ()");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Types
        }
    ));

    // A `type` declaration carries no annotation, and a `let` without one
    // carries none either.
    let (_, out) = built("type T = ()  let u = ()");
    assert!(out.program.types.values().all(|d| d.annotation.is_none()));
    assert!(out.program.terms.values().all(|d| d.annotation.is_none()));

    let (mint, out) = built("type T = ()  let u : T = ()");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "u")]
        .annotation
        .as_ref()
        .expect("the ascription was lowered");
    assert!(matches!(annotation.tracked, TypeKind::Ident(s) if mint.name(s) == "T"));
}

#[test]
fn primitives_are_resolved_from_their_spelling() {
    // `Nat` needs no declaration and mints no symbol...
    let (mint, out) = built("type T = Nat");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "T")]
            .value
            .tracked,
        TypeKind::Prim(Prim::Nat)
    ));
    assert_eq!(mint.symbols().count(), 1);
    assert_eq!(
        display_program("type T = Nat -> Nat"),
        "type T = Nat -> Nat"
    );

    // ...but a declaration of one's own is what the name then means.
    let (mint, out) = built("type Nat = ()  type T = Nat");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "T")]
            .value
            .tracked,
        TypeKind::Ident(_)
    ));
}

/// Types are hoisted above terms and above each other: every type's name is
/// bound before any type's body is read, so a declaration can name one written
/// below it, and — the point of the ordering — can name itself.
#[test]
fn types_are_hoisted_above_terms_and_above_each_other() {
    // A type above the declaration sees it, and a declaration of `Nat` beats
    // the built-in wherever it is written.
    let (mint, out) = built("type T = Nat  type Nat = ()");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "T")]
            .value
            .tracked,
        TypeKind::Ident(_)
    ));

    // A term above it does, because every type is lowered first.
    let (mint, out) = built("let u : Nat = ()  type Nat = ()");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "u")]
        .annotation
        .as_ref()
        .expect("the ascription was lowered");
    assert!(matches!(annotation.tracked, TypeKind::Ident(_)));

    // Which holds for a name of the program's own just the same.
    let (mint, out) = built("let u : T = ()  type T = ()");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "u")]
        .annotation
        .as_ref()
        .expect("the ascription was lowered");
    assert!(matches!(annotation.tracked, TypeKind::Ident(s) if mint.name(s) == "T"));
}

#[test]
fn unit_is_the_empty_struct() {
    // The surface syntax spells it as punctuation, but the type it denotes
    // is the struct with no fields — nothing the type language needs a node
    // of its own to say.
    let (mint, out) = built("type T = ()");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "T")]
            .value
            .tracked,
        TypeKind::Struct { ref fields, tail: None } if fields.is_empty()
    ));
    assert_eq!(mint.symbols().count(), 1);
    assert_eq!(
        display_program("type T = () -> ()  let u : () = ()"),
        "type T = {} -> {}\nlet u : {} = {}"
    );

    // Punctuation is not a name, so — unlike `Nat` — no declaration can
    // take the spelling and shadow it.
    let (mint, out) = built("type Nat = ()  type T = ()");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "T")]
            .value
            .tracked,
        TypeKind::Struct { ref fields, tail: None } if fields.is_empty()
    ));

    // The unit *value* folds the same way: `()` the value and `()` the type
    // are different things in different namespaces, but both lower to the
    // struct with no fields.
    let (mint, out) = built("let u = ()");
    assert!(matches!(
        term_value(&mint, &out, "u"),
        TermKind::Struct(fields) if fields.is_empty()
    ));
}

#[test]
fn displays_types_before_terms() {
    assert_eq!(
        display_program("let x = ()  type T = { f: () }  let y = ()"),
        "type T = { f: {} }\nlet x = {}\nlet y = {}"
    );
}

#[test]
fn displays_empty_program_as_nothing() {
    let mut mint = dummy_mint();
    let out = build(&mut mint, Vec::new());
    assert_eq!(print::ir::program(&out.program, &mint).to_string(), "");
}

#[test]
fn names_are_not_visible_before_their_definition() {
    let (_, out) = build_src("let a = b  let b = ()");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
    ));
}

#[test]
fn a_definition_cannot_see_itself() {
    // No recursion: the body is lowered before the name is bound.
    let (_, out) = build_src("let f = f");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
}

#[test]
fn duplicate_definitions_keep_the_first() {
    let (mint, out) = build_src("let x = ()  let x = fn a => a");

    // Reported at the repeat, pointing back at what it repeats.
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 16);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Duplicate {
            namespace: Namespace::Terms,
            previous,
        } if previous.start == 4
    ));

    // One symbol, and it still holds the first definition's body.
    assert_eq!(out.program.terms.len(), 1);
    assert!(matches!(
        term_value(&mint, &out, "x"),
        TermKind::Struct(fields) if fields.is_empty()
    ));
}

#[test]
fn namespaces_do_not_leak() {
    let (_, out) = build_src("type T = ()  let x = T");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
    ));

    let (_, out) = build_src("let x = ()  type T = x");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Types
        }
    ));
}

#[test]
fn arguments_shadow_definitions_and_release() {
    let (mint, out) = built("let x = ()  let f = fn x => x  let g = x");

    let global = term_symbol(&mint, &out, "x");
    let TermKind::Fn { arg, body } = term_value(&mint, &out, "f") else {
        panic!("expected a function");
    };
    // The argument hides the definition for the length of the body...
    assert_ne!(arg.tracked, global);
    assert!(matches!(body.kind, TermKind::Ident(s) if s == arg.tracked));
    // ...and the definition is back in scope afterwards.
    assert!(matches!(term_value(&mint, &out, "g"), TermKind::Ident(s) if *s == global));
}

#[test]
fn sibling_lambdas_bind_distinct_symbols() {
    let (mint, out) = built("let f = fn x => x  let g = fn x => x");

    let arg_of = |name| match term_value(&mint, &out, name) {
        TermKind::Fn { arg, .. } => arg.tracked,
        other => panic!("expected a function, got {other:?}"),
    };
    assert_ne!(arg_of("f"), arg_of("g"));
    assert_ne!(mint.mangle(arg_of("f")), mint.mangle(arg_of("g")));
}

#[test]
fn fields_are_keyed_by_name_in_source_order() {
    let (mint, out) = built("let p = { x: (), y: () }");

    let fields = term_fields(&mint, &out, "p");
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["x", "y"]
    );
    // The name is the key, but the span it was written at is still kept.
    assert_eq!(fields["x"].name_span.start, 10);
    assert_eq!(fields["x"].name_span.width, 1);
    assert_eq!(fields["y"].name_span.start, 17);
}

#[test]
fn duplicate_term_fields_are_rejected() {
    let (mint, out) = build_src("let p = fn a b => { x: a, x: b }");

    // Reported at the offending repeat, not at the first occurrence.
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 26);
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateField));

    // The first occurrence is the one that survives.
    let fields = term_fields(&mint, &out, "p");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields["x"].name_span.start, 20);
    assert!(matches!(fields["x"].value.kind, TermKind::Ident(s) if mint.name(s) == "a"));
}

/// A repeat in a struct *type* is the same complaint, in the same place, as a
/// repeat in a struct literal: the two are re-keyed by one piece of code, and a
/// reader who has learned what the message means about a value should not have
/// to learn it again about a type.
#[test]
fn duplicate_type_fields_are_rejected() {
    let src = "type A = ()  type B = Nat  type T = { a: A, a: B }";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);

    // Reported at the offending repeat, not at the first occurrence.
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateField));
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("a: B").expect("the repeat")
    );

    // The first occurrence is the one that survives, spans and all.
    let fields = type_fields(&mint, &out, "T");
    assert_eq!(fields.len(), 1);
    assert_eq!(
        fields["a"].name_span.start,
        src.find("a: A").expect("the first")
    );
    assert!(matches!(fields["a"].value.tracked, TypeKind::Ident(s) if mint.name(s) == "A"));

    // Annotations go the same way, and the `?` only they may carry rides
    // through the re-keying with the field it was written on.
    let src = "let f : { a?: Nat, a: Nat } -> Nat = fn p => p.a";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateField));
    assert_eq!(
        out.errors[0].span.start,
        src.find("a: Nat").expect("the repeat")
    );

    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .clone()
        .expect("the annotation");
    let TypeKind::Arrow { from, .. } = annotation.tracked else {
        panic!("expected an arrow, got {:?}", annotation.tracked);
    };
    let TypeKind::Struct { fields, .. } = from.tracked else {
        panic!("expected a struct parameter");
    };
    assert_eq!(fields.len(), 1);
    assert!(fields["a"].optional);
}

#[test]
fn a_declared_type_must_be_closed() {
    // A tail or an optional field stands for something a definition gets to
    // decide, and a declaration decides for everyone: each `..` and `?` is
    // refused, and the struct that carried it lowers to the error type.
    for src in [
        "type T = { x: Nat, .. }",
        "type T = { x: Nat, ..r }",
        "type T = { x?: Nat }",
        "type T = { a: { b: Nat, .. } }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::OpenDeclaredType)),
            "{src}: {:#?}",
            out.errors
        );
    }

    // One report per marker, in source order.
    let (_, out) = build_src("type T = { x?: Nat, y?: Nat, .. }");
    assert_eq!(out.errors.len(), 3, "errors: {:#?}", out.errors);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::OpenDeclaredType)),
        "errors: {:#?}",
        out.errors
    );

    // A name that fails to resolve inside an open declaration is still its
    // own report: fixing the `..` should not reveal it.
    let (_, out) = build_src("type T = { x: Missing, .. }");
    assert_eq!(out.errors.len(), 2, "errors: {:#?}", out.errors);

    // An annotation is where openness belongs, and it passes through whole.
    let (_, out) = build_src("let f : { x?: Nat, ..r } -> Nat = fn p => p.x");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

#[test]
fn duplicate_fields_are_rejected_when_nested() {
    let (_, out) = build_src("let p = fn a b => { outer: { y: a, y: b } }");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn repeated_names_in_sibling_structs_are_fine() {
    // Field names are scoped to their own struct.
    let (_, out) = build_src("let p = fn a b => { x: a, inner: { x: b } }");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// A declaration's parameters are bound for the length of its body and gone
/// after it, the way a lambda's arguments are. Each one lowers to its position,
/// which is what unfolding later hands an argument to.
#[test]
fn a_declaration_binds_its_parameters() {
    let (_, out) = built("type Pair A B = { a: A, b: B }");
    let decl = out.program.types.values().next().expect("one declaration");
    assert_eq!(decl.params.len(), 2);

    let TypeKind::Struct { fields, .. } = &decl.value.tracked else {
        panic!("expected a struct: {:#?}", decl.value);
    };
    let indices: Vec<u32> = fields
        .values()
        .map(|field| match field.value.tracked {
            TypeKind::Param { index, .. } => index,
            ref other => panic!("expected a parameter: {other:#?}"),
        })
        .collect();
    assert_eq!(indices, vec![0, 1]);
}

/// A parameter is scoped to the declaration that binds it, so the same name
/// somewhere else is undefined rather than a reference to it.
#[test]
fn a_parameter_is_out_of_scope_outside_its_declaration() {
    let (_, out) = build_src("type Pair A B = { a: A, b: B }  type Other = A");
    assert_eq!(
        out.errors
            .iter()
            .filter(|error| matches!(
                error.kind,
                ErrorKind::Undefined {
                    namespace: Namespace::Types
                }
            ))
            .count(),
        1,
        "errors: {:#?}",
        out.errors
    );
}

/// A parameter hides a declared type of the same name for the length of the
/// body. That is what a scope is for, so it is not a repeat.
#[test]
fn a_parameter_shadows_a_declared_type() {
    let (_, out) = built("type Nat = { n: Nat }  type Box Nat = { it: Nat }");
    let boxed = out
        .program
        .types
        .values()
        .find(|decl| decl.params.len() == 1)
        .expect("the parameterized declaration");
    let TypeKind::Struct { fields, .. } = &boxed.value.tracked else {
        panic!("expected a struct: {:#?}", boxed.value);
    };
    assert!(matches!(
        fields["it"].value.tracked,
        TypeKind::Param { index: 0, .. }
    ));
}

/// A declaration takes what it takes wherever it is written: too few is the
/// same complaint as too many, and a name written bare that takes some is too
/// few.
#[test]
fn a_type_takes_the_arguments_it_declares() {
    for (src, expected, found) in [
        ("type Pair A B = { a: A, b: B }  type M = Pair Nat", 2, 1),
        (
            "type Pair A B = { a: A, b: B }  type M = Pair Nat Nat Nat",
            2,
            3,
        ),
        ("type Pair A B = { a: A, b: B }  type M = Pair", 2, 0),
        ("type T = Nat  type M = T Nat", 0, 1),
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::Arity { expected: e, found: f } if e == expected && f == found
            )),
            "{src}: {:#?}",
            out.errors
        );
    }
}

/// The head of an application nobody could have applied is still a written
/// type, and everything wrong inside it is still reported. Fixing the
/// application should not be the price of hearing about a name that does not
/// exist, any more than it is for a bad name in an argument.
#[test]
fn a_head_that_cannot_be_applied_is_still_lowered() {
    // The same complaint the reader would get without the arguments, plus the
    // one about the arguments.
    let (_, out) = build_src("let f : { x: Bogus } Nat = 1");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "undefined-type"]);

    let (_, out) = build_src("let f : { x: Bogus } = 1");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "undefined-type");

    // A `?` field or a bare `..` inside a head is refused where it stands, for
    // the same reason.
    let (_, out) = build_src("type A = { x: Nat, .. } Nat");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "open-declared-type"]);

    let (_, out) = build_src("type A = { x?: Nat } Nat");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "open-declared-type"]);
}

/// Every complaint in the order the reader would meet it, whichever pass raised
/// it. What a parameter stands for is not known until every body is in, and
/// what a row argument may be is not known until every annotation is, so the
/// passes cannot run in source order — which left the driver printing line 3
/// before line 2, since it prints them in the order they arrive.
#[test]
fn errors_are_reported_in_source_order() {
    for src in [
        "type W r = { x: Nat, ..r }\nlet a : W Nat -> Nat = fn v => 1\nlet b = nosuchname",
        "type W r = { x: Nat, ..r }\nlet a = nosuchname\nlet b : W Nat -> Nat = fn v => 1",
        "let f : { x: Bogus } Nat = 1",
        "type Bad a = { x: a, ..a }\nlet c = alsomissing",
        "type L a = { next: L { x: a } }\nlet d = missing",
    ] {
        let (_, out) = build_src(src);
        let offsets: Vec<usize> = out.errors.iter().map(|error| error.span.start).collect();
        assert!(
            offsets.windows(2).all(|pair| pair[0] <= pair[1]),
            "{src}: {offsets:?}"
        );
    }
}

/// Only a declared type can be applied. A primitive is counted rather than
/// refused outright — it is a type that exists and was given too much — while a
/// struct or a parenthesized arrow is not the sort of thing that takes anything
/// at all.
#[test]
fn only_a_declared_type_can_be_applied() {
    let (_, out) = build_src("type M = Nat Nat");
    assert!(
        out.errors.iter().any(|error| matches!(
            error.kind,
            ErrorKind::Arity {
                expected: 0,
                found: 1
            }
        )),
        "errors: {:#?}",
        out.errors
    );

    for src in ["type M = { x: Nat } Nat", "type M = (Nat -> Nat) Nat"] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::NotAConstructor)),
            "{src}: {:#?}",
            out.errors
        );
    }
}

/// An arity complaint is about the whole application, head and arguments
/// together, because counting the arguments is what the reader has to do and a
/// span around the name alone shows none of what was counted. Pinned because
/// the head's own span is carried right beside it, for the complaints that
/// *are* about the name.
#[test]
fn a_wrong_argument_count_underlines_the_whole_application() {
    let src = "type Pair a b = { f: a, s: b }  let p: Pair Nat Nat Nat = 1";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::Arity { .. }));

    let written = "Pair Nat Nat Nat";
    let at = src.rfind(written).expect("the application");
    assert_eq!(out.errors[0].span.start, at);
    assert_eq!(out.errors[0].span.width, written.len());
}

/// Two declarations are in one group when each leads to the other, and only a
/// group's own members restrict each other. The groups are worked out once for
/// the whole table rather than rebuilt per declaration, so a program with
/// several of them is what says the partition kept them apart.
#[test]
fn separate_recursive_groups_do_not_restrict_each_other() {
    // Two mutual pairs and a chain into one of them. Each pair may hand on its
    // own parameter; neither is the other's business, and the declaration that
    // merely leads into one belongs to no group at all.
    let (_, out) = build_src(
        "type A a = { x: B a }  type B b = { x: A b }  \
         type C c = { x: D { y: c } }  type D d = { x: C d }  \
         type E e = { x: A e }",
    );
    let offenders: Vec<usize> = out
        .errors
        .iter()
        .filter(|error| matches!(error.kind, ErrorKind::GrowingRecursion))
        .map(|error| error.span.start)
        .collect();
    // Only `C`, and only where it hands `D` a type built out of what it takes.
    assert_eq!(offenders.len(), 1, "{:#?}", out.errors);
}

/// A parameter stands for one type, never for something still waiting for types
/// of its own. Refusing this is what keeps the language free of higher kinds.
#[test]
fn a_parameter_may_not_be_applied() {
    let (_, out) = build_src("type Flip f a = f a");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::ParameterApplied)),
        "errors: {:#?}",
        out.errors
    );
}

#[test]
fn a_declaration_binds_each_parameter_once() {
    let (_, out) = build_src("type Pair A A = { a: A }");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::DuplicateParameter { .. })),
        "errors: {:#?}",
        out.errors
    );
}

/// A repeated parameter binds once, so the declaration takes one thing. The
/// count the arity check uses is the list the scheme binds; counting the list
/// that was written asked for an argument no body could name.
#[test]
fn a_repeated_parameter_is_not_counted_twice() {
    let (mint, out) = build_src("type P A A = { x: A }  let v : P Nat = { x: 1 }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::DuplicateParameter { .. }
    ));

    let decl = &out.program.types[&type_symbol(&mint, &out, "P")];
    assert_eq!(decl.params.len(), 1);
}

/// Applying a name is still only a name, so a pair of declarations that hand
/// each other their parameters says nothing, exactly as a pair of bare names
/// does. A body that is a parameter is not the same thing: it says its argument.
#[test]
fn a_type_defined_only_as_another_name_is_still_circular() {
    let (_, out) = build_src("type A a = B a  type B b = A b");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Circular)),
        "errors: {:#?}",
        out.errors
    );

    // The identity constructor: useless, but it says what its argument is.
    let (_, out) = build_src("type Id a = a");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// A loop can close through a declaration that hands back what it is given:
/// `A` stands for its argument, so `B` is declared as itself with `A` in
/// between and says exactly as much as `type B = B` does.
#[test]
fn a_loop_can_close_through_a_parameter() {
    for src in [
        "type A a = a  type B = A B",
        "type A a = a  type B = A C  type C = A B",
        // The loop that goes out through an argument and comes back in.
        "type S a = U (S a)  type U b = b",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::Circular)),
            "{src}: {:#?}",
            out.errors
        );
    }

    // And what must not be caught with it. Following a pass-through twice is
    // not following it round: each step hands back a smaller piece of what was
    // written.
    for src in [
        "type Id a = a  type Y = Id (Id Nat)",
        // The case a per-slot approximation reports wrongly: `Id` is handed an
        // `X` somewhere else, which says nothing about `X`.
        "type Id a = a  type X = Id Nat  type Y = { f: Id X }",
        "type G g = g  type F a = G (G a)  type H = F Nat",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    // Only the declarations on the loop are told; one that merely names one
    // has nothing to fix.
    let (mint, out) = build_src("type A a = a  type B = A B  type P = B");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let b = out.program.types[&type_symbol(&mint, &out, "B")].name_span;
    assert!(out.errors[0].span.start >= b.start);
}

/// A type that leads back to itself may not hand on an argument built out of
/// what it takes. Unfolding one that grows its argument never comes back round,
/// so there would be no finite answer to whether two of them are the same type.
///
/// Growth is the whole of the rule. It used to be stricter — a mention inside a
/// group had to carry that member's own parameters verbatim — because the
/// solver's assumption was keyed on a pair of names and so could not tell one
/// declaration's two argument lists apart. The key now carries the arguments
/// (see `Solve::unfold`), so a fixed argument, and a permuted one, are ordinary
/// programs and are accepted here.
#[test]
fn recursion_may_not_make_its_argument_bigger() {
    for src in [
        "type List a = { head: a, tail: List a }",
        // Mutual recursion, each member handing on its own parameter.
        "type Tree a = { node: a, kids: Forest a }  \
         type Forest a = { head: Tree a, tail: Forest a }",
        // A different group's argument is unrestricted: only the `Rose a`
        // inside `List` is the group's business, and it hands on `a`.
        "type List a = { head: a, tail: List a }  \
         type Rose a = { node: a, kids: List (Rose a) }",
        // A longer circle, every member in one group.
        "type A a = { x: B a }  type B b = { x: C b }  type C c = { x: A c }",
        // An argument that mentions no parameter is written out in the program
        // and is the same type every round, so it cannot grow.
        "type T a = { next: T Nat }",
        "type Tree a = { value: a, kids: Forest }  \
         type Forest = { head: Tree Nat, tail: Forest }",
        // Both at once, in one group.
        "type T a = { next: T a, other: T Nat }",
        // Order and repetition are free: a permutation only ever shuffles what
        // came in.
        "type A a b = { x: B b a }  type B c d = { x: A d c }",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    for src in [
        "type T a = { next: T { x: a } }",
        "type A a = { x: B { y: a } }  type B b = { x: A b }",
        // A parameter in a `..` tail is as much a way of growing as one in a
        // field, and is the one place a parameter is not written as a name.
        "type A r = { x: A { ..r } }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::GrowingRecursion)),
            "{src}: {:#?}",
            out.errors
        );
    }
}

/// A parameter's kind is read off the body: a name in a `..` tail stands for a
/// row, a name anywhere else stands for a type, and one nothing says anything
/// about is a type because that is what a reader will expect.
///
/// A row carries what it may not stand for as well as that it is one: the
/// labels written out beside it are already named, and a `..` covers what is
/// not named.
#[test]
fn a_parameter_stands_for_what_the_body_uses_it_as() {
    for (src, expected) in [
        ("type WithX r = { x: Nat, ..r }", vec![row(&["x"])]),
        ("type Box A = { it: A }", vec![ParamKind::Type]),
        ("type Ghost a = Nat", vec![ParamKind::Type]),
        ("type Bare r = { ..r }", vec![row(&[])]),
        (
            "type Two r = { x: Nat, y: Nat, ..r }",
            vec![row(&["x", "y"])],
        ),
        // Two rows in one body, each naming its own fields: the parameter may
        // not stand for anything either of them writes out.
        (
            "type Twice r = { a: { x: Nat, ..r }, b: { y: Nat, ..r } }",
            vec![row(&["x", "y"])],
        ),
        (
            "type Both A r = { it: A, ..r }",
            vec![ParamKind::Type, row(&["it"])],
        ),
        (
            "type Fn A B = A -> B",
            vec![ParamKind::Type, ParamKind::Type],
        ),
    ] {
        let (_, out) = built(src);
        let decl = out.program.types.values().next().expect("one declaration");
        let kinds: Vec<ParamKind> = decl.params.iter().map(|param| param.kind.clone()).collect();
        assert_eq!(kinds, expected, "{src}");
    }
}

/// A parameter handed straight to another declaration stands for whatever that
/// declaration's parameter in that position stands for. Declarations are
/// hoisted and may name each other, so this has to hold around a circle too —
/// which is why the kinds are joined rather than assigned in some order.
#[test]
fn a_kind_travels_through_an_argument() {
    // Along a chain, against the order the declarations are written in. What
    // the row may not name travels with it: `Outer` names no field of its own,
    // and its argument still ends up where `Inner`'s `x` sits.
    let (_, out) = built("type Outer s = Inner s  type Inner r = { x: Nat, ..r }");
    for decl in out.program.types.values() {
        assert_eq!(decl.params[0].kind, row(&["x"]));
    }

    // And around a circle, where no declaration decides its own.
    let (_, out) = built(
        "type A x = { hop: B x, ..x }\n\
         type B y = { hop: A y }",
    );
    for decl in out.program.types.values() {
        assert_eq!(decl.params[0].kind, row(&["hop"]));
    }

    // A row written out as an argument is not the parameter handed on, but its
    // own tail still lands where the callee's tail sat — so it collects the
    // callee's labels as well as the ones written beside it.
    let (mint, out) = built("type Inner r = { x: Nat, ..r }  type Wrap s = Inner { y: Nat, ..s }");
    let wrap = &out.program.types[&type_symbol(&mint, &out, "Wrap")];
    assert_eq!(wrap.params[0].kind, row(&["y", "x"]));
}

/// A parameter used both ways leaves nothing to read off, and neither use is
/// the wrong one — so the declaration is what gets told.
#[test]
fn a_parameter_may_not_stand_for_both() {
    for src in [
        "type Bad r = { it: r, ..r }",
        // Through an argument: `Inner` makes `r` a row, the field makes it a
        // type.
        "type Inner r = { x: Nat, ..r }  type Bad a = { it: a, more: Inner a }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::MixedParameter)),
            "{src}: {:#?}",
            out.errors
        );
    }
}

/// What a parameter stands for is read off the declaration that binds it, in
/// whichever order the declarations were written. A use site handing it the
/// wrong thing is the use site's mistake, not evidence about the declaration.
#[test]
fn a_parameter_kind_does_not_depend_on_declaration_order() {
    for src in [
        "type WithX r = { x: Nat, ..r }  type Bad = WithX Nat",
        "type Bad = WithX Nat  type WithX r = { x: Nat, ..r }",
    ] {
        let (mint, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(matches!(out.errors[0].kind, ErrorKind::NotARow), "{src}");
        assert_eq!(
            out.errors[0].span.start,
            src.find("WithX Nat").expect("the use") + "WithX ".len(),
            "{src}: reported at the argument"
        );
        let with_x = &out.program.types[&type_symbol(&mint, &out, "WithX")];
        assert_eq!(with_x.params[0].kind, row(&["x"]), "{src}");
    }
}

/// A parameter used two ways is reported against the parameter that was used
/// two ways. The other declaration is right and is left alone — which the
/// order the two were written in must not decide.
#[test]
fn a_mixed_parameter_names_the_declaration_that_mixed_it() {
    for src in [
        "type V a = { f: a, g: W a }  type W r = { x: Nat, ..r }",
        "type W r = { x: Nat, ..r }  type V a = { f: a, g: W a }",
    ] {
        let (mint, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(out.errors[0].kind, ErrorKind::MixedParameter),
            "{src}"
        );

        let v = &out.program.types[&type_symbol(&mint, &out, "V")];
        assert_eq!(out.errors[0].span, v.params[0].span, "{src}: at `a` in `V`");
        // A mixed parameter is still shown as a row, since that is what the
        // body said of it — but the body itself is gone, which is what keeps
        // the one mistake to one complaint. See the erasure test below.
        assert_eq!(v.params[0].kind, row(&["x"]), "{src}");
        assert!(
            matches!(v.value.tracked, TypeKind::Error),
            "{src}: the body absorbs: {:#?}",
            v.value
        );

        let w = &out.program.types[&type_symbol(&mint, &out, "W")];
        assert_eq!(w.params[0].kind, row(&["x"]), "{src}: `W` is right");
        assert!(
            !matches!(w.value.tracked, TypeKind::Error),
            "{src}: `W` keeps its body"
        );
    }
}

/// A declaration whose parameter could not be read one way absorbs, the way a
/// circular one does: the body is erased and nothing is asked of the arguments
/// written at it. Without that, one declaration nobody could read produced a
/// complaint at every use site — about ordinary types, at code the reader got
/// right.
#[test]
fn a_mixed_parameter_absorbs_its_own_use_sites() {
    let src = "type Bad a = { x: a, ..a }\n\
               let f: Bad Nat = { x: 1 }\n\
               let g: Bad Nat -> Nat = fn v => v.x\n\
               let h: Bad Nat -> Nat = fn v => 1";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::MixedParameter));

    let bad = &out.program.types[&type_symbol(&mint, &out, "Bad")];
    assert!(
        matches!(bad.value.tracked, TypeKind::Error),
        "{:#?}",
        bad.value
    );
}

/// One declaration nobody could read is one complaint, however many
/// declarations hand the parameter on afterwards. Only the declaration that
/// brought the two readings together has anything to change; the rest say one
/// thing about their own parameter and are right about it.
#[test]
fn a_mixed_parameter_is_reported_once_along_a_chain() {
    let src = "type W r = { x: Nat, ..r }\n\
               type U t = { a: W t, b: t }\n\
               type V u = U u\n\
               type Q q = V q";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::MixedParameter));

    // At `t` in `U`, which is where the field and the hand-off meet.
    let u = &out.program.types[&type_symbol(&mint, &out, "U")];
    assert_eq!(out.errors[0].span, u.params[0].span);

    // Everything the clash reaches still absorbs, told or not: a body left
    // standing would put whatever a use site handed it into a row.
    for name in ["U", "V", "Q"] {
        let decl = &out.program.types[&type_symbol(&mint, &out, name)];
        assert!(
            matches!(decl.value.tracked, TypeKind::Error),
            "{name}: {:#?}",
            decl.value
        );
    }
}

/// Only something that could stand for a set of fields may be written where a
/// row parameter goes. Anything else would leave a row holding what no row can
/// hold — which nothing downstream checks, so it is refused here.
///
/// This once passed for an annotation while catching the same mistake in a
/// declaration, because the check ran before annotations were lowered. What
/// reached the reader then was `expected `Nat`, found `∅`` — a complaint about
/// the empty row, naming a symbol nobody had written.
#[test]
fn only_a_row_may_be_written_where_a_row_goes() {
    for src in [
        "type WithX r = { x: Nat, ..r }  let f : WithX Nat -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX (Nat -> Nat) -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  type Bad = WithX Nat",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::NotARow)),
            "{src}: {:#?}",
            out.errors
        );
    }

    // A struct is one, and so is another row parameter handed straight on.
    for src in [
        "type WithX r = { x: Nat, ..r }  let f : WithX { y: Nat } -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX {} -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  type Pass s = WithX s",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A `..` covers only the fields its row does not already name, so a row
/// written where a row parameter goes may not name any of them. Which labels
/// those are is part of what the parameter stands for, so it is known here —
/// at the argument, where the reader can act on it — rather than only wherever
/// something later happened to flatten the row.
#[test]
fn a_row_argument_may_not_name_what_the_declaration_names() {
    let src = "type WithX r = { x: Nat, ..r }  let f : WithX { x: Nat } -> Nat = fn p => p.x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field } if field == "x"),
        "{:#?}",
        out.errors
    );
    // At the argument, which is the whole of what the reader can change.
    assert_eq!(
        out.errors[0].span.start,
        src.find("{ x: Nat }").expect("the argument")
    );

    // The labels travel with the parameter, so a declaration that hands its
    // own on refuses the same argument without naming anything itself.
    let (_, out) = build_src(
        "type WithX r = { x: Nat, ..r }  type Pass s = WithX s  \
                              let f : Pass { x: Nat } -> Nat = fn p => p.x",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::RepeatedRowField { .. }
    ));

    // A label the declaration does not name is what a `..` is for.
    for src in [
        "type WithX r = { x: Nat, ..r }  let f : WithX { y: Nat } -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX {} -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  type Pass s = WithX s",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A `..` naming a parameter is the one way a declared type may be left open,
/// because what it stands for is supplied at every use rather than decided
/// here. A bare `..` and a `?` are still refused, and so is a name that binds
/// nothing.
#[test]
fn a_declaration_is_open_only_through_a_parameter() {
    let (_, out) = built("type WithX r = { x: Nat, ..r }");
    assert!(matches!(
        out.program
            .types
            .values()
            .next()
            .expect("one declaration")
            .value
            .tracked,
        TypeKind::Struct { tail: Some(_), .. }
    ));

    for src in [
        "type T = { x: Nat, .. }",
        "type T = { x: Nat, ..r }",
        "type T A = { x: Nat, ..r }",
        "type T r = { x?: Nat, ..r }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::OpenDeclaredType)),
            "{src}: {:#?}",
            out.errors
        );
    }
}

/// A parameter hides a primitive for the length of the body, the way a
/// declaration of the same name already did. Outside the body the primitive is
/// back, so `type Box Nat = { it: Nat }` takes one type and `Box Nat` hands it
/// the built-in — confusing to write, but the resolution order is the one the
/// rest of the language already uses, and it should not quietly be a third.
#[test]
fn a_parameter_shadows_a_primitive() {
    let (_, out) = built("type Box Nat = { it: Nat }  let b : Box Nat -> Nat = fn p => p.it");

    let boxed = out.program.types.values().next().expect("the declaration");
    let TypeKind::Struct { fields, .. } = &boxed.value.tracked else {
        panic!("expected a struct: {:#?}", boxed.value);
    };
    assert!(
        matches!(fields["it"].value.tracked, TypeKind::Param { index: 0, .. }),
        "inside the body the parameter wins: {:#?}",
        fields["it"].value
    );

    let annotation = out
        .program
        .terms
        .values()
        .next()
        .and_then(|decl| decl.annotation.as_ref())
        .expect("the annotation");
    let TypeKind::Arrow { from, .. } = &annotation.tracked else {
        panic!("expected an arrow: {annotation:#?}");
    };
    let TypeKind::Apply { args, .. } = &from.tracked else {
        panic!("expected an application: {from:#?}");
    };
    assert!(
        matches!(args[0].tracked, TypeKind::Prim(Prim::Nat)),
        "outside it the primitive is back: {:#?}",
        args[0]
    );
}
