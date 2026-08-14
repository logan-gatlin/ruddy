//! Tests for [`ruddy::ir`].

use indexmap::IndexMap;
use ruddy::{
    ir::{
        ClauseKind, Effect, ErrorKind, Field, Output, PatternKind, SumCase, Term, TermKind,
        TypeField, TypeKind, build,
    },
    parse,
    symbol::{Bundle, Mint, Namespace, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::{ParamKind, Prim, Sense, Shape},
};
use ruddy_debug::print;

/// A parameter standing for a whole type that may not name any of `labels` —
/// which is what a struct's `..r` is, since the rest of a struct is its core.
fn row(labels: &[&str]) -> ParamKind {
    ParamKind::Type {
        lacks: labels.iter().map(|label| label.to_string()).collect(),
    }
}

/// A parameter standing for the rest of a sum's cases, which may not name any
/// of `labels`.
fn cases(labels: &[&str]) -> ParamKind {
    ParamKind::Cases {
        lacks: labels.iter().map(|label| label.to_string()).collect(),
    }
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

/// The binding groups as the names a reader would see, in the order lowering
/// published them.
fn groups<'a>(mint: &'a Mint, out: &Output) -> Vec<Vec<&'a str>> {
    out.program
        .groups
        .iter()
        .map(|group| {
            group
                .members
                .iter()
                .map(|symbol| mint.name(*symbol))
                .collect()
        })
        .collect()
}

/// The definitions reported circular, named, in the order they were reported.
/// Each is found by the span it was reported at, which is its value's — so this
/// pins where the complaint lands as much as which definitions get one.
fn circular<'a>(mint: &'a Mint, out: &'a Output) -> Vec<&'a str> {
    out.errors
        .iter()
        .filter(|error| matches!(error.kind, ErrorKind::Circular { .. }))
        .map(|error| {
            out.program
                .terms
                .iter()
                .find(|(_, decl)| decl.value.span == error.span)
                .map(|(symbol, _)| mint.name(*symbol))
                .unwrap_or_else(|| panic!("no definition at {:?}", error.span))
        })
        .collect()
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
    assert!(matches!(annotation.ty.tracked, TypeKind::Ident(s) if mint.name(s) == "T"));
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
    assert!(matches!(annotation.ty.tracked, TypeKind::Ident(_)));

    // Which holds for a name of the program's own just the same.
    let (mint, out) = built("let u : T = ()  type T = ()");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "u")]
        .annotation
        .as_ref()
        .expect("the ascription was lowered");
    assert!(matches!(annotation.ty.tracked, TypeKind::Ident(s) if mint.name(s) == "T"));
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

/// Every top-level name is bound before any body is lowered, so where a
/// definition sits on the page decides nothing about what can see it.
#[test]
fn a_definition_may_name_one_written_below_it() {
    let (mint, out) = built("let a = b  let b = fn z => z");

    let b = term_symbol(&mint, &out, "b");
    assert!(matches!(term_value(&mint, &out, "a"), TermKind::Ident(s) if *s == b));
}

/// Which leaves `Undefined` meaning "defined nowhere in this file" rather than
/// "defined below here".
#[test]
fn a_name_defined_nowhere_is_still_undefined() {
    let (_, out) = build_src("let a = nope");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
    ));
}

/// A definition can see itself, which is the whole point of the hoist: the name
/// in the body is the definition's own symbol rather than a complaint.
#[test]
fn a_definition_can_see_itself() {
    let (mint, out) = built("let f = fn n => f n");

    let f = term_symbol(&mint, &out, "f");
    let TermKind::Fn { body, .. } = term_value(&mint, &out, "f") else {
        panic!("expected a function");
    };
    let TermKind::Apply { func, .. } = &body.kind else {
        panic!("expected an application");
    };
    assert!(matches!(func.kind, TermKind::Ident(s) if s == f));
}

/// And two definitions can see each other, in both directions at once.
#[test]
fn two_definitions_can_name_each_other() {
    let (mint, out) = built("let even = fn n => odd n  let odd = fn n => even n");

    let named = |name| {
        let TermKind::Fn { body, .. } = term_value(&mint, &out, name) else {
            panic!("expected a function");
        };
        let TermKind::Apply { func, .. } = &body.kind else {
            panic!("expected an application");
        };
        match func.kind {
            TermKind::Ident(symbol) => symbol,
            ref other => panic!("expected a name, got {other:?}"),
        }
    };
    assert_eq!(named("even"), term_symbol(&mint, &out, "odd"));
    assert_eq!(named("odd"), term_symbol(&mint, &out, "even"));
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

    // The repeat's body is lowered all the same, though nothing keeps it: a
    // bad name inside one is still the reader's to fix, and hearing about it
    // should not be the price of fixing the name above.
    let (_, out) = build_src("let x = ()  let x = nope");
    assert_eq!(out.errors.len(), 2, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Duplicate {
            namespace: Namespace::Terms,
            ..
        }
    ));
    assert!(matches!(
        out.errors[1].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
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
        fields["a"].name_span().start,
        src.find("a: A").expect("the first")
    );
    assert!(matches!(
        fields["a"].value().map(|value| &value.tracked),
        Some(&TypeKind::Ident(s)) if mint.name(s) == "A"
    ));

    // Annotations go the same way, and the `when` clause only they may carry
    // rides through the re-keying with the field it was written on.
    let src = "let f : { a when a: Nat, a: Nat } -> Nat = fn p => p.a";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateField));
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("a: Nat").expect("the repeat")
    );

    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .clone()
        .expect("the annotation");
    let TypeKind::Arrow { from, .. } = annotation.ty.tracked else {
        panic!("expected an arrow, got {:?}", annotation.ty.tracked);
    };
    let TypeKind::Struct { fields, .. } = from.tracked else {
        panic!("expected a struct parameter");
    };
    assert_eq!(fields.len(), 1);
    assert!(matches!(
        fields["a"],
        TypeField::Written { when: Some(_), .. }
    ));
}

#[test]
fn a_declared_type_must_be_closed() {
    // A tail or a `when` clause stands for something a definition gets to
    // decide, and a declaration decides for everyone: each `..` and `when` is
    // refused, and the struct that carried it lowers to the error type.
    for src in [
        "type T = { x: Nat, .. }",
        "type T = { x: Nat, ..r }",
        "type T = { x when a: Nat }",
        "type T = { a: { b: Nat, .. } }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::OpenDeclaredType {
                    shape: Shape::Struct
                }
            )),
            "{src}: {:#?}",
            out.errors
        );
    }

    // One report per marker, in source order.
    let (_, out) = build_src("type T = { x when a: Nat, y when b: Nat, .. }");
    assert_eq!(out.errors.len(), 3, "errors: {:#?}", out.errors);
    assert!(
        out.errors.iter().all(|error| matches!(
            error.kind,
            ErrorKind::OpenDeclaredType {
                shape: Shape::Struct
            }
        )),
        "errors: {:#?}",
        out.errors
    );

    // A name that fails to resolve inside an open declaration is still its
    // own report: fixing the `..` should not reveal it.
    let (_, out) = build_src("type T = { x: Missing, .. }");
    assert_eq!(out.errors.len(), 2, "errors: {:#?}", out.errors);

    // An annotation is where openness belongs, and it passes through whole.
    let (_, out) = build_src("let f : { x when a: Nat, ..r } -> Nat = fn p => p.x");
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
        .map(|field| match field.value().map(|value| &value.tracked) {
            Some(&TypeKind::Param { index, .. }) => index,
            other => panic!("expected a parameter: {other:#?}"),
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
        fields["it"].value().map(|value| &value.tracked),
        Some(TypeKind::Param { index: 0, .. })
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

    // A `when` field or a bare `..` inside a head is refused where it stands, for
    // the same reason.
    let (_, out) = build_src("type A = { x: Nat, .. } Nat");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "open-declared-type"]);

    let (_, out) = build_src("type A = { x when a: Nat } Nat");
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
            .any(|error| matches!(error.kind, ErrorKind::Circular { .. })),
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
                .any(|error| matches!(error.kind, ErrorKind::Circular { .. })),
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
        // Every other way a type can be built out of a parameter: handed to
        // another declaration, put on either side of an arrow, or made a
        // case's payload. Each of them is a type that gets bigger every round,
        // and the walk that looks for a parameter has to reach all of them.
        "type Box b = { it: b }  type T a = { next: T (Box a) }",
        "type T a = { next: T (a -> Nat) }",
        "type T a = { next: T (Nat -> a) }",
        "type T a = { next: T (`A a) }",
        "type T r = { next: T (`A Nat | ..r) }",
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
        ("type Box A = { it: A }", vec![row(&[])]),
        ("type Ghost a = Nat", vec![row(&[])]),
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
            vec![row(&[]), row(&["it"])],
        ),
        ("type Fn A B = A -> B", vec![row(&[]), row(&[])]),
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
///
/// Both ways is a whole type and the rest of a sum's cases, and nothing else:
/// the rest of a *struct* is a whole type, so the two readings that used to
/// clash there now agree.
#[test]
fn a_parameter_may_not_stand_for_both() {
    for src in [
        "type Bad r = { g: (`A | ..r), f: r }",
        // Through an argument: `Inner` makes `r` a sum's rest, the field makes
        // it a type.
        "type Inner r = `A Nat | ..r  type Bad a = { it: a, more: Inner a }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::MixedParameter { .. })),
            "{src}: {:#?}",
            out.errors
        );
    }

    // And a parameter used as a field's type and as a struct's `..` is not one:
    // both say "a type", so the declaration simply takes the union of what they
    // demand, which is the lacks set.
    let (mint, out) = built("type W r = { f: r, ..r }");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let w = &out.program.types[&type_symbol(&mint, &out, "W")];
    assert_eq!(w.params[0].kind, row(&["f"]));
}

/// What a parameter stands for is read off the declaration that binds it, in
/// whichever order the declarations were written. A use site handing it the
/// wrong thing is the use site's mistake, not evidence about the declaration.
#[test]
fn a_parameter_kind_does_not_depend_on_declaration_order() {
    for src in [
        "type Or r = `A | ..r  type Bad = Or Nat",
        "type Bad = Or Nat  type Or r = `A | ..r",
    ] {
        let (mint, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::NotARow {
                    sense: Sense::Cases
                }
            ),
            "{src}"
        );
        assert_eq!(
            out.errors[0].span.start,
            src.find("Or Nat").expect("the use") + "Or ".len(),
            "{src}: reported at the argument"
        );
        let or = &out.program.types[&type_symbol(&mint, &out, "Or")];
        assert_eq!(or.params[0].kind, cases(&["A"]), "{src}");
    }

    // A struct's `..` reads the same either way round, and takes anything at
    // all: `WithX Nat` is well-formed however the two were ordered.
    for src in [
        "type WithX r = { x: Nat, ..r }  type Fine = WithX Nat",
        "type Fine = WithX Nat  type WithX r = { x: Nat, ..r }",
    ] {
        let (mint, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
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
        "type V a = { f: a, g: W a }  type W r = `X Nat | ..r",
        "type W r = `X Nat | ..r  type V a = { f: a, g: W a }",
    ] {
        let (mint, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(out.errors[0].kind, ErrorKind::MixedParameter { .. }),
            "{src}"
        );

        let v = &out.program.types[&type_symbol(&mint, &out, "V")];
        assert_eq!(out.errors[0].span, v.params[0].span, "{src}: at `a` in `V`");
        // A mixed parameter is still shown as a sum's rest, since that is what
        // the body said of it — but the body itself is gone, which is what keeps
        // the one mistake to one complaint. See the erasure test below.
        assert_eq!(v.params[0].kind, cases(&["X"]), "{src}");
        assert!(
            matches!(v.value.tracked, TypeKind::Error),
            "{src}: the body absorbs: {:#?}",
            v.value
        );

        let w = &out.program.types[&type_symbol(&mint, &out, "W")];
        assert_eq!(w.params[0].kind, cases(&["X"]), "{src}: `W` is right");
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
    let src = "type Bad a = { x: (`A | ..a), y: a }\n\
               let f: Bad Nat = { x: `A, y: 1 }\n\
               let g: Bad Nat -> Nat = fn v => v.y\n\
               let h: Bad Nat -> Nat = fn v => 1";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::MixedParameter { .. }
    ));

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
    let src = "type W r = `X Nat | ..r\n\
               type U t = { a: W t, b: t }\n\
               type V u = U u\n\
               type Q q = V q";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::MixedParameter { .. }
    ));

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

