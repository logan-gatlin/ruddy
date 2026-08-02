//! Tests for [`ruddy::ir`].

use indexmap::IndexMap;
use ruddy::{
    ir::{ErrorKind, Field, Output, Term, TermKind, TypeField, TypeKind, build},
    parse,
    symbol::{Bundle, Mint, Namespace, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::Prim,
};
use ruddy_debug::print;

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