/// Only something that could stand for a set of cases may be written where a
/// *sum's* row parameter goes. Anything else would leave a row holding what no
/// row can hold — which nothing downstream checks, so it is refused here.
///
/// This once passed for an annotation while catching the same mistake in a
/// declaration, because the check ran before annotations were lowered. What
/// reached the reader then was `expected `Nat`, found `∅`` — a complaint about
/// the empty row, naming a symbol nobody had written.
#[test]
fn only_a_row_may_be_written_where_a_row_goes() {
    for src in [
        "type Or r = `A | ..r  let f : Or Nat -> Nat = fn p => 1",
        "type Or r = `A | ..r  let f : Or (Nat -> Nat) -> Nat = fn p => 1",
        "type Or r = `A | ..r  let f : Or { y: Nat } -> Nat = fn p => 1",
        "type Or r = `A | ..r  type Bad = Or Nat",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::NotARow { .. })),
            "{src}: {:#?}",
            out.errors
        );
    }

    // A sum is one, and so is another sum's row parameter handed straight on.
    for src in [
        "type Or r = `A | ..r  let f : Or (`B Nat) -> Nat = fn p => 1",
        "type Or r = `A | ..r  let f : Or (|) -> Nat = fn p => 1",
        "type Or r = `A | ..r  type Pass s = Or s",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A struct's `..` admits any argument at all, because it *is* the type's core
/// and every type is one of those. `WithX Nat` is a type nothing constructs a
/// term of, and that is allowed.
#[test]
fn a_struct_tail_admits_any_argument() {
    for src in [
        "type WithX r = { x: Nat, ..r }  let f : WithX Nat -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX (Nat -> Nat) -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX (`A | `B) -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX { y: Nat } -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  let f : WithX { y: Nat, .. } -> Nat = fn p => p.x",
        "type WithX r = { x: Nat, ..r }  type Foo = { y: Nat }  \
         let f : WithX Foo -> Nat = fn p => p.y",
        "type WithX r = { x: Nat, ..r }  type Pass s = WithX s",
        "type WithX r = { x: Nat, ..r }  let f : WithX {} -> Nat = fn p => p.x",
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
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field, .. } if field == "x"),
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
/// here. A bare `..` and a `when` are still refused, and so is a name that binds
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
        "type T r = { x when a: Nat, ..r }",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::OpenDeclaredType {
                    shape: Shape::Struct
                }
            )),
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
        matches!(
            fields["it"].value().map(|value| &value.tracked),
            Some(TypeKind::Param { index: 0, .. })
        ),
        "inside the body the parameter wins: {:#?}",
        fields["it"]
    );

    let annotation = out
        .program
        .terms
        .values()
        .next()
        .and_then(|decl| decl.annotation.as_ref())
        .expect("the annotation");
    let TypeKind::Arrow { from, .. } = &annotation.ty.tracked else {
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

/// A sum lowers to the cases it names, keyed by name and carrying what each
/// one was written with. The `payload` stays `None` where nothing was written:
/// it means unit, but the unit is inference's to supply, not the tree's.
#[test]
fn lowers_a_sum_type() {
    let src = "type Option T = `Some T | `None";
    let (mint, out) = built(src);
    let decl = &out.program.types[&type_symbol(&mint, &out, "Option")];
    let TypeKind::Sum { cases, tail } = &decl.value.tracked else {
        panic!("expected a sum, got {:#?}", decl.value.tracked);
    };
    assert_eq!(cases.keys().collect::<Vec<_>>(), vec!["Some", "None"]);
    // The case's own span is the name it was written at, backtick included.
    assert_eq!(
        cases["Some"].name_span().start,
        src.find("`Some").expect("the case")
    );
    assert_eq!(cases["Some"].name_span().width, 5);
    assert!(matches!(
        cases["Some"].payload().map(|ty| &ty.tracked),
        Some(TypeKind::Param { index: 0, .. })
    ));
    assert!(cases["None"].payload().is_none());
    assert!(matches!(cases["Some"], SumCase::Written { when: None, .. }));
    // A declaration lists every case there is, so there is no tail.
    assert!(tail.is_none());
}

/// A tag lowers to the case it names and what it carries — and, like a field
/// name, the case is a label rather than a symbol, so nothing resolves it and
/// nothing about it can fail to resolve.
#[test]
fn lowers_a_tag() {
    let (mint, out) = built("let v = `Some 1  let n = `None");
    let TermKind::Tag { name, payload } = term_value(&mint, &out, "v") else {
        panic!("expected a tag");
    };
    assert_eq!(name.tracked, "Some");
    assert!(matches!(
        payload.as_ref().map(|term| &term.kind),
        Some(TermKind::Natural(1))
    ));

    let TermKind::Tag { name, payload } = term_value(&mint, &out, "n") else {
        panic!("expected a tag");
    };
    assert_eq!(name.tracked, "None");
    assert!(payload.is_none());
}

/// A name written twice in one sum is one case, reported at the repeat — the
/// same rule a struct's fields keep, worded as the case it is about.
#[test]
fn a_repeated_case_is_reported_once() {
    let (mint, out) = build_src("type T = `A Nat | `A Nat");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateCase));
    // The first occurrence is the one that stands.
    let decl = &out.program.types[&type_symbol(&mint, &out, "T")];
    let TypeKind::Sum { cases, .. } = &decl.value.tracked else {
        panic!("expected a sum");
    };
    assert_eq!(cases.len(), 1);
}

/// A declaration holds for every definition, so it cannot leave a question a
/// definition would answer — which is as true of a case that may or may not be
/// allowed as it is of a field that may or may not be there.
///
/// The complaint carries the shape it was raised about, because that is the
/// only way the sentence it becomes can be about cases: a sum told to list its
/// fields describes a type its writer never wrote.
#[test]
fn a_declared_sum_must_list_its_cases() {
    for src in [
        "type T = `A Nat | ..",
        "type T = `A Nat | ..r",
        "type T = `A (when a) Nat | `B",
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::OpenDeclaredType { shape: Shape::Sum }
            )),
            "{src}: {:#?}",
            out.errors
        );
    }
    // A tail naming one of the declaration's own parameters is the exception,
    // for the reason it is the exception for a struct: what it stands for is
    // supplied at every use rather than decided here.
    let (mint, out) = built("type Tagged r = `Err Nat | ..r");
    let decl = &out.program.types[&type_symbol(&mint, &out, "Tagged")];
    assert_eq!(decl.params[0].kind, cases(&["Err"]));
}

/// A sum's tail stands for cases, so a struct written at one is refused — while
/// a struct's tail is the type's core and takes anything, including a sum.
#[test]
fn a_row_parameter_knows_which_shape_it_is() {
    let (_, out) = build_src("type Cases r = `A Nat | ..r  type Bad = Cases { y: Nat }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::NotARow { .. }));

    let (_, out) = build_src("type WithX r = { x: Nat, ..r }  type Fine = WithX (`A Nat)");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    // And a parameter handed to both is a parameter that has to say which it
    // meant: a whole type in one place, the rest of a sum in the other.
    let (_, out) = build_src(
        "type WithX r = { x: Nat, ..r }  type Cases s = `A Nat | ..s  \
         type Bad t = { it: WithX t, also: Cases t }",
    );
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::MixedParameter { .. })),
        "{:#?}",
        out.errors
    );
}

/// A row handed to a sum's tail may not name a case the declaration already
/// names, for the reason it may not name such a field: the type would allow
/// the case twice, and the two copies could carry different things.
#[test]
fn a_sum_argument_may_not_repeat_a_case() {
    let (_, out) = build_src("type Cases r = `A Nat | ..r  type Bad = Cases (`A Nat)");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field, shape }
            if field == "A" && *shape == Shape::Sum),
        "{:#?}",
        out.errors
    );

    // And the cases travel back through a sum written out as an argument, the
    // way they do through a struct: `OuterC`'s own tail ends up where `Cases`'s
    // sat, so it may not stand for `` `A `` either. Written out and not just
    // handed on — that is the edge a sum used to be left off, and the argument
    // it let through was accepted with the case carrying the wrong type.
    let src = "type Cases q = `A Nat | ..q  type OuterC p = Cases (`B Nat | ..p)  \
               type Bad = OuterC (`A Nat)";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field, shape }
            if field == "A" && *shape == Shape::Sum),
        "{:#?}",
        out.errors
    );
    // At the argument, parentheses and all: they are what the reader wrote
    // around it, and the span is the text they can change.
    assert_eq!(
        out.errors[0].span.start,
        src.find("(`A Nat)").expect("the argument")
    );

    // A case neither of them names is what the `..` is for.
    let (_, out) = build_src(
        "type Cases q = `A Nat | ..q  type OuterC p = Cases (`B Nat | ..p)  \
         type Fine = OuterC (`C Nat)",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// A type may lead back to itself through a sum as much as through a struct: a
/// case is a shape one step in, which is all a declaration has to reach.
#[test]
fn a_sum_makes_a_type_recursive_rather_than_circular() {
    let (mint, out) = built("type List a = `Nil | `Cons { head: a, tail: List a }");
    let decl = &out.program.types[&type_symbol(&mint, &out, "List")];
    assert!(matches!(decl.value.tracked, TypeKind::Sum { .. }));
    // And the parameter reaches a position of what the declaration stands
    // for, through the case's payload, so congruence may be taken on it.
    assert!(decl.params[0].relevant);

    // The recursion that still cannot be allowed is the one that grows.
    let (_, out) = build_src("type T a = `Next (T { x: a })");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::GrowingRecursion)),
        "{:#?}",
        out.errors
    );
}

/// Naming a tail is for saying that two `..`s stand for one rest, and one rest
/// is one shape. A name given both is refused at the second use — the one that
/// brought the two together — and the row it was written in absorbs.
///
/// Left standing, the two tails really would share a variable, and a field
/// pushed into one would come back out of the other as a case, with nothing
/// anywhere telling the reader why.
#[test]
fn one_tail_name_is_one_shape_of_rest() {
    let src = "let f : { x: Nat, ..r } -> (`A Nat | ..r) -> Nat = fn a => fn b => 1";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::MixedTail {
                first: Sense::Type,
                second: Sense::Cases,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );
    // At the second `..r`, which is the one the writer has to choose about —
    // and pointing back at the first, which is the other half of what went
    // wrong and is somewhere else on the page.
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("..r").expect("the tail")
    );
    let ErrorKind::MixedTail { previous, .. } = out.errors[0].kind else {
        panic!("the clash was just matched");
    };
    assert_eq!(previous.start, src.find("..r").expect("the first tail"));

    // Either order: the row that absorbs is the second one written, whichever
    // shape that is.
    let (_, out) =
        build_src("let f : (`A Nat | ..r) -> { x: Nat, ..r } -> Nat = fn a => fn b => 1");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::MixedTail {
                first: Sense::Cases,
                second: Sense::Type,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );

    // Two tails of one shape are what naming one is *for*, and stay one rest.
    let (_, out) = build_src("let f : { x: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let (_, out) = build_src("let f : (`A Nat | ..r) -> (| ..r) -> Nat = fn a => fn b => 1");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    // And the scope is one written type, so two annotations may each use `r`
    // for a rest of their own.
    let (_, out) = build_src(
        "let f : { x: Nat, ..r } -> Nat = fn a => 1\n\
         let g : (`A Nat | ..r) -> Nat = fn b => 2",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// A type declared twice keeps the first, exactly as a term does: the repeat is
/// reported against it and lowers to nothing, so the name still stands for one
/// declaration and everything that mentions it means that one.
#[test]
fn duplicate_type_declarations_keep_the_first() {
    let (mint, out) = build_src("type T = Nat  type T = { x: Nat }  let v : T = 1");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Duplicate {
                namespace: Namespace::Types,
                previous,
            } if previous.start == 5
        ),
        "{:#?}",
        out.errors
    );

    // One declaration, and it is the first one's body.
    assert_eq!(out.program.types.len(), 1);
    let symbol = type_symbol(&mint, &out, "T");
    assert!(matches!(
        out.program.types[&symbol].value.tracked,
        TypeKind::Prim(Prim::Nat)
    ));
}

/// Applying a name nothing declares is an undefined type, reported at the name
/// rather than at the application: the arguments are not what went wrong, and
/// the reader has a name to fix. A primitive is the other way round — it exists
/// and takes nothing — which is why the two are told apart here at all.
#[test]
fn applying_an_undeclared_name_is_an_undefined_type() {
    let src = "type P = Missing Nat";
    let (_, out) = build_src(src);

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                namespace: Namespace::Types
            }
        ),
        "{:#?}",
        out.errors
    );
    assert_eq!(out.errors[0].span.start, src.find("Missing").expect("head"));

    // The primitive beside it says how many it takes, at the whole application.
    let (_, out) = build_src("type P = Nat Nat");
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Arity {
                expected: 0,
                found: 1
            }
        ),
        "{:#?}",
        out.errors
    );
}

/// A `..` naming a declared type is not a rest — a rest is a parameter or
/// nothing — so in an annotation it is read as a name of its own, the way any
/// other unbound tail name is. The declaration it happens to share a spelling
/// with has nothing to do with it.
#[test]
fn a_tail_naming_a_declared_type_is_still_just_a_name() {
    let (_, out) = build_src("type T = Nat  let f : { x: Nat, ..T } -> Nat = fn r => r.x");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);

    // And it is one rest like any other, so giving it two shapes still clashes.
    let (_, out) = build_src(
        "type T = Nat  let f : { x: Nat, ..T } -> (`A Nat | ..T) -> Nat = fn a => fn b => 1",
    );
    assert!(
        matches!(out.errors[0].kind, ErrorKind::MixedTail { .. }),
        "{:#?}",
        out.errors
    );
}

/// An argument written where a row parameter goes has to be a row of that
/// shape. A type that already went wrong is not held to it: it absorbed a
/// complaint, and a second one about the same words would tell the reader
/// nothing they can act on.
#[test]
fn an_erroneous_row_argument_absorbs() {
    let (_, out) = build_src("type WithX r = { x: Nat, ..r }  type P = WithX Missing");

    // The undefined name, and nothing about the shape it failed to have.
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                namespace: Namespace::Types
            }
        ),
        "{:#?}",
        out.errors
    );
}

/// A value given as a name that leads back to itself is never given one, which
/// is the term half of the rule `type t = t` already breaks. The value is
/// erased, so inference is never handed the loop.
#[test]
fn a_definition_given_only_as_a_name_is_circular() {
    let (mint, out) = build_src("let x = x");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Circular {
            namespace: Namespace::Terms
        }
    ));
    assert!(matches!(term_value(&mint, &out, "x"), TermKind::Error));
}

/// Every definition on the loop is told, in the order they were written, and
/// only the ones on it: a definition that merely leads into one has nothing to
/// fix.
#[test]
fn every_definition_on_a_loop_is_told() {
    let (mint, out) = build_src("let a = b  let b = a");
    assert_eq!(circular(&mint, &out), ["a", "b"]);

    let (mint, out) = build_src("let a = b  let b = c  let c = a");
    assert_eq!(circular(&mint, &out), ["a", "b", "c"]);

    // `d` leads into the loop and is not on it.
    let (mint, out) = build_src("let d = a  let a = b  let b = a");
    assert_eq!(circular(&mint, &out), ["a", "b"]);
    assert!(matches!(term_value(&mint, &out, "d"), TermKind::Ident(_)));
}

/// A shape ends the chain, however the definition names itself. `fn` is a
/// shape, so this is the recursion the language is for rather than a loop of
/// bare names — and so is everything else a value can be.
#[test]
fn a_definition_reaching_a_shape_is_not_circular() {
    for src in [
        "let f = fn n => f n",
        "let s = { a: s }",
        "let t = `Some t",
        "let p = q.field  let q = p",
        "let u = u 1",
        "let z = 1  let y = z",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    // And nothing was erased on the way.
    let (mint, out) = built("let f = fn n => f n");
    assert!(matches!(term_value(&mint, &out, "f"), TermKind::Fn { .. }));
}

/// A file of definitions that name nobody is a group apiece, in source order.
#[test]
fn independent_definitions_are_a_group_each() {
    let (mint, out) = built("let a = 1  let b = 2  let c = 3");

    assert_eq!(groups(&mint, &out), [vec!["a"], vec!["b"], vec!["c"]]);
    assert!(out.program.groups.iter().all(|group| !group.recursive));
}

/// A definition that names itself is a group of one that says so, which is the
/// case the flag exists for: the members alone cannot tell it from the group
/// above.
#[test]
fn a_self_naming_definition_is_a_recursive_group_of_one() {
    let (mint, out) = built("let f = fn n => f n");

    assert_eq!(groups(&mint, &out), [vec!["f"]]);
    assert!(out.program.groups[0].recursive);
}

/// Two definitions that name each other are one group, members in source
/// order.
#[test]
fn a_mutual_pair_is_one_group() {
    let (mint, out) = built("let even = fn n => odd n  let odd = fn n => even n");

    assert_eq!(groups(&mint, &out), [vec!["even", "odd"]]);
    assert!(out.program.groups[0].recursive);
}

/// Groups come out in dependency order, so what a definition names is always
/// solved first — including when it is written below.
#[test]
fn a_dependency_comes_before_what_names_it() {
    let (mint, out) = built("let id = fn x => x  let a = id 1");
    assert_eq!(groups(&mint, &out), [vec!["id"], vec!["a"]]);

    let (mint, out) = built("let a = id 1  let id = fn x => x");
    assert_eq!(groups(&mint, &out), [vec!["id"], vec!["a"]]);

    // The exact vector, and not merely the pairwise order: a hash-ordered
    // grouping passes every "this is before that" assertion and still hands
    // inference a different order every run.
    let (mint, out) =
        built("let top = mid 1  let mid = fn n => bot n  let bot = fn n => n  let lone = 0");
    // `lone` names nobody and nobody names it, so it sits after the three that
    // have to come in that order — its own name being the last of the four
    // written.
    assert_eq!(
        groups(&mint, &out),
        [vec!["bot"], vec!["mid"], vec!["top"], vec!["lone"]]
    );
}

/// A refused loop is erased, so what is left names nobody: the graph is read
/// off the values as they finally stand rather than as they were written.
#[test]
fn a_refused_loop_is_a_group_of_one() {
    let (mint, out) = build_src("let a = b  let b = a  let c = 1");

    assert_eq!(groups(&mint, &out), [vec!["a"], vec!["b"], vec!["c"]]);
    assert!(out.program.groups.iter().all(|group| !group.recursive));
}

/// Every definition is in exactly one group, and the groups hold nothing else
/// — not a lambda binder, which is no definition at all, and not a name that
/// failed to resolve.
#[test]
fn the_groups_hold_every_definition_once() {
    let (mint, out) = build_src(
        "let id = fn x => x  let even = fn n => odd n  let odd = fn n => even n  \
         let used = id even  let broken = nope",
    );

    let held: Vec<Symbol> = out
        .program
        .groups
        .iter()
        .flat_map(|group| group.members.iter().copied())
        .collect();
    let defined: Vec<Symbol> = out.program.terms.keys().copied().collect();
    let mut sorted = held.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), held.len(), "a definition is in two groups");
    let mut expected = defined;
    expected.sort();
    assert_eq!(sorted, expected);

    assert_eq!(
        groups(&mint, &out),
        [
            vec!["id"],
            vec!["even", "odd"],
            vec!["used"],
            vec!["broken"]
        ]
    );
}

/// A declaration whose fields never run out is refused, and told apart from one
/// that reaches no shape at all by the one step that distinguishes them: a
/// struct whose `..` names a parameter stands for whatever is written there,
/// with the fields beside it added on the way.
///
/// `type T = WithX T` reaches a shape every time round and has an `x` more each
/// time, so there is no finite set of fields for it to have. `type A = B` with
/// `type B = A` reaches no shape at all, which is a different thing gone wrong
/// and keeps its own wording.
#[test]
fn a_declaration_that_adds_fields_to_itself_is_refused() {
    let (mint, out) = build_src("type WithX r = { x: Nat, ..r }\ntype T = WithX T");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "endless-fields");
    let t = &out.program.types[&type_symbol(&mint, &out, "T")];
    assert!(matches!(t.value.tracked, TypeKind::Error), "{:#?}", t.value);
    // And the declaration it goes through is not on the loop, so it keeps its
    // body: only what leads back to itself has anything to fix.
    let with_x = &out.program.types[&type_symbol(&mint, &out, "WithX")];
    assert!(
        !matches!(with_x.value.tracked, TypeKind::Error),
        "{:#?}",
        with_x.value
    );

    // Every declaration on the loop is told, and every one is erased — the way
    // a circular one is.
    let (mint, out) = build_src(
        "type WithX r = { x: Nat, ..r }\n\
         type A r = B r\n\
         type B r = WithX (A r)",
    );
    assert_eq!(out.errors.len(), 2, "{:#?}", out.errors);
    for error in &out.errors {
        assert_eq!(error.kind.code(), "endless-fields");
    }
    for name in ["A", "B"] {
        let decl = &out.program.types[&type_symbol(&mint, &out, name)];
        assert!(
            matches!(decl.value.tracked, TypeKind::Error),
            "{name}: {:#?}",
            decl.value
        );
    }

    // A loop with no such step on it is still `Circular`, worded as it was.
    let (_, out) = build_src("type A = B\ntype B = A");
    assert_eq!(out.errors.len(), 2, "{:#?}", out.errors);
    for error in &out.errors {
        assert_eq!(error.kind.code(), "circular-type");
    }

    // And a declaration that names itself inside a *field* is on no core loop
    // at all: its core is unit, and the recursion is in what a field holds.
    let (_, out) = build_src("type List = { next: List }");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// An argument written at a struct's `..` may not name a field the declaration
/// already names — and what it names reaches through a declared name as much as
/// it is written out, since a `..` handed a name ends up carrying whatever that
/// name carries.
#[test]
fn what_an_argument_carries_is_read_through_its_name() {
    // Written out: the field is right there.
    let (_, out) = build_src("type WithX r = { x: Nat, ..r }  type Bad = WithX { x: Nat }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "repeated-row-field");

    // Through the name: `WithX Nat` carries an `x`, so handing it to `WithX`
    // again names `x` twice.
    let (_, out) = build_src("type WithX r = { x: Nat, ..r }  type Bad = WithX (WithX Nat)");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field, .. } if field == "x"),
        "{:#?}",
        out.errors
    );

    // And a name that carries something else is fine, however deep it is.
    for src in [
        "type WithX r = { x: Nat, ..r }  type Foo = { y: Nat }  type Fine = WithX Foo",
        "type WithY r = { y: Nat, ..r }  type WithX r = { x: Nat, ..r }  \
         type Also = WithX (WithY Nat)",
        "type WithX r = { x: Nat, ..r }  type Id a = a  type Fine = WithX (Id Nat)",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// An argument lowering has already erased is let through wherever a sum's rest
/// goes: the complaint about it was made where it was written, and a second one
/// about a row nobody wrote would be that mistake said twice.
#[test]
fn an_erased_argument_is_let_through_a_sum_tail() {
    let (_, out) = build_src("type Or r = `A | ..r  type Bad = Or Bogus");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "undefined-type");
}

/// The name a nested `let` binds is in scope for both halves — its own value,
/// so a binding may name itself, and the body written after the `in`.
#[test]
fn a_nested_let_binds_its_name_for_the_value_and_the_body() {
    let (mint, out) = built("let a = let f = fn n => f n in f");
    let TermKind::Let {
        name, value, body, ..
    } = term_value(&mint, &out, "a")
    else {
        panic!("expected a let: {:#?}", term_value(&mint, &out, "a"));
    };
    // The body is the name, and the recursive use inside the value is the same
    // symbol — one binding, seen from both sides.
    assert!(matches!(body.kind, TermKind::Ident(symbol) if symbol == name.tracked));
    let TermKind::Fn { body: inner, .. } = &value.kind else {
        panic!("expected a lambda: {:#?}", value.kind);
    };
    let TermKind::Apply { func, .. } = &inner.kind else {
        panic!("expected an application: {:#?}", inner.kind);
    };
    assert!(matches!(func.kind, TermKind::Ident(symbol) if symbol == name.tracked));

    // And the name is a local, minted beside the module the way a lambda's
    // argument is rather than declared as a definition.
    assert_eq!(mint.name(name.tracked), "f");
    assert!(mint.is_local(name.tracked));
    assert!(!out.program.terms.contains_key(&name.tracked));
}

/// And released at the end of the body: a name bound by a nested `let` is not
/// visible after the expression it was written in.
#[test]
fn a_nested_let_releases_its_name_after_the_body() {
    let src = "let leaked = let n = 1 in n\nlet after = n";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
    ));
    assert_eq!(out.errors[0].span.start, src.rfind('n').expect("the use"));
    assert!(matches!(term_value(&mint, &out, "after"), TermKind::Error));
}

/// A nested binding shadows whatever the name meant outside it, silently — the
/// way a lambda's argument already does. Two *definitions* of one name are a
/// different thing and still complain: a scope inside a definition is not a
/// second definition.
#[test]
fn a_nested_let_shadows_without_complaint() {
    let (mint, out) = built("let n = 1\nlet a = let n = { v: 2 } in n");
    let TermKind::Let { name, body, .. } = term_value(&mint, &out, "a") else {
        panic!("expected a let");
    };
    // The use in the body is the inner binding, not the definition above it.
    assert!(matches!(body.kind, TermKind::Ident(symbol) if symbol == name.tracked));
    assert_ne!(name.tracked, term_symbol(&mint, &out, "n"));

    let (_, out) = build_src("let n = 1  let n = 2");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::Duplicate { .. }));
}

/// Two nested lets cannot name each other. There is no block form to bind a
/// group of them together, so `b` is simply not in scope where `a`'s value is
/// written, and naming it is an unresolved name like any other.
#[test]
fn two_nested_lets_cannot_name_each_other() {
    let src = "let e = let a = b in let b = 1 in a";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Terms
        }
    ));
    assert_eq!(out.errors[0].span.start, src.find("b in").expect("the use"));
}

/// A nested binding given as itself is the same complaint, in the same words,
/// that `let x = x` gets at the top level: reported at the value's span, with
/// the value erased so that inference is never handed the loop.
#[test]
fn a_nested_let_given_as_itself_is_circular() {
    let src = "let e = let x = x in x";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "circular-term");
    assert_eq!(
        out.errors[0].span.start,
        src.find("x in").expect("the value")
    );

    let TermKind::Let { value, .. } = term_value(&mint, &out, "e") else {
        panic!("expected a let");
    };
    assert!(matches!(value.kind, TermKind::Error));

    // Including one written under a `fn`, which no definition's own chain
    // reaches: the walk stops at a lambda, so every nested binding is followed
    // from itself as well.
    let src = "let g = fn p => let q = q in q";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "circular-term");
    assert_eq!(
        out.errors[0].span.start,
        src.find("q in").expect("the value")
    );
}

/// A loop closed through two nested bindings tells both of them.
///
/// Two lets written side by side cannot reach each other — `b` is not in scope
/// where `a`'s value is written — so the way two of them close a loop is by
/// nesting: `x` is in scope inside its own value, so `y` written there can name
/// it, and `x` is given as the `let` that stands for `y`.
#[test]
fn a_loop_through_two_nested_lets_reports_both() {
    let src = "let e = let x = let y = x in y in x";
    let (_, out) = build_src(src);
    let spans: Vec<usize> = out
        .errors
        .iter()
        .filter(|error| error.kind.code() == "circular-term")
        .map(|error| error.span.start)
        .collect();
    assert_eq!(
        spans,
        [
            src.find("let y").expect("x's value"),
            src.find("x in y").expect("y's value"),
        ],
        "errors: {:#?}",
        out.errors
    );
}

/// A shape ends a nested chain as surely as it ends a definition's, and the
/// walk reads through a `let` to whatever its *body* stands for.
#[test]
fn a_nested_let_reaching_a_shape_is_not_circular() {
    for src in [
        "let e = let f = fn n => f n in f",
        "let e = let x = 1 in let y = x in y",
        "let e = fn p => let q = p in q",
        "let a = let x = 1 in x  let b = a",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A nested annotation is a written type like any other, so what a row
/// argument may be is asked of one too. An annotation this walk never reached
/// would be a `RepeatedRowField` never reported.
#[test]
fn a_nested_annotation_reaches_the_row_argument_check() {
    let src = "type WithX r = { x: Nat, ..r }\nlet e = let n : WithX { x: Nat } = 1 in n";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::RepeatedRowField {
            shape: Shape::Struct,
            ..
        }
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.find("{ x: Nat }").expect("the argument")
    );
}

/// Grouping reads through a nested `let`: a top-level name mentioned only from
/// inside one still puts the two definitions in one group, and the local the
/// `let` binds is not a definition for a group to be about.
#[test]
fn grouping_sees_through_a_nested_let() {
    let (mint, out) = built("let a = let x = b in { v: x }  let b = let y = a in { w: y }");
    assert_eq!(groups(&mint, &out), [vec!["a", "b"]]);
    assert!(out.program.groups[0].recursive);

    // And a definition whose nested binding names nobody is a group of one.
    let (mint, out) = built("let a = let x = 1 in x");
    assert_eq!(groups(&mint, &out), [vec!["a"]]);
    assert!(!out.program.groups[0].recursive);
}

/// The IR prints as surface syntax, and a nested `let` is no exception: it
/// reads back as what was written, and re-lowers to the same program.
#[test]
fn displays_nested_lets() {
    assert_eq!(
        display_program("let a = let x = 1 in x"),
        "let a = let x = 1 in x"
    );
    assert_eq!(
        display_program("let a = let x : Nat = 1 in x"),
        "let a = let x : Nat = 1 in x"
    );
    // A `let` in argument position wears the parentheses that make it one.
    assert_eq!(
        display_program("let f = fn x => x  let a = f (let x = 1 in x)"),
        "let f = fn x => x\nlet a = f (let x = 1 in x)"
    );
}

/// An explicitly absent label lowers to an absent entry, in the written
/// position among the other labels, spanning the whole `\name`.
#[test]
fn lowers_absent_labels_in_written_position() {
    let src = "let f : { x: Nat, \\y, z: Nat, .. } -> Nat = fn a => a.x";
    let (mint, out) = built(src);
    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .clone()
        .expect("the annotation");
    let TypeKind::Arrow { from, .. } = annotation.ty.tracked else {
        panic!("expected an arrow, got {:?}", annotation.ty.tracked);
    };
    let TypeKind::Struct { fields, .. } = from.tracked else {
        panic!("expected a struct parameter");
    };
    assert_eq!(fields.keys().collect::<Vec<_>>(), vec!["x", "y", "z"]);
    assert!(matches!(fields["y"], TypeField::Absent { .. }));
    assert_eq!(
        fields["y"].name_span().start,
        src.find("\\y").expect("the mark")
    );
    assert_eq!(fields["y"].name_span().width, 2);

    // The sum counterpart: the case keeps its backtick, and the span keeps
    // the `\` in front of it.
    let src = "let x : `Ok Nat | \\`Err | .. = `Ok 1";
    let (mint, out) = built(src);
    let annotation = out.program.terms[&term_symbol(&mint, &out, "x")]
        .annotation
        .clone()
        .expect("the annotation");
    let TypeKind::Sum { cases, .. } = annotation.ty.tracked else {
        panic!("expected a sum, got {:?}", annotation.ty.tracked);
    };
    assert_eq!(cases.keys().collect::<Vec<_>>(), vec!["Ok", "Err"]);
    assert!(matches!(cases["Err"], SumCase::Absent { .. }));
    assert_eq!(
        cases["Err"].name_span().start,
        src.find("\\`Err").expect("the mark")
    );
    assert_eq!(cases["Err"].name_span().width, 5);
}

/// An explicitly absent label in a closed composite is refused: the `\` says
/// the `..` beside it may not stand for the label, and a type with no `..`
/// already says that. Reported at the `\name`, and the composite absorbs.
#[test]
fn an_absent_label_needs_a_tail() {
    let src = "let x : { a: Nat, \\y } = 1";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::AbsentInClosed {
            shape: Shape::Struct,
            label,
        } if label == "y"
    ));
    assert_eq!(out.errors[0].span.start, src.find("\\y").expect("the mark"));
    assert_eq!(out.errors[0].span.width, 2);
    let annotation = out.program.terms[&term_symbol(&mint, &out, "x")]
        .annotation
        .clone()
        .expect("the annotation");
    assert!(
        matches!(annotation.ty.tracked, TypeKind::Error),
        "the struct absorbs: {annotation:#?}"
    );

    let src = "let x : `A | \\`B = 1";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::AbsentInClosed {
            shape: Shape::Sum,
            label,
        } if label == "B"
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.find("\\`B").expect("the mark")
    );
    assert_eq!(out.errors[0].span.width, 3);

    // Every absence in the row is its own report, the way every `when` in a
    // declared type is: each is a mark the reader can act on.
    let (_, out) = build_src("let x : { \\y, \\z } = 1");
    assert_eq!(out.errors.len(), 2, "errors: {:#?}", out.errors);

    // A declaration's tail must name a parameter (the existing rule), so a
    // `\` beside an unbound `..r` is that one complaint, unchanged — the
    // absence itself is fine wherever the tail is.
    let (_, out) = build_src("type T = { \\y, ..r }");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::OpenDeclaredType {
            shape: Shape::Struct
        }
    ));

    // And a closed declaration gets the same complaint an annotation does.
    let (_, out) = build_src("type T = { \\y }");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::AbsentInClosed { .. }
    ));
}

/// One composite naming a label both ways is a duplicate, reported where the
/// second mention is written — and so are two absences of one name. The first
/// occurrence is the one that survives.
#[test]
fn a_label_named_present_and_absent_is_a_duplicate() {
    for (src, second) in [
        ("let x : { x: Nat, \\x, .. } = 1", "\\x"),
        ("let x : { \\x, x: Nat, .. } = 1", "x: Nat"),
        ("let x : { \\x, \\x, .. } = 1", "\\x"),
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(out.errors[0].kind, ErrorKind::DuplicateField),
            "{src}: {:#?}",
            out.errors
        );
        assert_eq!(
            out.errors[0].span.start,
            src.rfind(second).expect("the second mention"),
            "{src}"
        );
    }

    let src = "let x : `A | \\`A | .. = 1";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::DuplicateCase));
    assert_eq!(
        out.errors[0].span.start,
        src.find("\\`A").expect("the second mention")
    );
}

/// An absent label joins its row parameter's lacks set exactly as a written
/// field does: `\y` says the tail has no `y`, which is the same sentence a
/// field named `y` makes it say.
#[test]
fn an_absent_label_joins_a_parameters_lacks() {
    let (_, out) = built("type T r = { x: Nat, \\y, ..r }");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params.len(), 1);
    assert_eq!(decl.params[0].kind, row(&["x", "y"]));

    let (_, out) = built("type NoErr r = `Ok Nat | \\`Err | ..r");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params[0].kind, cases(&["Ok", "Err"]));
}

/// An argument naming an explicitly absent label is refused at the argument,
/// with the existing complaint for one naming a label the declaration already
/// names.
#[test]
fn an_argument_may_not_name_an_absent_label() {
    let src = "type T r = { x: Nat, \\y, ..r }\nlet v : T { y: Nat } = { x: 1, y: 2 }";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::RepeatedRowField {
            shape: Shape::Struct,
            field,
        } if field == "y"
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.find("{ y: Nat }").expect("the argument")
    );

    let src = "type NoErr r = `Ok Nat | \\`Err | ..r\nlet x : NoErr (`Err Nat) = `Ok 1";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::RepeatedRowField {
            shape: Shape::Sum,
            field,
        } if field == "Err"
    ));

    // An argument that names none of them is welcome.
    let (_, out) =
        build_src("type NoErr r = `Ok Nat | \\`Err | ..r\nlet x : NoErr (`Warn Nat) = `Ok 1");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// The IR prints an absence as written, and the rendering re-lowers to the
/// same program.
#[test]
fn displays_absent_labels_as_written() {
    assert_eq!(
        display_program("type T r = { x: Nat, \\y, ..r }"),
        "type T r = { x: Nat, \\y, ..r }"
    );
    assert_eq!(
        display_program("type NoErr r = `Ok Nat | \\`Err | ..r"),
        "type NoErr r = `Ok Nat | \\`Err | ..r"
    );
    assert_eq!(
        display_program("let f : { \\y, ..r } -> { ..r } = fn a => a"),
        "let f : { \\y, ..r } -> { ..r } = fn a => a"
    );
}

/// A recursive declaration may write an absence beside its recursion: the
/// growth walk skips an absent label, there being no argument under it to
/// grow — in either shape.
#[test]
fn a_recursive_declaration_may_write_an_absence() {
    let (_, out) = built("type T r = { \\y, next: T r, ..r }");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params[0].kind, row(&["y", "next"]));

    let (_, out) = built("type S r = `A (S r) | \\`B | ..r");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params[0].kind, cases(&["A", "B"]));
}

/// Lower a program that may complain and render it back, plus the complaints
/// as `code@start` pairs. The rendering is not re-parsed: fresh temporaries
/// print with the `%` that marks them as the compiler's own, which no
/// identifier can spell.
fn lowered_with_errors(src: &str) -> (String, Vec<String>) {
    let (mint, out) = build_src(src);
    let printed = print::ir::program(&out.program, &mint).to_string();
    let errors = out
        .errors
        .iter()
        .map(|error| format!("{}@{}", error.kind.code(), error.span.start))
        .collect();
    (printed, errors)
}

/// [`lowered_with_errors`] for a program lowering should not complain about.
fn lowered(src: &str) -> String {
    let (printed, errors) = lowered_with_errors(src);
    assert!(errors.is_empty(), "ir errors for {src:?}: {errors:#?}");
    printed
}

/// An identifier `let` is byte-for-byte the binding it always was: no
/// temporary, no projections, and the value still able to name the binding
/// recursively.
#[test]
fn an_identifier_let_lowers_unchanged() {
    assert_eq!(lowered("let x = 1"), "let x = 1");
    assert_eq!(lowered("let f = fn n => f n"), "let f = fn n => f n");
    assert_eq!(lowered("let a = let x = 1 in x"), "let a = let x = 1 in x");
}

/// R6's statement half: `let {x, y} = e` is a fresh definition holding `e`,
/// then one definition per field, in written order — N+1 definitions. The
/// temporary wears the exact pattern's demand as an annotation: exactly the
/// named fields, each field's type a hole for the solver.
#[test]
fn a_struct_let_statement_makes_a_definition_per_field() {
    assert_eq!(
        lowered("let {x, y} = { x: 1, y: 2 }"),
        "let %struct : { x: ?, y: ? } = { x: 1, y: 2 }\nlet x = %struct.x\nlet y = %struct.y"
    );
    // Renaming and annotation: the written annotation is the contract on the
    // whole value, so it holds the value on a binding of its own, and the
    // exact demand rides on the temporary the projections read.
    assert_eq!(
        lowered("let {x: a} : { x: Nat } = { x: 1 }"),
        "let %value : { x: Nat } = { x: 1 }\nlet %struct : { x: ? } = %value\nlet a = %struct.x"
    );
    // `let {} = e` is the unit pattern by another spelling: exactly no
    // fields.
    assert_eq!(lowered("let {} = {}"), "let %unit : {} = {}");
    // With the `..` there is no demand beyond the projections': the pattern
    // is open, and the temporary is bare.
    assert_eq!(
        lowered("let {x, ..} = { x: 1, y: 2 }"),
        "let %struct = { x: 1, y: 2 }\nlet x = %struct.x"
    );
    // Nesting chains through intermediate temporaries, still in field order,
    // each exact level with a demand of its own.
    assert_eq!(
        lowered("let {pos: {x, y}, tag: t} = p  let p = { pos: { x: 1, y: 2 }, tag: 3 }"),
        "let %struct : { pos: ?, tag: ? } = p\n\
         let %struct : { x: ?, y: ? } = %struct.pos\n\
         let x = %struct.x\n\
         let y = %struct.y\n\
         let t = %struct.tag\n\
         let p = { pos: { x: 1, y: 2 }, tag: 3 }"
    );
}

/// R6's expression half: `let {x, y: a} = e in b` is a fresh temporary and one
/// nested `let` per field.
#[test]
fn a_struct_let_expression_chains_through_a_temporary() {
    assert_eq!(
        lowered("let d = fn e => let {x, y: a} = e in x"),
        "let d = fn e => let %struct : { x: ?, y: ? } = e in \
         let x = %struct.x in let a = %struct.y in x"
    );
    // Open, the pattern demands only what the projections do, so the
    // temporary is bare.
    assert_eq!(
        lowered("let d = fn e => let {x, ..} = e in x"),
        "let d = fn e => let %struct = e in let x = %struct.x in x"
    );
    // A written annotation holds the whole value on a binding of its own, and
    // the exact demand rides on the temporary the projections read.
    assert_eq!(
        lowered("let a = let {x} : { x: Nat } = { x: 1 } in x"),
        "let a = let %value : { x: Nat } = { x: 1 } in \
         let %struct : { x: ? } = %value in let x = %struct.x in x"
    );
    // The spec's own nested example.
    assert_eq!(
        lowered("let dist = fn p => let {pos: {x, y}} = p in add x y  let add = fn a b => a"),
        "let dist = fn p => \
         let %struct : { pos: ? } = p in \
         let %struct : { x: ?, y: ? } = %struct.pos in \
         let x = %struct.x in let y = %struct.y in add x y\n\
         let add = fn a => fn b => a"
    );
    // `()` constrains the value to unit through an annotated fresh binding.
    assert_eq!(
        lowered("let u = let () = {} in 1"),
        "let u = let %unit : {} = {} in 1"
    );
}

/// R4/R5 of the wildcard spec: `let _ = e` is a definition under a name
/// nothing can write, so any number of them coexist — with each other, and
/// with every named definition — and no duplicate complaint can ever mention
/// `_`. The annotation rides on the hidden definition, so `let _ : T = e` is
/// a type assertion.
#[test]
fn a_wildcard_let_statement_defines_a_hidden_fresh_name() {
    assert_eq!(lowered("let _ = 1"), "let %discard = 1");
    // The headline: two of them, plus a third asserting a type — no
    // duplicate-definition error anywhere.
    assert_eq!(
        lowered("let _ = f 1  let _ = f 2  let _ : Nat = f 3  let f = fn x => x"),
        "let %discard = f 1\nlet %discard = f 2\nlet %discard : Nat = f 3\nlet f = fn x => x"
    );
    // Nor a collision with a named definition of any spelling.
    assert_eq!(
        lowered("let x = 1  let _ = 2  let x2 = 3"),
        "let x = 1\nlet %discard = 2\nlet x2 = 3"
    );

    // The hidden definitions are distinct symbols, each holding its own value.
    let (mint, out) = built("let _ = 1  let _ = 2");
    let discards: Vec<Symbol> = out
        .program
        .terms
        .keys()
        .copied()
        .filter(|symbol| mint.name(*symbol) == "%discard")
        .collect();
    assert_eq!(discards.len(), 2);
    assert_ne!(discards[0], discards[1]);
}

/// R5's expression half: `let _ = e in b` is a `Let` through a fresh symbol —
/// the value still on the page, typechecked, and nothing in `b` able to name
/// it, because there is no name.
#[test]
fn a_wildcard_let_expression_binds_a_hidden_fresh_name() {
    assert_eq!(
        lowered("let a = let _ = f 1 in 2  let f = fn x => x"),
        "let a = let %discard = f 1 in 2\nlet f = fn x => x"
    );
    // The annotation stays the contract on the value.
    assert_eq!(
        lowered("let a = let _ : Nat = 1 in 2"),
        "let a = let %discard : Nat = 1 in 2"
    );

    // The fresh symbol is bound into no scope: the body's `2` aside, nothing
    // references it, and the tree says so.
    let (mint, out) = built("let a = let _ = 1 in 2");
    let TermKind::Let { name, body, .. } = term_value(&mint, &out, "a") else {
        panic!("a lowers to a let");
    };
    let mut names = Vec::new();
    references_of(body, &mut names);
    assert!(!names.contains(&name.tracked), "the body names the discard");
}

/// Every symbol a term mentions, for the wildcard tests to assert nothing
/// mentions a hidden one.
fn references_of(term: &Term, out: &mut Vec<Symbol>) {
    match &term.kind {
        TermKind::Ident(symbol) => out.push(*symbol),
        TermKind::Apply { func, arg } => {
            references_of(func, out);
            references_of(arg, out);
        }
        TermKind::Fn { body, .. } => references_of(body, out),
        TermKind::Handle { body, handler } => {
            references_of(body, out);
            for arm in &handler.arms {
                references_of(&arm.body, out);
            }
            if let Some(ret) = &handler.ret {
                references_of(&ret.body, out);
            }
        }
        TermKind::Raise(value) => references_of(value, out),
        TermKind::Operation { .. } => {}
        TermKind::Let { value, body, .. } => {
            references_of(value, out);
            references_of(body, out);
        }
        TermKind::Struct(fields) => {
            for field in fields.values() {
                references_of(&field.value, out);
            }
        }
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                references_of(payload, out);
            }
        }
        TermKind::Project { base, .. } => references_of(base, out),
        TermKind::Match { scrutinee, arms } => {
            references_of(scrutinee, out);
            for (_, body) in arms {
                references_of(body, out);
            }
        }
        TermKind::Natural(_) | TermKind::Error => {}
    }
}

/// R6: a wildcard leaf of a struct-pattern `let` keeps the field demand — a
/// hidden fresh definition holds the projection — while binding nothing. Both
/// forms of the `let`.
#[test]
fn a_wildcard_struct_leaf_keeps_the_projection() {
    assert_eq!(
        lowered("let {x: _, y} = { x: 1, y: 2 }"),
        "let %struct : { x: ?, y: ? } = { x: 1, y: 2 }\n\
         let %discard = %struct.x\nlet y = %struct.y"
    );
    assert_eq!(
        lowered("let use_y = fn p => let {x: _, y} = p in y"),
        "let use_y = fn p => let %struct : { x: ?, y: ? } = p in \
         let %discard = %struct.x in let y = %struct.y in y"
    );
}

/// R3: `_` never joins the duplicate-binder check — `{a: _, b: _}` is legal,
/// and `_` beside a named binder is too — while a real name bound twice is
/// still the R13 mistake it was.
#[test]
fn a_wildcard_is_exempt_from_the_duplicate_binder_check() {
    assert_eq!(
        lowered("let f = fn e => match e with | `Pair { a: _, b: _ } => 1 | _ => 2 end"),
        "let f = fn e => match e with | `Pair { a: _, b: _ } => 1 | _ => 2 end"
    );
    assert_eq!(
        lowered("let f = fn e => match e with { a: _, b: x } => x end"),
        "let f = fn e => match e with | { a: _, b: x } => x end"
    );

    // The same pattern with a name where the wildcards were still fails.
    let (_, errors) = lowered_with_errors("let f = fn e => match e with { a: x, b: x } => x end");
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].starts_with("duplicate-binding@"), "{errors:#?}");
}

/// R7: `fn _ => e` lowers to the ordinary `Fn` node with a fresh symbol
/// nothing references — the shape does not change — and any number of `_`
/// arguments mix with names.
#[test]
fn a_wildcard_fn_argument_binds_a_symbol_nothing_references() {
    let (mint, out) = built("let f = fn _ => 1");
    let TermKind::Fn { arg, body } = term_value(&mint, &out, "f") else {
        panic!("f is a function");
    };
    assert_eq!(mint.name(arg.tracked), "%discard");
    let mut names = Vec::new();
    references_of(body, &mut names);
    assert!(!names.contains(&arg.tracked));

    // Mixing works, and the middle name still resolves to its own argument.
    assert_eq!(
        lowered("let f = fn _ x _ => x"),
        "let f = fn %discard => fn x => fn %discard => x"
    );
    let (mint, out) = built("let f = fn _ x _ => x");
    let TermKind::Fn { arg: first, body } = term_value(&mint, &out, "f") else {
        panic!("f is a function");
    };
    let TermKind::Fn { arg: x, body } = &body.kind else {
        panic!("a second argument");
    };
    let TermKind::Fn { arg: second, body } = &body.kind else {
        panic!("a third argument");
    };
    // Two discards, two symbols.
    assert_ne!(first.tracked, second.tracked);
    assert!(matches!(&body.kind, TermKind::Ident(symbol) if *symbol == x.tracked));
}

/// R8: a wildcard does not launder the refutability of what surrounds it —
/// `` let `Some _ = e `` is still the refutable-binding error, in both forms,
/// with the unchanged wording.
#[test]
fn a_wildcard_does_not_make_a_refutable_binding_calm() {
    let src = "let `Some _ = opt  let opt = `Some 1";
    let (_, errors) = lowered_with_errors(src);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(
        errors[0],
        format!("binding-can-fail@{}", src.find("`Some").expect("the tag"))
    );

    let src = "let a = let `Some _ = opt in 1  let opt = `Some 1";
    let (_, errors) = lowered_with_errors(src);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].starts_with("binding-can-fail@"), "{errors:#?}");
}

/// A `_` arm anywhere is lowered and kept: where a catch-all belongs is the
/// `patterns` phase's question, asked after inference so the starved arms are
/// still typechecked. Lowering says nothing and drops nothing.
#[test]
fn a_wildcard_arm_lowers_wherever_it_was_written() {
    assert_eq!(
        lowered("let f = fn n => match n with _ => 1 | 0 => 2 end"),
        "let f = fn n => match n with | _ => 1 | 0 => 2 end"
    );
    assert_eq!(
        lowered("let f = fn e => match e with _ => 1 | _ => 2 end"),
        "let f = fn e => match e with | _ => 1 | _ => 2 end"
    );
    // Last, it is what a named catch-all is: an ordinary final arm.
    assert_eq!(
        lowered("let f = fn n => match n with 0 => 1 | _ => 2 end"),
        "let f = fn n => match n with | 0 => 1 | _ => 2 end"
    );
}

/// R5: a match lowers to the one matrix node, arms as written and normalized
/// — puns expanded, symbols resolved, each body exactly once — and prints
/// back as the match the reader wrote.
#[test]
fn a_match_lowers_to_one_matrix_node() {
    assert_eq!(
        lowered("let get = fn opt => match opt with | `Some x => x | `None => 0 end"),
        "let get = fn opt => match opt with | `Some x => x | `None => 0 end"
    );
    // A pun arrives expanded — field "x" carrying x's own symbol — and
    // nothing else changes shape.
    assert_eq!(
        lowered("let f = fn e => match e with | { x, y: a } => x end"),
        "let f = fn e => match e with | { x: x, y: a } => x end"
    );
}

/// No artifact of any compilation exists: no scrutinee temporary, no join
/// point, no fresh fallthrough or case binder — the tree the reader gets is
/// the tree they wrote.
#[test]
fn no_tree_artifacts_exist() {
    for src in [
        "let f = fn e => match e with | `A `X x => 1 | r => 2 end",
        "let f = fn e => match e with | { a: `A, b: `B } => 1 | r => 2 end",
        "let f = fn e => match e with | `A 0 => 1 | `A `X => 2 | r => 3 end",
    ] {
        let (printed, _) = lowered_with_errors(src);
        for artifact in ["%scrut", "%join", "%fall", "%case"] {
            assert!(!printed.contains(artifact), "{src}: {printed}");
        }
    }
}

/// The spec's nested example: three arms, two testing the same tag with
/// different sub-patterns, all kept apart — the IR is the matrix, not a
/// merged tree — and the two `tail` binders are distinct symbols.
#[test]
fn nested_arms_stay_written() {
    let src = "let pick = fn l => match l with \
               | `Cons { head: `Some x, tail: t } => x \
               | `Cons { head: `None, tail: t } => 0 \
               | `Nil => 0 \
               end";
    assert_eq!(
        lowered(src),
        "let pick = fn l => match l with \
         | `Cons { head: `Some x, tail: t } => x \
         | `Cons { head: `None, tail: t } => 0 \
         | `Nil => 0 \
         end"
    );

    // The two `t`s are two binders: one symbol apiece, not one shared.
    let (mint, out) = built(src);
    let TermKind::Fn { body, .. } = term_value(&mint, &out, "pick") else {
        panic!("pick is a function");
    };
    let TermKind::Match { arms, .. } = &body.kind else {
        panic!("the body is a match");
    };
    assert_eq!(arms.len(), 3);
    let tail_binder = |pattern: &ruddy::ir::Pattern| -> Symbol {
        let PatternKind::Tag {
            payload: Some(payload),
            ..
        } = &pattern.tracked
        else {
            panic!("a Cons arm");
        };
        let PatternKind::Struct { fields, .. } = &payload.tracked else {
            panic!("a struct payload");
        };
        let PatternKind::Bind(name) = &fields["tail"].value.tracked else {
            panic!("tail binds");
        };
        name.tracked
    };
    assert_ne!(tail_binder(&arms[0].0), tail_binder(&arms[1].0));
}

/// The sole irrefutable arm is legal and stays a match: one arm, binding the
/// whole value — `match e with x => b end` means `let x = e in b`, and the
/// meaning is typing's to give.
#[test]
fn a_sole_catch_all_keeps_its_shape() {
    assert_eq!(
        lowered("let same = fn v => match v with w => w end"),
        "let same = fn v => match v with | w => w end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with {x} => x end"),
        "let f = fn v => match v with | { x: x } => x end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with () => 1 end"),
        "let f = fn v => match v with | () => 1 end"
    );
    // `{}` reaches into nothing: it binds nothing, tests nothing, and is as
    // much a catch-all as a bare name.
    assert_eq!(
        lowered("let f = fn v => match v with {} => 1 end"),
        "let f = fn v => match v with | {} => 1 end"
    );
}

/// The empty match survives as a match with no arms: the empty sum's
/// eliminator, with nothing for the matrix checks to say — no value exists
/// to go unhandled.
#[test]
fn an_empty_match_keeps_its_shape() {
    assert_eq!(
        lowered("let absurd = fn v => match v with end"),
        "let absurd = fn v => match v with end"
    );
}

/// A catch-all after tag arms is an ordinary last arm of the matrix.
#[test]
fn a_catch_all_is_an_ordinary_last_arm() {
    assert_eq!(
        lowered("let first = fn v => match v with | `Some x => x | rest => rest end"),
        "let first = fn v => match v with | `Some x => x | rest => rest end"
    );
}

/// Natural arms are arms like any other.
#[test]
fn natural_arms_lower_flat() {
    assert_eq!(
        lowered("let d = fn n => match n with | 0 => `Zero | 1 => `One | k => `Many end"),
        "let d = fn n => match n with | 0 => `Zero | 1 => `One | k => `Many end"
    );
}

/// A match inside a `let`'s own value is a shape, like a projection: the
/// binding asks something of the name, so the loop still describes a value —
/// a sole catch-all included, since the match survives as a node rather than
/// collapsing into a chain of names.
#[test]
fn a_match_is_a_shape_for_the_circularity_walk() {
    assert_eq!(
        lowered("let x = match x with | `A y => y | r => r end"),
        "let x = match x with | `A y => y | r => r end"
    );
    let (_, errors) =
        lowered_with_errors("let a = let x = match x with | `A y => y | r => r end in x");
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = lowered_with_errors("let x = match x with w => w end");
    assert!(errors.is_empty(), "{errors:#?}");
}

/// Grouping still sees a reference made from inside an arm's body, so two
/// definitions reaching each other through a match share a group.
#[test]
fn grouping_sees_references_inside_arm_bodies() {
    let (mint, out) = built(
        "let a = fn v => match v with | `Go x => b x | r => 0 end\n\
         let b = fn x => a x",
    );
    assert_eq!(groups(&mint, &out), [vec!["a", "b"]]);
}

/// A pattern that can fail is refused on a `let` — pointing at the tag or the
/// number that makes it able to — and lowering stays total: every name it
/// would have bound is still bound, to error values, so downstream uses
/// resolve and one mistake makes one complaint.
#[test]
fn a_refutable_let_is_refused_and_its_names_still_bind() {
    // The statement form: the value keeps its definition, the names become
    // error-valued ones — so `x` resolves in `use` and nothing cascades.
    let (printed, errors) =
        lowered_with_errors("let opt = `Some 1  let `Some x = opt  let use = x");
    assert_eq!(errors, ["binding-can-fail@23"], "{errors:#?}");
    assert_eq!(
        printed,
        "let opt = `Some 1\nlet %value = opt\nlet x = <error>\nlet use = x"
    );

    // The expression form, and the number as the refuter: the complaint
    // points at the `0`.
    let (printed, errors) = lowered_with_errors("let a = let {a: 0} = { a: 1 } in 2");
    assert_eq!(errors, ["binding-can-fail@16"], "{errors:#?}");
    assert_eq!(printed, "let a = let %value = { a: 1 } in 2");
    let (printed, errors) = lowered_with_errors("let a = let `Some x = `Some 1 in x");
    assert_eq!(errors, ["binding-can-fail@12"], "{errors:#?}");
    assert_eq!(
        printed,
        "let a = let %value = `Some 1 in let x = <error> in x"
    );
}

/// Every arm is lowered, kept, and printed back — an arm nothing can reach, a
/// catch-all anywhere, a match with no final arm, a mixed match. The checks
/// that used to prune or refuse these moved behind inference, into the
/// `patterns` phase, so lowering's whole job is to keep the matrix as
/// written; the only complaint left below is the one that never moved, a name
/// inside an arm's body that resolves to nothing.
#[test]
fn every_arm_is_lowered_and_kept() {
    // A shadowed arm survives with its body.
    assert_eq!(
        lowered("let f = fn e => match e with | `A x => 1 | `A y => 2 end"),
        "let f = fn e => match e with | `A x => 1 | `A y => 2 end"
    );
    // A misplaced catch-all starves nothing here: the arms after it stay.
    assert_eq!(
        lowered("let f = fn e => match e with | r => 1 | `A x => 2 end"),
        "let f = fn e => match e with | r => 1 | `A x => 2 end"
    );
    // A natural match with no final arm keeps its shape and raises nothing.
    assert_eq!(
        lowered("let f = fn n => match n with | 0 => 1 end"),
        "let f = fn n => match n with | 0 => 1 end"
    );
    // A mixed match is not lowering's to refuse any more: the solver's
    // ordinary mismatch is the only complaint such a program gets.
    assert_eq!(
        lowered("let f = fn e => match e with | 0 => 1 | `A x => 2 | k => 3 end"),
        "let f = fn e => match e with | 0 => 1 | `A x => 2 | k => 3 end"
    );
    // A name inside a kept arm's body still resolves — or fails to, which is
    // still lowering's own complaint.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | `A x => 1 | `A y => oops end");
    assert_eq!(errors, ["undefined-term@51"], "{errors:#?}");
}

/// The column unions that broke revision 1 lower clean: positions are typed
/// and checked as the union of what the whole match tests there, never
/// per-arm, so overlapping arms with different sub-patterns are simply the
/// matrix they are.
#[test]
fn overlapping_arms_are_clean() {
    lowered(
        "let f = fn e => match e with \
         | { a: `A, b: `X, c: x } => 1 | { a: `A, b: `Y, c: y } => 2 end",
    );
    lowered(
        "let f = fn e => match e with \
         | { a: `A, b: x } => 1 | { a: `B, b: 0 } => 2 | { a: `B, b: k } => 3 end",
    );
    lowered("let f = fn e => match e with | `A `X x => 1 | `A w => 2 | r => 3 end");
}

/// The `..` survives normalization: exactness is read off the rest marker by
/// inference and the pattern checks, so the marker has to arrive — and the
/// printed pattern has to show it, so the IR tab reads as the source did.
#[test]
fn a_rest_marker_survives_normalization() {
    assert_eq!(
        lowered("let f = fn v => match v with {x, ..} => x | {} => 0 end"),
        "let f = fn v => match v with | { x: x, .. } => x | {} => 0 end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with {..} => 1 end"),
        "let f = fn v => match v with | { .. } => 1 end"
    );
    let (mint, out) = built("let f = fn v => match v with {x, ..} => x end");
    let TermKind::Fn { body, .. } = term_value(&mint, &out, "f") else {
        panic!("f is a function");
    };
    let TermKind::Match { arms, .. } = &body.kind else {
        panic!("the body is a match");
    };
    let PatternKind::Struct { fields, rest } = &arms[0].0.tracked else {
        panic!("a struct pattern");
    };
    assert!(rest.is_some());
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["x"]);
}

/// One pattern binding one name twice is reported at the repeat, in any
/// nesting; two different arms may of course bind the same name.
#[test]
fn a_pattern_binding_a_name_twice_is_refused() {
    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {x, x} => 1 end");
    assert_eq!(errors, ["duplicate-binding@35"], "{errors:#?}");

    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {a: x, b: x} => x end");
    assert_eq!(errors, ["duplicate-binding@41"], "{errors:#?}");

    // Across nesting in one pattern.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | {a: x, b: {c: x}} => x end");
    assert_eq!(errors, ["duplicate-binding@45"], "{errors:#?}");

    // Two arms binding one name are two scopes, not a repeat.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | `A x => x | `B x => x end");
    assert!(errors.is_empty(), "{errors:#?}");
}

/// A struct pattern naming one field twice is the struct expression's
/// complaint, made about a pattern — unless both entries are puns, which is
/// the same name bound twice and worded as that instead. The first field
/// stands; the repeat's binders are bound to error values, so a body naming
/// one resolves and nothing cascades.
#[test]
fn a_struct_pattern_naming_a_field_twice_is_refused() {
    let (printed, errors) =
        lowered_with_errors("let f = fn e => match e with | {x: a, x: b} => b end");
    assert_eq!(errors, ["duplicate-field@38"], "{errors:#?}");
    assert_eq!(
        printed,
        "let f = fn e => match e with | { x: a } => let b = <error> in b end"
    );

    // A pun beside a rename of the same field is still the field named twice,
    // whichever order the two were written in.
    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {x, x: b} => b end");
    assert_eq!(errors, ["duplicate-field@35"], "{errors:#?}");
    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {x: a, x} => a end");
    assert_eq!(errors, ["duplicate-field@38"], "{errors:#?}");
}

/// The corners of pattern `let`s the main tests walk past: `()` with and
/// without a written annotation, in both forms, and a statement pattern
/// repeating a name.
#[test]
fn pattern_let_corners() {
    // `let () = e` as a statement: one fresh definition annotated unit.
    assert_eq!(lowered("let () = {}"), "let %unit : {} = {}");
    // With a written annotation, the annotation is the contract on the value
    // and the unit demand goes on a second binding of it.
    assert_eq!(
        lowered("let () : {} = {}"),
        "let %value : {} = {}\nlet %unit : {} = %value"
    );
    assert_eq!(
        lowered("let u = let () : {} = {} in 1"),
        "let u = let %value : {} = {} in let %unit : {} = %value in 1"
    );

    // A statement pattern repeating a name: the repeat is the pattern's own
    // complaint, not a second definition, and the walk stays total — the
    // repeat binds its stand-in to an error value.
    let (printed, errors) = lowered_with_errors("let {x, x} = { x: 1 }");
    assert_eq!(errors, ["duplicate-binding@8"], "{errors:#?}");
    assert_eq!(
        printed,
        "let %struct : { x: ? } = { x: 1 }\nlet x = %struct.x\nlet x = <error>"
    );

    // A refutable statement pattern with a number: the complaint quotes it.
    let (_, errors) = lowered_with_errors("let {a: 0} = { a: 1 }");
    assert_eq!(errors, ["binding-can-fail@8"], "{errors:#?}");

    // A bare tag on a statement `let` is refused the same way, binding
    // nothing — there is nothing for it to bind.
    let (printed, errors) = lowered_with_errors("let `None = {}");
    assert_eq!(errors, ["binding-can-fail@4"], "{errors:#?}");
    assert_eq!(printed, "let %value = {}");

    // A refused binding still binds the names inside its calm corners — the
    // nested struct's — and points at the tag past a field that is fine.
    let (printed, errors) = lowered_with_errors("let a = let {p: {q}, r: `Bad} = {} in q");
    assert_eq!(errors, ["binding-can-fail@24"], "{errors:#?}");
    assert_eq!(printed, "let a = let %value = {} in let q = <error> in q");
}

/// A `where` clause survives lowering as the formula it was written as, over
/// the names the type's `when`s bound — and prints back as itself, since an
/// annotation is a contract and a contract that came out spelled differently
/// would be a different one.
#[test]
fn an_annotation_keeps_its_clause() {
    let (mint, out) = built("let f : { x when a: Nat, y when b: Nat } where a != b = { x: 1 }");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .as_ref()
        .expect("the annotation");
    let clause = annotation.clause.as_ref().expect("the clause");
    assert!(matches!(clause.tracked, ClauseKind::NotEqual(..)));
    assert_eq!(
        print::ir::clause(&clause.tracked, &mint).to_string(),
        "a != b"
    );

    // And the labels keep the names they were written with, which is what the
    // clause resolves against.
    let TypeKind::Struct { fields, .. } = &annotation.ty.tracked else {
        panic!("expected a struct annotation");
    };
    let names: Vec<Option<&str>> = fields
        .values()
        .map(|field| match field {
            TypeField::Written { when, .. } => when.as_ref().and_then(|w| w.name.as_deref()),
            TypeField::Absent { .. } => None,
        })
        .collect();
    assert_eq!(names, [Some("a"), Some("b")]);
}

/// `when _` binds nothing, which is the whole of what makes it unnameable: the
/// presence is minted like any other and no clause can be about it.
#[test]
fn the_anonymous_presence_binds_no_name() {
    let (mint, out) = built("let f : { x when _: Nat } = { x: 1 }");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .as_ref()
        .expect("the annotation");
    let TypeKind::Struct { fields, .. } = &annotation.ty.tracked else {
        panic!("expected a struct annotation");
    };
    let TypeField::Written {
        when: Some(when), ..
    } = &fields["x"]
    else {
        panic!("expected a written field with a clause");
    };
    assert!(when.name.is_none());

    let (_, out) = build_src("let f : { x when _: Nat } where x = { x: 1 }");
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["unbound-presence"]
    );
}

/// A clause names presences the type beside it binds, and nothing else: a name
/// no `when` bound stands for nothing at all, so it is refused where it was
/// written and the clause absorbs whole.
#[test]
fn a_clause_may_not_name_what_the_type_does_not_bind() {
    let src = "let f : { x when a: Nat } where c = { x: 1 }";
    let (mint, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        &error.kind,
        ErrorKind::UnboundPresence { name } if name == "c"
    ));
    assert_eq!(error.span.start, src.rfind('c').expect("the name"));

    // The clause absorbs rather than keeping the half that resolved: half a
    // contract is a contract nobody wrote.
    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .as_ref()
        .expect("the annotation");
    assert!(annotation.clause.is_none());

    // Both sides are lowered before either is judged, so a clause naming two
    // unbound presences reports both — and the clause still absorbs whole,
    // through every connective one can be written with.
    for (src, count) in [
        ("let f : { x when a: Nat } where c or d = { x: 1 }", 2),
        ("let f : { x when a: Nat } where c and d = { x: 1 }", 2),
        ("let f : { x when a: Nat } where c = d = { x: 1 }", 2),
        ("let f : { x when a: Nat } where c != d = { x: 1 }", 2),
        ("let f : { x when a: Nat } where not c = { x: 1 }", 1),
        ("let f : { x when a: Nat } where a or c = { x: 1 }", 1),
        ("let f : { x when a: Nat } where a and c = { x: 1 }", 1),
        ("let f : { x when a: Nat } where a = c = { x: 1 }", 1),
        ("let f : { x when a: Nat } where a != c = { x: 1 }", 1),
    ] {
        let (mint, out) = build_src(src);
        assert_eq!(out.errors.len(), count, "{src}: {:#?}", out.errors);
        let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
            .annotation
            .as_ref()
            .expect("the annotation");
        assert!(annotation.clause.is_none(), "{src}");
    }
}

/// A declaration says the same thing wherever it is used, so it has no presence
/// of its own for a clause to relate — the refusal `..` already gets, about the
/// thing beside the type rather than a label inside it.
#[test]
fn a_declaration_may_not_carry_a_clause() {
    let src = "type T = { x: Nat } where a";
    let (_, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(error.kind, ErrorKind::ClauseInDeclaration));
    assert_eq!(error.span.start, src.rfind('a').expect("the clause"));

    // A `when` inside one is the same refusal about a label, and it is the one
    // `..` already gets: the declaration's body lowers to the error type.
    let (_, out) = build_src("type T = `A (when a) Nat");
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["open-declared-type"]
    );
}

/// Every complaint one source made, as the codes a reporter keys on, in the
/// order the reader would meet them.
fn codes_of(src: &str) -> Vec<&'static str> {
    let (_, out) = build_src(src);
    out.errors.iter().map(|error| error.kind.code()).collect()
}

/// One effect declaration's lowered value, by the name it was declared under.
fn effect_of<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a Effect {
    let symbol = *out
        .program
        .effects
        .keys()
        .find(|symbol| mint.name(**symbol) == name)
        .unwrap_or_else(|| panic!("no effect named {name}"));
    &out.program.effects[&symbol].value
}

/// An operation declaration keeps its operations in the order they were
/// written, each as the two sides of the plain closed arrow performing it has —
/// which is what a handler arm's binder and body type come off. The empty
/// effect declares nothing and is still an effect a row may name.
#[test]
fn an_effect_declares_its_operations() {
    let (mint, out) = built("effect Log = `write : Nat -> () | `flush : () -> ()");
    let Effect::Operations(operations) = effect_of(&mint, &out, "Log") else {
        panic!("expected an operation declaration");
    };
    assert_eq!(
        operations.keys().collect::<Vec<_>>(),
        ["write", "flush"].iter().collect::<Vec<_>>()
    );
    let write = &operations["write"];
    assert!(matches!(write.from.tracked, TypeKind::Prim(Prim::Nat)));
    assert!(matches!(
        write.to.tracked,
        TypeKind::Struct { ref fields, tail: None } if fields.is_empty()
    ));

    let (mint, out) = built("effect Nil = |");
    let Effect::Operations(operations) = effect_of(&mint, &out, "Nil") else {
        panic!("the empty effect declares operations, of which it has none");
    };
    assert!(operations.is_empty());
}

/// An alias names effects and declares nothing, and expands to the effects it
/// names wherever a row mentions it — so no alias survives into what a
/// definition is checked against, and none can be performed through.
#[test]
fn an_alias_expands_to_the_effects_it_names() {
    let src = "effect Log = `write : Nat -> ()\n\
               effect IO = `print : Nat -> ()\n\
               effect Console = `Log | `IO\n\
               effect All = `Console\n\
               let f : Nat -> Nat ! `All = fn x => x";
    let (mint, out) = built(src);
    let Effect::Alias(named) = effect_of(&mint, &out, "Console") else {
        panic!("expected an alias");
    };
    assert_eq!(
        named.keys().collect::<Vec<_>>(),
        ["Log", "IO"].iter().collect::<Vec<_>>()
    );

    // And an alias of an alias reaches through: what a row writes is the
    // effects, however many names stand between.
    let effects = annotation_effects(&mint, &out, "f");
    assert_eq!(effects, ["Log", "IO"]);
}

/// The effects a definition's annotation says it may perform, in the order the
/// lowered row names them.
fn annotation_effects(mint: &Mint, out: &Output, name: &str) -> Vec<String> {
    let decl = &out.program.terms[&term_symbol(mint, out, name)];
    let TypeKind::Arrow { effects, .. } = &decl
        .annotation
        .as_ref()
        .expect("the definition is annotated")
        .ty
        .tracked
    else {
        panic!("expected an arrow");
    };
    effects.effects.keys().cloned().collect()
}

/// A bare `A -> B` carries the empty closed row, which is what pure means; the
/// `! |` that spells it out lowers to the same thing and is told apart only by
/// what the debugger prints back.
#[test]
fn a_bare_arrow_is_pure() {
    let (mint, out) = built("let f : Nat -> Nat = fn x => x");
    let decl = &out.program.terms[&term_symbol(&mint, &out, "f")];
    let TypeKind::Arrow { effects, .. } = &decl.annotation.as_ref().expect("annotated").ty.tracked
    else {
        panic!("expected an arrow");
    };
    assert!(effects.effects.is_empty() && effects.tail.is_none());
    assert!(!effects.written);

    let (mint, out) = built("let f : Nat -> Nat ! | = fn x => x");
    let decl = &out.program.terms[&term_symbol(&mint, &out, "f")];
    let TypeKind::Arrow { effects, .. } = &decl.annotation.as_ref().expect("annotated").ty.tracked
    else {
        panic!("expected an arrow");
    };
    assert!(effects.effects.is_empty() && effects.tail.is_none());
    assert!(effects.written, "the reader wrote the row out");
}

/// Every way an effect declaration can be refused, each once and at the thing
/// the reader can change.
#[test]
fn an_effect_declaration_is_held_to_its_form() {
    assert_eq!(
        codes_of("effect Log = `write : Nat -> () | `write : () -> ()"),
        ["duplicate-operation"]
    );
    assert_eq!(codes_of("effect Log = `here : Nat"), ["not-an-operation"]);
    assert_eq!(
        codes_of("effect IO = `print : Nat -> ()\neffect Log = `w : Nat -> () ! `IO"),
        ["impure-operation"]
    );
    // The three an operation's signature may not leave open are one complaint,
    // because one sentence naming all three is what a reader can act on.
    for source in [
        "effect Log = `w : { x: Nat, .. } -> ()",
        "effect Log = `w : { x when a: Nat } -> ()",
        "effect Log = `w : Nat -> (`A | ..r)",
    ] {
        assert_eq!(codes_of(source), ["impure-operation"], "{source}");
    }
    assert_eq!(
        codes_of("effect Log = `write : Nat -> ()\neffect Bad = `Log | `w : Nat -> ()"),
        ["mixed-effect-form"]
    );
    // A repeated effect name is a duplicate like any other, in its own
    // namespace.
    let out = build_src("effect Log = |\neffect Log = |").1;
    assert_eq!(out.errors[0].kind.code(), "duplicate-effect");
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Duplicate {
            namespace: Namespace::Effects,
            ..
        }
    ));
}

/// An effect and a type of one name are two unrelated things, which is what a
/// namespace of its own buys.
#[test]
fn effects_have_a_namespace_of_their_own() {
    let (mint, out) = built(
        "type Log = Nat\neffect Log = `write : Nat -> ()\nlet f : Log -> Log ! `Log = fn x => x",
    );
    assert_eq!(annotation_effects(&mint, &out, "f"), ["Log"]);
    assert_eq!(
        mint.namespace(*out.program.effects.keys().next().expect("one effect")),
        Namespace::Effects
    );
}

/// An operation reference resolves to the effect that declares it, and every
/// way of failing to has a complaint of its own: no such effect, an alias —
/// which declares nothing — and an effect with no such operation.
#[test]
fn an_operation_reference_resolves_through_its_effect() {
    let base = "effect Log = `write : Nat -> ()\n\
                effect IO = `print : Nat -> ()\n\
                effect Console = `Log | `IO\n";
    let (mint, out) = built(&format!("{base}let g = fn _ => Log.`write 1"));
    let mut node = term_value(&mint, &out, "g");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Apply { func, .. } = node else {
        panic!("expected an application");
    };
    let TermKind::Operation { effect, op } = &func.kind else {
        panic!("expected an operation, got {:?}", func.kind);
    };
    assert_eq!(mint.name(effect.tracked), "Log");
    assert_eq!(op.tracked, "write");

    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => Lg.`write 1")),
        ["undefined-effect"]
    );
    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => Log.`writ 1")),
        ["unknown-operation"]
    );
    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => Console.`write 1")),
        ["operation-on-alias"]
    );
}

/// Which effects a handler discharges is settled here, before inference: every
/// effect an arm names must have an arm for each of its operations, and a
/// half-covered one leaves the set undecidable.
#[test]
fn a_handler_must_cover_every_effect_it_names() {
    let base = "effect Log = `write : Nat -> () | `flush : () -> ()\n\
                let p : () -> Nat ! `Log = fn _ => 0\n";
    let (mint, out) = built(&format!(
        "{base}let h = fn _ => handle p () with | Log.`write s => () | Log.`flush u => () end"
    ));
    let mut node = term_value(&mint, &out, "h");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected a handler");
    };
    assert_eq!(handler.arms.len(), 2);
    assert!(handler.ret.is_none());
    let discharged: Vec<&str> = handler
        .discharges
        .iter()
        .map(|effect| mint.name(effect.tracked))
        .collect();
    assert_eq!(discharged, ["Log"]);

    // One arm short, and the complaint names the operation with none.
    let (_, out) = build_src(&format!(
        "{base}let h = fn _ => handle p () with | Log.`write s => () end"
    ));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "partial-handler");
    assert_eq!(
        error.kind.to_string(),
        "handling `Log` needs an arm for ``flush` too"
    );
    assert!(matches!(
        error.kind,
        ErrorKind::PartialHandler { ref missing, .. } if missing == &["flush".to_string()]
    ));
}

/// A duplicate arm and a second `return` arm are refused where a duplicate
/// anything else is: at the repeat, with the first the one that stands.
#[test]
fn a_handler_takes_each_arm_once() {
    let base = "effect Log = `write : Nat -> ()\nlet p : () -> Nat ! `Log = fn _ => 0\n";
    let (_, out) = build_src(&format!(
        "{base}let h = fn _ => handle p () with | Log.`write s => () | Log.`write t => () end"
    ));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "duplicate-arm");
    assert_eq!(error.kind.to_string(), "duplicate arm for `Log.`write`");

    let (mint, out) = build_src(&format!(
        "{base}let h = fn _ => handle p () with | return a => a | return b => b end"
    ));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "duplicate-return-arm");
    // The first is the one that stands, the way a repeated definition is.
    let mut node = term_value(&mint, &out, "h");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected a handler");
    };
    let ret = handler.ret.as_ref().expect("a return arm");
    assert_eq!(mint.name(ret.binder.tracked), "a");
}

/// R17: a `raise` answers the innermost arm that lexically encloses it, and a
/// `fn` between the two is a closure that could outlive the handler.
#[test]
fn raise_belongs_to_the_arm_around_it() {
    let base = "effect Log = `write : Nat -> ()\nlet p : () -> Nat ! `Log = fn _ => 0\n";
    let arm =
        |body: &str| format!("{base}let h = fn _ => handle p () with | Log.`write s => {body} end");
    // No arm anywhere.
    assert_eq!(
        codes_of(&format!("{base}let a = raise 1")),
        ["raise-outside-arm"]
    );
    assert_eq!(
        codes_of(&format!("{base}let a = fn _ => raise 1")),
        ["raise-outside-arm"]
    );
    // The handled expression of a handler with no arm above it is the ordinary
    // "no enclosing arm" case.
    assert_eq!(
        codes_of(&format!("{base}let a = fn _ => handle raise 1 with end")),
        ["raise-outside-arm"]
    );

    // A `fn` between the `raise` and its arm — including one applied on the
    // spot, since what matters is that a closure was written at all.
    assert_eq!(
        codes_of(&arm("{ g: fn _ => raise 0 }")),
        ["raise-in-function"]
    );
    assert_eq!(codes_of(&arm("(fn _ => raise 0) 1")), ["raise-in-function"]);

    // And every position an expression may sit in, none of which needs a rule.
    for body in [
        "raise 0",
        "let a = raise 0 in ()",
        "f (raise 0)",
        "{ g: raise 0 }",
        "match s with | 0 => raise 0 | _ => () end",
        // A nested `handle`'s *body* keeps the outer arm: the outer handler is
        // still on the stack while the inner one runs.
        "handle raise 0 with end",
    ] {
        let src = format!(
            "{base}let f = fn x => x\nlet h = fn _ => handle p () with | Log.`write s => {body} end"
        );
        let (_, out) = build_src(&src);
        let raised: Vec<&str> = out
            .errors
            .iter()
            .map(|error| error.kind.code())
            .filter(|code| code.starts_with("raise-"))
            .collect();
        assert!(raised.is_empty(), "{body}: {:#?}", out.errors);
    }

    // A `return` arm is an arm, so a `raise` in one is at home too.
    assert_eq!(
        codes_of(&format!(
            "{base}let h = fn _ => handle p () with | Log.`write s => () | return x => raise x end"
        )),
        Vec::<&str>::new()
    );
}

/// A `type` declaration may name effects in an arrow it writes, and may take a
/// parameter standing for an arrow's effects — a third reading beside a type
/// and a sum's rest.
#[test]
fn a_declaration_may_write_and_take_effects() {
    let (mint, out) = built(
        "effect Log = `write : Nat -> ()\n\
         type Logger = Nat -> Nat ! `Log\n\
         type Runner e = (Nat -> Nat ! ..e) -> Nat ! ..e",
    );
    let TypeKind::Arrow { effects, .. } = &out.program.types[&type_symbol(&mint, &out, "Logger")]
        .value
        .tracked
    else {
        panic!("expected an arrow");
    };
    assert_eq!(effects.effects.keys().collect::<Vec<_>>(), ["Log"]);

    let runner = &out.program.types[&type_symbol(&mint, &out, "Runner")];
    assert_eq!(runner.params.len(), 1);
    assert_eq!(
        runner.params[0].kind,
        ParamKind::Effects { lacks: [].into() }
    );
}

/// The three a declaration may not leave open are refused in the effect
/// position exactly as they are in a struct's or a sum's — unless the tail
/// names one of the declaration's own parameters, which is supplied at every
/// use rather than decided here.
#[test]
fn a_declared_effect_row_must_be_closed() {
    let base = "effect Log = `write : Nat -> ()\n";
    for source in [
        "type T = Nat -> Nat ! ..",
        "type T = Nat -> Nat ! ..e",
        "type T = Nat -> Nat ! `Log (when a)",
    ] {
        let src = format!("{base}{source}");
        let out = build_src(&src).1;
        assert_eq!(
            out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
            ["open-declared-type"],
            "{source}"
        );
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::OpenDeclaredType {
                    shape: Shape::Effect
                }
            ),
            "{source}: {:#?}",
            out.errors[0].kind
        );
    }
    assert_eq!(
        out_message(&format!("{base}type T = Nat -> Nat ! ..e")),
        "a declared type must list its effects exactly; `..` and `when` belong in annotations"
    );
}

/// The first complaint one source made, worded.
fn out_message(src: &str) -> String {
    let (_, out) = build_src(src);
    out.errors
        .first()
        .unwrap_or_else(|| panic!("{src}: no complaint"))
        .kind
        .to_string()
}

/// One name is one rest, and an arrow's effects are a third thing a rest can
/// be — so a parameter used as an effect tail in one place and as a type in
/// another is the mixed parameter it always was, with the third reading named.
#[test]
fn a_parameter_read_two_ways_names_both() {
    let src = "effect Log = `write : Nat -> ()\n\
               type M e = { f: e, g: (Nat -> Nat ! ..e) }";
    let (_, out) = build_src(src);
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "mixed-parameter");
    assert!(matches!(
        error.kind,
        ErrorKind::MixedParameter {
            first: Sense::Type,
            second: Sense::Effects
        }
    ));
    assert_eq!(
        error.kind.to_string(),
        "this stands for a whole type in one place and for the rest of an arrow's effects in another"
    );

    // And a name given two rests in one *annotation* is the same mistake.
    let src = "effect Log = `write : Nat -> ()\n\
               let f : { x: Nat, ..r } -> Nat -> Nat ! ..r = fn p => fn n => n";
    let (_, out) = build_src(src);
    let mixed: Vec<&str> = out
        .errors
        .iter()
        .map(|error| error.kind.code())
        .filter(|code| *code == "mixed-tail")
        .collect();
    assert_eq!(mixed, ["mixed-tail"], "{:#?}", out.errors);
}

/// An effect written absent needs a `..` to speak about, the way a struct's
/// `\y` and a sum's `` \`B `` do: a row with no tail already says every effect
/// it does not name is not performed.
#[test]
fn an_absent_effect_needs_a_tail() {
    let src = "effect Log = `write : Nat -> ()\nlet f : Nat -> Nat ! \\`Log = fn x => x";
    let (_, out) = build_src(src);
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "absent-in-closed");
    assert_eq!(
        error.kind.to_string(),
        "a type with no `..` already says ``Log` is not there"
    );
}

/// An effect named twice in one row is a duplicate, however it got there — the
/// two names of an alias that overlap another label included.
#[test]
fn an_effect_row_names_each_effect_once() {
    let src = "effect Log = `write : Nat -> ()\nlet f : Nat -> Nat ! `Log | `Log = fn x => x";
    assert_eq!(codes_of(src), ["duplicate-case"]);
}

/// An alias's cases are effect names and are resolved like any other: one that
/// names nothing is undefined, and one named twice is a duplicate. Both are
/// dropped where they stood, so the declaration still stands for whatever else
/// it named.
#[test]
fn an_aliass_cases_are_resolved_and_deduplicated() {
    assert_eq!(
        codes_of("effect Log = `write : Nat -> ()\neffect Console = `Log | `Nope"),
        ["undefined-effect"]
    );
    let (mint, out) = build_src("effect Log = `write : Nat -> ()\neffect Console = `Log | `Log");
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["duplicate-case"]
    );
    let Effect::Alias(named) = effect_of(&mint, &out, "Console") else {
        panic!("expected an alias");
    };
    assert_eq!(named.len(), 1);
}

/// An effect a row names has to be one: an unknown name is dropped from the
/// row where it stood, with the undefined-name complaint in the effect
/// namespace, and whatever else the row named still stands.
#[test]
fn a_row_may_only_name_declared_effects() {
    let (mint, out) =
        build_src("effect Log = `write : Nat -> ()\nlet f : Nat -> Nat ! `Log | `Lg = fn x => x");
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["undefined-effect"]
    );
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Effects
        }
    ));
    assert_eq!(annotation_effects(&mint, &out, "f"), ["Log"]);
}

/// An arm whose operation does not resolve is dropped, so the effect it named
/// is not counted as covered and nothing downstream is told a second time.
#[test]
fn an_arm_that_does_not_resolve_covers_nothing() {
    let (mint, out) = build_src(
        "effect Log = `write : Nat -> ()\n\
         let p : () -> Nat ! `Log = fn _ => 0\n\
         let h = fn _ => handle p () with | Log.`writ s => () end",
    );
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["unknown-operation"]
    );
    let mut node = term_value(&mint, &out, "h");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected a handler");
    };
    assert!(handler.arms.is_empty());
    assert!(handler.discharges.is_empty());
}

/// A `\` on an effect is refused in a declaration for the reason a bare `..`
/// is — and the row still runs its own absent-in-closed check, so a `\` with
/// no `..` to speak about is told about too.
#[test]
fn a_declared_effect_row_refuses_an_absence() {
    let codes = codes_of("effect Log = `write : Nat -> ()\ntype T = Nat -> Nat ! \\`Log");
    assert_eq!(codes, ["absent-in-closed"]);
}

/// A declaration that mixes the two forms is read as whichever form its first
/// case is, and every case of the other form is dropped — reported once, at
/// the first of them, so one mistake makes one complaint however many cases
/// went the wrong way.
#[test]
fn a_mixed_effect_declaration_keeps_the_form_it_opened_with() {
    // Opening with an operation, so the bare tags are what is dropped.
    let (mint, out) = build_src(
        "effect Log = `write : Nat -> ()\n\
         effect Bad = `w : Nat -> () | `Log | `Log",
    );
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["mixed-effect-form"]
    );
    let Effect::Operations(operations) = effect_of(&mint, &out, "Bad") else {
        panic!("the first case decided the form");
    };
    assert_eq!(operations.keys().collect::<Vec<_>>(), ["w"]);

    // And opening with a bare tag, so the signatures are — including a second
    // one, which says nothing new.
    let (mint, out) = build_src(
        "effect Log = `write : Nat -> ()\n\
         effect Bad = `Log | `w : Nat -> () | `v : Nat -> ()",
    );
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["mixed-effect-form"]
    );
    let Effect::Alias(named) = effect_of(&mint, &out, "Bad") else {
        panic!("the first case decided the form");
    };
    assert_eq!(named.keys().collect::<Vec<_>>(), ["Log"]);
}

/// A signature that failed to lower is already a complaint, so it is not told a
/// second time that it is not a function: the error type absorbs, here as
/// everywhere.
#[test]
fn a_signature_that_did_not_lower_is_not_told_twice() {
    assert_eq!(codes_of("effect Log = `write : Bogus"), ["undefined-type"]);
}

/// A row that names nothing may still say something: `! ..e` ends in a tail,
/// so the arrow may perform whatever the tail stands for, and only a row with
/// neither is the pure one.
#[test]
fn a_row_that_is_only_a_tail_says_something() {
    let (mint, out) = built("let f : Nat -> Nat ! ..e = fn x => x");
    let decl = &out.program.terms[&term_symbol(&mint, &out, "f")];
    let TypeKind::Arrow { effects, .. } = &decl.annotation.as_ref().expect("annotated").ty.tracked
    else {
        panic!("expected an arrow");
    };
    assert!(effects.effects.is_empty());
    assert!(effects.tail.is_some());
    assert!(effects.written);
}

/// An effect tail counts as a use of a parameter, so a recursion handing on a
/// type built out of one through a `!` grows exactly as one handing it on
/// through a field does.
#[test]
fn an_effect_tail_makes_an_argument_grow() {
    let codes = codes_of(
        "effect Log = `write : Nat -> ()\n\
         type T a = { next: T (Nat -> Nat ! ..a) }",
    );
    assert_eq!(codes, ["growing-recursion"]);
}

/// `return` is contextual: it heads a handler arm and is an ordinary name
/// everywhere else, so a definition may still be called one.
#[test]
fn return_is_a_name_everywhere_but_an_arms_head() {
    let (mint, out) = built(
        "effect Log = `write : Nat -> ()\n\
         let return = 1\n\
         let p : () -> Nat ! `Log = fn _ => return\n\
         let h = fn _ => handle p () with | Log.`write s => () | return x => x end",
    );
    assert!(matches!(
        term_value(&mint, &out, "return"),
        TermKind::Natural(1)
    ));
    let mut node = term_value(&mint, &out, "h");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected a handler");
    };
    assert!(handler.ret.is_some());
}

/// An arrow's effects are spliced into a row, so only a row can be written at a
/// parameter that stands for them — and only one naming none of the effects the
/// declaration already writes out. The two conditions a sum's rest is held to,
/// in the effect reading.
#[test]
fn an_argument_at_an_effect_parameter_has_to_be_a_row() {
    let base = "effect Log = `write : Nat -> ()\n\
                type Runner e = (Nat -> Nat ! `Log | ..e) -> Nat\n";
    // Something a row cannot hold.
    let (_, out) = build_src(&format!("{base}let f : Runner Nat -> Nat = fn r => 1"));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "not-a-row");
    assert!(matches!(
        error.kind,
        ErrorKind::NotARow {
            sense: Sense::Effects
        }
    ));
    assert_eq!(
        error.kind.to_string(),
        "the rest of an arrow's effects goes here, and this is not that"
    );

    // And a row naming what the declaration already names: the `..` covers
    // only what its own row leaves out.
    let (_, out) = build_src(&format!("{base}let f : Runner (`Log) -> Nat = fn r => 1"));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "repeated-row-field");
    assert_eq!(
        error.kind.to_string(),
        "this names ``Log`, which the function it goes into already has"
    );
}
