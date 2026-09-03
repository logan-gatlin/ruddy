//! Tests for [`ruddy::ir`].

use indexmap::IndexMap;
use ruddy::{
    artifact as a, inference,
    ir::{
        Annotation, ClauseKind, DependencyImport, Effect, EffectLabel, ErrorKind, ExternTypeKind,
        Field, OperationSelector, OperationTypeProblem, Output, PatternKind, PresenceOwnership,
        Row, SumCase, Tail, Term, TermKind, TypeField, TypeKind, build, build_with_dependencies,
        build_with_dependency_imports,
    },
    parse,
    symbol::{Bundle, Mint, Namespace, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::{ParamKind, Presence, Prim, Rest, Sense, Shape, Ty},
};
use ruddy_debug::print;

/// A parameter standing for a whole type that may not name any of `labels` —
/// which is what a struct's `..'r` is, since the rest of a struct is its core.
fn row(labels: &[&str]) -> ParamKind {
    ParamKind::Fields {
        lacks: labels.iter().map(|label| label.to_string()).collect(),
    }
}

fn whole() -> ParamKind {
    ParamKind::Type {
        lacks: Default::default(),
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

/// Surface conditionals deliberately disappear at the IR boundary. The
/// existing match node receives true and false arms in source order, and an
/// else-if is another match in the outer false body.
#[test]
fn if_expressions_lower_to_boolean_matches() {
    let (mint, out) = built("let choose = if true then 1n else 2n end");
    let TermKind::Match { scrutinee, arms } = term_value(&mint, &out, "choose") else {
        panic!("an if lowers to a match");
    };
    assert!(matches!(scrutinee.kind, TermKind::Boolean(true)));
    assert_eq!(arms.len(), 2);
    assert!(matches!(arms[0].0.tracked, PatternKind::Boolean(true)));
    assert_eq!(arms[0].0.span, scrutinee.span);
    assert!(matches!(arms[0].1.kind, TermKind::Natural(1)));
    assert!(matches!(arms[1].0.tracked, PatternKind::Boolean(false)));
    assert_eq!(arms[1].0.span, scrutinee.span);
    assert!(matches!(arms[1].1.kind, TermKind::Natural(2)));

    let (mint, out) = built("let choose = if true then 1n else if false then 2n else 3n end");
    let TermKind::Match { arms, .. } = term_value(&mint, &out, "choose") else {
        panic!("the outer if lowers to a match");
    };
    let TermKind::Match {
        scrutinee,
        arms: nested,
    } = &arms[1].1.kind
    else {
        panic!("the else-if lowers in the false branch");
    };
    assert!(matches!(scrutinee.kind, TermKind::Boolean(false)));
    assert_eq!(nested.len(), 2);
    assert!(matches!(nested[0].0.tracked, PatternKind::Boolean(true)));
    assert!(matches!(nested[1].0.tracked, PatternKind::Boolean(false)));
}

/// The lowered annotation of a top-level definition, which is where the
/// variables a `where 'let` declared and the sorts lowering read them at live.
fn annotation_of<'a>(mint: &Mint, out: &'a Output, name: &str) -> &'a Annotation {
    out.program.terms[&term_symbol(mint, out, name)]
        .annotation
        .as_ref()
        .expect("the definition is annotated")
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
    assert_eq!(display_program("let u = ()"), "let u = ()");
    assert_eq!(
        display_program("type T = { items: Nat, next: () }"),
        "type T = { items: Nat, next: () }"
    );
}

#[test]
fn displays_naturals() {
    assert_eq!(display_program("let n = 0n"), "let n = 0n");
    assert_eq!(
        display_program("let p = fn f => f 1n (f 2n 3n)"),
        "let p = fn f => f 1n (f 2n 3n)"
    );
    assert_eq!(
        display_program("let s = { width: 3n, height: 4n }"),
        "let s = { width: 3n, height: 4n }"
    );
}

#[test]
fn a_natural_lowers_to_its_value_and_span() {
    let (mint, out) = built("let n = 4096n");

    let TermKind::Natural(value) = term_value(&mint, &out, "n") else {
        panic!("expected a natural");
    };
    assert_eq!(*value, 4096);

    let span = out.program.terms[&term_symbol(&mint, &out, "n")].value.span;
    assert_eq!(span.start, 8);
    assert_eq!(span.width, 5);
}

#[test]
fn a_natural_names_nothing() {
    // A literal is not a name, so lowering one mints no symbol and looks
    // none up: only the `let` itself is minted...
    let (mint, _) = built("let n = 7n");
    assert_eq!(mint.symbols().count(), 1);

    // ...and an undefined name beside a literal is still the one error.
    let (_, out) = build_src("let m = f 7n");
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
        "type G = Nat -> Nat -> ()"
    );
    // The one grouping the printer has to reconstruct: an arrow on the left
    // of an arrow, which would otherwise re-parse as the right half of one.
    assert_eq!(
        display_program("type H = (Nat -> Nat) -> ()"),
        "type H = (Nat -> Nat) -> ()"
    );
    // A struct is atomic on either side, and its fields are types in their
    // own right.
    assert_eq!(
        display_program("type K = { f: Nat -> Nat, g: () -> Nat }"),
        "type K = { f: Nat -> Nat, g: () -> Nat }"
    );
    // A declaration reads the same as a primitive wherever a type may stand.
    assert_eq!(
        display_program("type A = ()  type B = A -> A"),
        "type A = ()\ntype B = A -> A"
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

/// Lowering keeps the decoded projection key and the complete quoted label
/// span. The label is structural data, never a term name to resolve.
#[test]
fn a_quoted_projection_lowers_to_its_decoded_label() {
    let src = r###"let a = fn p => p."field name""###;
    let (mint, out) = built(src);

    let TermKind::Fn { body, .. } = term_value(&mint, &out, "a") else {
        panic!("expected a function");
    };
    let TermKind::Project { base, field } = &body.kind else {
        panic!("expected a projection, got {:?}", body.kind);
    };
    assert_eq!(field.tracked, "field name");
    assert_eq!(field.span.start, src.find('"').expect("the quote"));
    assert_eq!(field.span.width, r###""field name""###.len());
    assert!(matches!(base.kind, TermKind::Ident(s) if mint.name(s) == "p"));

    // Only the base is a name lookup.
    let (_, out) = build_src(r###"let b = q."field name""###);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "q"
    ));
}

#[test]
fn displays_ascriptions() {
    assert_eq!(display_program("let x : () = ()"), "let x : () = ()");
    assert_eq!(
        display_program("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"),
        "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"
    );
    // The annotation resolves in the type namespace, and a declaration is
    // in scope for it exactly as it would be in a `type` body.
    assert_eq!(
        display_program("type T = ()  let u : T = ()"),
        "type T = ()\nlet u : T = ()"
    );
    // Printing hoists types the way lowering does, so an interleaved program
    // comes back in the order the builder saw it — and re-lowering the
    // rendering, which `display_program` does, lands on the same program.
    assert_eq!(
        display_program("let u : T = ()  type T = ()"),
        "type T = ()\nlet u : T = ()"
    );
    assert_eq!(
        display_program("type A = ()  let x : B = ()  type B = A  let y : A = ()"),
        "type A = ()\ntype B = A\nlet x : B = ()\nlet y : A = ()"
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
            ref name,
            namespace: Namespace::Types,
        } if name == "T"
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

    // ...'but a declaration of one's own is what the name then means.
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
        "type T = () -> ()\nlet u : () = ()"
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
        "type T = { f: () }\nlet x = ()\nlet y = ()"
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

/// Which leaves `Undefined` meaning "defined nowhere 'in this file" rather than
/// "defined below here".
#[test]
fn a_name_defined_nowhere_is_still_undefined() {
    let (_, out) = build_src("let a = nope");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 8);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "nope"
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
            ref name,
            namespace: Namespace::Terms,
            previous,
        } if name == "x" && previous.start == 4
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
            namespace: Namespace::Terms,
            ..
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
            namespace: Namespace::Terms,
            ..
        }
    ));

    let (_, out) = build_src("let x = ()  type T = x");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            namespace: Namespace::Types,
            ..
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

/// Quoted struct labels become the same decoded map keys as bare fields. Their
/// source spans still include the quotes, and IR printing chooses the minimal
/// unambiguous spelling.
#[test]
fn quoted_fields_lower_to_decoded_keys_and_render_canonically() {
    let src = r###"let p = fn x y z => {"field name": x, "plain": y, "line\nname": z}"###;
    let (mint, out) = built(src);
    let fields = term_fields(&mint, &out, "p");
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["field name", "plain", "line\nname"]
    );
    assert_eq!(
        fields["field name"].name_span.start,
        src.find(r###""field name""###).expect("the field")
    );
    assert_eq!(
        fields["field name"].name_span.width,
        r###""field name""###.len()
    );
    assert_eq!(
        display_program(src),
        "let p = fn x => fn y => fn z => { \"field name\": x, plain: y, \"line\\nname\": z }"
    );

    let (mint, out) = built(r###"type T 'r = { "field name": Nat, \"gone field", ..'r }"###);
    let fields = type_fields(&mint, &out, "T");
    assert!(matches!(fields["field name"], TypeField::Written { .. }));
    assert!(matches!(fields["gone field"], TypeField::Absent { .. }));
}

#[test]
fn duplicate_term_fields_are_rejected() {
    let src = "let p = fn a b => { x: a, x: b }";
    let (mint, out) = build_src(src);

    // Reported at the offending repeat, not at the first occurrence.
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, 26);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateField { name, previous }
            if name == "x" && previous.start == src.find("x: a").expect("the first")
    ));

    // The first occurrence is the one that survives.
    let fields = term_fields(&mint, &out, "p");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields["x"].name_span.start, 20);
    assert!(matches!(fields["x"].value.kind, TermKind::Ident(s) if mint.name(s) == "a"));
}

#[test]
fn bare_numeric_and_quoted_canonical_labels_are_duplicates() {
    for src in [
        "let p = fn a b => { 001: a, \"1\": b }",
        "type T = { 001: Nat, \"1\": Nat }",
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "errors for {src:?}: {:#?}", out.errors);
        assert!(matches!(
            &out.errors[0].kind,
            ErrorKind::DuplicateField { name, previous }
                if name == "1" && previous.start == src.find("001").expect("the first")
        ));
        assert_eq!(
            out.errors[0].span.start,
            src.rfind("\"1\"").expect("the duplicate")
        );
    }
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
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateField { name, previous }
            if name == "a" && previous.start == src.find("a: A").expect("the first")
    ));
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
    let src = "let f : { a when 'a: Nat, a: Nat } -> Nat = fn p => p.a";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateField { name, previous }
            if name == "a" && previous.start == src.find("a when").expect("the first")
    ));
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
        "type T = { x when 'a: Nat }",
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
    let (_, out) = build_src("type T = { x when 'a: Nat, y when 'b: Nat, .. }");
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

    // An annotation is where 'openness belongs, and it passes through whole.
    let (_, out) = build_src("let f : { x when 'a: Nat, ..'r } -> Nat = fn p => p.x");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

#[test]
fn duplicate_fields_are_rejected_when_nested() {
    let src = "let p = fn a b => { outer: { y: a, y: b } }";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateField { name, previous }
            if name == "y" && previous.start == src.find("y: a").expect("the first")
    ));
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
    let (_, out) = built("type Pair 'A 'B = { a: 'A, b: 'B }");
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
/// somewhere 'else is undefined rather than a reference to it.
#[test]
fn a_parameter_is_out_of_scope_outside_its_declaration() {
    let (_, out) = build_src("type Pair 'A 'B = { a: 'A, b: 'B }  type Other = A");
    assert_eq!(
        out.errors
            .iter()
            .filter(|error| matches!(
                error.kind,
                ErrorKind::Undefined {
                    ref name,
                    namespace: Namespace::Types,
                } if name == "A"
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
    let (_, out) = built("type Nat = { n: Nat }  type Box 'Nat = { it: 'Nat }");
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
        (
            "type Pair 'A 'B = { a: 'A, b: 'B }  type M = Pair Nat",
            2,
            1,
        ),
        (
            "type Pair 'A 'B = { a: 'A, b: 'B }  type M = Pair Nat Nat Nat",
            2,
            3,
        ),
        ("type Pair 'A 'B = { a: 'A, b: 'B }  type M = Pair", 2, 0),
        ("type T = Nat  type M = T Nat", 0, 1),
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::Arity { expected: e, found: f, .. } if e == expected && f == found
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
    let (_, out) = build_src("let f : { x: Bogus } Nat = 1n");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "undefined-type"]);

    let (_, out) = build_src("let f : { x: Bogus } = 1n");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "undefined-type");

    // A `when` field or a bare `..` inside a head is refused where it stands, for
    // the same reason.
    let (_, out) = build_src("type A = { x: Nat, .. } Nat");
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(codes, ["not-a-type-constructor", "open-declared-type"]);

    let (_, out) = build_src("type A = { x when 'a: Nat } Nat");
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
        "type W 'r = { x: Nat, ..'r }\nlet a : W Nat -> Nat = fn v => 1n\nlet b = nosuchname",
        "type W 'r = { x: Nat, ..'r }\nlet a = nosuchname\nlet b : W Nat -> Nat = fn v => 1n",
        "let f : { x: Bogus } Nat = 1n",
        "type Bad 'a = { x: 'a, ..'a }\nlet c = alsomissing",
        "type L 'a = { next: L { x: 'a } }\nlet d = missing",
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
                found: 1,
                ..
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
    let src = "type Pair 'a 'b = { f: 'a, s: 'b }  let p: Pair Nat Nat Nat = 1n";
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
        "type A 'a = { x: B 'a }  type B 'b = { x: A 'b }  \
         type C 'c = { x: D { y: 'c } }  type D 'd = { x: C 'd }  \
         type E 'e = { x: A 'e }",
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
    let (_, out) = build_src("type Flip 'f 'a = 'f 'a");
    assert!(
        out.errors.iter().any(|error| matches!(
            error.kind,
            ErrorKind::ParameterApplied { ref name } if name == "f"
        )),
        "errors: {:#?}",
        out.errors
    );
}

#[test]
fn a_declaration_binds_each_parameter_once() {
    let (_, out) = build_src("type Pair 'A 'A = { a: 'A }");
    assert!(
        out.errors.iter().any(|error| matches!(
            error.kind,
            ErrorKind::DuplicateParameter { ref name, .. } if name == "A"
        )),
        "errors: {:#?}",
        out.errors
    );
}

/// A repeated parameter binds once, so the declaration takes one thing. The
/// count the arity check uses is the list the scheme binds; counting the list
/// that was written asked for an argument no body could name.
#[test]
fn a_repeated_parameter_is_not_counted_twice() {
    let (mint, out) = build_src("type P 'A 'A = { x: 'A }  let v : P Nat = { x: 1n }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::DuplicateParameter { ref name, .. } if name == "A"
    ));

    let decl = &out.program.types[&type_symbol(&mint, &out, "P")];
    assert_eq!(decl.params.len(), 1);
}

/// Applying a name is still only a name, so a pair of declarations that hand
/// each other their parameters says nothing, exactly as a pair of bare names
/// does. A body that is a parameter is not the same thing: it says its argument.
#[test]
fn a_type_defined_only_as_another_name_is_still_circular() {
    let (_, out) = build_src("type A 'a = B 'a  type B 'b = A 'b");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Circular { .. })),
        "errors: {:#?}",
        out.errors
    );

    // The identity constructor: useless, but it says what its argument is.
    let (_, out) = build_src("type Id 'a = 'a");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// A loop can close through a declaration that hands back what it is given:
/// `A` stands for its argument, so `B` is declared as itself with `A` in
/// between and says exactly as much as `type B = B` does.
#[test]
fn a_loop_can_close_through_a_parameter() {
    for src in [
        "type A 'a = 'a  type B = A B",
        "type A 'a = 'a  type B = A C  type C = A B",
        // The loop that goes out through an argument and comes back in.
        "type S 'a = U (S 'a)  type U 'b = 'b",
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
        "type Id 'a = 'a  type Y = Id (Id Nat)",
        // The case a per-slot approximation reports wrongly: `Id` is handed an
        // `X` somewhere 'else, which says nothing about `X`.
        "type Id 'a = 'a  type X = Id Nat  type Y = { f: Id X }",
        "type G 'g = 'g  type F 'a = G (G 'a)  type H = F Nat",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    // Only the declarations on the loop are told; one that merely names one
    // has nothing to fix.
    let (mint, out) = build_src("type A 'a = 'a  type B = A B  type P = B");
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
        "type List 'a = { head: 'a, tail: List 'a }",
        // Mutual recursion, each member handing on its own parameter.
        "type Tree 'a = { node: 'a, kids: Forest 'a }  \
         type Forest 'a = { head: Tree 'a, tail: Forest 'a }",
        // A different group's argument is unrestricted: only the `Rose a`
        // inside `List` is the group's business, and it hands on `a`.
        "type List 'a = { head: 'a, tail: List 'a }  \
         type Rose 'a = { node: 'a, kids: List (Rose 'a) }",
        // A longer circle, every member in one group.
        "type A 'a = { x: B 'a }  type B 'b = { x: C 'b }  type C 'c = { x: A 'c }",
        // An argument that mentions no parameter is written out in the program
        // and is the same type every round, so it cannot grow.
        "type T 'a = { next: T Nat }",
        "type Tree 'a = { value: 'a, kids: Forest }  \
         type Forest = { head: Tree Nat, tail: Forest }",
        // Both at once, in one group.
        "type T 'a = { next: T 'a, other: T Nat }",
        // Order and repetition are free: a permutation only ever shuffles what
        // came in.
        "type A 'a 'b = { x: B 'b 'a }  type B 'c 'd = { x: A 'd 'c }",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    for src in [
        "type T 'a = { next: T { x: 'a } }",
        "type A 'a = { x: B { y: 'a } }  type B 'b = { x: A 'b }",
        // A parameter in a `..` tail is as much a way of growing as one in a
        // field, and is the one place a parameter is not written as a name.
        "type A 'r = { x: A { ..'r } }",
        // Every other way a type can be built out of a parameter: handed to
        // another declaration, put on either side of an arrow, or made a
        // case's payload. Each of them is a type that gets bigger every round,
        // and the walk that looks for a parameter has to reach all of them.
        "type Box 'b = { it: 'b }  type T 'a = { next: T (Box 'a) }",
        "type T 'a = { next: T ('a -> Nat) }",
        "type T 'a = { next: T (Nat -> 'a) }",
        "type T 'a = { next: T (#A 'a) }",
        "type T 'r = { next: T (#A Nat | ..'r) }",
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
/// row, a name anywhere 'else stands for a type, and one nothing says anything
/// about is a type because that is what a reader will expect.
///
/// A row carries what it may not stand for as well as that it is one: the
/// labels written out beside it are already named, and a `..` covers what is
/// not named.
#[test]
fn a_parameter_stands_for_what_the_body_uses_it_as() {
    for (src, expected) in [
        ("type WithX 'r = { x: Nat, ..'r }", vec![row(&["x"])]),
        ("type Box 'A = { it: 'A }", vec![whole()]),
        ("type Ghost 'a = Nat", vec![whole()]),
        ("type Bare 'r = { ..'r }", vec![row(&[])]),
        (
            "type Two 'r = { x: Nat, y: Nat, ..'r }",
            vec![row(&["x", "y"])],
        ),
        // Two rows in one body, each naming its own fields: the parameter may
        // not stand for anything either of them writes out.
        (
            "type Twice 'r = { a: { x: Nat, ..'r }, b: { y: Nat, ..'r } }",
            vec![row(&["x", "y"])],
        ),
        (
            "type Both 'A 'r = { it: 'A, ..'r }",
            vec![whole(), row(&["it"])],
        ),
        ("type Fn 'A 'B = 'A -> 'B", vec![whole(), whole()]),
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
    let (_, out) = built("type Outer 's = Inner 's  type Inner 'r = { x: Nat, ..'r }");
    for decl in out.program.types.values() {
        assert_eq!(decl.params[0].kind, row(&["x"]));
    }

    // And around a circle, where 'no declaration decides its own.
    let (_, out) = built(
        "type A 'x = { hop: B 'x, ..'x }\n\
         type B 'y = { hop: A 'y }",
    );
    for decl in out.program.types.values() {
        assert_eq!(decl.params[0].kind, row(&["hop"]));
    }

    // A row written out as an argument is not the parameter handed on, but its
    // own tail still lands where the callee's tail sat — so it collects the
    // callee's labels as well as the ones written beside it.
    let (mint, out) =
        built("type Inner 'r = { x: Nat, ..'r }  type Wrap 's = Inner { y: Nat, ..'s }");
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
        "type Bad 'r = { g: (#A | ..'r), f: 'r }",
        // Through an argument: `Inner` makes `r` a sum's rest, the field makes
        // it a type.
        "type Inner 'r = #A Nat | ..'r  type Bad 'a = { it: 'a, more: Inner 'a }",
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

    // A field payload is a whole type while a struct tail is a field row.
    let (_, out) = build_src("type W 'r = { f: 'r, ..'r }");
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::MixedParameter {
                first: Sense::Type,
                second: Sense::Fields,
            },
            ..
        }]
    ));
}

/// What a parameter stands for is read off the declaration that binds it, in
/// whichever order the declarations were written. A use site handing it the
/// wrong thing is the use site's mistake, not evidence about the declaration.
/// A parameter used two ways is reported against the parameter that was used
/// two ways. The other declaration is right and is left alone — which the
/// order the two were written in must not decide.
#[test]
fn a_mixed_parameter_names_the_declaration_that_mixed_it() {
    for src in [
        "type V 'a = { f: 'a, g: W 'a }  type W 'r = #X Nat | ..'r",
        "type W 'r = #X Nat | ..'r  type V 'a = { f: 'a, g: W 'a }",
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
    let src = "type Bad 'a = { x: (#A | ..'a), y: 'a }\n\
               let f: Bad Nat = { x: #A, y: 1n }\n\
               let g: Bad Nat -> Nat = fn v => v.y\n\
               let h: Bad Nat -> Nat = fn v => 1n";
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
    let src = "type W 'r = #X Nat | ..'r\n\
               type U 't = { a: W 't, b: 't }\n\
               type V 'u = U 'u\n\
               type Q 'q = V 'q";
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
/// reached the reader then was ``expected `Nat`, found `∅` `` — a complaint about
/// the empty row, naming a symbol nobody had written.
#[test]
fn only_a_row_may_be_written_where_a_row_goes() {
    for src in [
        "type Or 'r = #A | ..'r  let f : Or Nat -> Nat = fn p => 1n",
        "type Or 'r = #A | ..'r  let f : Or (Nat -> Nat) -> Nat = fn p => 1n",
        "type Or 'r = #A | ..'r  let f : Or { y: Nat } -> Nat = fn p => 1n",
        "type Or 'r = #A | ..'r  type Bad = Or Nat",
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
        "type Or 'r = #A | ..'r  let f : Or (#B Nat) -> Nat = fn p => 1n",
        "type Or 'r = #A | ..'r  let f : Or (|) -> Nat = fn p => 1n",
        "type Or 'r = #A | ..'r  type Pass 's = Or 's",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A struct's `..` admits any argument at all, because it *is* the type's core
/// and every type is one of those. `WithX Nat` is a type nothing constructs a
/// term of, and that is allowed.
/// A `..` covers only the fields its row does not already name, so a row
/// written where a row parameter goes may not name any of them. Which labels
/// those are is part of what the parameter stands for, so it is known here —
/// at the argument, where the reader can act on it — rather than only wherever
/// something later happened to flatten the row.
#[test]
fn a_row_argument_may_not_name_what_the_declaration_names() {
    let src = "type WithX 'r = { x: Nat, ..'r }  let f : WithX { x: Nat } -> Nat = fn p => p.x";
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
        "type WithX 'r = { x: Nat, ..'r }  type Pass 's = WithX 's  \
                              let f : Pass { x: Nat } -> Nat = fn p => p.x",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::RepeatedRowField { .. }
    ));

    // A label the declaration does not name is what a `..` is for.
    for src in [
        "type WithX 'r = { x: Nat, ..'r }  let f : WithX { y: Nat } -> Nat = fn p => p.x",
        "type WithX 'r = { x: Nat, ..'r }  let f : WithX {} -> Nat = fn p => p.x",
        "type WithX 'r = { x: Nat, ..'r }  type Pass 's = WithX 's",
    ] {
        let (_, out) = build_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A `..` naming a parameter is the one way a declared type may be left open,
/// because what it stands for is supplied at every use rather than decided
/// here. A bare `..` and a `when` are still refused; a name the header does not
/// bind is refused too, in the words a variable written in a declaration gets.
#[test]
fn a_declaration_is_open_only_through_a_parameter() {
    let (_, out) = built("type WithX 'r = { x: Nat, ..'r }");
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

    for src in ["type T = { x: Nat, ..'r }", "type T 'a = { x: Nat, ..'r }"] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| matches!(
                error.kind,
                ErrorKind::VariableInDeclaration { ref name } if name == "r"
            )),
            "{src}: {:#?}",
            out.errors
        );
    }

    for src in [
        "type T = { x: Nat, .. }",
        "type T 'r = { x when 'a: Nat, ..'r }",
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

/// A parameter cannot hide anything, whatever it is named. The sigil is what
/// keeps the two apart: `'Nat` is the parameter and `Nat` is the built-in, so
/// `type Box 'Nat = { it: 'Nat }` takes one type and `Box Nat` hands it the
/// primitive — no resolution order to get wrong, and no name to be surprised by.
#[test]
fn a_parameter_cannot_hide_a_primitive() {
    let (_, out) = built("type Box 'Nat = { it: 'Nat }  let b : Box Nat -> Nat = fn p => p.it");

    let boxed = out.program.types.values().next().expect("the declaration");
    let TypeKind::Struct { fields, .. } = &boxed.value.tracked else {
        panic!("expected a struct: {:#?}", boxed.value);
    };
    assert!(
        matches!(
            fields["it"].value().map(|value| &value.tracked),
            Some(TypeKind::Param { index: 0, .. })
        ),
        "inside the body the sigil names the parameter: {:#?}",
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
        "and the bare name is the primitive: {:#?}",
        args[0]
    );
}

/// A sum lowers to the cases it names, keyed by name and carrying what each
/// one was written with. The `payload` stays `None` where nothing was written:
/// it means unit, but the unit is inference's to supply, not the tree's.
#[test]
fn lowers_a_sum_type() {
    let src = "type Option 'T = #Some 'T | #None";
    let (mint, out) = built(src);
    let decl = &out.program.types[&type_symbol(&mint, &out, "Option")];
    let TypeKind::Sum { cases, tail } = &decl.value.tracked else {
        panic!("expected a sum, got {:#?}", decl.value.tracked);
    };
    assert_eq!(cases.keys().collect::<Vec<_>>(), vec!["Some", "None"]);
    // The case's own span is the name it was written at, `#` included.
    assert_eq!(
        cases["Some"].name_span().start,
        src.find("#Some").expect("the case")
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
    let (mint, out) = built("let v = #Some 1n  let n = #None");
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
    let src = "type T = #A Nat | #A Nat";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateCase { name, previous, .. }
            if name == "A" && previous.start == src.find("#A").expect("the first")
    ));
    // The first occurrence is the one that stands.
    let decl = &out.program.types[&type_symbol(&mint, &out, "T")];
    let TypeKind::Sum { cases, .. } = &decl.value.tracked else {
        panic!("expected a sum");
    };
    assert_eq!(cases.len(), 1);
}

/// Quoted tags and sum cases lower to decoded labels in terms, patterns, and
/// rows. The sigilled token's span includes both the `#` and quoted spelling.
#[test]
fn quoted_tags_lower_to_decoded_labels_everywhere() {
    let src = r###"let v = #"case name" 1n"###;
    let (mint, out) = built(src);
    let TermKind::Tag { name, payload } = term_value(&mint, &out, "v") else {
        panic!("expected a tag");
    };
    assert_eq!(name.tracked, "case name");
    assert_eq!(name.span.start, src.find('#').expect("the tag"));
    assert_eq!(name.span.width, r###"#"case name""###.len());
    assert!(matches!(
        payload.as_deref().map(|term| &term.kind),
        Some(TermKind::Natural(1))
    ));

    let src = r###"let f = fn x => match x with | #"case name" y => y end"###;
    let (mint, out) = built(src);
    let TermKind::Fn { body, .. } = term_value(&mint, &out, "f") else {
        panic!("expected a function");
    };
    let TermKind::Match { arms, .. } = &body.kind else {
        panic!("expected a match");
    };
    let PatternKind::Tag { name, payload } = &arms[0].0.tracked else {
        panic!("expected a tag pattern");
    };
    assert_eq!(name.tracked, "case name");
    assert!(payload.is_some());

    let (mint, out) = built(r###"type T 'r = #"case name" Nat | \#"gone case" | ..'r"###);
    let TypeKind::Sum { cases, .. } = &out.program.types[&type_symbol(&mint, &out, "T")]
        .value
        .tracked
    else {
        panic!("expected a sum");
    };
    assert!(matches!(cases["case name"], SumCase::Written { .. }));
    assert!(matches!(cases["gone case"], SumCase::Absent { .. }));
}

/// Bare and quoted aliases are one structural key, so duplicate checking must
/// reject mixed spellings just as it rejects two bare spellings.
#[test]
fn bare_and_quoted_labels_are_duplicates() {
    for src in [
        r###"let p = fn a b => { same: a, "same": b }"###,
        r###"type T = { same: Nat, "same": Nat }"###,
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(matches!(
            &out.errors[0].kind,
            ErrorKind::DuplicateField { name, previous }
                if name == "same" && previous.start == src.find("same").expect("the first")
        ));
        assert_eq!(
            out.errors[0].span.start,
            src.rfind(r###""same""###).unwrap()
        );
    }

    let src = r###"type T = #Same Nat | #"Same" Nat"###;
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateCase { name, previous, .. }
            if name == "Same" && previous.start == src.find("#Same").expect("the first")
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.rfind(r###"#"Same""###).unwrap()
    );
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
    for src in ["type T = #A Nat | ..", "type T = #A (when 'a) Nat | #B"] {
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
    let (mint, out) = built("type Tagged 'r = #Err Nat | ..'r");
    let decl = &out.program.types[&type_symbol(&mint, &out, "Tagged")];
    assert_eq!(decl.params[0].kind, cases(&["Err"]));
}

/// A sum's tail stands for cases, so a struct written at one is refused — while
/// a struct's tail is the type's core and takes anything, including a sum.
#[test]
fn a_row_parameter_knows_which_shape_it_is() {
    let (_, out) = build_src("type Cases 'r = #A Nat | ..'r  type Bad = Cases { y: Nat }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::NotARow { .. }));

    let (_, out) = build_src("type WithX 'r = { x: Nat, ..'r }  type Bad = WithX (#A Nat)");
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::NotARow {
                sense: Sense::Fields
            },
            ..
        }]
    ));

    // And a parameter handed to both is a parameter that has to say which it
    // meant: a whole type in one place, the rest of a sum in the other.
    let (_, out) = build_src(
        "type WithX 'r = { x: Nat, ..'r }  type Cases 's = #A Nat | ..'s  \
         type Bad 't = { it: WithX 't, also: Cases 't }",
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
fn operation_signatures_validate_every_row_argument_sense() {
    let (_, out) = build_src(
        "type Fields 'r = { ..'r }\n\
         type Cases 'r = | ..'r\n\
         type Effects 'r = () -> () + ..'r\n\
         effect Bad = { fields: Fields Nat -> Nat, cases: Cases Nat -> Nat, effects: Effects Nat -> Nat }",
    );
    assert_eq!(out.errors.len(), 3, "{:#?}", out.errors);
    assert_eq!(
        out.errors
            .iter()
            .filter_map(|error| match error.kind {
                ErrorKind::NotARow { sense } => Some(sense),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [Sense::Fields, Sense::Cases, Sense::Effects]
    );
}

#[test]
fn a_sum_argument_may_not_repeat_a_case() {
    let (_, out) = build_src("type Cases 'r = #A Nat | ..'r  type Bad = Cases (#A Nat)");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(&out.errors[0].kind, ErrorKind::RepeatedRowField { field, shape }
            if field == "A" && *shape == Shape::Sum),
        "{:#?}",
        out.errors
    );

    // And the cases travel back through a sum written out as an argument, the
    // way they do through a struct: `OuterC`'s own tail ends up where `Cases`'s
    // sat, so it may not stand for `#A` either. Written out and not just
    // handed on — that is the edge a sum used to be left off, and the argument
    // it let through was accepted with the case carrying the wrong type.
    let src = "type Cases 'q = #A Nat | ..'q  type OuterC 'p = Cases (#B Nat | ..'p)  \
               type Bad = OuterC (#A Nat)";
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
        src.find("(#A Nat)").expect("the argument")
    );

    // A case neither of them names is what the `..` is for.
    let (_, out) = build_src(
        "type Cases 'q = #A Nat | ..'q  type OuterC 'p = Cases (#B Nat | ..'p)  \
         type Fine = OuterC (#C Nat)",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// A type may lead back to itself through a sum as much as through a struct: a
/// case is a shape one step in, which is all a declaration has to reach.
#[test]
fn a_sum_makes_a_type_recursive_rather_than_circular() {
    let (mint, out) = built("type List 'a = #Nil | #Cons { head: 'a, tail: List 'a }");
    let decl = &out.program.types[&type_symbol(&mint, &out, "List")];
    assert!(matches!(decl.value.tracked, TypeKind::Sum { .. }));
    // And the parameter reaches a position of what the declaration stands
    // for, through the case's payload, so congruence may be taken on it.
    assert!(decl.params[0].relevant);

    // The recursion that still cannot be allowed is the one that grows.
    let (_, out) = build_src("type T 'a = #Next (T { x: 'a })");
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
/// anywhere 'telling the reader why.
#[test]
fn one_tail_name_is_one_shape_of_rest() {
    let src = "let f : { x: Nat, ..'r } -> (#A Nat | ..'r) -> Nat = fn a => fn b => 1n";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::MixedTail {
                first: Sense::Fields,
                second: Sense::Cases,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );
    // At the second use's *name*, which is the one thing the writer can change
    // — and pointing back at the first, which is the other half of what went
    // wrong and is somewhere else on the page.
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("..'r").expect("the tail") + 2
    );
    let ErrorKind::MixedTail { previous, .. } = out.errors[0].kind else {
        panic!("the clash was just matched");
    };
    assert_eq!(
        previous.start,
        src.find("..'r").expect("the first tail") + 2
    );

    // Either order: the row that absorbs is the second one written, whichever
    // shape that is.
    let (_, out) =
        build_src("let f : (#A Nat | ..'r) -> { x: Nat, ..'r } -> Nat = fn a => fn b => 1n");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::MixedTail {
                first: Sense::Cases,
                second: Sense::Fields,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );

    // Two tails of one shape are what naming one is *for*, and stay one rest.
    let (_, out) = build_src("let f : { x: Nat, ..'r } -> { ..'r } -> Nat = fn a => fn b => 1n");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let (_, out) = build_src("let f : (#A Nat | ..'r) -> (| ..'r) -> Nat = fn a => fn b => 1n");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    // And the scope is one written type, so two annotations may each use `r`
    // for a rest of their own.
    let (_, out) = build_src(
        "let f : { x: Nat, ..'r } -> Nat = fn a => 1n\n\
         let g : (#A Nat | ..'r) -> Nat = fn b => 2n",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// A type declared twice keeps the first, exactly as a term does: the repeat is
/// reported against it and lowers to nothing, so the name still stands for one
/// declaration and everything that mentions it means that one.
#[test]
fn duplicate_type_declarations_keep_the_first() {
    let (mint, out) = build_src("type T = Nat  type T = { x: Nat }  let v : T = 1n");

    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Duplicate {
                ref name,
                namespace: Namespace::Types,
                previous,
            } if name == "T" && previous.start == 5
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
                ref name,
                namespace: Namespace::Types,
            } if name == "Missing"
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
                found: 1,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );
}

/// A `..` names a rest, and a rest is a variable: a declaration's parameter, or
/// one of the annotation it is written in. A declared *type* of the same
/// spelling has nothing to do with it, and cannot even be written there — a
/// bare name after the dots is refused where it is read.
#[test]
fn a_tail_naming_a_declared_type_is_not_a_rest() {
    let src = "type T = Nat  let f : { x: Nat, ..T } -> Nat = fn r => r.x";
    let out = ruddy::parse::parse(ruddy::token::lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.rfind('T').expect("the tail"));

    // Written with its sigil it is a rest like any other, and the declaration
    // it shares a spelling with is untouched: a variable is not a name of
    // anything, so the two cannot collide.
    let (_, out) = build_src("type T = Nat  let f : { x: Nat, ..'T } -> Nat = fn r => r.x");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// An argument written where a row parameter goes has to be a row of that
/// shape. A type that already went wrong is not held to it: it absorbed a
/// complaint, and a second one about the same words would tell the reader
/// nothing they can act on.
#[test]
fn an_erroneous_row_argument_absorbs() {
    let (_, out) = build_src("type WithX 'r = { x: Nat, ..'r }  type P = WithX Missing");

    // The undefined name, and nothing about the shape it failed to have.
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Undefined {
                ref name,
                namespace: Namespace::Types,
            } if name == "Missing"
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
        "let t = #Some t",
        "let p = q.field  let q = p",
        "let u = u 1n",
        "let z = 1n  let y = z",
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
    let (mint, out) = built("let a = 1n  let b = 2n  let c = 3n");

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
    let (mint, out) = built("let id = fn x => x  let a = id 1n");
    assert_eq!(groups(&mint, &out), [vec!["id"], vec!["a"]]);

    let (mint, out) = built("let a = id 1n  let id = fn x => x");
    assert_eq!(groups(&mint, &out), [vec!["id"], vec!["a"]]);

    // The exact vector, and not merely the pairwise order: a hash-ordered
    // grouping passes every "this is before that" assertion and still hands
    // inference a different order every run.
    let (mint, out) =
        built("let top = mid 1n  let mid = fn n => bot n  let bot = fn n => n  let lone = 0n");
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
    let (mint, out) = build_src("let a = b  let b = a  let c = 1n");

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
    let (mint, out) = build_src("type WithX 'r = { x: Nat, ..'r }\ntype T = WithX T");
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
        "type WithX 'r = { x: Nat, ..'r }\n\
         type A 'r = B 'r\n\
         type B 'r = WithX (A 'r)",
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

    // An empty struct-row forwarding constructor adds no field either. Its
    // braces say which kind of parameter this is, but do not make the cycle
    // endless by themselves.
    let (_, out) = build_src("type RowId 'r = { ..'r }\ntype T = RowId T");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "circular-type");

    // An absent label constrains the tail but adds no field on the way round.
    // A present or optional label can be there, so each is genuinely endless.
    let (_, out) = build_src("type NoY 'r = { \\y, ..'r }\ntype T = NoY T");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "circular-type");
    let (_, out) = build_src("type Add 'r = { y: Nat, ..'r }\ntype T = Add T");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "endless-fields");

    // And a declaration that names itself inside a *field* is on no core loop
    // at all: its core is unit, and the recursion is in what a field holds.
    let (_, out) = build_src("type List = { next: List }");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// An argument written at a struct's `..` may not name a field the declaration
/// already names — and what it names reaches through a declared name as much as
/// it is written out, since a `..` handed a name ends up carrying whatever that
/// name carries.
/// An argument lowering has already erased is let through wherever a sum's rest
/// goes: the complaint about it was made where it was written, and a second one
/// about a row nobody wrote would be that mistake said twice.
#[test]
fn an_erased_argument_is_let_through_a_sum_tail() {
    let (_, out) = build_src("type Or 'r = #A | ..'r  type Bad = Or Bogus");
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
    let src = "let leaked = let n = 1n in n\nlet after = n";
    let (mint, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "n"
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
    let (mint, out) = built("let n = 1n\nlet a = let n = { v: 2n } in n");
    let TermKind::Let { name, body, .. } = term_value(&mint, &out, "a") else {
        panic!("expected a let");
    };
    // The use in the body is the inner binding, not the definition above it.
    assert!(matches!(body.kind, TermKind::Ident(symbol) if symbol == name.tracked));
    assert_ne!(name.tracked, term_symbol(&mint, &out, "n"));

    let (_, out) = build_src("let n = 1n  let n = 2n");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::Duplicate { .. }));
}

/// Two nested lets cannot name each other. There is no block form to bind a
/// group of them together, so `b` is simply not in scope where `a`'s value is
/// written, and naming it is an unresolved name like any other.
#[test]
fn two_nested_lets_cannot_name_each_other() {
    let src = "let e = let a = b in let b = 1n in a";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "b"
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
        "let e = let x = 1n in let y = x in y",
        "let e = fn p => let q = p in q",
        "let a = let x = 1n in x  let b = a",
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
    let src = "type WithX 'r = { x: Nat, ..'r }\nlet e = let n : WithX { x: Nat } = 1n in n";
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
    let (mint, out) = built("let a = let x = 1n in x");
    assert_eq!(groups(&mint, &out), [vec!["a"]]);
    assert!(!out.program.groups[0].recursive);
}

/// The IR prints as surface syntax, and a nested `let` is no exception: it
/// reads back as what was written, and re-lowers to the same program.
#[test]
fn displays_nested_lets() {
    assert_eq!(
        display_program("let a = let x = 1n in x"),
        "let a = let x = 1n in x"
    );
    assert_eq!(
        display_program("let a = let x : Nat = 1n in x"),
        "let a = let x : Nat = 1n in x"
    );
    // A `let` in argument position wears the parentheses that make it one.
    assert_eq!(
        display_program("let f = fn x => x  let a = f (let x = 1n in x)"),
        "let f = fn x => x\nlet a = f (let x = 1n in x)"
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

    // The sum counterpart: the case keeps its `#`, and the span keeps
    // the `\` in front of it.
    let src = "let x : #Ok Nat | \\#Err | .. = #Ok 1n";
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
        src.find("\\#Err").expect("the mark")
    );
    assert_eq!(cases["Err"].name_span().width, 5);
}

/// An explicitly absent label in a closed composite is refused: the `\` says
/// the `..` beside it may not stand for the label, and a type with no `..`
/// already says that. Reported at the `\name`, and the composite absorbs.
#[test]
fn an_absent_label_needs_a_tail() {
    let src = "let x : { a: Nat, \\y } = 1n";
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

    let src = "let x : #A | \\#B = 1n";
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
        src.find("\\#B").expect("the mark")
    );
    assert_eq!(out.errors[0].span.width, 3);

    // Every absence in the row is its own report, the way every `when` in a
    // declared type is: each is a mark the reader can act on.
    let (_, out) = build_src("let x : { \\y, \\z } = 1n");
    assert_eq!(out.errors.len(), 2, "errors: {:#?}", out.errors);

    // A declaration's tail must name a parameter (the existing rule), so a
    // `\` beside an unbound `..'r` is that one complaint, unchanged — the
    // absence itself is fine wherever the tail is.
    let (_, out) = build_src("type T = { \\y, ..'r }");
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::VariableInDeclaration { ref name } if name == "r"
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
    for (src, first, second) in [
        ("let x : { x: Nat, \\x, .. } = 1n", "x: Nat", "\\x"),
        ("let x : { \\x, x: Nat, .. } = 1n", "\\x", "x: Nat"),
        ("let x : { \\x, \\x, .. } = 1n", "\\x", "\\x"),
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(
                &out.errors[0].kind,
                ErrorKind::DuplicateField { name, previous }
                    if name == "x" && previous.start == src.find(first).expect("the first mention")
            ),
            "{src}: {:#?}",
            out.errors
        );
        assert_eq!(
            out.errors[0].span.start,
            src.rfind(second).expect("the second mention"),
            "{src}"
        );
    }

    let src = "let x : #A | \\#A | .. = 1n";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ErrorKind::DuplicateCase { name, previous, .. }
            if name == "A" && previous.start == src.find("#A").expect("the first mention")
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.find("\\#A").expect("the second mention")
    );
}

/// An absent label joins its row parameter's lacks set exactly as a written
/// field does: `\y` says the tail has no `y`, which is the same sentence a
/// field named `y` makes it say.
#[test]
fn an_absent_label_joins_a_parameters_lacks() {
    let (_, out) = built("type T 'r = { x: Nat, \\y, ..'r }");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params.len(), 1);
    assert_eq!(decl.params[0].kind, row(&["x", "y"]));

    let (_, out) = built("type NoErr 'r = #Ok Nat | \\#Err | ..'r");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params[0].kind, cases(&["Ok", "Err"]));
}

/// An argument naming an explicitly absent label is refused at the argument,
/// with the existing complaint for one naming a label the declaration already
/// names.
#[test]
fn an_argument_may_not_name_an_absent_label() {
    let src = "type T 'r = { x: Nat, \\y, ..'r }\nlet v : T { y: Nat } = { x: 1n, y: 2n }";
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

    let src = "type NoErr 'r = #Ok Nat | \\#Err | ..'r\nlet x : NoErr (#Err Nat) = #Ok 1n";
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
        build_src("type NoErr 'r = #Ok Nat | \\#Err | ..'r\nlet x : NoErr (#Warn Nat) = #Ok 1n");
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
}

/// The IR prints an absence as written, and the rendering re-lowers to the
/// same program.
#[test]
fn displays_absent_labels_as_written() {
    assert_eq!(
        display_program("type T 'r = { x: Nat, \\y, ..'r }"),
        "type T 'r = { x: Nat, \\y, ..'r }"
    );
    assert_eq!(
        display_program("type NoErr 'r = #Ok Nat | \\#Err | ..'r"),
        "type NoErr 'r = #Ok Nat | \\#Err | ..'r"
    );
    assert_eq!(
        display_program("let f : { \\y, ..'r } -> { ..'r } = fn a => a"),
        "let f : { \\y, ..'r } -> { ..'r } = fn a => a"
    );
}

/// A recursive declaration may write an absence beside its recursion: the
/// growth walk skips an absent label, there being no argument under it to
/// grow — in either shape.
#[test]
fn a_recursive_declaration_may_write_an_absence() {
    let (_, out) = built("type T 'r = { \\y, next: T 'r, ..'r }");
    let decl = out.program.types.values().next().expect("the declaration");
    assert_eq!(decl.params[0].kind, row(&["y", "next"]));

    let (_, out) = built("type S 'r = #A (S 'r) | \\#B | ..'r");
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
    assert_eq!(lowered("let x = 1n"), "let x = 1n");
    assert_eq!(lowered("let f = fn n => f n"), "let f = fn n => f n");
    assert_eq!(
        lowered("let a = let x = 1n in x"),
        "let a = let x = 1n in x"
    );
}

/// R6's statement half: `let {x, y} = e` is a fresh definition holding `e`,
/// then one definition per field, in written order — N+1 definitions. The
/// temporary wears the exact pattern's demand as an annotation: exactly the
/// named fields, each field's type a hole for the solver.
#[test]
fn a_struct_let_statement_makes_a_definition_per_field() {
    assert_eq!(
        lowered("let {x, y} = { x: 1n, y: 2n }"),
        "let %struct : { x: _, y: _ } = { x: 1n, y: 2n }\nlet x = %struct.x\nlet y = %struct.y"
    );
    // Renaming and annotation: the written annotation is the contract on the
    // whole value, so it holds the value on a binding of its own, and the
    // exact demand rides on the temporary the projections read.
    assert_eq!(
        lowered("let {x: a} : { x: Nat } = { x: 1n }"),
        "let %value : { x: Nat } = { x: 1n }\nlet %struct : { x: _ } = %value\nlet a = %struct.x"
    );
    // `let {} = e` is the unit pattern by another spelling: exactly no
    // fields.
    assert_eq!(lowered("let {} = {}"), "let %unit : () = ()");
    // With the `..` there is no demand beyond the projections': the pattern
    // is open, and the temporary is bare.
    assert_eq!(
        lowered("let {x, ..} = { x: 1n, y: 2n }"),
        "let %struct = { x: 1n, y: 2n }\nlet x = %struct.x"
    );
    // Nesting chains through intermediate temporaries, still in field order,
    // each exact level with a demand of its own.
    assert_eq!(
        lowered("let {pos: {x, y}, tag: t} = p  let p = { pos: { x: 1n, y: 2n }, tag: 3n }"),
        "let %struct : { pos: _, tag: _ } = p\n\
         let %struct : { x: _, y: _ } = %struct.pos\n\
         let x = %struct.x\n\
         let y = %struct.y\n\
         let t = %struct.tag\n\
         let p = { pos: { x: 1n, y: 2n }, tag: 3n }"
    );
}

/// R6's expression half: `let {x, y: a} = e in b` is a fresh temporary and one
/// nested `let` per field.
#[test]
fn a_struct_let_expression_chains_through_a_temporary() {
    assert_eq!(
        lowered("let d = fn e => let {x, y: a} = e in x"),
        "let d = fn e => let %struct : { x: _, y: _ } = e in \
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
        lowered("let a = let {x} : { x: Nat } = { x: 1n } in x"),
        "let a = let %value : { x: Nat } = { x: 1n } in \
         let %struct : { x: _ } = %value in let x = %struct.x in x"
    );
    // The spec's own nested example.
    assert_eq!(
        lowered("let dist = fn p => let {pos: {x, y}} = p in add x y  let add = fn a b => a"),
        "let dist = fn p => \
         let %struct : { pos: _ } = p in \
         let %struct : { x: _, y: _ } = %struct.pos in \
         let x = %struct.x in let y = %struct.y in add x y\n\
         let add = fn a => fn b => a"
    );
    // `()` constrains the value to unit through an annotated fresh binding.
    assert_eq!(
        lowered("let u = let () = {} in 1n"),
        "let u = let %unit : () = () in 1n"
    );
}

/// R4/R5 of the wildcard spec: `let _ = e` is a definition under a name
/// nothing can write, so any number of them coexist — with each other, and
/// with every named definition — and no duplicate complaint can ever mention
/// `_`. The annotation rides on the hidden definition, so `let _ : T = e` is
/// a type assertion.
#[test]
fn a_wildcard_let_statement_defines_a_hidden_fresh_name() {
    assert_eq!(lowered("let _ = 1n"), "let %discard = 1n");
    // The headline: two of them, plus a third asserting a type — no
    // duplicate-definition error anywhere.
    assert_eq!(
        lowered("let _ = f 1n  let _ = f 2n  let _ : Nat = f 3n  let f = fn x => x"),
        "let %discard = f 1n\nlet %discard = f 2n\nlet %discard : Nat = f 3n\nlet f = fn x => x"
    );
    // Nor a collision with a named definition of any spelling.
    assert_eq!(
        lowered("let x = 1n  let _ = 2n  let x2 = 3n"),
        "let x = 1n\nlet %discard = 2n\nlet x2 = 3n"
    );

    // The hidden definitions are distinct symbols, each holding its own value.
    let (mint, out) = built("let _ = 1n  let _ = 2n");
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
        lowered("let a = let _ = f 1n in 2n  let f = fn x => x"),
        "let a = let %discard = f 1n in 2n\nlet f = fn x => x"
    );
    // The annotation stays the contract on the value.
    assert_eq!(
        lowered("let a = let _ : Nat = 1n in 2n"),
        "let a = let %discard : Nat = 1n in 2n"
    );

    // The fresh symbol is bound into no scope: the body's `2` aside, nothing
    // references it, and the tree says so.
    let (mint, out) = built("let a = let _ = 1n in 2n");
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
        TermKind::Unary { value, .. } => references_of(value, out),
        TermKind::Binary { left, right, .. } => {
            references_of(left, out);
            references_of(right, out);
        }
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
        TermKind::Natural(_)
        | TermKind::Integer(_)
        | TermKind::Real(_)
        | TermKind::String(_)
        | TermKind::Boolean(_)
        | TermKind::Error => {}
    }
}

/// R6: a wildcard leaf of a struct-pattern `let` keeps the field demand — a
/// hidden fresh definition holds the projection — while binding nothing. Both
/// forms of the `let`.
#[test]
fn a_wildcard_struct_leaf_keeps_the_projection() {
    assert_eq!(
        lowered("let {x: _, y} = { x: 1n, y: 2n }"),
        "let %struct : { x: _, y: _ } = { x: 1n, y: 2n }\n\
         let %discard = %struct.x\nlet y = %struct.y"
    );
    assert_eq!(
        lowered("let use_y = fn p => let {x: _, y} = p in y"),
        "let use_y = fn p => let %struct : { x: _, y: _ } = p in \
         let %discard = %struct.x in let y = %struct.y in y"
    );
}

/// R3: `_` never joins the duplicate-binder check — `{a: _, b: _}` is legal,
/// and `_` beside a named binder is too — while a real name bound twice is
/// still the R13 mistake it was.
#[test]
fn a_wildcard_is_exempt_from_the_duplicate_binder_check() {
    assert_eq!(
        lowered("let f = fn e => match e with | #Pair { a: _, b: _ } => 1n | _ => 2n end"),
        "let f = fn e => match e with | #Pair { a: _, b: _ } => 1n | _ => 2n end"
    );
    assert_eq!(
        lowered("let f = fn e => match e with | { a: _, b: x } => x end"),
        "let f = fn e => match e with | { a: _, b: x } => x end"
    );

    // The same pattern with a name where the wildcards were still fails.
    let (_, errors) = lowered_with_errors("let f = fn e => match e with | { a: x, b: x } => x end");
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].starts_with("duplicate-binding@"), "{errors:#?}");
}

/// R7: `fn _ => e` lowers to the ordinary `Fn` node with a fresh symbol
/// nothing references — the shape does not change — and any number of `_`
/// arguments mix with names.
#[test]
fn a_wildcard_fn_argument_binds_a_symbol_nothing_references() {
    let (mint, out) = built("let f = fn _ => 1n");
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
/// `let #Some _ = e` is still the refutable-binding error, in both forms,
/// with the unchanged wording.
#[test]
fn a_wildcard_does_not_make_a_refutable_binding_calm() {
    let src = "let #Some _ = opt  let opt = #Some 1n";
    let (_, errors) = lowered_with_errors(src);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(
        errors[0],
        format!("binding-can-fail@{}", src.find("#Some").expect("the tag"))
    );

    let src = "let a = let #Some _ = opt in 1n  let opt = #Some 1n";
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
        lowered("let f = fn n => match n with | _ => 1n | 0n => 2n end"),
        "let f = fn n => match n with | _ => 1n | 0n => 2n end"
    );
    assert_eq!(
        lowered("let f = fn e => match e with | _ => 1n | _ => 2n end"),
        "let f = fn e => match e with | _ => 1n | _ => 2n end"
    );
    // Last, it is what a named catch-all is: an ordinary final arm.
    assert_eq!(
        lowered("let f = fn n => match n with | 0n => 1n | _ => 2n end"),
        "let f = fn n => match n with | 0n => 1n | _ => 2n end"
    );
}

/// R5: a match lowers to the one matrix node, arms as written and normalized
/// — puns expanded, symbols resolved, each body exactly once — and prints
/// back as the match the reader wrote.
#[test]
fn a_match_lowers_to_one_matrix_node() {
    assert_eq!(
        lowered("let get = fn opt => match opt with | #Some x => x | #None => 0n end"),
        "let get = fn opt => match opt with | #Some x => x | #None => 0n end"
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
        "let f = fn e => match e with | #A #X x => 1n | r => 2n end",
        "let f = fn e => match e with | { a: #A, b: #B } => 1n | r => 2n end",
        "let f = fn e => match e with | #A 0n => 1n | #A #X => 2n | r => 3n end",
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
               | #Cons { head: #Some x, tail: t } => x \
               | #Cons { head: #None, tail: t } => 0n \
               | #Nil => 0n \
               end";
    assert_eq!(
        lowered(src),
        "let pick = fn l => match l with \
         | #Cons { head: #Some x, tail: t } => x \
         | #Cons { head: #None, tail: t } => 0n \
         | #Nil => 0n \
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
/// whole value — `match e with | x => b end` means `let x = e in b`, and the
/// meaning is typing's to give.
#[test]
fn a_sole_catch_all_keeps_its_shape() {
    assert_eq!(
        lowered("let same = fn v => match v with | w => w end"),
        "let same = fn v => match v with | w => w end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with | {x} => x end"),
        "let f = fn v => match v with | { x: x } => x end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with | () => 1n end"),
        "let f = fn v => match v with | () => 1n end"
    );
    // `{}` reaches into nothing: it binds nothing, tests nothing, and is as
    // much a catch-all as a bare name.
    assert_eq!(
        lowered("let f = fn v => match v with | {} => 1n end"),
        "let f = fn v => match v with | () => 1n end"
    );
}

/// Refining a later tag compares it with every earlier arm, including scalar
/// tests nested in other tags. Each primitive keeps its exact representation
/// while the matrix asks which cases remain possible.
#[test]
fn matrix_refinement_reads_every_nested_scalar_pattern() {
    let (mint, mut out) = built(
        "let f = fn value => match value with \
         | #Integer 1i => 0n \
         | #Real 1.5 => 0n \
         | #String \"x\" => 0n \
         | #Done x => 0n \
         | rest => 0n \
         end",
    );
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
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
        lowered("let first = fn v => match v with | #Some x => x | rest => rest end"),
        "let first = fn v => match v with | #Some x => x | rest => rest end"
    );
}

/// Natural arms are arms like any other.
#[test]
fn natural_arms_lower_flat() {
    assert_eq!(
        lowered("let d = fn n => match n with | 0n => #Zero | 1n => #One | k => #Many end"),
        "let d = fn n => match n with | 0n => #Zero | 1n => #One | k => #Many end"
    );
}

/// A match inside a `let`'s own value is a shape, like a projection: the
/// binding asks something of the name, so the loop still describes a value —
/// a sole catch-all included, since the match survives as a node rather than
/// collapsing into a chain of names.
#[test]
fn a_match_is_a_shape_for_the_circularity_walk() {
    assert_eq!(
        lowered("let x = match x with | #A y => y | r => r end"),
        "let x = match x with | #A y => y | r => r end"
    );
    let (_, errors) =
        lowered_with_errors("let a = let x = match x with | #A y => y | r => r end in x");
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = lowered_with_errors("let x = match x with | w => w end");
    assert!(errors.is_empty(), "{errors:#?}");
}

/// Grouping still sees a reference made from inside an arm's body, so two
/// definitions reaching each other through a match share a group.
#[test]
fn grouping_sees_references_inside_arm_bodies() {
    let (mint, out) = built(
        "let a = fn v => match v with | #Go x => b x | r => 0n end\n\
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
        lowered_with_errors("let opt = #Some 1n  let #Some x = opt  let use = x");
    assert_eq!(errors, ["binding-can-fail@24"], "{errors:#?}");
    assert_eq!(
        printed,
        "let opt = #Some 1n\nlet %value = opt\nlet x = <error>\nlet use = x"
    );

    // The expression form, and the number as the refuter: the complaint
    // points at the `0`.
    let (printed, errors) = lowered_with_errors("let a = let {a: 0n} = { a: 1n } in 2n");
    assert_eq!(errors, ["binding-can-fail@16"], "{errors:#?}");
    assert_eq!(printed, "let a = let %value = { a: 1n } in 2n");
    let (printed, errors) = lowered_with_errors("let a = let #Some x = #Some 1n in x");
    assert_eq!(errors, ["binding-can-fail@12"], "{errors:#?}");
    assert_eq!(
        printed,
        "let a = let %value = #Some 1n in let x = <error> in x"
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
        lowered("let f = fn e => match e with | #A x => 1n | #A y => 2n end"),
        "let f = fn e => match e with | #A x => 1n | #A y => 2n end"
    );
    // A misplaced catch-all starves nothing here: the arms after it stay.
    assert_eq!(
        lowered("let f = fn e => match e with | r => 1n | #A x => 2n end"),
        "let f = fn e => match e with | r => 1n | #A x => 2n end"
    );
    // A natural match with no final arm keeps its shape and raises nothing.
    assert_eq!(
        lowered("let f = fn n => match n with | 0n => 1n end"),
        "let f = fn n => match n with | 0n => 1n end"
    );
    // A mixed match is not lowering's to refuse any more: the solver's
    // ordinary mismatch is the only complaint such a program gets.
    assert_eq!(
        lowered("let f = fn e => match e with | 0n => 1n | #A x => 2n | k => 3n end"),
        "let f = fn e => match e with | 0n => 1n | #A x => 2n | k => 3n end"
    );
    // A name inside a kept arm's body still resolves — or fails to, which is
    // still lowering's own complaint.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | #A x => 1n | #A y => oops end");
    assert_eq!(errors, ["undefined-term@52"], "{errors:#?}");
}

/// The column unions that broke revision 1 lower clean: positions are typed
/// and checked as the union of what the whole match tests there, never
/// per-arm, so overlapping arms with different sub-patterns are simply the
/// matrix they are.
#[test]
fn overlapping_arms_are_clean() {
    lowered(
        "let f = fn e => match e with \
         | { a: #A, b: #X, c: x } => 1n | { a: #A, b: #Y, c: y } => 2n end",
    );
    lowered(
        "let f = fn e => match e with \
         | { a: #A, b: x } => 1n | { a: #B, b: 0n } => 2n | { a: #B, b: k } => 3n end",
    );
    lowered("let f = fn e => match e with | #A #X x => 1n | #A w => 2n | r => 3n end");
}

/// The `..` survives normalization: exactness is read off the rest marker by
/// inference and the pattern checks, so the marker has to arrive — and the
/// printed pattern has to show it, so the IR tab reads as the source did.
#[test]
fn a_rest_marker_survives_normalization() {
    assert_eq!(
        lowered("let f = fn v => match v with | {x, ..} => x | {} => 0n end"),
        "let f = fn v => match v with | { x: x, .. } => x | () => 0n end"
    );
    assert_eq!(
        lowered("let f = fn v => match v with | {..} => 1n end"),
        "let f = fn v => match v with | { .. } => 1n end"
    );
    let (mint, out) = built("let f = fn v => match v with | {x, ..} => x end");
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

/// Nonempty tuples disappear at the IR boundary as closed structs with
/// canonical zero-based decimal fields. The generated field spans come from
/// their elements, and tuple patterns use the same exact struct shape.
#[test]
fn tuples_lower_to_canonical_structs() {
    let (mint, out) = built("let value : (Nat, String) = (1n, \"x\")");
    let symbol = term_symbol(&mint, &out, "value");
    let TermKind::Struct(fields) = &out.program.terms[&symbol].value.kind else {
        panic!("a tuple expression lowers to a struct");
    };
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["0", "1"]
    );
    assert!(matches!(fields["0"].value.kind, TermKind::Natural(1)));
    assert!(matches!(fields["1"].value.kind, TermKind::String(ref value) if value == "x"));
    assert_eq!(fields["0"].name_span, fields["0"].value.span);
    assert_eq!(fields["1"].name_span, fields["1"].value.span);

    let annotation = out.program.terms[&symbol]
        .annotation
        .as_ref()
        .expect("the tuple type annotation");
    let TypeKind::Struct { fields, tail } = &annotation.ty.tracked else {
        panic!("a tuple type lowers to a struct type");
    };
    assert!(tail.is_none());
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["0", "1"]
    );
    for field in fields.values() {
        let TypeField::Written {
            name_span,
            when,
            value,
        } = field
        else {
            panic!("tuple fields are present");
        };
        assert!(when.is_none());
        assert_eq!(*name_span, value.span);
    }

    let (mint, out) =
        built("let pick = fn value => match value with | (first, (_, second)) => second end");
    let TermKind::Fn { body, .. } = term_value(&mint, &out, "pick") else {
        panic!("pick is a function");
    };
    let TermKind::Match { arms, .. } = &body.kind else {
        panic!("pick matches its argument");
    };
    let PatternKind::Struct { fields, rest } = &arms[0].0.tracked else {
        panic!("a tuple pattern lowers to a struct pattern");
    };
    assert!(rest.is_none());
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["0", "1"]
    );
    let PatternKind::Struct {
        fields: nested,
        rest: nested_rest,
    } = &fields["1"].value.tracked
    else {
        panic!("a nested tuple remains a nested canonical struct");
    };
    assert!(nested_rest.is_none());
    assert_eq!(
        nested.keys().map(String::as_str).collect::<Vec<_>>(),
        ["0", "1"]
    );
    assert!(matches!(nested["0"].value.tracked, PatternKind::Wildcard));
    let PatternKind::Bind(second) = nested["1"].value.tracked else {
        panic!("the second element binds second");
    };
    assert!(matches!(arms[0].1.kind, TermKind::Ident(symbol) if symbol == second.tracked));
}

/// Both surface-pattern walks recurse through tuple elements: top-level tuple
/// destructuring declares every binder, and the lowering walk diagnoses a
/// duplicate across nested tuple/struct boundaries rather than losing it.
#[test]
fn tuple_pattern_walks_are_exhaustive() {
    let (mint, out) = built("let (first, second) = (1n, 2n)");
    assert!(
        out.program
            .terms
            .keys()
            .any(|symbol| mint.name(*symbol) == "first")
    );
    assert!(
        out.program
            .terms
            .keys()
            .any(|symbol| mint.name(*symbol) == "second")
    );

    let (_, out) = build_src(
        "let f = fn value => match value with | (same, { nested: (other, same) }) => same end",
    );
    assert_eq!(
        out.errors
            .iter()
            .filter(|error| matches!(error.kind, ErrorKind::DuplicateBinding { .. }))
            .count(),
        1
    );
}

/// One pattern binding one name twice is reported at the repeat, in any
/// nesting; two different arms may of course bind the same name.
#[test]
fn a_pattern_binding_a_name_twice_is_refused() {
    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {x, x} => 1n end");
    assert_eq!(errors, ["duplicate-binding@35"], "{errors:#?}");

    let (_, errors) = lowered_with_errors("let f = fn e => match e with | {a: x, b: x} => x end");
    assert_eq!(errors, ["duplicate-binding@41"], "{errors:#?}");

    // Across nesting in one pattern.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | {a: x, b: {c: x}} => x end");
    assert_eq!(errors, ["duplicate-binding@45"], "{errors:#?}");

    // Two arms binding one name are two scopes, not a repeat.
    let (_, errors) =
        lowered_with_errors("let f = fn e => match e with | #A x => x | #B x => x end");
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
    assert_eq!(lowered("let () = {}"), "let %unit : () = ()");
    // With a written annotation, the annotation is the contract on the value
    // and the unit demand goes on a second binding of it.
    assert_eq!(
        lowered("let () : {} = {}"),
        "let %value : () = ()\nlet %unit : () = %value"
    );
    assert_eq!(
        lowered("let u = let () : {} = {} in 1n"),
        "let u = let %value : () = () in let %unit : () = %value in 1n"
    );

    // A statement pattern repeating a name: the repeat is the pattern's own
    // complaint, not a second definition, and the walk stays total — the
    // repeat binds its stand-in to an error value.
    let (printed, errors) = lowered_with_errors("let {x, x} = { x: 1n }");
    assert_eq!(errors, ["duplicate-binding@8"], "{errors:#?}");
    assert_eq!(
        printed,
        "let %struct : { x: _ } = { x: 1n }\nlet x = %struct.x\nlet x = <error>"
    );

    // A refutable statement pattern with a number: the complaint quotes it.
    let (_, errors) = lowered_with_errors("let {a: 0n} = { a: 1n }");
    assert_eq!(errors, ["binding-can-fail@8"], "{errors:#?}");

    // A bare tag on a statement `let` is refused the same way, binding
    // nothing — there is nothing for it to bind.
    let (printed, errors) = lowered_with_errors("let #None = {}");
    assert_eq!(errors, ["binding-can-fail@4"], "{errors:#?}");
    assert_eq!(printed, "let %value = ()");

    // A refused binding still binds the names inside its calm corners — the
    // nested struct's — and points at the tag past a field that is fine.
    let (printed, errors) = lowered_with_errors("let a = let {p: {q}, r: #Bad} = {} in q");
    assert_eq!(errors, ["binding-can-fail@24"], "{errors:#?}");
    assert_eq!(printed, "let a = let %value = () in let q = <error> in q");
}

/// A `where` clause survives lowering as the formula it was written as, over
/// the names the type's `when`s bound — and prints back as itself, since an
/// annotation is a contract and a contract that came out spelled differently
/// would be a different one.
#[test]
fn an_annotation_keeps_its_clause() {
    let (mint, out) =
        built("let f : { x when 'a: Nat, y when 'b: Nat } where 'a != 'b = { x: 1n }");
    let annotation = out.program.terms[&term_symbol(&mint, &out, "f")]
        .annotation
        .as_ref()
        .expect("the annotation");
    let clause = annotation.clause.as_ref().expect("the clause");
    assert!(matches!(clause.tracked, ClauseKind::NotEqual(..)));
    assert_eq!(
        print::ir::clause(&clause.tracked, &mint).to_string(),
        "'a != 'b"
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
    let (mint, out) = built("let f : { x when _: Nat } = { x: 1n }");
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

    let (_, out) = build_src("let f : { x when _: Nat } where 'x = { x: 1n }");
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
    let src = "let f : { x when 'a: Nat } where 'c = { x: 1n }";
    let (mint, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        &error.kind,
        ErrorKind::UnboundPresence { name } if name == "c"
    ));
    assert_eq!(error.span.start, src.rfind("'c").expect("the name"));

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
        ("let f : { x when 'a: Nat } where 'c or 'd = { x: 1n }", 2),
        ("let f : { x when 'a: Nat } where 'c and 'd = { x: 1n }", 2),
        ("let f : { x when 'a: Nat } where 'c = 'd = { x: 1n }", 2),
        ("let f : { x when 'a: Nat } where 'c != 'd = { x: 1n }", 2),
        ("let f : { x when 'a: Nat } where not 'c = { x: 1n }", 1),
        ("let f : { x when 'a: Nat } where 'a or 'c = { x: 1n }", 1),
        ("let f : { x when 'a: Nat } where 'a and 'c = { x: 1n }", 1),
        ("let f : { x when 'a: Nat } where 'a = 'c = { x: 1n }", 1),
        ("let f : { x when 'a: Nat } where 'a != 'c = { x: 1n }", 1),
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
    let src = "type T = { x: Nat } where 'a";
    let (_, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(error.kind, ErrorKind::ClauseInDeclaration));
    assert_eq!(error.span.start, src.rfind("'a").expect("the clause"));

    // A `when` inside one is the same refusal about a label, and it is the one
    // `..` already gets: the declaration's body lowers to the error type.
    let (_, out) = build_src("type T = #A (when 'a) Nat");
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

/// An effect binds parameters the way a type declaration does: its
/// operations may mention them, a row applies it to exactly that many
/// arguments, and a variable no parameter binds is still refused.
#[test]
fn an_effect_binds_parameters_its_operations_mention() {
    let source = "effect Ask 'a = { get: () -> 'a }\n\
                  let f : () -> Nat + !Ask Nat = fn _ => 0n";
    let (mint, out) = built(source);
    let symbol = *out
        .program
        .effects
        .keys()
        .find(|symbol| mint.name(**symbol) == "Ask")
        .expect("the effect is declared");
    let decl = &out.program.effects[&symbol];
    assert_eq!(decl.params.len(), 1);
    assert_eq!(mint.name(decl.params[0].symbol), "a");
    assert_eq!(decl.params[0].kind, ParamKind::Type { lacks: [].into() });
    let Effect::Operations(operations) = &decl.value else {
        panic!("expected operations");
    };
    let get = &operations[&OperationSelector::Named("get".to_string())];
    assert!(matches!(get.to.tracked, TypeKind::Param { index: 0, .. }));

    let term = &out.program.terms[&term_symbol(&mint, &out, "f")];
    let TypeKind::Arrow { effects, .. } = &term.annotation.as_ref().expect("annotated").ty.tracked
    else {
        panic!("expected an arrow");
    };
    let (id, label) = effects.effects.first().expect("the row names the effect");
    assert_eq!(id.name(), "Ask");
    let EffectLabel::Written { args, .. } = label else {
        panic!("expected a written label");
    };
    assert!(matches!(args.as_slice(), [arg] if matches!(arg.tracked, TypeKind::Prim(Prim::Nat))));

    // Too few or too many arguments is counted at the application, and the
    // label is dropped rather than paired up by guesswork.
    for row in ["!Ask", "!Ask Nat Nat", "\\!Ask + ..'e"] {
        let src =
            format!("effect Ask 'a = {{ get: () -> 'a }}\nlet f : () -> Nat + {row} = fn _ => 0n");
        let (_, out) = build_src(&src);
        let [error] = &out.errors[..] else {
            panic!("{row}: {:#?}", out.errors);
        };
        assert_eq!(error.kind.code(), "effect-arity", "{row}");
        assert!(
            matches!(
                &error.kind,
                ErrorKind::EffectArity { name, expected: 1, .. } if name == "Ask"
            ),
            "{row}: {:#?}",
            error.kind
        );
    }
    let (_, out) =
        build_src("effect Ask 'a = { get: () -> 'a }\nlet f : () -> Nat + !Ask = fn _ => 0n");
    let diagnostic = out.errors[0].diagnostic();
    assert_eq!(
        diagnostic.title,
        "effect `!Ask` expects one argument, but none was written"
    );
    assert_eq!(diagnostic.help, ["add the missing effect arguments"]);
    // And an effect declared without parameters takes none, as before.
    assert_eq!(
        codes_of("effect Log = Nat -> ()\nlet f : () -> Nat + !Log Nat = fn _ => 0n"),
        ["effect-arity"]
    );
    // A variable no parameter binds is the one an operation cannot have.
    let (_, out) = build_src("effect Ask 'a = { get: () -> 'b }");
    assert!(
        matches!(
            out.errors.as_slice(),
            [error] if matches!(
                error.kind,
                ErrorKind::ImpureOperation { found: OperationTypeProblem::Variable(ref name) } if name == "b"
            )
        ),
        "{:#?}",
        out.errors
    );
}

/// What an effect's parameter stands for is read off its operations the way a
/// type's is read off its body — the same fixpoint, so a parameter handed on
/// to an effect inherits that effect's reading, and one used two ways is
/// refused at its declaration. An operation may carry effects on an arrow
/// nested inside its signature; only its own outer arrow stays pure.
#[test]
fn an_effect_parameter_stands_for_what_its_operations_use_it_as() {
    let source = "effect Log = { write: Nat -> () }\n\
                  effect Ask 'a = { get: () -> 'a }\n\
                  effect State 'r = { get: () -> { x: Nat, ..'r }, put: { x: Nat, ..'r } -> () }\n\
                  effect Choose 'c = { pick: (#A | ..'c) -> () }\n\
                  effect Run 'e = { run: (() -> () + !Log + ..'e) -> () }\n\
                  effect Wrap 'f = { wrap: (() -> () + !Run 'f) -> () }\n\
                  effect Nil 'u\n\
                  type Runner 'g = () -> () + !Run 'g";
    let (mint, out) = built(source);
    let kind = |name: &str| {
        let symbol = *out
            .program
            .effects
            .keys()
            .find(|symbol| mint.name(**symbol) == name)
            .unwrap_or_else(|| panic!("no effect named {name}"));
        out.program.effects[&symbol].params[0].kind.clone()
    };
    assert_eq!(kind("Ask"), ParamKind::Type { lacks: [].into() });
    assert_eq!(
        kind("State"),
        ParamKind::Fields {
            lacks: ["x".to_string()].into()
        }
    );
    assert_eq!(
        kind("Choose"),
        ParamKind::Cases {
            lacks: ["A".to_string()].into()
        }
    );
    let log = out.program.effect_ids[&out
        .program
        .effects
        .keys()
        .copied()
        .find(|symbol| mint.name(*symbol) == "Log")
        .expect("Log is declared")]
        .row_key();
    assert_eq!(
        kind("Run"),
        ParamKind::Effects {
            lacks: [log.clone()].into()
        }
    );
    // Handed straight on to `Run`, `Wrap`'s parameter reads as `Run`'s does,
    // and so does the type declaration's.
    assert_eq!(
        kind("Wrap"),
        ParamKind::Effects {
            lacks: [log].into()
        }
    );
    let runner = &out.program.types[&type_symbol(&mint, &out, "Runner")];
    assert_eq!(runner.params[0].kind.sense(), Sense::Effects);
    // A parameter nothing uses stands for a type.
    assert_eq!(kind("Nil"), ParamKind::Type { lacks: [].into() });

    // Read as a type by one operation and as a sum's rest by another: the
    // declaration is told, at the parameter.
    let (_, out) = build_src("effect Bad 'a = { one: 'a -> (), two: (#A | ..'a) -> () }");
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "mixed-parameter");
    assert!(matches!(
        error.kind,
        ErrorKind::MixedParameter {
            first: Sense::Type,
            second: Sense::Cases
        }
    ));

    // The outer arrow of an operation is still the one place effects cannot
    // be declared.
    let (_, out) =
        build_src("effect Log = { write: Nat -> () }\neffect Bad = { op: Nat -> () + !Log }");
    assert!(
        matches!(
            out.errors.as_slice(),
            [error] if matches!(error.kind, ErrorKind::ImpureOperation { found: OperationTypeProblem::Effects })
        ),
        "{:#?}",
        out.errors
    );
    // And a nested row is held to an operation's rules: nothing anonymous.
    for source in [
        "effect Bad = { op: (() -> () + ..) -> () }",
        "effect Log = { write: Nat -> () }\neffect Bad = { op: (() -> () + !Log (when 'p)) -> () }",
        "effect Bad = { op: (() -> () + ..'e) -> () }",
    ] {
        let (_, out) = build_src(source);
        assert!(
            matches!(
                out.errors.as_slice(),
                [error] if matches!(error.kind, ErrorKind::ImpureOperation { .. })
            ),
            "{source}: {:#?}",
            out.errors
        );
    }
}

/// An argument handed to an effect is held to the parameter's reading the way
/// a type application's is: a row goes where a row is asked for, and it may
/// not name what the declaration already names beside the parameter.
#[test]
fn an_effect_argument_is_held_to_its_parameters_reading() {
    let base = "effect Log = { write: Nat -> () }\n\
                effect Run 'e = { run: (() -> () + !Log + ..'e) -> () }\n\
                effect State 'r = { get: () -> { x: Nat, ..'r } }\n\
                effect Choose 'c = { pick: (#A | ..'c) -> () }\n";
    for (row, sense) in [
        ("!Run Nat", Sense::Effects),
        ("!State Nat", Sense::Fields),
        ("!Choose Nat", Sense::Cases),
    ] {
        let (_, out) = build_src(&format!("{base}let f : () -> Nat + {row} = fn _ => 0n"));
        assert!(
            matches!(
                out.errors.as_slice(),
                [error] if matches!(error.kind, ErrorKind::NotARow { sense: found } if found == sense)
            ),
            "{row}: {:#?}",
            out.errors
        );
    }
    let (mint, out) = built(base);
    let log = out.program.effect_ids[&out
        .program
        .effects
        .keys()
        .copied()
        .find(|symbol| mint.name(*symbol) == "Log")
        .expect("Log is declared")]
        .row_key();
    for (row, shape, field) in [
        ("!Run (!Log)", Shape::Effect, log.as_str()),
        ("!State { x: Nat }", Shape::Struct, "x"),
        ("!Choose (#A)", Shape::Sum, "A"),
    ] {
        let (_, out) = build_src(&format!("{base}let f : () -> Nat + {row} = fn _ => 0n"));
        assert!(
            matches!(
                out.errors.as_slice(),
                [error] if matches!(
                    &error.kind,
                    ErrorKind::RepeatedRowField { shape: found, field: name } if *found == shape && name == field
                )
            ),
            "{row}: {:#?}",
            out.errors
        );
    }
    // Wherever a row is written: a declared type's body, an operation's
    // nested arrow, and an extern's annotation are checked alike.
    for source in [
        format!("{base}type T = () -> () + !Run Nat"),
        format!("{base}effect Nested = {{ op: (() -> () + !Run Nat) -> () }}"),
        format!("{base}extern host : () -> Nat + !Run Nat = \"host\""),
    ] {
        assert_eq!(codes_of(&source), ["not-a-row"], "{source}");
    }
    // And an argument that keeps to the reading is taken as written.
    let (mint, out) = built(&format!(
        "{base}effect IO = {{ print: Nat -> () }}\nlet f : () -> Nat + !Run (!IO + ..'e) = fn _ => 0n"
    ));
    let term = &out.program.terms[&term_symbol(&mint, &out, "f")];
    let TypeKind::Arrow { effects, .. } = &term.annotation.as_ref().expect("annotated").ty.tracked
    else {
        panic!("expected an arrow");
    };
    let label = effects.effects.values().next().expect("the row names Run");
    assert!(matches!(label.args(), [arg] if matches!(arg.tracked, TypeKind::Effects(_))));
}

/// Structural identity belongs to the generic constructor: its leaf name, how
/// many parameters it takes and what each stands for, and its operations with
/// the parameters as positions. Names of parameters are not part of it; a
/// parameter no operation mentions still is; and an effect applied inside an
/// interface is told apart by its arguments.
#[test]
fn structural_identity_includes_parameters_and_their_positions() {
    let source = "module Renamed = effect Ask 'a = { get: () -> 'a } end\n\
                  module Same = effect Ask 'b = { get: () -> 'b } end\n\
                  module Wider = effect Ask 'a 'b = { get: () -> 'a } end\n\
                  module Other = effect Ask 'a 'b = { get: () -> 'b } end\n\
                  module Fixed = effect Ask 'a = { get: () -> Nat } end\n\
                  module Fields = effect Ask 'a = { get: () -> { x: Nat, ..'a } } end\n\
                  module Bare = effect Ask = { get: () -> Nat } end\n\
                  module Phantom = effect Ask 'a = { get: () -> Nat } end";
    let (mint, out) = built(source);
    let identity = |module: &str| {
        let symbol = *out
            .program
            .effects
            .keys()
            .find(|symbol| {
                mint.name(**symbol) == "Ask"
                    && mint
                        .parent(**symbol)
                        .is_some_and(|parent| mint.name(parent.symbol()) == module)
            })
            .unwrap_or_else(|| panic!("no Ask in {module}"));
        out.program.effect_ids[&symbol].clone()
    };
    assert_eq!(identity("Renamed"), identity("Same"));
    assert_eq!(identity("Fixed"), identity("Phantom"));
    let distinct = [
        identity("Renamed"),
        identity("Wider"),
        identity("Other"),
        identity("Fixed"),
        identity("Fields"),
        identity("Bare"),
    ];
    for (at, left) in distinct.iter().enumerate() {
        for right in &distinct[at + 1..] {
            assert_ne!(left, right);
        }
    }

    // Applied inside an interface, an effect's arguments are part of what the
    // interface says, so two interfaces differing only there are two.
    let source = "effect Ask 'a = { get: () -> 'a }\n\
                  module N = effect Probe = { run: (() -> () + !Ask Nat) -> () } end\n\
                  module S = effect Probe = { run: (() -> () + !Ask String) -> () } end\n\
                  module M = effect Probe = { run: (() -> () + !Ask Nat) -> () } end";
    let (mint, out) = built(source);
    let probes: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity.clone())
        .collect();
    assert_eq!(probes.len(), 3);
    assert_ne!(probes[0], probes[1]);
    assert_eq!(probes[0], probes[2]);

    // An interface may refer back to its own parameterized effect.
    let (_, out) = built("effect Visit 'a = { run: (() -> 'a + !Visit 'a) -> 'a }");
    assert_eq!(out.program.effect_ids.len(), 1);
}

/// A parameterized alias is a way of writing a row: its arguments are
/// substituted into the effects it names, an effects parameter it ends in is
/// spliced as the row's tail, and every alias it names is expanded in turn.
#[test]
fn a_parameterized_alias_expands_to_the_row_it_writes() {
    let base = "effect Log = { write: Nat -> () }\n\
                effect IO = { print: Nat -> () }\n\
                effect Ask 'a = { get: () -> 'a }\n\
                effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
                effect Forward 'e = ..'e\n\
                effect Nested 'x 'e = !Both 'x (!IO + ..'e)\n";
    let (mint, out) = built(&format!(
        "{base}let f : () -> Nat + !Nested Nat (..'r) = fn _ => 0n\n\
         let g : () -> Nat + !Forward (!Log) = fn _ => 0n\n\
         let h : () -> Nat + !Both String (!IO) (when 'p) = fn _ => 0n\n\
         let i : () -> Nat + \\!Both String (!IO) + ..'e = fn _ => 0n"
    ));
    let row = |name: &str| {
        let term = &out.program.terms[&term_symbol(&mint, &out, name)];
        let TypeKind::Arrow { effects, .. } =
            &term.annotation.as_ref().expect("annotated").ty.tracked
        else {
            panic!("expected an arrow");
        };
        (*effects).clone()
    };
    let f = row("f");
    let names: Vec<_> = f.effects.keys().map(|id| id.name().to_string()).collect();
    assert_eq!(names, ["Ask", "Log", "IO"]);
    let ask = f.effects.values().next().expect("Ask");
    assert!(matches!(ask.args(), [arg] if matches!(arg.tracked, TypeKind::Prim(Prim::Nat))));
    assert!(ask.expanded());
    assert!(matches!(&f.tail, Some(Tail { of: Row::Named(name), .. }) if name == "r"));
    let g = row("g");
    assert_eq!(g.effects.len(), 1);
    assert!(g.tail.is_none());
    // A closed application takes a modifier, which distributes to every
    // effect it stands for.
    let h = row("h");
    assert!(h.effects.values().all(|label| label.when().is_some()));
    let i = row("i");
    assert_eq!(i.effects.len(), 3);
    assert!(
        i.effects
            .values()
            .all(|label| matches!(label, EffectLabel::Absent { .. }))
    );

    // The alias's own parameters are read off the row it comes to.
    let kind = |name: &str| {
        let symbol = *out
            .program
            .effects
            .keys()
            .find(|symbol| mint.name(**symbol) == name)
            .unwrap_or_else(|| panic!("no effect named {name}"));
        out.program.effects[&symbol]
            .params
            .iter()
            .map(|param| param.kind.sense())
            .collect::<Vec<_>>()
    };
    assert_eq!(kind("Both"), [Sense::Type, Sense::Effects]);
    assert_eq!(kind("Nested"), [Sense::Type, Sense::Effects]);
    assert_eq!(kind("Forward"), [Sense::Effects]);

    // A modifier on an application that stays open has no row to distribute
    // over; a type where the tail's row goes is not a row; a second tail is
    // one too many; and an alias declares no operation to perform.
    for (source, code) in [
        (
            "let f : () -> Nat + \\!Both Nat (..'e) + ..'e = fn _ => 0n",
            "modified-open-alias",
        ),
        (
            "let f : () -> Nat + !Both Nat (..'e) (when 'p) + ..'e = fn _ => 0n",
            "modified-open-alias",
        ),
        ("let f : () -> Nat + !Forward Nat = fn _ => 0n", "not-a-row"),
        (
            "let f : () -> Nat + !Both Nat (..'e) + ..'f = fn _ => 0n",
            "two-tails",
        ),
        ("let f = fn _ => !Both.get ()", "operation-on-alias"),
    ] {
        assert_eq!(codes_of(&format!("{base}{source}")), [code], "{source}");
    }

    // Every cycle of aliases is refused: one that only forwards, and one that
    // grows the row on the way round, each in its own words.
    assert_eq!(
        codes_of("effect A 'e = !B 'e\neffect B 'e = !A 'e"),
        ["alias-cycle"]
    );
    let (_, out) = build_src("effect Log = { write: Nat -> () }\neffect Grow 'e = !Log + !Grow 'e");
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "growing-alias-cycle");
    assert!(matches!(&error.kind, ErrorKind::AliasCycle { name, growing: true } if name == "Grow"));
    // A recursive concrete interface is not a cycle of aliases.
    assert!(
        build_src("effect Visit 'a = { run: (() -> 'a + !Visit 'a) -> 'a }")
            .1
            .errors
            .is_empty()
    );
    // Two branches reaching one constructor overlap whatever the arguments.
    assert_eq!(
        codes_of("effect Ask 'a = { get: () -> 'a }\neffect Twice 'a 'b = !Ask 'a + !Ask 'b"),
        ["duplicate-case"]
    );
}

/// An operation declaration keeps its operations in the order they were
/// written, each as the two sides of the plain closed arrow performing it has —
/// which is what a handler arm's binder and body type come off. The empty
/// effect declares nothing and is still an effect a row may name.
#[test]
fn an_effect_declares_its_operations() {
    let (mint, out) = built("effect Log = { write: Nat -> (), flush: () -> () }");
    let Effect::Operations(operations) = effect_of(&mint, &out, "Log") else {
        panic!("expected an operation declaration");
    };
    assert_eq!(
        operations.keys().collect::<Vec<_>>(),
        [
            &OperationSelector::Named("write".to_string()),
            &OperationSelector::Named("flush".to_string()),
        ]
    );
    let write = &operations[&OperationSelector::Named("write".to_string())];
    assert!(matches!(write.from.tracked, TypeKind::Prim(Prim::Nat)));
    assert!(matches!(
        write.to.tracked,
        TypeKind::Struct { ref fields, tail: None } if fields.is_empty()
    ));

    let (mint, out) = built("effect Nil");
    let Effect::Operations(operations) = effect_of(&mint, &out, "Nil") else {
        panic!("the empty effect declares operations, of which it has none");
    };
    assert!(operations.is_empty());
}

#[test]
fn recursive_types_have_a_finite_structural_effect_signature() {
    let (_, out) = built(
        "type Rec = { next: Rec }\n\
         effect Visit = { run: Rec -> Rec }",
    );
    assert_eq!(out.program.effect_ids.len(), 1);
}

/// An alias names effects and declares nothing, and expands to the effects it
/// names wherever a row mentions it — so no alias survives into what a
/// definition is checked against, and none can be performed through.
#[test]
fn an_alias_expands_to_the_effects_it_names() {
    let src = "effect Log = { write: Nat -> () }\n\
               effect IO = { print: Nat -> () }\n\
               effect Console = !Log + !IO\n\
               effect All = !Console\n\
               let f : Nat -> Nat + !All = fn x => x";
    let (mint, out) = built(src);
    let Effect::Alias(alias) = effect_of(&mint, &out, "Console") else {
        panic!("expected an alias");
    };
    assert_eq!(
        alias
            .body
            .cases
            .iter()
            .map(|case| mint.name(case.symbol))
            .collect::<Vec<_>>(),
        ["Log", "IO"]
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
    effects
        .effects
        .keys()
        .map(|effect| effect.name().to_string())
        .collect()
}

/// A bare `A -> B` carries the empty closed row, which is what pure means; the
/// `+ |` that spells it out lowers to the same thing and is told apart only by
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

    let (mint, out) = built("let f : Nat -> Nat + | = fn x => x");
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
        codes_of("effect Log = { write: Nat -> (), write: () -> () }"),
        ["duplicate-operation"]
    );
    let source = "effect IO = { print: Nat -> () }\neffect Log = { w: Nat -> () + !IO }";
    let (_, out) = build_src(source);
    assert!(matches!(
        out.errors.as_slice(),
        [error]
            if matches!(
                error.kind,
                ErrorKind::ImpureOperation {
                    found: OperationTypeProblem::Effects,
                }
            )
    ));

    // A bare tail or a conditional field leaves part of the signature open.
    for source in [
        "effect Log = { w: { x: Nat, .. } -> () }",
        "effect Log = { w: { x when 'a: Nat } -> () }",
    ] {
        let (_, out) = build_src(source);
        assert!(
            matches!(
                out.errors.as_slice(),
                [error]
                    if matches!(
                        error.kind,
                        ErrorKind::ImpureOperation {
                            found: OperationTypeProblem::OpenPart,
                        }
                    )
            ),
            "{source}: {:#?}",
            out.errors
        );
    }

    // A named tail is a variable an operation declaration cannot bind.
    let source = "effect Log = { w: Nat -> (#A | ..'r) }";
    let (_, out) = build_src(source);
    assert!(
        matches!(
            out.errors.as_slice(),
            [error]
                if matches!(
                    error.kind,
                    ErrorKind::ImpureOperation {
                        found: OperationTypeProblem::Variable(ref name),
                    } if name == "r"
                )
        ),
        "{source}: {:#?}",
        out.errors
    );
    // A repeated effect name is a duplicate like any other, in its own
    // namespace.
    let out = build_src("effect Log\neffect Log").1;
    assert_eq!(out.errors[0].kind.code(), "duplicate-effect");
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Duplicate {
            ref name,
            namespace: Namespace::Effects,
            ..
        } if name == "Log"
    ));
}

/// An effect and a type of one name are two unrelated things, which is what a
/// namespace of its own buys.
#[test]
fn effects_have_a_namespace_of_their_own() {
    let (mint, out) = built(
        "type Log = Nat\neffect Log = { write: Nat -> () }\nlet f : Log -> Log + !Log = fn x => x",
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
    let base = "effect Log = { write: Nat -> () }\n\
                effect IO = { print: Nat -> () }\n\
                effect Console = !Log + !IO\n";
    let (mint, out) = built(&format!("{base}let g = fn _ => !Log.write 1n"));
    let mut node = term_value(&mint, &out, "g");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Apply { func, .. } = node else {
        panic!("expected an application");
    };
    let TermKind::Operation { effect, selector } = &func.kind else {
        panic!("expected an operation, got {:?}", func.kind);
    };
    assert_eq!(mint.name(effect.tracked), "Log");
    assert_eq!(
        selector.tracked,
        OperationSelector::Named("write".to_string())
    );

    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => !Lg.write 1n")),
        ["undefined-effect"]
    );
    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => !Log.writ 1n")),
        ["unknown-operation"]
    );
    assert_eq!(
        codes_of(&format!("{base}let g = fn _ => !Console.write 1n")),
        ["operation-on-alias"]
    );
    assert_eq!(
        codes_of(&format!("{base}let g = !Log")),
        ["bare-operation-unavailable"]
    );
    assert_eq!(
        codes_of("effect Log = Nat -> ()\nlet g = !Log.write"),
        ["named-operation-on-unnamed"]
    );
    assert_eq!(
        codes_of("effect Nil\nlet g = !Nil"),
        ["bare-operation-unavailable"]
    );

    let (mint, out) = built(
        "module Sys = effect Log = Nat -> () end\n\
         let g = Sys::!Log",
    );
    let TermKind::Operation { selector, .. } = term_value(&mint, &out, "g") else {
        panic!("expected qualified bare operation")
    };
    assert_eq!(selector.tracked, OperationSelector::Unnamed);
    assert_eq!(
        codes_of(
            "effect Log = Nat -> ()\n\
             let g = handle 0n with | !Log x => () | !Log y => () end"
        ),
        ["duplicate-arm"]
    );
}

/// Which effects a handler discharges is settled here, before inference: every
/// effect an arm names must have an arm for each of its operations, and a
/// half-covered one leaves the set undecidable.
#[test]
fn a_handler_must_cover_every_effect_it_names() {
    let base = "effect Log = { write: Nat -> (), flush: () -> () }\n\
                let p : () -> Nat + !Log = fn _ => 0n\n";
    let (mint, out) = built(&format!(
        "{base}let h = fn _ => handle p () with | !Log.write s => () | !Log.flush u => () end"
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
        "{base}let h = fn _ => handle p () with | !Log.write s => () end"
    ));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "partial-handler");
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.title,
        "this handler does not cover effect `!Log`"
    );
    assert_eq!(diagnostic.primary.message, "missing an arm for `flush`");
    assert_eq!(diagnostic.help, ["add an arm for `flush`"]);
    assert!(matches!(
        error.kind,
        ErrorKind::PartialHandler { ref missing, .. } if missing == &["flush".to_string()]
    ));

    // Equivalent declarations are one semantic interface. Its coverage may be
    // split across their source spellings, and complete coverage discharges one
    // representative row label.
    let structural = "module Foo =\n  effect Log = { write: Nat -> (), flush: () -> () }\nend\n\
                      module Bar =\n  effect Log = { write: Nat -> (), flush: () -> () }\nend\n\
                      let p : () -> Nat + Foo::!Log = fn _ => 0n\n\
                      let h = fn _ => handle p () with | Foo::!Log.write n => () | Bar::!Log.flush u => () end";
    let (mint, out) = built(structural);
    let mut node = term_value(&mint, &out, "h");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected a structural handler");
    };
    assert_eq!(handler.arms.len(), 2);
    assert_ne!(
        handler.arms[0].effect.tracked,
        handler.arms[1].effect.tracked
    );
    assert_eq!(handler.discharges.len(), 1);
    assert_eq!(mint.name(handler.discharges[0].tracked), "Log");
}

/// A duplicate arm and a second `return` arm are refused where a duplicate
/// anything else is: at the repeat, with the first the one that stands.
#[test]
fn a_handler_takes_each_arm_once() {
    let base = "effect Log = { write: Nat -> () }\nlet p : () -> Nat + !Log = fn _ => 0n\n";
    let (_, out) = build_src(&format!(
        "{base}let h = fn _ => handle p () with | !Log.write s => () | !Log.write t => () end"
    ));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "duplicate-arm");
    assert_eq!(error.kind.to_string(), "duplicate arm for `!Log.write`");

    let bare = "effect Log = Nat -> ()\n\
                let h = fn n => handle !Log n with | !Log _ => () | !Log _ => () end";
    let (_, out) = build_src(bare);
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.to_string(), "duplicate arm for `!Log`");
    assert_eq!(OperationSelector::Unnamed.source_name(), "<unnamed>");

    // Source paths resolve operations, but equivalent interfaces are one
    // effect in rows, so their operation arms must not choose competing
    // implementations for the same evidence entry.
    let structural = "module Foo =\n  effect Log = { write: Nat -> () }\nend\n\
                      module Bar =\n  effect Log = { write: Nat -> () }\nend\n\
                      let h = fn n => handle Foo::!Log.write n with | Foo::!Log.write _ => () | Bar::!Log.write _ => () end";
    let (_, out) = build_src(structural);
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "duplicate-arm");
    assert_eq!(error.kind.to_string(), "duplicate arm for `!Log.write`");

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
    let base = "effect Log = { write: Nat -> () }\nlet p : () -> Nat + !Log = fn _ => 0n\n";
    let arm =
        |body: &str| format!("{base}let h = fn _ => handle p () with | !Log.write s => {body} end");
    // No arm anywhere.
    assert_eq!(
        codes_of(&format!("{base}let a = raise 1n")),
        ["raise-outside-arm"]
    );
    assert_eq!(
        codes_of(&format!("{base}let a = fn _ => raise 1n")),
        ["raise-outside-arm"]
    );
    // The handled expression of a handler with no arm above it is the ordinary
    // "no enclosing arm" case.
    assert_eq!(
        codes_of(&format!("{base}let a = fn _ => handle raise 1n with end")),
        ["raise-outside-arm"]
    );

    // A `fn` between the `raise` and its arm — including one applied on the
    // spot, since what matters is that a closure was written at all.
    assert_eq!(
        codes_of(&arm("{ g: fn _ => raise 0n }")),
        ["raise-in-function"]
    );
    assert_eq!(
        codes_of(&arm("(fn _ => raise 0n) 1n")),
        ["raise-in-function"]
    );

    // And every position an expression may sit in, none of which needs a rule.
    for body in [
        "raise 0n",
        "let a = raise 0n in ()",
        "f (raise 0n)",
        "{ g: raise 0n }",
        "match s with | 0n => raise 0n | _ => () end",
        // A nested `handle`'s *body* keeps the outer arm: the outer handler is
        // still on the stack while the inner one runs.
        "handle raise 0n with end",
    ] {
        let src = format!(
            "{base}let f = fn x => x\nlet h = fn _ => handle p () with | !Log.write s => {body} end"
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
            "{base}let h = fn _ => handle p () with | !Log.write s => () | return x => raise x end"
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
        "effect Log = { write: Nat -> () }\n\
         type Logger = Nat -> Nat + !Log\n\
         type Runner 'e = (Nat -> Nat + ..'e) -> Nat + ..'e",
    );
    let TypeKind::Arrow { effects, .. } = &out.program.types[&type_symbol(&mint, &out, "Logger")]
        .value
        .tracked
    else {
        panic!("expected an arrow");
    };
    assert_eq!(
        effects
            .effects
            .keys()
            .map(|effect| effect.name())
            .collect::<Vec<_>>(),
        ["Log"]
    );

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
    let base = "effect Log = { write: Nat -> () }\n";
    for source in [
        "type T = Nat -> Nat + ..",
        "type T = Nat -> Nat + !Log (when 'a)",
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
    let out = build_src(&format!("{base}type T = Nat -> Nat + ..")).1;
    let diagnostic = out.errors[0].diagnostic();
    assert_eq!(
        diagnostic.title,
        "a declared type must list its effects exactly"
    );
    assert_eq!(
        diagnostic.primary.message,
        "this leaves part of the declared type undecided"
    );
    assert_eq!(
        diagnostic.help,
        [
            "list every label, use one of the declaration's parameters, or move this type to an annotation"
        ]
    );
}

/// One name is one rest, and an arrow's effects are a third thing a rest can
/// be — so a parameter used as an effect tail in one place and as a type in
/// another is the mixed parameter it always was, with the third reading named.
#[test]
fn a_parameter_read_two_ways_names_both() {
    let src = "effect Log = { write: Nat -> () }\n\
               type M 'e = { f: 'e, g: (Nat -> Nat + ..'e) }";
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
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.title,
        "this parameter is used as a whole type and as the rest of an arrow's effects"
    );
    assert_eq!(
        diagnostic.primary.message,
        "some uses need a whole type, while others need the rest of an arrow's effects"
    );
    assert_eq!(
        diagnostic.help,
        ["use a separate parameter name for each purpose"]
    );

    // And a name given two rests in one *annotation* is the same mistake.
    let src = "effect Log = { write: Nat -> () }\n\
               let f : { x: Nat, ..'r } -> Nat -> Nat + ..'r = fn p => fn n => n";
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
/// `\y` and a sum's `\#B` do: a row with no tail already says every effect
/// it does not name is not performed.
#[test]
fn an_absent_effect_needs_a_tail() {
    let src = "effect Log = { write: Nat -> () }\nlet f : Nat -> Nat + \\!Log = fn x => x";
    let (_, out) = build_src(src);
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "absent-in-closed");
    assert_eq!(
        error.kind.to_string(),
        "a type with no `..` already says `!Log` is not there"
    );
}

/// An effect named twice in one row is a duplicate, however it got there — the
/// two names of an alias that overlap another label included.
#[test]
fn an_effect_row_names_each_effect_once() {
    let src = "effect Log = { write: Nat -> () }\nlet f : Nat -> Nat + !Log + !Log = fn x => x";
    assert_eq!(codes_of(src), ["duplicate-case"]);
}

#[test]
fn structural_identity_distinguishes_empty_unnamed_and_named_effects() {
    let (mint, out) = built(
        "module Empty = effect E end\n\
         module Unnamed = effect E = Nat -> () end\n\
         module Named = effect E = { op: Nat -> () } end\n\
         effect All = Empty::!E + Unnamed::!E + Named::!E",
    );
    let ids: Vec<_> = out
        .program
        .effects
        .keys()
        .filter(|symbol| mint.name(**symbol) == "E")
        .map(|symbol| &out.program.effect_ids[symbol])
        .collect();
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
    assert_ne!(ids[1], ids[2]);
}

/// Structural identities coalesce same-named, same-interface declarations,
/// including aliases in their operation signatures; a row or alias cannot name
/// that one semantic effect twice.
#[test]
fn structural_effect_duplicates_are_rejected() {
    let src = "type Payload = Nat\n\
               module Foo =\n  effect Log = { write: Payload -> () }\nend\n\
               module Bar =\n  effect Log = { write: Nat -> () }\nend\n\
               effect Both = Foo::!Log + Bar::!Log\n\
               let f : Nat -> Nat + Foo::!Log + Bar::!Log = fn n => n";
    let (_, out) = build_src(src);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-case", "duplicate-case"]
    );
}

/// Effect identity is equality of regular trees, not equality of how many
/// times a recursive declaration was unrolled before its back edge.
#[test]
fn recursive_effect_interfaces_ignore_finite_unrolling() {
    let src = "module A =\n  type a = { n: a }\n  effect Loop = { step: a -> () }\nend\n\
               module B =\n  type b = { n: { n: b } }\n  effect Loop = { step: b -> () }\nend\n\
               effect Both = A::!Loop + B::!Loop";
    assert_eq!(codes_of(src), ["duplicate-case"]);
}

/// A back edge retains which regular-tree state it returns to. In particular,
/// a branch at the root cannot be confused with a loop at the root's child.
#[test]
fn recursive_effect_interfaces_preserve_branching_back_edges() {
    let src = "module A =\n  type a = { left: middle, right: Nat }\n  type middle = { next: a }\n  effect Loop = { step: a -> () }\nend\n\
               module B =\n  type b = { left: middle, right: Nat }\n  type middle = { next: middle }\n  effect Loop = { step: b -> () }\nend\n\
               effect Both = A::!Loop + B::!Loop";
    let (_, out) = build_src(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

#[test]
fn transitive_alias_overlap_is_rejected_at_the_overlapping_case() {
    let src = "effect Log = { write: Nat -> () }\n\
               effect A = !Log\n\
               effect B = !Log\n\
               effect Both = !A + !B";
    let (_, out) = build_src(src);
    assert_eq!(codes_of(src), ["duplicate-case"]);
    assert_eq!(out.errors[0].span.start, src.rfind("!B").unwrap());
}

/// Effect rows inside an operation's input/output signature participate in
/// identity too. Their dependencies are compared structurally, not by the
/// source module that declared them.
#[test]
fn effectful_operation_signatures_compare_their_dependencies_structurally() {
    let src = "module Foo =\n  effect IO = { run: () -> () }\n  effect Log = { call: (() -> Nat + !IO) -> () }\nend\n\
               module Bar =\n  effect IO = { run: () -> () }\n  effect Log = { call: (() -> Nat + !IO) -> () }\nend\n\
               module Other =\n  effect Other = { run: () -> () }\nend\n\
               module Quux =\n  effect Log = { call: (() -> Nat + Other::!Other) -> () }\nend";
    let (mint, out) = build_src(src);
    // Impure operation signatures remain rejected, but their recovered
    // interfaces must still be complete enough to distinguish effects.
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. }))
    );
    let logs: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Log")
        .map(|(_, id)| id)
        .collect();
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0], logs[1]);
    assert_ne!(logs[0], logs[2]);
}

/// Same-named effects with incompatible interfaces remain distinct even when
/// their declarations live in different modules.
#[test]
fn same_named_effects_in_different_modules_do_not_collide() {
    let src = "module A =\n  effect Log = { write: Nat -> () }\nend\n\
               module B =\n  effect Log = { flush: () -> () }\nend\n\
               effect Both = A::!Log + B::!Log\n\
               let f : Nat -> Nat + A::!Log + B::!Log = fn x => x";
    let (_, out) = build_src(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let (mint, out) = built(src);
    let Effect::Alias(alias) = effect_of(&mint, &out, "Both") else {
        panic!("expected an alias");
    };
    assert_eq!(alias.body.cases.len(), 2);
}

/// An alias's cases are effect names and are resolved like any other: one that
/// names nothing is undefined, and one named twice is a duplicate. Both are
/// dropped where they stood, so the declaration still stands for whatever else
/// it named.
#[test]
fn an_aliass_cases_are_resolved_and_deduplicated() {
    assert_eq!(
        codes_of("effect Log = { write: Nat -> () }\neffect Console = !Log + !Nope"),
        ["undefined-effect"]
    );
    let (mint, out) = build_src("effect Log = { write: Nat -> () }\neffect Console = !Log + !Log");
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["duplicate-case"]
    );
    let Effect::Alias(alias) = effect_of(&mint, &out, "Console") else {
        panic!("expected an alias");
    };
    assert_eq!(alias.body.cases.len(), 1);
}

/// An effect a row names has to be one: an unknown name is dropped from the
/// row where it stood, with the undefined-name complaint in the effect
/// namespace, and whatever else the row named still stands.
#[test]
fn a_row_may_only_name_declared_effects() {
    let (mint, out) =
        build_src("effect Log = { write: Nat -> () }\nlet f : Nat -> Nat + !Log + !Lg = fn x => x");
    assert_eq!(
        out.errors.iter().map(|e| e.kind.code()).collect::<Vec<_>>(),
        ["undefined-effect"]
    );
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Effects,
        } if name == "Lg"
    ));
    assert_eq!(annotation_effects(&mint, &out, "f"), ["Log"]);
}

/// An arm whose operation does not resolve is dropped, so the effect it named
/// is not counted as covered and nothing downstream is told a second time.
#[test]
fn an_arm_that_does_not_resolve_covers_nothing() {
    let (mint, out) = build_src(
        "effect Log = { write: Nat -> () }\n\
         let p : () -> Nat + !Log = fn _ => 0n\n\
         let h = fn _ => handle p () with | !Log.writ s => () end",
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
    let codes = codes_of("effect Log = { write: Nat -> () }\ntype T = Nat -> Nat + \\!Log");
    assert_eq!(codes, ["absent-in-closed"]);
}

/// A signature that failed to lower is already a complaint, so it is not told a
/// second time that it is not a function: the error type absorbs, here as
/// everywhere.
#[test]
fn a_signature_that_did_not_lower_is_not_told_twice() {
    assert_eq!(
        codes_of("effect Log = { write: Bogus -> () }"),
        ["undefined-type"]
    );

    // Lowering remains total when a consumer deliberately passes the retained
    // AST from a failed parse through the public API.
    let parsed = parse::parse(lex("effect Log = { write: Nat }", FileID::GENERATED).tokens);
    assert_eq!(parsed.errors.len(), 1);
    let mut mint = dummy_mint();
    let out = build(&mut mint, parsed.stmts);
    assert_eq!(out.errors[0].kind.code(), "not-an-operation");

    let parsed = parse::parse(lex("effect Log = { write: Bogus }", FileID::GENERATED).tokens);
    assert_eq!(parsed.errors.len(), 1);
    let mut mint = dummy_mint();
    let out = build(&mut mint, parsed.stmts);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["undefined-type"]
    );
}

/// A row that names nothing may still say something: `+ ..'e` ends in a tail,
/// so the arrow may perform whatever the tail stands for, and only a row with
/// neither is the pure one.
#[test]
fn a_row_that_is_only_a_tail_says_something() {
    let (mint, out) = built("let f : Nat -> Nat + ..'e = fn x => x");
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
/// type built out of one through a `+` grows exactly as one handing it on
/// through a field does.
#[test]
fn an_effect_tail_makes_an_argument_grow() {
    let codes = codes_of(
        "effect Log = { write: Nat -> () }\n\
         type T 'a = { next: T (Nat -> Nat + ..'a) }",
    );
    assert_eq!(codes, ["growing-recursion"]);
}

/// `return` is contextual: it heads a handler arm and is an ordinary name
/// everywhere 'else, so a definition may still be called one.
#[test]
fn return_is_a_name_everywhere_but_an_arms_head() {
    let (mint, out) = built(
        "effect Log = { write: Nat -> () }\n\
         let return = 1n\n\
         let p : () -> Nat + !Log = fn _ => return\n\
         let h = fn _ => handle p () with | !Log.write s => () | return x => x end",
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
    let base = "effect Log = { write: Nat -> () }\n\
                type Runner 'e = (Nat -> Nat + !Log + ..'e) -> Nat\n";
    // Something a row cannot hold.
    let (_, out) = build_src(&format!("{base}let f : Runner Nat -> Nat = fn r => 1n"));
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
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.title,
        "this argument must provide the rest of an arrow's effects"
    );
    assert_eq!(
        diagnostic.primary.message,
        "this type cannot provide the rest of an arrow's effects"
    );

    // And a row naming what the declaration already names: the `..` covers
    // only what its own row leaves out.
    let (_, out) = build_src(&format!("{base}let f : Runner (!Log) -> Nat = fn r => 1n"));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "repeated-row-field");
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.title,
        "`!Log` would appear twice in this function"
    );
    assert_eq!(
        diagnostic.primary.message,
        "the declaration and this argument both provide this name"
    );
    assert_eq!(
        diagnostic.help,
        ["remove or rename the repeated field or case"]
    );
}

/// A row of effects written as an argument: the one position a row reaches
/// without an arrow to carry it, and the reason effects have a sigil of their
/// own to be written with there.
///
/// Resolved exactly as an arrow's row is — the names are effects, an alias
/// expands to what it stands for, and a `\` and a `..` mean what they mean on
/// an arrow — because it goes through the same lowering.
#[test]
fn a_row_of_effects_may_be_an_argument() {
    let base = "effect Log = { write: Nat -> () }\n\
                effect IO = { print: Nat -> () }\n\
                effect Console = !Log + !IO\n\
                type Runner 'e = (Nat -> Nat + ..'e) -> Nat\n";

    // One effect, written back as the row it is.
    let (mint, out) = build_src(&format!("{base}let f : Runner (!Log) -> Nat = fn r => 1n"));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let printed = print::ir::program(&out.program, &mint).to_string();
    assert!(
        printed.contains("let f : Runner (!Log) -> Nat"),
        "{printed}"
    );

    // An alias expands where it is written, as it does on an arrow.
    let (mint, out) = build_src(&format!(
        "{base}let g : Runner (!Console) -> Nat = fn r => 1n"
    ));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let printed = print::ir::program(&out.program, &mint).to_string();
    assert!(
        printed.contains("let g : Runner (!Log + !IO) -> Nat"),
        "{printed}"
    );

    // A row of several, one of them written absent, and a tail beside them.
    let (_, out) = build_src(&format!(
        "{base}let h : Runner (!Log + \\!IO + ..'r) -> Nat = fn q => 1n"
    ));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    // And the names in one are effects: an undefined one is reported where it
    // was written, the way a row on an arrow reports it.
    let (_, out) = build_src(&format!("{base}let i : Runner (!Nope) -> Nat = fn r => 1n"));
    let [error] = &out.errors[..] else {
        panic!("{:#?}", out.errors);
    };
    assert_eq!(error.kind.code(), "undefined-effect");

    // A row of nothing but a rest: the variable an annotation declared, handed
    // to the declaration by itself. Every other spelling names an effect
    // beside it, and there is nothing to name here.
    let (mint, out) = build_src(&format!(
        "{base}let rest : Runner (..'r) -> Nat = fn q => 1n"
    ));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let printed = print::ir::program(&out.program, &mint).to_string();
    assert!(
        printed.contains("let rest : Runner (..'r) -> Nat"),
        "{printed}"
    );

    // A declaration may hand one on, tail and all: what the tail names is used
    // as effects, exactly as a tail on the declaration's own arrow would be.
    let (mint, out) = build_src(&format!("{base}type Wrap 'e = Runner (!Log + ..'e)"));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let printed = print::ir::program(&out.program, &mint).to_string();
    assert!(
        printed.contains("type Wrap 'e = Runner (!Log + ..'e)"),
        "{printed}"
    );

    // An operation's signature carries no effects, and a row written as an
    // argument in one is exactly that: refused, and the argument absorbs.
    let (_, out) = build_src(&format!(
        "{base}effect Bad = {{ op: Runner (!Log + ..) -> Nat }}"
    ));
    assert!(
        out.errors
            .iter()
            .any(|error| error.kind.code() == "impure-operation"),
        "{:#?}",
        out.errors
    );

    // A declaration says the same thing wherever it is used, so a row written
    // open in one is refused there too — and the argument absorbs, the way a
    // refused struct or sum argument does.
    let (_, out) = build_src(&format!("{base}type Open = Runner (!Log + ..)"));
    assert!(
        out.errors
            .iter()
            .any(|error| error.kind.code() == "open-declared-type"),
        "{:#?}",
        out.errors
    );

    // A row handed round a recursion unchanged is not growth: what a
    // declaration hands itself has to get bigger to be one, and a row with no
    // tail to splice into is the same row every time round.
    let (_, out) = build_src(&format!("{base}type Same 'e = {{ next: Same (!Log) }}"));
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    // And a row handed round a recursion grows, exactly as a struct handed on
    // with a field added does: the declaration would never come back round.
    let (_, out) = build_src(&format!(
        "{base}type Loop 'e = {{ next: Loop (!Log + ..'e) }}"
    ));
    assert!(
        out.errors
            .iter()
            .any(|error| error.kind.code() == "growing-recursion"),
        "{:#?}",
        out.errors
    );
}

/// A row is not a type, so writing one anywhere 'but an argument is refused
/// where it stands — and the effects it names are still resolved, so a reader
/// who wrote one wrong is told about that too.
#[test]
fn a_row_of_effects_is_refused_where_a_type_goes() {
    let base = "effect Log = { write: Nat -> () }\n";
    for source in [
        "let x : !Log = 1n",
        "let x : { f: !Log } = { f: 1n }",
        "type T = !Log",
        "let x : !Log -> Nat = fn v => 1n",
    ] {
        let (_, out) = build_src(&format!("{base}{source}"));
        assert!(
            out.errors
                .iter()
                .any(|error| error.kind.code() == "effects-outside-row"),
            "{source}: {:#?}",
            out.errors
        );
    }

    // The row is read for its own complaints before it is refused.
    let (_, out) = build_src(&format!("{base}let x : !Nope = 1n"));
    let codes: Vec<&str> = out.errors.iter().map(|e| e.kind.code()).collect();
    assert_eq!(codes, ["undefined-effect", "effects-outside-row"]);
}

/// A bare name in a type resolves in one order and no other: a declaration's
/// parameter, a `where 'let` variable, a declared type, a primitive. The first
/// two never share a scope — a declaration has no `where 'let` — so what the
/// order really decides is that a declared variable shadows a declared type and
/// the built-in alike, for the extent of its own annotation.
#[test]
fn a_bare_name_resolves_in_one_order() {
    // A declared type, which is what a bare name has always been.
    let (mint, out) = built("type T = Nat  let f : T -> Nat = fn x => 0n");
    let annotation = annotation_of(&mint, &out, "f");
    let TypeKind::Arrow { from, .. } = &annotation.ty.tracked else {
        panic!("expected an arrow");
    };
    assert!(matches!(from.tracked, TypeKind::Ident(_)));

    // The same spelling with a sigil is a variable, and the two sit side by
    // side: nothing a variable is spelled like can shadow it or be shadowed.
    let (mint, out) = built("type T = Nat  let f : 'T -> T = fn x => 0n");
    let annotation = annotation_of(&mint, &out, "f");
    let TypeKind::Arrow { from, to, .. } = &annotation.ty.tracked else {
        panic!("expected an arrow");
    };
    assert!(matches!(&from.tracked, TypeKind::Var(name) if name == "T"));
    assert!(matches!(to.tracked, TypeKind::Ident(_)));

    // And the built-in goes the same way.
    let (mint, out) = built("let f : 'Nat -> Nat = fn x => 0n");
    let annotation = annotation_of(&mint, &out, "f");
    let TypeKind::Arrow { from, to, .. } = &annotation.ty.tracked else {
        panic!("expected an arrow");
    };
    assert!(matches!(&from.tracked, TypeKind::Var(name) if name == "Nat"));
    assert!(matches!(to.tracked, TypeKind::Prim(_)));

    // A declaration's parameter still comes first, and a declaration has no
    // variables at all for it to be shadowed by.
    let (mint, out) = built("type Box 'a = { it: 'a }");
    let symbol = type_symbol(&mint, &out, "Box");
    let TypeKind::Struct { fields, .. } = &out.program.types[&symbol].value.tracked else {
        panic!("expected a struct");
    };
    let TypeField::Written { value, .. } = &fields["it"] else {
        panic!("expected a written field");
    };
    assert!(matches!(value.tracked, TypeKind::Param { .. }));
}

/// A bare name in a type position resolves to something declared elsewhere —
/// and a name nothing declared stands for nothing at all. The sigil is what
/// buys that back: `'a` needs no declaration because it is not a name of
/// anything.
#[test]
fn an_undeclared_name_is_undefined_wherever_it_is_used() {
    // A type position, twice — one report per use.
    let src = "let bad : a -> a = fn x => x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 2, "{src}: {:#?}", out.errors);
    for error in &out.errors {
        assert!(
            matches!(
                error.kind,
                ErrorKind::Undefined {
                    ref name,
                    namespace: Namespace::Types,
                } if name == "a"
            ),
            "{src}: {:#?}",
            out.errors
        );
    }

    // A `..` and a `when` take no bare name at all: each stands for something
    // the sigil introduces, so there is nothing there for a name to resolve to
    // and the parser never builds one.
    for src in [
        "let bad : { x: Nat, ..r } -> Nat = fn p => p.x",
        "let bad : (#A Nat | ..r) -> Nat = fn p => 0n",
        "let bad : { x when a: Nat } -> Nat = fn p => 0n",
    ] {
        let out = ruddy::parse::parse(ruddy::token::lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }

    // A sigilled name is a variable wherever it is written, and needs nothing.
    let (_, out) = build_src("let good : 'a -> { x: Nat, ..'r } -> Nat = fn x => fn p => p.x");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

#[test]
fn a_declared_variable_takes_its_sort_from_its_uses() {
    for (src, sense) in [
        ("let f : 'a -> 'a = fn x => x", Sense::Type),
        (
            "let f : { x: Nat, ..'a } -> Nat = fn p => p.x",
            Sense::Fields,
        ),
        ("let f : (#A Nat | ..'a) -> Nat = fn p => 0n", Sense::Cases),
        (
            "let f : { x when 'a: Nat } -> Nat = fn p => 0n",
            Sense::Presence,
        ),
    ] {
        let (mint, out) = built(src);
        let annotation = annotation_of(&mint, &out, "f");
        let [variable] = annotation.variables.as_slice() else {
            panic!("{src}: expected one variable: {:#?}", annotation.variables);
        };
        assert_eq!(variable.name, "a", "{src}");
        assert_eq!(variable.sense, sense, "{src}");
        assert_eq!(variable.span.start, src.find("'a").expect("the first use"));
    }

    // Several of them, in the order they were declared, each with the sort its
    // own uses gave it — and each with an id nothing else in the program has.
    let src = "let f : { x when 'a: 'c, ..'b } -> 'c = fn p => p.x";
    let (mint, out) = built(src);
    let annotation = annotation_of(&mint, &out, "f");
    let senses: Vec<(&str, Sense)> = annotation
        .variables
        .iter()
        .map(|variable| (variable.name.as_str(), variable.sense))
        .collect();
    assert_eq!(
        senses,
        [
            ("a", Sense::Presence),
            ("c", Sense::Type),
            ("b", Sense::Fields)
        ]
    );
    let ids: Vec<u32> = annotation.variables.iter().map(|v| v.id).collect();
    assert_eq!(ids.len(), 3);
    assert!(
        ids.iter()
            .all(|id| ids.iter().filter(|o| *o == id).count() == 1)
    );

    // And two annotations that each write `a` declare two variables, however
    // alike they look.
    let (mint, out) = built(
        "let f : 'a -> 'a = fn x => x\n\
         let g : 'a -> 'a = fn x => x",
    );
    let one = annotation_of(&mint, &out, "f").variables[0].id;
    let other = annotation_of(&mint, &out, "g").variables[0].id;
    assert_ne!(one, other);
}

/// One variable stands for one thing, and there are three things it could be.
/// A use that disagrees with the first one is refused where it was written,
/// naming both readings and pointing back at the use that decided it.
#[test]
fn a_declared_variable_used_at_two_sorts_is_refused() {
    for (src, first, second) in [
        (
            "let f : { x: Nat, ..'a } -> (#A Nat | ..'a) -> Nat = fn p => fn q => 0n",
            Sense::Fields,
            Sense::Cases,
        ),
        (
            "let f : (#A Nat | ..'a) -> { x: Nat, ..'a } -> Nat = fn p => fn q => 0n",
            Sense::Cases,
            Sense::Fields,
        ),
        (
            "let f : { x when 'a: Nat } -> 'a = fn r => r",
            Sense::Presence,
            Sense::Type,
        ),
        (
            "let f : 'a -> { x when 'a: Nat } = fn r => r",
            Sense::Type,
            Sense::Presence,
        ),
        (
            "let f : (#A Nat | ..'a) -> { x when 'a: Nat } = fn r => r",
            Sense::Cases,
            Sense::Presence,
        ),
        (
            "let f : { x when 'a: Nat } -> (#A Nat | ..'a) = fn r => r",
            Sense::Presence,
            Sense::Cases,
        ),
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src}: {:#?}", out.errors);
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::MixedTail {
                    first: one,
                    second: two,
                    ..
                } if one == first && two == second
            ),
            "{src}: {:#?}",
            out.errors
        );
    }

    // A name in a formula is a presence use, since a formula is written about
    // presences and nothing else.
    let src = "let f : 'a -> 'a where 'a = fn x => x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::MixedTail {
                first: Sense::Type,
                second: Sense::Presence,
                ..
            }
        ),
        "{:#?}",
        out.errors
    );
}

/// One annotation's `a` is one variable, however often it is written: the
/// first use mints it and every use after finds it, which is what makes two
/// `..'r` in one annotation stand for one rest and two `when a` for one
/// presence.
///
/// Two annotations that each write `a` write two variables. The scope is the
/// one annotation, which is the whole of what a variable's scope ever was.
#[test]
fn a_variable_is_minted_by_its_first_use() {
    let (mint, out) = build_src("let f : 'a -> 'a = fn x => x");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let annotation = annotation_of(&mint, &out, "f");
    assert_eq!(annotation.variables.len(), 1);
    assert_eq!(annotation.variables[0].name, "a");
    // Spanned at the first use, which is where a complaint about what the rest
    // of the annotation did with it points back to.
    assert_eq!(
        annotation.variables[0].span.start,
        "let f : ".len(),
        "{:#?}",
        annotation.variables[0]
    );

    let (mint, out) = build_src(
        "let f : 'a -> 'a = fn x => x\n\
         let g : 'a -> 'a = fn x => x",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let one = &annotation_of(&mint, &out, "f").variables[0];
    let other = &annotation_of(&mint, &out, "g").variables[0];
    assert_eq!(one.name, other.name);
    assert_ne!(one.id, other.id);
}

/// Presence ownership follows the complete annotation's polarity. Results are
/// producer-chosen, inputs remain caller-chosen, and two arrow reversals make a
/// presence positive again. A curried result is owned at its final result
/// boundary rather than at the whole function.
#[test]
fn presence_ownership_is_inferred_from_polarity_and_result_boundaries() {
    for (source, expected) in [
        ("let value : { x when 'p: Nat } = { x: 0n }", "existential"),
        (
            "let take : { x when 'p: Nat } -> Nat = fn x => 0n",
            "universal",
        ),
        (
            "let keep : { x when 'p: Nat } -> { x when 'p: Nat } = fn x => x",
            "universal",
        ),
        (
            "let twice : ({ x when 'p: Nat } -> Nat) -> Nat = fn f => 0n",
            "existential",
        ),
    ] {
        let (mint, out) = built(source);
        let variable = &annotation_of(&mint, &out, source[4..].split_whitespace().next().unwrap())
            .variables[0];
        match (expected, variable.ownership) {
            ("existential", PresenceOwnership::Existential { .. })
            | ("universal", PresenceOwnership::Universal) => {}
            _ => panic!(
                "{source}: expected {expected}, got {:?}",
                variable.ownership
            ),
        }
    }

    let source = "let choose : Nat -> Boolean -> { x when 'p: Nat } = fn n => fn b => { x: n }";
    let (mint, out) = built(source);
    let annotation = annotation_of(&mint, &out, "choose");
    let PresenceOwnership::Existential { boundary } = annotation.variables[0].ownership else {
        panic!("the final result presence should be producer-chosen");
    };
    // The package starts at the final struct result, not at either arrow.
    assert_eq!(
        &source[boundary.start..boundary.end()],
        "{ x when 'p: Nat }"
    );
}

#[test]
fn anonymous_presences_have_distinct_positive_package_owners() {
    let source = "let choose : { x when _: Nat, y when _: Nat } = { x: 0n, y: 0n }";
    let (mint, out) = built(source);
    let annotation = annotation_of(&mint, &out, "choose");
    assert_eq!(annotation.anonymous_existentials.len(), 2);
    assert_ne!(
        annotation.anonymous_existentials[0].0,
        annotation.anonymous_existentials[1].0
    );
    assert_eq!(
        annotation.anonymous_existentials[0].1,
        annotation.anonymous_existentials[1].1
    );

    let source = "let take : { x when _: Nat } -> Nat = fn x => 0n";
    let (mint, out) = built(source);
    assert!(
        annotation_of(&mint, &out, "take")
            .anonymous_existentials
            .is_empty()
    );
}

#[test]
fn alias_parameter_variance_reaches_annotation_ownership_fixpoint() {
    for (declarations, use_ty, existential) in [
        (
            "type Cov 'a = { value: 'a }",
            "Cov { x when 'p: Nat }",
            true,
        ),
        (
            "type Contra 'a = 'a -> Nat",
            "Contra { x when 'p: Nat }",
            false,
        ),
        (
            "type Inv 'a = { value: 'a -> 'a }",
            "Inv { x when 'p: Nat }",
            false,
        ),
        ("type Erased 'a = Nat", "Erased { x when 'p: Nat }", false),
        (
            "type A 'a = { next: B 'a }\ntype B 'b = { next: A 'b, value: 'b }",
            "A { x when 'p: Nat }",
            true,
        ),
    ] {
        let source = format!("{declarations}\nlet value : {use_ty} = 0n");
        let (mint, out) = build_src(&source);
        assert!(out.errors.is_empty(), "{source}: {:#?}", out.errors);
        let variable = &annotation_of(&mint, &out, "value").variables[0];
        assert_eq!(
            matches!(variable.ownership, PresenceOwnership::Existential { .. }),
            existential,
            "{source}: {:?}",
            variable.ownership
        );
    }
}

/// A source name denotes one witness, while each positive result boundary is
/// a distinct package lifetime. Neither sibling results nor a containing value
/// and its nested result can silently widen that witness to scheme scope. The
/// refusal is independent of field traversal order.
#[test]
fn presence_ownership_rejects_incompatible_production_lifetimes_deterministically() {
    for source in [
        "let bad : { one: Nat -> { x when 'p: Nat }, two: Nat -> { y when 'p: Nat } } = {}",
        "let bad : { two: Nat -> { y when 'p: Nat }, one: Nat -> { x when 'p: Nat } } = {}",
        "let bad : { root when 'p: Nat, nested: Nat -> { x when 'p: Nat } } = {}",
        "let bad : { nested: Nat -> { x when 'p: Nat }, root when 'p: Nat } = {}",
    ] {
        let (_, out) = build_src(source);
        assert_eq!(out.errors.len(), 1, "{source}: {:#?}", out.errors);
        assert!(
            matches!(
                &out.errors[0].kind,
                ErrorKind::IncompatiblePresenceOwnership { name, .. } if name == "p"
            ),
            "{source}: {:#?}",
            out.errors
        );
        let ErrorKind::IncompatiblePresenceOwnership { previous, .. } = out.errors[0].kind else {
            unreachable!()
        };
        assert_ne!(previous, out.errors[0].span);
        assert!(source[out.errors[0].span.start..out.errors[0].span.end()].contains('{'));
        assert!(source[previous.start..previous.end()].contains('{'));
    }

    // Repetition within one exact package remains one producer-owned witness.
    let source = "let good : { x when 'p: Nat, y when 'p: Nat } = { x: 0n, y: 0n }";
    let (mint, out) = built(source);
    assert!(matches!(
        annotation_of(&mint, &out, "good").variables[0].ownership,
        PresenceOwnership::Existential { .. }
    ));
}

/// Conditional effect labels have the polarity of the arrow carrying them;
/// entering an outer parameter has already reversed that polarity.
#[test]
fn conditional_effect_presence_uses_the_carrying_arrows_polarity() {
    let source =
        "effect Log = { op: () -> () }\nlet run : (() -> () + !Log (when 'p)) -> Nat = fn f => 0n";
    let (mint, out) = built(source);
    assert_eq!(
        annotation_of(&mint, &out, "run").variables[0].ownership,
        PresenceOwnership::Universal
    );
}

/// Callback polarity is not approximated from the nearest arrow. The result of
/// a callback supplied to a callback is positive after two reversals, and its
/// hidden choice belongs to that innermost result rather than either wrapper.
#[test]
fn nested_callback_results_keep_their_own_existential_boundary() {
    let source = "let nested : ((Nat -> { x when 'p: Nat }) -> Nat) -> Nat = fn use => 0n";
    let (mint, out) = built(source);
    let PresenceOwnership::Existential { boundary } =
        annotation_of(&mint, &out, "nested").variables[0].ownership
    else {
        panic!("a twice-reversed callback result should be producer-owned");
    };
    assert_eq!(
        &source[boundary.start..boundary.end()],
        "{ x when 'p: Nat }"
    );
}

/// An arrow's conditional effect is produced at the invocation boundary while
/// its return field is produced inside that boundary. One source presence may
/// not be widened to the invocation boundary: doing so would erase the exact
/// package lifetime of the returned value.
#[test]
fn result_and_effect_occurrences_do_not_merge_package_boundaries() {
    let source = "effect Log = { op: () -> () }\n\
                  let mixed : Nat -> { x when 'p: Nat } + !Log (when 'p) = fn n => { x: n }";
    let (_, out) = build_src(source);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::IncompatiblePresenceOwnership { ref name, .. } if name == "p"
    ));
}

/// A formula is written about presences, and a presence is what a `when` puts
/// on a label — so a name no `when` wears is a name the formula has nothing to
/// say about, whether or not anything declared it.
#[test]
fn a_formula_names_only_what_a_when_wears() {
    for src in [
        // Declared, and never worn by a label.
        "let f : { x when 'a: Nat } -> Nat where 'a = 'b = fn p => 0n",
        // Not declared at all, which the reader fixes the same way.
        "let f : { x when 'a: Nat } -> Nat where 'a = 'b = fn p => 0n",
    ] {
        let (_, out) = build_src(src);
        let complaints: Vec<&ErrorKind> = out
            .errors
            .iter()
            .map(|error| &error.kind)
            .filter(|kind| matches!(kind, ErrorKind::UnboundPresence { name } if name == "b"))
            .collect();
        assert_eq!(complaints.len(), 1, "{src}: {:#?}", out.errors);
    }
}

/// A declaration's variables are its parameters, so a variable written in one's
/// body is refused where it stands — and its `where` has nothing for a formula
/// to relate either. Each is reported at its own name, so a body with several
/// reports each.
#[test]
fn a_declaration_may_declare_nothing_in_its_where() {
    let src = "type Bad = { x: Nat, ..'r }";
    let (mint, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        error.kind,
        ErrorKind::VariableInDeclaration { ref name } if name == "r"
    ));
    assert_eq!(error.span.start, src.rfind("'r").expect("the variable"));
    // The body absorbs, the way one left open through anything but a parameter
    // does: what the declaration would stand for is exactly what was refused.
    let symbol = type_symbol(&mint, &out, "Bad");
    assert!(matches!(
        out.program.types[&symbol].value.tracked,
        TypeKind::Error
    ));

    // A variable in a type position goes the same way, and an operation's
    // signature refuses one in its own words: it is the one place the `+`, the
    // `..` and the `when` are refused by the same sentence.
    for (src, code) in [
        ("type Bad = { x: 'a }", "variable-in-declaration"),
        ("effect E = { op: 'a -> Nat }", "impure-operation"),
        (
            "effect E = { op: { x: Nat, ..'r } -> Nat }",
            "impure-operation",
        ),
    ] {
        let (_, out) = build_src(src);
        assert!(
            out.errors.iter().any(|error| error.kind.code() == code),
            "{src}: {:#?}",
            out.errors
        );
    }

    // One report per name, and the clause is refused for existing at all.
    let src = "type Bad = { x when 'a: Nat, ..'r } where 'a";
    let (_, out) = build_src(src);
    let codes: Vec<&str> = out.errors.iter().map(|error| error.kind.code()).collect();
    assert_eq!(
        codes,
        [
            "open-declared-type",
            "variable-in-declaration",
            "declared-where-clause"
        ]
    );
}

/// Several constraint statements are conjoined in written order: `where 'a; 'b`
/// says what `where 'a and 'b` says, so the definition is held to both.
#[test]
fn several_constraint_statements_are_conjoined() {
    let (mint, out) =
        built("let f : { x when 'a: Nat, y when 'b: Nat } -> Nat where 'a; 'b = fn p => 0n");
    let annotation = annotation_of(&mint, &out, "f");
    let clause = annotation.clause.as_ref().expect("the clause");
    assert_eq!(
        print::ir::clause(&clause.tracked, &mint).to_string(),
        "'a and 'b"
    );
}

/// A `where 'let` variable stands for one type outright, the way a declaration's
/// parameter does, so there is nothing there to give arguments to — and the
/// name still counts as used, since a second complaint about it would be the
/// first one said again.
#[test]
fn a_declared_variable_may_not_be_applied() {
    let src = "let f : 'a Nat -> Nat = fn x => 0n";
    let (_, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        error.kind,
        ErrorKind::ParameterApplied { ref name } if name == "a"
    ));
    assert_eq!(error.span.start, src.find("'a").expect("the head"));
}

/// A declaration says the same thing wherever it is used, so there is nothing
/// in one for a `_` to leave open. Refused where it stands, and the type
/// absorbs — the treatment an open row written there already gets.
#[test]
fn a_declaration_may_not_hold_a_hole() {
    let src = "type Bad = { x: _ }";
    let (mint, out) = build_src(src);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(error.kind, ErrorKind::HoleInDeclaration));
    assert_eq!(error.span.start, src.find('_').expect("the hole"));
    let symbol = type_symbol(&mint, &out, "Bad");
    let TypeKind::Struct { fields, .. } = &out.program.types[&symbol].value.tracked else {
        panic!("expected a struct");
    };
    let TypeField::Written { value, .. } = &fields["x"] else {
        panic!("expected a written field");
    };
    assert!(matches!(value.tracked, TypeKind::Error));

    // An annotation is where a hole belongs, and it lowers to one.
    let (mint, out) = built("let k : _ -> Nat = fn x => 0n");
    let annotation = annotation_of(&mint, &out, "k");
    let TypeKind::Arrow { from, .. } = &annotation.ty.tracked else {
        panic!("expected an arrow");
    };
    assert!(matches!(from.tracked, TypeKind::Hole));
}

/// A symbol declared in a module is minted with that module as its parent, so
/// its path reads as the source spells it. Nothing in `symbol.rs` changed for
/// this: modules were always in the design, and the hoist is what finally puts
/// something in them.
#[test]
fn a_symbol_is_minted_under_the_module_it_was_declared_in() {
    let (mint, out) = built("module A =\n  module B =\n    let x = 1n\n  end\nend");
    let x = term_symbol(&mint, &out, "x");
    assert_eq!(mint.path(x).to_string(), "test::A::B::x");

    // The modules themselves are symbols like any other, in their own
    // namespace, and nested the way they were written.
    let b = mint.parent(x).expect("x sits in a module");
    assert_eq!(mint.namespace(b.symbol()), Namespace::Modules);
    assert_eq!(mint.name(b.symbol()), "B");
    let a = mint.parent(b.symbol()).expect("B sits in a module");
    assert_eq!(mint.name(a.symbol()), "A");
    assert!(mint.parent(a.symbol()).is_none());
}

/// R9's walk: locals first, then the module the name is written in, then each
/// enclosing module, then the bundle root. The first match wins, so a module's
/// own definition shadows the root's.
#[test]
fn an_unqualified_name_walks_outward_and_the_inner_one_wins() {
    let src = "let top = 1n\n\
               module A =\n  let x = top\n  module B =\n    let y = x\n  end\nend\n\
               module C =\n  let top = 2n\n  let z = top\n  let w = A::x\nend";
    let (mint, out) = built(src);

    // `A::x` reaches the root's `top`, since `A` declares none of its own.
    let root = term_symbol(&mint, &out, "top");
    let inner = out
        .program
        .terms
        .keys()
        .copied()
        .find(|symbol| mint.path(*symbol).to_string() == "test::C::top")
        .expect("C declares a top of its own");
    assert_ne!(root, inner);

    let names = |name: &str| -> Symbol {
        let TermKind::Ident(symbol) = term_value(&mint, &out, name) else {
            panic!("{name} is not given as a name");
        };
        *symbol
    };
    assert_eq!(names("x"), root);
    assert_eq!(names("z"), inner);
    // And a sibling module's name is reached only through the path that names
    // it, which is what `w` is written as.
    assert_eq!(names("w"), term_symbol(&mint, &out, "x"));
}

/// A sibling module's names are not in scope: reaching one unqualified is the
/// undefined term it is, because the walk goes outward and never sideways.
#[test]
fn a_sibling_module_is_reached_only_by_a_path() {
    let src = "module A =\n  let x = 1n\nend\nmodule C =\n  let w = x\nend";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "x"
    ));
}

/// Everything is hoisted bundle-wide: every module, then every type, then every
/// effect, then every term. Where a declaration is written never decides what
/// can see it, so a path may name a module declared below it.
#[test]
fn hoisting_reaches_a_module_declared_below_its_use() {
    let (mint, out) =
        built("let four = Math::double 2n\nmodule Math =\n  let double = fn x => x\nend");
    let TermKind::Apply { func, .. } = term_value(&mint, &out, "four") else {
        panic!("expected an application");
    };
    let TermKind::Ident(symbol) = func.kind else {
        panic!("expected a name");
    };
    assert_eq!(mint.path(symbol).to_string(), "test::Math::double");
}

/// Modules are their own namespace, so one name may be a module, a type, a term
/// and an effect at once without any of them colliding.
#[test]
fn a_module_a_type_a_term_and_an_effect_may_share_a_name() {
    let (mint, out) = built("module P = end\ntype P = { x: Nat }\nlet P = 1n\neffect P");
    assert!(out.program.types.keys().any(|s| mint.name(*s) == "P"));
    assert!(out.program.terms.keys().any(|s| mint.name(*s) == "P"));
    assert!(out.program.effects.keys().any(|s| mint.name(*s) == "P"));
    let modules: Vec<_> = mint
        .symbols()
        .filter(|s| mint.namespace(*s) == Namespace::Modules)
        .collect();
    assert_eq!(modules.len(), 1);
    assert_eq!(mint.name(modules[0]), "P");
}

/// Two modules of one name in one scope are the repeat they are, reported at
/// the second and pointing back at the first — the existing `Duplicate` with
/// the namespace modules were always minted in.
#[test]
fn a_repeated_module_is_a_duplicate() {
    let src = "module A = end\nmodule A = end";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let ErrorKind::Duplicate {
        ref name,
        namespace,
        previous,
    } = out.errors[0].kind
    else {
        panic!("expected a duplicate: {:#?}", out.errors);
    };
    assert_eq!(name, "A");
    assert_eq!(namespace, Namespace::Modules);
    assert_eq!(
        out.errors[0].span.start,
        src.rfind('A').expect("the second")
    );
    assert_eq!(previous.start, src.find('A').expect("the first"));

    // The same name in two different scopes is not a repeat: a module inside a
    // module is somewhere else.
    let (_, out) = built("module A =\n  module A = end\nend");
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

/// A path's first segment resolves by R9's walk, so a segment naming no module
/// anywhere out to the root is reported as the undefined module it is — at the
/// segment, not at the name after it.
#[test]
fn an_undefined_first_segment_is_reported_at_the_segment() {
    let src = "let a = Nope::x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Modules,
        } if name == "Nope"
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.find("Nope").expect("the segment")
    );
    assert_eq!(out.errors[0].span.width, "Nope".len());
}

/// Every segment after the first resolves strictly inside the module the
/// previous one named — no outward walk. So a middle segment naming a module
/// that exists at the root, but not inside the one before it, is undefined.
#[test]
fn a_later_segment_does_not_walk_outward() {
    let src = "module Outer = end\nmodule A = end\nlet a = A::Outer::x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Modules,
        } if name == "Outer"
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("Outer").expect("the middle segment")
    );
}

/// A type position takes a path as readily as an expression does, so a segment
/// naming no module is reported there too — bare, and at the head of an
/// application, which are the two shapes a written type can be.
#[test]
fn an_undefined_segment_in_a_type_is_reported_once() {
    for src in ["type T = Nope::X", "type T = Nope::Pair Nat"] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::Undefined {
                    ref name,
                    namespace: Namespace::Modules,
                } if name == "Nope"
            ),
            "{src:?}: {:#?}",
            out.errors[0].kind
        );
        assert_eq!(
            out.errors[0].span.start,
            src.find("Nope").expect("the segment"),
            "{src:?}"
        );
    }
}

/// A final name the module does not declare is reported at that name, in the
/// namespace the position it was written at demands — one rule, four wordings,
/// and the module the path named is not blamed for any of them.
#[test]
fn an_undefined_final_name_is_reported_in_its_own_namespace() {
    let cases = [
        (
            "module M = end\nlet a = M::missing",
            Namespace::Terms,
            "missing",
            "missing",
        ),
        (
            "module M = end\ntype T = M::Missing",
            Namespace::Types,
            "Missing",
            "Missing",
        ),
        (
            "module M = end\nlet f : () -> Nat + M::!Log = fn _ => 0n",
            Namespace::Effects,
            "Log",
            // A label's span wears its sigil, the way every other label's does.
            "!Log",
        ),
    ];
    for (src, namespace, name, at) in cases {
        let (_, out) = build_src(src);
        let undefined: Vec<_> = out
            .errors
            .iter()
            .filter(|error| matches!(error.kind, ErrorKind::Undefined { .. }))
            .collect();
        assert_eq!(undefined.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(
                undefined[0].kind,
                ErrorKind::Undefined { name: ref found, namespace: found_namespace }
                    if found == name && found_namespace == namespace
            ),
            "{src:?}: {:#?}",
            undefined[0].kind
        );
        assert_eq!(
            undefined[0].span.start,
            src.rfind(at).expect("the name"),
            "{src:?}"
        );
    }
}

/// A module that exists but whose next segment does not: the complaint is about
/// the segment that named nothing, and the one before it is left alone.
#[test]
fn a_middle_segment_naming_no_module_is_reported_once() {
    let src = "module A =\n  module B = end\nend\nlet a = A::Nope::x";
    let (_, out) = build_src(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("Nope").expect("the segment")
    );
}

/// A path reaches a type, an effect and an operation as readily as a term, in
/// every position R6 and R7 allow. One program, because what is being pinned is
/// that each position resolves at all.
#[test]
fn paths_resolve_in_every_position() {
    let src = "module Math =\n  type Pair 'a 'b = { first: 'a, second: 'b }\nend\n\
               module Sys =\n  effect Log = { write: Nat -> () }\nend\n\
               let p : Math::Pair Nat Nat = { first: 1n, second: 2n }\n\
               let greet : () -> Nat + Sys::!Log = fn _ => let _ = Sys::!Log.write 1n in 0n\n\
               let quiet : () -> Nat = fn _ =>\n\
                 handle greet () with | Sys::!Log.write s => () | return x => x end";
    let (mint, out) = built(src);
    let pair = out
        .program
        .types
        .keys()
        .copied()
        .find(|symbol| mint.name(*symbol) == "Pair")
        .expect("Math declares Pair");
    assert_eq!(mint.path(pair).to_string(), "test::Math::Pair");
}

/// A primitive lives in no module, so a path can never reach one: `M::Nat` is
/// the undefined type it is rather than the built-in. Bare and applied alike,
/// since applying a primitive is the one place a name that exists is counted
/// rather than resolved.
#[test]
fn a_path_never_reaches_a_primitive() {
    for src in [
        "module M = end\ntype T = M::Nat",
        "module M = end\ntype T = M::Nat Nat",
    ] {
        let (_, out) = build_src(src);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::Undefined {
                    ref name,
                    namespace: Namespace::Types,
                } if name == "Nat"
            ),
            "{src:?}: {:#?}",
            out.errors[0].kind
        );
    }
}

/// A file module the loader could not fill in has no body, and lowering treats
/// it as the empty one it is: the module is still declared, so a path naming it
/// resolves and one missing file costs one complaint.
#[test]
fn a_module_with_no_body_is_declared_and_empty() {
    let (mint, out) = build_src("module A\nlet a = A::x");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Undefined {
            ref name,
            namespace: Namespace::Terms,
        } if name == "x"
    ));
    assert!(
        mint.symbols()
            .any(|s| mint.namespace(s) == Namespace::Modules && mint.name(s) == "A")
    );
}

fn artifact_type(ty: a::Type) -> a::Type {
    ty
}
fn artifact_unit() -> a::Type {
    artifact_struct(Vec::new())
}
fn artifact_struct(labels: Vec<(String, a::RowField)>) -> a::Type {
    a::Type::Struct(a::Row {
        labels,
        rest: a::Rest::Closed,
    })
}

fn artifact_scheme(body: a::Type) -> a::Scheme {
    a::Scheme {
        count: 0,
        presences: 0,
        existentials: Vec::new(),
        formula: a::Formula::True,
        body,
    }
}

/// Hand-built dependency data crosses the same strict artifact boundary as a
/// bundle read from disk before the IR may see it. Validation renders and
/// decodes canonical text, which is not what the bounded-stack regressions in
/// this file measure, so it always runs on a generously sized thread of its
/// own: the deliberately small stacks then bound only the IR and inference
/// work under test.
fn checked(dependency: a::UncheckedArtifact) -> a::Artifact {
    std::thread::Builder::new()
        .name("artifact-validation".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || dependency.validate())
        .expect("the artifact validation thread starts")
        .join()
        .expect("artifact validation completes")
        .expect("hand-built dependency validates")
}

/// [`checked`] for a dependency that is deliberately malformed in part: the
/// tolerant boundary a compiler caller crosses keeps every declaration that
/// validates on its own and reports the rest as recovery facts.
fn recovered(dependency: a::UncheckedArtifact) -> (a::Artifact, Vec<a::RecoveryFact>) {
    std::thread::Builder::new()
        .name("artifact-recovery".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || dependency.recover())
        .expect("the artifact recovery thread starts")
        .join()
        .expect("artifact recovery completes")
}

#[test]
fn imported_schemes_preserve_and_sanitize_existential_ownership() {
    let mut dependency = prelude_artifact("dep");
    dependency.header.values[0].scheme = a::Scheme {
        count: 1,
        presences: 1,
        existentials: vec![0],
        formula: a::Formula::True,
        body: a::Type::Package(Box::new(artifact_struct(vec![(
            "choice".into(),
            a::RowField {
                presence: a::Presence::Bound(0),
                ty: a::Type::Nat,
            },
        )]))),
    };
    dependency.header.values[1].scheme.existentials = vec![9];

    let parsed = parse::parse(lex("let value = dep::prelude::shared", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    // A witness position outside the scheme is not sanitized into a plain
    // quantifier: the value carrying it is discarded at the boundary, and the
    // one with a coherent witness keeps it.
    let (dependency, facts) = recovered(dependency);
    assert!(
        matches!(
            facts.as_slice(),
            [a::RecoveryFact::ValueDiscarded { index: 1, .. }]
        ),
        "{facts:#?}"
    );
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[dependency]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut schemes = out.program.external_schemes.values();
    assert!(
        schemes
            .next()
            .is_some_and(|scheme| scheme.is_existential(0))
    );
    assert!(schemes.next().is_none());
}

#[test]
fn canonical_bundle_import_preserves_mixed_input_result_guarantee() {
    let mut dependency = prelude_artifact("dep");
    dependency.header.values[0].scheme = a::Scheme {
        count: 2,
        presences: 2,
        existentials: vec![1],
        formula: a::Formula::Owned(
            0,
            Box::new(a::Formula::Iff(
                Box::new(a::Formula::Bound(0)),
                Box::new(a::Formula::Bound(1)),
            )),
        ),
        body: a::Type::Arrow(
            Box::new(artifact_struct(vec![(
                "input".into(),
                a::RowField {
                    presence: a::Presence::Bound(0),
                    ty: a::Type::Nat,
                },
            )])),
            Box::new(a::Type::Package(Box::new(artifact_struct(vec![(
                "result".into(),
                a::RowField {
                    presence: a::Presence::Bound(1),
                    ty: a::Type::Nat,
                },
            )])))),
            a::Row {
                labels: Vec::new(),
                rest: a::Rest::Closed,
            },
        ),
    };

    // Exercise the complete bundle disk boundary before importing it. The
    // universal atom remains in scope even though existential atoms determine
    // which result package owns the indivisible proposition.
    let text = checked(dependency).print();
    let dependency = a::Artifact::try_parse(&text)
        .expect("canonical dependency parses")
        .validate()
        .expect("canonical dependency validates");
    let parsed = parse::parse(lex("let value = dep::prelude::shared", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[dependency]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let scheme = out
        .program
        .external_schemes
        .values()
        .next()
        .expect("the imported value scheme exists");
    assert!(scheme.is_existential(1));
    assert!(
        matches!(scheme.formula(), ruddy::types::Formula::Owned(0, inner) if matches!(&**inner, ruddy::types::Formula::Iff(..)))
    );
}

fn prelude_artifact(bundle: &str) -> a::UncheckedArtifact {
    let qualified = |name: &str| format!("{bundle}@1.0.0::prelude::{name}");
    a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: bundle.into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: vec![
                a::Value {
                    name: qualified("shared"),
                    scheme: artifact_scheme(artifact_type(a::Type::Nat)),
                },
                a::Value {
                    name: qualified("Tools::inside"),
                    scheme: artifact_scheme(artifact_type(a::Type::Nat)),
                },
            ],
            types: vec![
                a::DeclaredType {
                    name: qualified("Shared"),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::String)),
                },
                a::DeclaredType {
                    name: qualified("Nat"),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::String)),
                },
            ],
            effects: vec![a::DeclaredEffect {
                name: qualified("Shared"),
                identity: Some(a::EffectIdentity {
                    name: "Shared".into(),
                    interface: "shared:{}->{}".into(),
                }),
                kind: a::EffectKind::Operations(Vec::new()),
            }],
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
}

fn build_imported(src: &str, alias: &str, dependency: &a::UncheckedArtifact) -> (Mint, Output) {
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let dependency = checked(dependency.clone());
    let imports = [DependencyImport {
        alias,
        artifact: &dependency,
    }];
    let mut mint = dummy_mint();
    let out = build_with_dependency_imports(&mut mint, parsed.stmts, &imports, &[]);
    (mint, out)
}

#[test]
fn configured_std_prelude_opens_only_direct_members_in_every_namespace() {
    let dependency = prelude_artifact("foundation");
    let src = "let bare = shared\n\
               let qualified = std::prelude::shared\n\
               type Alias = Shared\n\
               effect Alias = !Shared\n\
               let child = Tools::inside\n\
               module Nested =\n  let value = shared\nend";
    let (mint, out) = build_imported(src, "std", &dependency);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    let external = |symbol| out.program.external_names.get(&symbol).map(String::as_str);
    let TermKind::Ident(bare) = term_value(&mint, &out, "bare") else {
        panic!("bare prelude term did not lower to an identifier")
    };
    let TermKind::Ident(qualified) = term_value(&mint, &out, "qualified") else {
        panic!("qualified prelude term did not lower to an identifier")
    };
    assert_eq!(bare, qualified);
    assert_eq!(external(*bare), Some("foundation@1.0.0::prelude::shared"));
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "Alias")].value.tracked,
        TypeKind::Ident(symbol)
            if external(symbol) == Some("foundation@1.0.0::prelude::Shared")
    ));
    assert!(
        matches!(effect_of(&mint, &out, "Alias"), Effect::Alias(alias)
        if alias.body.cases.iter().any(|case| external(case.symbol) == Some("foundation@1.0.0::prelude::Shared")))
    );
    let TermKind::Ident(child) = term_value(&mint, &out, "child") else {
        panic!("prelude child module was not available")
    };
    assert_eq!(
        external(*child),
        Some("foundation@1.0.0::prelude::Tools::inside")
    );
    let nested_value = out
        .program
        .terms
        .iter()
        .find(|(symbol, _)| mint.name(**symbol) == "value")
        .map(|(_, declaration)| &declaration.value.kind)
        .expect("nested value");
    assert!(
        matches!(nested_value, TermKind::Ident(symbol) if external(*symbol) == Some("foundation@1.0.0::prelude::shared"))
    );
}

#[test]
fn a_bundle_does_not_open_its_own_prelude_without_configured_std() {
    let (_, out) = build_src("module prelude =\n  let shared = 1n\nend\nlet bare = shared");
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::Undefined {
                namespace: Namespace::Terms,
                ..
            },
            ..
        }]
    ));
}

#[test]
fn std_prelude_does_not_flatten_descendants_and_requires_the_source_alias() {
    let dependency = prelude_artifact("std");
    let (_, out) = build_imported("let bad = inside", "other", &dependency);
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::Undefined {
                namespace: Namespace::Terms,
                ..
            },
            ..
        }]
    ));

    let dependency = prelude_artifact("foundation");
    let (_, out) = build_imported("let bad = inside", "std", &dependency);
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::Undefined {
                namespace: Namespace::Terms,
                ..
            },
            ..
        }]
    ));

    let mut no_prelude = prelude_artifact("foundation");
    for value in &mut no_prelude.header.values {
        value.name = value.name.replace("::prelude::", "::library::");
    }
    for ty in &mut no_prelude.header.types {
        ty.name = ty.name.replace("::prelude::", "::library::");
    }
    for effect in &mut no_prelude.header.effects {
        effect.name = effect.name.replace("::prelude::", "::library::");
    }
    let (_, out) = build_imported("let bad = shared", "std", &no_prelude);
    assert!(matches!(
        out.errors.as_slice(),
        [ruddy::ir::Error {
            kind: ErrorKind::Undefined {
                namespace: Namespace::Terms,
                ..
            },
            ..
        }]
    ));
}

#[test]
fn user_and_lexical_declarations_shadow_std_prelude_without_duplicates() {
    let dependency = prelude_artifact("foundation");
    let src = "let shared = 1n\n\
               let root = shared\n\
               let qualified = std::prelude::shared\n\
               module M =\n  let shared = 2n\n  let nested = shared\nend\n\
               let lexical = fn shared => shared";
    let (mint, out) = build_imported(src, "std", &dependency);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let root_shared = term_symbol(&mint, &out, "shared");
    assert!(
        matches!(term_value(&mint, &out, "root"), TermKind::Ident(symbol) if *symbol == root_shared)
    );
    assert!(matches!(
        term_value(&mint, &out, "qualified"),
        TermKind::Ident(symbol)
            if out.program.external_names.get(symbol).map(String::as_str)
                == Some("foundation@1.0.0::prelude::shared")
    ));
    let nested = out
        .program
        .terms
        .iter()
        .find(|(symbol, _)| mint.name(**symbol) == "nested")
        .unwrap()
        .1;
    let module_shared = out
        .program
        .terms
        .iter()
        .find(|(symbol, declaration)| {
            mint.name(**symbol) == "shared"
                && matches!(declaration.value.kind, TermKind::Natural(2))
        })
        .map(|(symbol, _)| *symbol)
        .unwrap();
    assert!(matches!(nested.value.kind, TermKind::Ident(symbol) if symbol == module_shared));
    let TermKind::Fn { arg, body } = term_value(&mint, &out, "lexical") else {
        panic!("lexical test is a lambda")
    };
    assert!(matches!(body.kind, TermKind::Ident(symbol) if symbol == arg.tracked));
}

#[test]
fn user_types_effects_and_child_modules_shadow_prelude_names_independently() {
    let dependency = prelude_artifact("foundation");
    let src = "type Shared = String\n\
               type Alias = Shared\n\
               effect Shared\n\
               effect Alias = !Shared\n\
               module Tools =\n  let inside = 2n\nend\n\
               let Tools = 3n\n\
               let child = Tools::inside\n\
               let term = Tools";
    let (mint, out) = build_imported(src, "std", &dependency);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    let local_type = type_symbol(&mint, &out, "Shared");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "Alias")].value.tracked,
        TypeKind::Ident(symbol) if symbol == local_type
    ));

    let local_effect = *out
        .program
        .effects
        .keys()
        .find(|symbol| mint.name(**symbol) == "Shared")
        .expect("local Shared effect");
    assert!(matches!(
        effect_of(&mint, &out, "Alias"),
        Effect::Alias(alias)
            if alias.body.cases.iter().any(|case| case.symbol == local_effect)
    ));

    let local_inside = out
        .program
        .terms
        .iter()
        .find(|(symbol, declaration)| {
            mint.name(**symbol) == "inside"
                && matches!(declaration.value.kind, TermKind::Natural(2))
        })
        .map(|(symbol, _)| *symbol)
        .expect("local Tools::inside");
    assert!(matches!(
        term_value(&mint, &out, "child"),
        TermKind::Ident(symbol) if *symbol == local_inside
    ));
    let local_tools_term = term_symbol(&mint, &out, "Tools");
    assert!(matches!(
        term_value(&mint, &out, "term"),
        TermKind::Ident(symbol) if *symbol == local_tools_term
    ));
}

#[test]
fn std_prelude_type_precedes_primitives_but_user_types_precede_the_prelude() {
    let dependency = prelude_artifact("foundation");
    let (mint, out) = build_imported("type Alias = Nat", "std", &dependency);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "Alias")].value.tracked,
        TypeKind::Ident(symbol)
            if out.program.external_names.get(&symbol).map(String::as_str)
                == Some("foundation@1.0.0::prelude::Nat")
    ));

    let (mint, out) = build_imported("type Nat = String\ntype Alias = Nat", "std", &dependency);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let local = type_symbol(&mint, &out, "Nat");
    assert!(matches!(
        out.program.types[&type_symbol(&mint, &out, "Alias")].value.tracked,
        TypeKind::Ident(symbol) if symbol == local
    ));
}

fn effect_artifact(bundle: &str, interface: &str) -> a::UncheckedArtifact {
    a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: bundle.into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: vec![a::DeclaredEffect {
                name: format!("{bundle}@1.0.0::IO"),
                identity: Some(a::EffectIdentity {
                    name: "IO".into(),
                    interface: interface.into(),
                }),
                kind: a::EffectKind::Operations(Vec::new()),
            }],
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    }
}

fn local_effect_interface(source: &str, effect: &str) -> String {
    let (mint, out) = built(source);
    out.program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| {
            (mint.name(*symbol) == effect).then(|| match identity {
                ruddy::types::EffectId::Structural { interface, .. } => interface.clone(),
                ruddy::types::EffectId::Pending(_) => panic!("effect identity was not finalized"),
            })
        })
        .unwrap_or_else(|| panic!("no effect named {effect}"))
}

#[test]
fn imported_unnamed_operation_selectors_are_preserved() {
    let mut dependency = effect_artifact("dep", "u");
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Unnamed,
        from: artifact_type(a::Type::Nat),
        to: artifact_type(artifact_unit()),
    });
    let parsed = parse::parse(lex("let operation = dep::!IO", FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(
        out.program
            .external_operations
            .keys()
            .any(|(_, selector)| *selector == OperationSelector::Unnamed)
    );
}

#[test]
fn imported_unnamed_handlers_are_complete_without_panicking() {
    let mut dependency = effect_artifact("dep", "u");
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Unnamed,
        from: artifact_type(a::Type::Nat),
        to: artifact_type(artifact_unit()),
    });
    let src = "let main = fn n => handle dep::!IO n with | dep::!IO value => () end";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut node = term_value(&mint, &out, "main");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected imported unnamed handler")
    };
    assert_eq!(handler.arms.len(), 1);
    assert_eq!(handler.discharges.len(), 1);
    assert_eq!(handler.discharges[0], handler.arms[0].effect);
}

#[test]
fn imported_effect_row_keys_follow_canonical_identities_by_shape() {
    let raw = "IO\u{1f}x";
    let canonical = ruddy::types::EffectId::structural("IO".into(), "0#o1:x;".into()).row_key();
    let effect_row = || a::Row {
        labels: Vec::new(),
        rest: a::Rest::More(Box::new(a::Row {
            labels: vec![(
                raw.into(),
                a::RowField {
                    presence: a::Presence::Present,
                    ty: artifact_type(artifact_unit()),
                },
            )],
            rest: a::Rest::Closed,
        })),
    };
    let nested_effect_argument = || {
        artifact_type(a::Type::Named {
            name: "dep@1.0.0::Effects".into(),
            args: vec![artifact_type(a::Type::Named {
                name: "dep@1.0.0::Effects".into(),
                args: vec![artifact_type(a::Type::Sum(effect_row()))],
            })],
        })
    };

    let mut dependency = effect_artifact("dep", "x");
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Unnamed,
        from: artifact_type(artifact_unit()),
        to: artifact_type(artifact_unit()),
    });
    dependency.header.values.extend([
        a::Value {
            name: "dep@1.0.0::action".into(),
            scheme: artifact_scheme(artifact_type(a::Type::Arrow(
                Box::new(artifact_type(artifact_unit())),
                Box::new(artifact_type(artifact_unit())),
                effect_row(),
            ))),
        },
        a::Value {
            name: "dep@1.0.0::nested".into(),
            scheme: artifact_scheme(nested_effect_argument()),
        },
        // Missing declarations have no published argument sense. Their sum is
        // recovery data, not an effect row, even when its label looks generated.
        a::Value {
            name: "dep@1.0.0::unresolved".into(),
            scheme: artifact_scheme(artifact_type(a::Type::Named {
                name: "missing@1.0.0::Unknown".into(),
                args: vec![artifact_type(a::Type::Sum(effect_row()))],
            })),
        },
    ]);
    dependency.header.types.extend([
        a::DeclaredType {
            name: "dep@1.0.0::Effects".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Effects,
                lacks: vec![raw.into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Bound(0)),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Cases".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Sum(a::Row {
                labels: vec![(
                    raw.into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: artifact_type(artifact_unit()),
                    },
                )],
                rest: a::Rest::Closed,
            }))),
        },
    ]);

    let parsed = parse::parse(
        lex(
            "let main : () -> () = fn unit => handle dep::action (dep::!IO unit) with | dep::!IO value => value end",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    let imported_value = |name: &str| {
        out.program
            .external_schemes
            .iter()
            .find_map(|(symbol, scheme)| (mint.name(*symbol) == name).then_some(scheme))
            .unwrap_or_else(|| panic!("missing imported value {name}"))
    };
    let Ty::Arrow(_, _, effects) = &**imported_value("dep@1.0.0::action").body() else {
        panic!("action is an arrow")
    };
    let Rest::More(more) = &effects.rest else {
        panic!("action keeps its composed row")
    };
    assert!(more.labels.contains_key(&canonical));

    let Ty::Named { args, .. } = &**imported_value("dep@1.0.0::nested").body() else {
        panic!("nested outer alias")
    };
    let Ty::Named { args, .. } = &*args[0] else {
        panic!("nested inner alias")
    };
    let Ty::Sum(row) = &*args[0] else {
        panic!("effect argument row")
    };
    let Rest::More(more) = &row.rest else {
        panic!("effect argument keeps its composed row")
    };
    assert!(more.labels.contains_key(&canonical));

    let Ty::Named { args, .. } = &**imported_value("dep@1.0.0::unresolved").body() else {
        panic!("unresolved named type")
    };
    let Ty::Sum(row) = &*args[0] else {
        panic!("unresolved ordinary sum argument")
    };
    let Rest::More(more) = &row.rest else {
        panic!("unresolved sum keeps its composed row")
    };
    assert!(more.labels.contains_key(raw));

    let imported_type = |name: &str| {
        out.program
            .external_types
            .iter()
            .find_map(|(symbol, declaration)| (mint.name(*symbol) == name).then_some(declaration))
            .unwrap_or_else(|| panic!("missing imported type {name}"))
    };
    assert!(matches!(
        &imported_type("dep@1.0.0::Effects").params[0],
        ParamKind::Effects { lacks } if lacks.contains(&canonical)
    ));
    let Ty::Sum(cases) = &**imported_type("dep@1.0.0::Cases").scheme.body() else {
        panic!("ordinary imported sum")
    };
    assert!(cases.labels.contains_key(raw));
    assert!(!cases.labels.contains_key(&canonical));

    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
}

#[test]
fn legacy_operation_effects_get_fallback_identity_before_row_normalization() {
    let mut dependency = effect_artifact("dep", "ignored");
    dependency.header.effects[0].identity = None;
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Named("run".into()),
        from: artifact_unit(),
        to: artifact_unit(),
    });
    let legacy_key = "IO\u{1f}unresolved:dep@1.0.0::IO";
    let effects = a::Row {
        labels: vec![(
            legacy_key.into(),
            a::RowField {
                presence: a::Presence::Present,
                ty: artifact_unit(),
            },
        )],
        rest: a::Rest::Closed,
    };
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::action".into(),
        scheme: artifact_scheme(a::Type::Arrow(
            Box::new(artifact_unit()),
            Box::new(artifact_unit()),
            effects,
        )),
    });
    dependency.header.types.push(a::DeclaredType {
        name: "dep@1.0.0::Effects".into(),
        params: vec![a::Parameter {
            sense: a::Sense::Effects,
            lacks: vec![legacy_key.into()],
            relevant: true,
        }],
        scheme: a::Scheme {
            count: 1,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Bound(0),
        },
    });

    let parsed = parse::parse(
        lex(
            "let main = fn unit => handle dep::action unit with | dep::!IO.run value => value end",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let identity = out
        .program
        .effect_ids
        .iter()
        .find_map(|(_, identity)| (identity.name() == "IO").then_some(identity))
        .expect("legacy operation identity");
    let key = identity.row_key();
    let action = out
        .program
        .external_schemes
        .iter()
        .find_map(|(symbol, scheme)| (mint.name(*symbol) == "dep@1.0.0::action").then_some(scheme))
        .expect("legacy action");
    let Ty::Arrow(_, _, effects) = &**action.body() else {
        panic!("action arrow")
    };
    assert!(effects.labels.contains_key(&key));
    let effects_type = out
        .program
        .external_types
        .iter()
        .find_map(|(symbol, declaration)| {
            (mint.name(*symbol) == "dep@1.0.0::Effects").then_some(declaration)
        })
        .expect("legacy effects type");
    assert!(matches!(
        &effects_type.params[0],
        ParamKind::Effects { lacks } if lacks.contains(&key)
    ));
}

#[test]
fn imported_named_handler_coverage_and_diagnostics_use_artifact_selectors() {
    let dependency = named_io_artifact("dep", "artifact-interface");
    let build = |src: &str| {
        let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
        assert!(parsed.errors.is_empty());
        let mut mint = dummy_mint();
        let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency.clone())]);
        (mint, out)
    };
    let complete = "let main = fn n => handle dep::!IO.write n with\n\
                    | dep::!IO.write value => ()\n\
                    | dep::!IO.flush unit => ()\n\
                    end";
    let (mint, out) = build(complete);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut node = term_value(&mint, &out, "main");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected imported named handler")
    };
    assert_eq!(handler.arms.len(), 2);
    assert_eq!(handler.discharges.len(), 1);

    let (_, partial) =
        build("let main = fn n => handle dep::!IO.write n with | dep::!IO.write value => () end");
    assert!(matches!(
        &partial.errors[..],
        [ruddy::ir::Error {
            kind: ErrorKind::PartialHandler { missing, .. },
            ..
        }] if missing == &["flush".to_string()]
    ));

    let (_, duplicate) = build(
        "let main = fn n => handle dep::!IO.write n with\n\
         | dep::!IO.write first => ()\n\
         | dep::!IO.write second => ()\n\
         | dep::!IO.flush unit => () end",
    );
    assert_eq!(
        duplicate
            .errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-arm"]
    );
}

fn named_io_artifact(bundle: &str, interface: &str) -> a::UncheckedArtifact {
    let mut dependency = effect_artifact(bundle, interface);
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.extend([
        a::Operation {
            selector: a::OperationSelector::Named("write".into()),
            from: artifact_type(a::Type::Nat),
            to: artifact_type(artifact_unit()),
        },
        a::Operation {
            selector: a::OperationSelector::Named("flush".into()),
            from: artifact_type(artifact_unit()),
            to: artifact_type(artifact_unit()),
        },
    ]);
    dependency
}

#[test]
fn dependency_and_local_structural_handlers_share_one_discharge() {
    let declaration = "effect IO = { write: Nat -> (), flush: () -> () }";
    let interface = local_effect_interface(declaration, "IO");
    let dependency = named_io_artifact("dep", &interface);
    let src = format!(
        "{declaration}\n\
         let main = fn n => handle dep::!IO.write n with\n\
           | dep::!IO.write value => ()\n\
           | !IO.flush unit => ()\n\
         end"
    );
    let parsed = parse::parse(lex(&src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut node = term_value(&mint, &out, "main");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected structural dependency handler")
    };
    assert_ne!(
        handler.arms[0].effect.tracked,
        handler.arms[1].effect.tracked
    );
    assert_eq!(handler.discharges, [handler.arms[0].effect]);
}

#[test]
fn structurally_equivalent_dependencies_share_coverage_and_duplicates() {
    let declaration = "effect IO = { write: Nat -> (), flush: () -> () }";
    let interface = local_effect_interface(declaration, "IO");
    let first = named_io_artifact("first", &interface);
    let second = named_io_artifact("second", &interface);
    let build = |src: &str| {
        let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
        assert!(parsed.errors.is_empty());
        let mut mint = dummy_mint();
        let out = build_with_dependencies(
            &mut mint,
            parsed.stmts,
            &[checked(first.clone()), checked(second.clone())],
        );
        (mint, out)
    };

    let (mint, out) = build(
        "let main = fn n => handle first::!IO.write n with\n\
         | first::!IO.write value => ()\n\
         | second::!IO.flush unit => () end",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let mut node = term_value(&mint, &out, "main");
    while let TermKind::Fn { body, .. } = node {
        node = &body.kind;
    }
    let TermKind::Handle { handler, .. } = node else {
        panic!("expected dependency handler")
    };
    assert_ne!(
        handler.arms[0].effect.tracked,
        handler.arms[1].effect.tracked
    );
    assert_eq!(handler.discharges, [handler.arms[0].effect]);

    let (_, duplicate) = build(
        "let main = fn n => handle first::!IO.write n with\n\
         | first::!IO.write value => ()\n\
         | second::!IO.write again => ()\n\
         | second::!IO.flush unit => () end",
    );
    assert_eq!(
        duplicate
            .errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-arm"]
    );
}

/// Artifact headers are the semantic boundary, so importing exercises every
/// portable type/formula/row form rather than relying on source re-lowering.
#[test]
fn a_direct_only_interface_with_a_transitive_type_recovers_without_panicking() {
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".to_string(),
                version: "1.0.0".to_string(),
            },
            dependencies: vec![a::Dependency {
                name: "base".to_string(),
                version: "1.0.0".to_string(),
            }],
            values: Vec::new(),
            types: vec![a::DeclaredType {
                name: "dep@1.0.0::Wrapper".to_string(),
                params: Vec::new(),
                scheme: artifact_scheme(artifact_type(a::Type::Named {
                    name: "base@1.0.0::Hidden".to_string(),
                    args: vec![artifact_type(a::Type::Nat)],
                })),
            }],
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let parsed = parse::parse(lex("let value : dep::Wrapper = 0n", FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let mut built = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    let _inferred = inference::infer(&mint, &mut built.program, inference::Trace::Complete);
    assert_eq!(built.program.external_types.len(), 2);
}

#[test]
fn missing_transitive_type_applications_keep_distinct_effect_identities() {
    let alias = |name: &str, argument| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(artifact_type(a::Type::Named {
            name: "base@1.0.0::Hidden".into(),
            args: vec![artifact_type(argument)],
        })),
    };
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: vec![a::Dependency {
                name: "base".into(),
                version: "1.0.0".into(),
            }],
            values: Vec::new(),
            types: vec![
                alias("Natural", a::Type::Nat),
                alias("Text", a::Type::String),
            ],
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "module N =\n  effect Pick = { get: dep::Natural -> () }\nend\n\
               module S =\n  effect Pick = { get: dep::Text -> () }\nend";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let identities: Vec<_> = out.program.effect_ids.values().collect();
    assert_eq!(identities.len(), 2);
    assert_ne!(
        identities[0], identities[1],
        "Hidden Nat and Hidden String must not canonicalize as one recovery type"
    );
}

#[test]
fn an_unproductive_local_alias_has_a_finite_effect_recovery_identity() {
    let (mint, out) = build_src("type Loop = Loop\neffect Probe = { inspect: Loop -> () }");
    assert!(
        out.errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Circular { .. })),
        "{:#?}",
        out.errors
    );
    let interface = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| {
            (mint.name(*symbol) == "Probe").then(|| match identity {
                ruddy::types::EffectId::Structural { interface, .. } => interface,
                ruddy::types::EffectId::Pending(_) => panic!("effect identity was not finalized"),
            })
        })
        .expect("Probe has a recovery identity");
    assert!(
        interface.contains("inspect") && interface.contains('?'),
        "{interface}"
    );
}

#[test]
fn fixed_argument_recursive_types_terminate_during_effect_canonicalization() {
    let (mint, out) = build_src(
        "type T 'a = { next: T Nat }\n\
         effect E = { op: T String -> () }",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(
        out.program
            .effect_ids
            .keys()
            .filter(|symbol| mint.name(**symbol) == "E")
            .count(),
        1
    );
}

#[test]
fn direct_only_transitive_effects_keep_qualified_recovery_identity() {
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: vec![a::Dependency {
                name: "base".into(),
                version: "1.0.0".into(),
            }],
            values: Vec::new(),
            types: Vec::new(),
            effects: [
                ("Console", "base@1.0.0::Hidden"),
                ("Network", "base@1.0.0::Other"),
            ]
            .into_iter()
            .map(|(name, target)| a::DeclaredEffect {
                name: format!("dep@1.0.0::{name}"),
                identity: None,
                kind: a::EffectKind::Alias(vec![target.into()]),
            })
            .chain(std::iter::once(a::DeclaredEffect {
                name: "dep@1.0.0::Legacy".into(),
                identity: None,
                kind: a::EffectKind::Operations(Vec::new()),
            }))
            .collect(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let parsed = parse::parse(
        lex(
            "let f : () -> () + dep::!Console = fn x => x\n\
         let g : () -> () + dep::!Network = fn x => x\n\
         let h : () -> () + dep::!Legacy = fn x => x",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let dependency = checked(dependency);
    let imports = [DependencyImport {
        alias: "dep",
        artifact: &dependency,
    }];
    let mut out = build_with_dependency_imports(&mut mint, parsed.stmts, &imports, &[]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let recovered: Vec<_> = out
        .program
        .effect_ids
        .values()
        .filter(|identity| {
            matches!(
                identity,
                ruddy::types::EffectId::Structural { interface, .. }
                    if interface.starts_with("unresolved:base@1.0.0::")
            )
        })
        .collect();
    assert_eq!(recovered.len(), 2);
    assert_ne!(recovered[0], recovered[1]);
    for declaration in out.program.terms.values() {
        let annotation = declaration.annotation.as_ref().unwrap();
        let TypeKind::Arrow { effects, .. } = &annotation.ty.tracked else {
            panic!("expected arrow annotation")
        };
        assert_eq!(
            effects.effects.len(),
            1,
            "missing effects must not become purity"
        );
    }

    // Recovery identities must survive the full inference path too. In
    // particular, direct-only unresolved effects are semantic row labels, not
    // names which inference may discard or attempt to look up transitively.
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    assert_eq!(inferred.semantics().schemes().len(), 3);
    for scheme in inferred.semantics().schemes().values() {
        let ruddy::types::Ty::Arrow(_, _, effects) = &**scheme.body() else {
            panic!("expected inferred arrow")
        };
        assert_eq!(effects.labels.len(), 1, "inference lost recovered effect");
    }
}

#[test]
fn recovery_signatures_keep_every_normalized_source_form_structural() {
    let src = "effect IO = { run: () -> () }\n\
               type Record 'r = { value: Nat, \\hidden, ..'r }\n\
               type Choice 'r = #Some Nat | #None | \\#Hidden | ..'r\n\
               type Runner 'e = () -> () + \\!IO + ..'e\n\
               effect Alias = !IO\n\
               effect Probe = {\n\
                 record: Record { extra: String } -> (),\n\
                 choice: Choice (#Other Boolean) -> (),\n\
                 runner: Runner (!IO) -> (),\n\
                 anonymous: (() -> () + !IO (when _)) -> (),\n\
                 named: (() -> () + !IO (when 'p)) -> (),\n\
                 aliased: (() -> () + !Alias) -> (),\n\
                 recursive: (() -> () + !Probe) -> (),\n\
               }\n\
               let uses_alias : () -> () + !Alias = fn x => x";
    let (mint, out) = build_src(src);
    assert!(
        out.errors.iter().all(|error| {
            matches!(
                error.kind,
                ErrorKind::ImpureOperation { .. }
                    | ErrorKind::RepeatedRowField {
                        shape: Shape::Effect,
                        ..
                    }
            )
        }),
        "{:#?}",
        out.errors
    );
    assert_eq!(
        out.errors
            .iter()
            .filter(|error| matches!(
                error.kind,
                ErrorKind::RepeatedRowField {
                    shape: Shape::Effect,
                    ..
                }
            ))
            .count(),
        1,
        "effect-row arguments in operation signatures are validated"
    );
    assert!(
        out.program
            .effect_ids
            .keys()
            .any(|symbol| mint.name(*symbol) == "Probe"),
        "recovery still gives the malformed signatures an identity"
    );

    let TypeKind::Arrow { effects, .. } = &out.program.types[&type_symbol(&mint, &out, "Runner")]
        .value
        .tracked
    else {
        panic!("Runner is an arrow");
    };
    let absent = effects
        .effects
        .values()
        .next()
        .expect("the absent IO label");
    assert!(!absent.expanded());
    assert!(absent.name_span().width > 0);

    let TypeKind::Arrow { effects, .. } = &annotation_of(&mint, &out, "uses_alias").ty.tracked
    else {
        panic!("uses_alias is an arrow");
    };
    let expanded = effects.effects.values().next().expect("expanded IO label");
    assert!(expanded.expanded());
    assert!(expanded.name_span().width > 0);
}

#[test]
fn generated_and_malformed_imported_effect_keys_decode_exactly_and_totally() {
    let local = "effect IO = { write: Nat -> () }\n\
                 effect Wrap = { use: (() -> () + !IO) -> () }";
    let interface = |source: &str, effect: &str| {
        let (mint, out) = build_src(source);
        out.program
            .effect_ids
            .iter()
            .find_map(|(symbol, identity)| {
                (mint.name(*symbol) == effect).then(|| match identity {
                    ruddy::types::EffectId::Structural { interface, .. } => interface.clone(),
                    ruddy::types::EffectId::Pending(_) => panic!("pending effect identity"),
                })
            })
            .expect("effect identity")
    };
    let expected = interface(local, "Wrap");
    let io = interface(local, "IO");
    let dependency = effect_artifact("dep", &io);
    let parsed = parse::parse(
        lex(
            "effect Wrap = { use: (() -> () + dep::!IO) -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    let imported = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| (mint.name(*symbol) == "Wrap").then_some(identity))
        .expect("wrapper identity");
    assert!(matches!(
        imported,
        ruddy::types::EffectId::Structural { interface, .. } if interface == &expected
    ));

    // Artifact identity text is untrusted. Exercise every delimiter and bounds
    // failure in the decoder; all are exact opaque recovery identities.
    let _ = build_src("effect Open = { get: { x: Nat, .. } -> () }");

    for malformed in [
        "",
        "x",
        "0x",
        "0#",
        "0#x",
        "0#1",
        "0#999:x",
        "0#1:é;",
        "0#1:a|",
        "0#1:a|1",
        "0#1:a|9:x",
        "0#1:a|1:x?",
        "0#1:a|1:x>",
        "0#1:a|1:x>0",
        "0#1:a?",
        "1#1:a;",
        "0#1:a|1:x>1;",
        "0#18446744073709551615:x",
        "18446744073709551616#1:a;",
        "0#1:a|1:x>18446744073709551616;",
    ] {
        let dependency = effect_artifact("bad", malformed);
        let parsed = parse::parse(
            lex(
                "effect Wrap = { use: (() -> () + bad::!IO) -> () }",
                FileID::GENERATED,
            )
            .tokens,
        );
        let mut mint = dummy_mint();
        let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
        assert!(out.program.effect_ids.values().any(|identity| matches!(
            identity,
            ruddy::types::EffectId::Structural { name, .. } if name == "Wrap"
        )));
    }

    let mut semantic = effect_artifact("semantic", &io);
    semantic.header.types.push(a::DeclaredType {
        name: "semantic@1.0.0::Carrier".into(),
        params: Vec::new(),
        scheme: artifact_scheme(a::Type::Arrow(
            Box::new(artifact_unit()),
            Box::new(artifact_unit()),
            a::Row {
                labels: vec![
                    (
                        format!("IO\u{1f}{io}"),
                        a::RowField {
                            presence: a::Presence::Absent,
                            ty: artifact_unit(),
                        },
                    ),
                    (
                        "Ghost".into(),
                        a::RowField {
                            presence: a::Presence::Undecided,
                            ty: artifact_unit(),
                        },
                    ),
                ],
                rest: a::Rest::Closed,
            },
        )),
    });
    let parsed = parse::parse(
        lex(
            "effect Probe = { inspect: semantic::Carrier -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(semantic)]);
    assert!(
        out.program
            .effect_ids
            .keys()
            .any(|symbol| mint.name(*symbol) == "Probe")
    );
}

#[test]
fn imported_presence_variables_keep_alpha_correlation_in_effect_identity() {
    let mut dependency = effect_artifact("dep", "presence-correlation");
    // Two quantified presences per declaration, so a bound position is in
    // range whether the two labels share one or take one each.
    let declaration = |name: &str, left, right| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: a::Scheme {
            count: 2,
            presences: 2,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Struct(a::Row {
                labels: vec![
                    (
                        "x".into(),
                        a::RowField {
                            presence: left,
                            ty: a::Type::Nat,
                        },
                    ),
                    (
                        "y".into(),
                        a::RowField {
                            presence: right,
                            ty: a::Type::Nat,
                        },
                    ),
                ],
                rest: a::Rest::Closed,
            }),
        },
    };
    dependency.header.types = vec![
        declaration("Bound", a::Presence::Bound(0), a::Presence::Bound(0)),
        declaration("Var", a::Presence::Var(41), a::Presence::Var(41)),
        declaration("Independent", a::Presence::Bound(0), a::Presence::Bound(1)),
        declaration("Unknown", a::Presence::Undecided, a::Presence::Undecided),
    ];
    let parsed = parse::parse(
        lex(
            "module B = effect Probe = { op: dep::Bound -> () } end\n\
             module V = effect Probe = { op: dep::Var -> () } end\n\
             module I = effect Probe = { op: dep::Independent -> () } end\n\
             module U = effect Probe = { op: dep::Unknown -> () } end",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 4);
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    assert_ne!(identities[0], identities[3]);
}

#[test]
fn effect_identity_alpha_normalizes_presence_variables_and_keeps_correlation() {
    let source = "module P =\n\
                    effect Probe = { op: { x when 'p: Nat, y when 'p: Nat } -> () }\n\
                  end\n\
                  module Q =\n\
                    effect Probe = { op: { x when 'q: Nat, y when 'q: Nat } -> () }\n\
                  end\n\
                  module Independent =\n\
                    effect Probe = { op: { x when 'p: Nat, y when 'q: Nat } -> () }\n\
                  end";
    let (mint, out) = build_src(source);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 3);
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
}

#[test]
fn operation_inputs_and_outputs_share_one_presence_alpha_scope() {
    let source = "module Shared =
                    effect Probe = { op: { x when 'p: Nat } -> { y when 'p: Nat } }
                  end
                  module Renamed =
                    effect Probe = { op: { x when 'q: Nat } -> { y when 'q: Nat } }
                  end
                  module Independent =
                    effect Probe = { op: { x when 'p: Nat } -> { y when 'q: Nat } }
                  end";
    let (mint, out) = build_src(source);
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 3);
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
}

#[test]
fn imported_bound_presences_are_local_to_each_scheme_instantiation() {
    let mut dependency = effect_artifact("dep", "bound-scopes");
    let declaration = |name: &str| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: a::Scheme {
            count: 1,
            presences: 1,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: artifact_struct(vec![
                (
                    "x".into(),
                    a::RowField {
                        presence: a::Presence::Bound(0),
                        ty: a::Type::Nat,
                    },
                ),
                (
                    "y".into(),
                    a::RowField {
                        presence: a::Presence::Bound(0),
                        ty: a::Type::Nat,
                    },
                ),
            ]),
        },
    };
    dependency.header.types = vec![declaration("A"), declaration("B")];
    let parsed = parse::parse(
        lex(
            "module Imported = effect Probe = { op: { a: dep::A, b: dep::B } -> () } end
\
             module Independent = effect Probe = { op: { a: { x when 'p: Nat, y when 'p: Nat }, b: { x when 'q: Nat, y when 'q: Nat } } -> () } end
\
             module Correlated = effect Probe = { op: { a: { x when 'p: Nat, y when 'p: Nat }, b: { x when 'p: Nat, y when 'p: Nat } } -> () } end",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 3, "{:#?}", out.errors);
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
}

#[test]
fn imported_presence_vars_use_a_disjoint_recovery_variant() {
    let mut dependency = effect_artifact("dep", "recovered-presence");
    dependency.header.types.push(a::DeclaredType {
        name: "dep@1.0.0::Carrier".into(),
        params: Vec::new(),
        scheme: a::Scheme {
            count: 1,
            presences: 1,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: artifact_struct(vec![
                (
                    "bound".into(),
                    a::RowField {
                        presence: a::Presence::Bound(0),
                        ty: a::Type::Nat,
                    },
                ),
                (
                    "foreign".into(),
                    a::RowField {
                        presence: a::Presence::Var(0x8000_0000),
                        ty: a::Type::Nat,
                    },
                ),
            ]),
        },
    });
    let parsed = parse::parse(
        lex(
            "effect Probe = { op: dep::Carrier -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    let carrier = out
        .program
        .external_types
        .iter()
        .find_map(|(symbol, declaration)| {
            (mint.name(*symbol) == "dep@1.0.0::Carrier").then_some(declaration)
        })
        .expect("the imported carrier");
    let Ty::Struct(row) = &**carrier.scheme.body() else {
        panic!("carrier body was not a struct")
    };
    assert!(matches!(row.labels["bound"].presence, Presence::Bound(0)));
    assert!(matches!(
        row.labels["foreign"].presence,
        Presence::Recovered(0x8000_0000)
    ));
}

#[test]
fn effect_identity_uses_compact_exact_backreferences_for_branching_and_recursion() {
    let mut source = String::from("effect Branch0 = { op: () -> () }\n");
    for depth in 1..=30 {
        let previous = depth - 1;
        source.push_str(&format!(
            "effect Branch{depth} = {{ op: (() -> () + !Branch{previous}) -> (() -> () + !Branch{previous}) }}\n"
        ));
    }
    source.push_str(
        "module A =\n  effect Loop = { op: (() -> () + !Loop) -> (() -> () + !Loop) }\nend\n\
         module B =\n  effect Loop = { op: (() -> () + !Loop) -> (() -> () + !Loop) }\nend\n\
         module C =\n  effect Loop = { other: (() -> () + !Loop) -> (() -> () + !Loop) }\nend\n\
         module D =\n  effect Loop = { op: (() -> () + E::!Loop) -> (() -> () + E::!Loop) }\nend\n\
         module E =\n  effect Loop = { op: (() -> () + D::!Loop) -> (() -> () + D::!Loop) }\nend",
    );
    let (mint, out) = build_src(&source);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let branch = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| (mint.name(*symbol) == "Branch30").then_some(identity))
        .expect("deep branching effect identity");
    let ruddy::types::EffectId::Structural { interface, .. } = branch else {
        panic!("effect identity was not finalized")
    };
    assert!(
        interface.len() < 100_000,
        "shared depth-30 graph was expanded to {} bytes",
        interface.len()
    );

    let loops: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Loop")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(loops.len(), 5);
    assert_eq!(loops[0], loops[1], "equivalent recursive graphs differ");
    assert_eq!(loops[0], loops[3], "one- and two-state cycles differ");
    assert_eq!(loops[0], loops[4], "cycle entry point changed identity");
    assert_ne!(loops[0], loops[2], "operation names disappeared");
}

#[test]
fn source_and_imported_absent_effect_payloads_have_one_identity() {
    let io_source = "effect IO = { run: () -> () }";
    let io = local_effect_interface(io_source, "IO");
    let mut dependency = effect_artifact("dep", &io);
    dependency.header.types.push(a::DeclaredType {
        name: "dep@1.0.0::Carrier".into(),
        params: Vec::new(),
        scheme: artifact_scheme(a::Type::Arrow(
            Box::new(artifact_unit()),
            Box::new(artifact_unit()),
            a::Row {
                labels: vec![(
                    format!("IO\u{1f}{io}"),
                    a::RowField {
                        presence: a::Presence::Absent,
                        // An absent effect payload is semantically ignored.
                        ty: a::Type::String,
                    },
                )],
                rest: a::Rest::Undecided,
            },
        )),
    });
    let source = format!(
        "{io_source}\n\
         module Source =\n\
           effect Probe = {{ op: (() -> () + \\!IO + ..) -> () }}\n\
         end\n\
         module Imported =\n\
           effect Probe = {{ op: dep::Carrier -> () }}\n\
         end"
    );
    let parsed = parse::parse(lex(&source, FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0], identities[1]);
}

#[test]
fn recovery_row_tails_remain_unknown_in_every_canonical_shape() {
    let src = "module Closed =\n\
                 effect Struct = { get: { x: Nat } -> () }\n\
                 effect Cases = { get: (#X) -> () }\n\
                 effect Effects = { get: (() -> ()) -> () }\n\
               end\n\
               module Open =\n\
                 effect Struct = { get: { x: Nat, .. } -> () }\n\
                 effect Cases = { get: (#X | ..) -> () }\n\
                 effect Effects = { get: (() -> () + ..) -> () }\n\
               end\n\
               effect StructBoth = Closed::!Struct + Open::!Struct\n\
               effect CasesBoth = Closed::!Cases + Open::!Cases\n\
               effect EffectsBoth = Closed::!Effects + Open::!Effects";
    let (mint, out) = build_src(src);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["impure-operation", "impure-operation", "impure-operation"],
        "an unknown recovery tail must not collapse to a closed tail"
    );
    for name in ["Struct", "Cases", "Effects"] {
        let identities: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(identities.len(), 2);
        assert_ne!(identities[0], identities[1], "{name} tail was closed");
    }
}

#[test]
fn canonicalization_substitutes_struct_sum_and_effect_row_tails() {
    let src = "type Record 'r = { x: Nat, ..'r }\n\
               type Cases 'r = #X | ..'r\n\
               effect A = { a: () -> () }\n\
               effect B = { b: () -> () }\n\
               type Runs 'e = () -> () + ..'e\n\
               module Y =\n  effect RecordPick = { get: Record { y: Nat } -> () }\n  effect CasePick = { get: Cases (#Y) -> () }\n  effect RunPick = { get: Runs (!A) -> () }\nend\n\
               module Z =\n  effect RecordPick = { get: Record { z: Nat } -> () }\n  effect CasePick = { get: Cases (#Z) -> () }\n  effect RunPick = { get: Runs (!B) -> () }\nend";
    // An arrow nested in an operation's signature may carry effects; only the
    // operation's own arrow may not.
    let (mint, out) = built(src);
    for name in ["RecordPick", "CasePick", "RunPick"] {
        let identities: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(identities.len(), 2);
        assert_ne!(identities[0], identities[1], "{name} lost its row argument");
    }
}

#[test]
fn local_row_applications_flatten_records_sums_and_effects_without_losing_labels() {
    let src = "type Record 'r = { x: Nat, ..'r }\n\
               type Cases 'r = #X | ..'r\n\
               effect A = { a: () -> () }\n\
               effect B = { b: () -> () }\n\
               effect C = { c: () -> () }\n\
               type Runs 'r = () -> () + !A + ..'r\n\
               module Flat =\n  effect Record = { get: { x: Nat, y: Nat } -> () }\n  effect Cases = { get: (#X | #Y) -> () }\n  effect Runs = { get: (() -> () + !A + !B) -> () }\nend\n\
               module Composed =\n  effect Record = { get: Record { y: Nat } -> () }\n  effect Cases = { get: Cases (#Y) -> () }\n  effect Runs = { get: Runs (!B) -> () }\nend\n\
               module Different =\n  effect Record = { get: Record { z: Nat } -> () }\n  effect Cases = { get: Cases (#Z) -> () }\n  effect Runs = { get: Runs (!C) -> () }\nend";
    let (mint, out) = built(src);
    for name in ["Record", "Cases", "Runs"] {
        let ids: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], ids[1], "{name} composition was not flattened");
        assert_ne!(ids[0], ids[2], "{name} labels were lost while flattening");
    }
}

#[test]
fn imported_structural_identity_respects_effect_and_case_parameter_senses() {
    let mut dependency = effect_artifact("dep", "x");
    let present = |ty| a::RowField {
        presence: a::Presence::Present,
        ty,
    };
    let effect_row = || a::Row {
        labels: vec![("IO\u{1f}x".into(), present(artifact_unit()))],
        rest: a::Rest::Closed,
    };
    let case_row = || a::Row {
        labels: vec![("IO\u{1f}x".into(), present(artifact_unit()))],
        rest: a::Rest::Closed,
    };
    let parameter = |sense| a::Parameter {
        sense,
        lacks: Vec::new(),
        relevant: true,
    };
    let declared = |name: &str, params, count, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params,
        scheme: a::Scheme {
            count,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body,
        },
    };
    dependency.header.types = vec![
        declared("EffectRows", Vec::new(), 0, a::Type::Sum(effect_row())),
        declared(
            "Effects",
            vec![parameter(a::Sense::Effects)],
            1,
            a::Type::Arrow(
                Box::new(artifact_unit()),
                Box::new(artifact_unit()),
                a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Bound(0),
                },
            ),
        ),
        declared(
            "ComposedEffects",
            Vec::new(),
            0,
            a::Type::Named {
                name: "dep@1.0.0::Effects".into(),
                args: vec![a::Type::Named {
                    name: "dep@1.0.0::EffectRows".into(),
                    args: Vec::new(),
                }],
            },
        ),
        declared(
            "FlatEffects",
            Vec::new(),
            0,
            a::Type::Arrow(
                Box::new(artifact_unit()),
                Box::new(artifact_unit()),
                effect_row(),
            ),
        ),
        declared(
            "Cases",
            vec![parameter(a::Sense::Cases)],
            1,
            a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Bound(0),
            }),
        ),
        declared(
            "ComposedCases",
            Vec::new(),
            0,
            a::Type::Named {
                name: "dep@1.0.0::Cases".into(),
                args: vec![a::Type::Sum(case_row())],
            },
        ),
        declared("FlatCases", Vec::new(), 0, a::Type::Sum(case_row())),
    ];
    let parsed = parse::parse(
        lex(
            "module EC =\n\
               effect Probe = { op: dep::ComposedEffects -> () }\n\
             end\n\
             module EF =\n\
               effect Probe = { op: dep::FlatEffects -> () }\n\
             end\n\
             module CC =\n\
               effect Probe = { op: dep::ComposedCases -> () }\n\
             end\n\
             module CF =\n\
               effect Probe = { op: dep::FlatCases -> () }\n\
             end",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let identities: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Probe")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(identities.len(), 4);
    assert_eq!(identities[0], identities[1]);
    assert_eq!(identities[2], identities[3]);
    assert_ne!(identities[0], identities[2]);
}

#[test]
fn row_composition_is_flattened_across_local_and_imported_types() {
    let present = |ty| a::RowField {
        presence: a::Presence::Present,
        ty,
    };
    let named = |name: &str| {
        artifact_type(a::Type::Named {
            name: format!("dep@1.0.0::{name}"),
            args: Vec::new(),
        })
    };
    let declared = |name: &str, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(body),
    };
    let row = |label: &str| a::Row {
        labels: vec![(label.into(), present(artifact_type(artifact_unit())))],
        rest: a::Rest::Closed,
    };
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: vec![
                declared(
                    "RecordTail",
                    artifact_struct(vec![("y".into(), present(artifact_type(a::Type::Nat)))]),
                ),
                declared(
                    "Record",
                    artifact_struct(vec![
                        ("x".into(), present(artifact_type(a::Type::Nat))),
                        ("y".into(), present(artifact_type(a::Type::Nat))),
                    ]),
                ),
                declared(
                    "OtherRecord",
                    artifact_struct(vec![("z".into(), present(artifact_type(a::Type::Nat)))]),
                ),
                declared("CaseTail", artifact_type(a::Type::Sum(row("Y")))),
                declared(
                    "Cases",
                    artifact_type(a::Type::Sum(a::Row {
                        labels: row("X").labels.clone(),
                        rest: a::Rest::More(Box::new(row("Y"))),
                    })),
                ),
                declared("OtherCases", artifact_type(a::Type::Sum(row("Z")))),
                // Keep a named indirection in the artifact too: flattening is
                // deliberately performed after all regular nodes are built.
                declared("RecordAlias", named("Record")),
            ],
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "type LocalRecord = { x: Nat, y: Nat }\n\
               type LocalCases = #X | #Y\n\
               module L =\n  effect Record = { get: LocalRecord -> () }\n  effect Cases = { get: LocalCases -> () }\nend\n\
               module I =\n  effect Record = { get: dep::Record -> () }\n  effect Cases = { get: dep::Cases -> () }\nend\n\
               module D =\n  effect Record = { get: dep::OtherRecord -> () }\n  effect Cases = { get: dep::OtherCases -> () }\nend";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    for name in ["Record", "Cases"] {
        let ids: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], ids[1], "{name} composition was not flattened");
        assert_ne!(ids[0], ids[2], "{name} labels were lost while flattening");
    }
}

#[test]
fn structural_rows_mask_inner_duplicates_and_preserve_ordinary_separator_labels() {
    let present = |ty| a::RowField {
        presence: a::Presence::Present,
        ty,
    };
    let absent = || a::RowField {
        presence: a::Presence::Absent,
        ty: a::Type::Undecided,
    };
    let sum = |name: &str, field| {
        a::Type::Sum(a::Row {
            labels: vec![(name.into(), field)],
            rest: a::Rest::Closed,
        })
    };
    let declared = |name: &str, params, count, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params,
        scheme: a::Scheme {
            count,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body,
        },
    };
    let named = |name: &str, args| a::Type::Named {
        name: format!("dep@1.0.0::{name}"),
        args,
    };
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: vec![
                declared(
                    "Mask",
                    vec![a::Parameter {
                        sense: a::Sense::Cases,
                        lacks: vec!["X".into()],
                        relevant: true,
                    }],
                    1,
                    a::Type::Sum(a::Row {
                        labels: vec![("X".into(), absent())],
                        rest: a::Rest::Bound(0),
                    }),
                ),
                declared(
                    "Composed",
                    Vec::new(),
                    0,
                    named("Mask", vec![sum("X", present(a::Type::Nat))]),
                ),
                declared("Flat", Vec::new(), 0, sum("X", absent())),
                declared(
                    "Separator",
                    Vec::new(),
                    0,
                    sum("A\u{1f}x", present(a::Type::Nat)),
                ),
                declared(
                    "LooksGenerated",
                    Vec::new(),
                    0,
                    sum("!A<x>", present(a::Type::Nat)),
                ),
            ],
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "module A = effect Shadow = { get: dep::Composed -> () } end\n\
               module B = effect Shadow = { get: dep::Flat -> () } end\n\
               module C = effect Separator = { get: dep::Separator -> () } end\n\
               module D = effect Separator = { get: dep::LooksGenerated -> () } end";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let identities = |name: &str| {
        out.program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity.clone())
            .collect::<Vec<_>>()
    };
    let shadow = identities("Shadow");
    assert_eq!(shadow.len(), 2);
    assert_eq!(shadow[0], shadow[1], "outer absence did not mask inner X");
    let separator = identities("Separator");
    assert_eq!(separator.len(), 2);
    assert_ne!(
        separator[0], separator[1],
        "ordinary sum separator was mistaken for an effect identity"
    );
}

#[test]
fn absent_semantic_payloads_do_not_affect_structural_identity() {
    let absent = |ty| a::RowField {
        presence: a::Presence::Absent,
        ty,
    };
    let declared = |name: &str, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(body),
    };
    let types = [a::Type::Nat, a::Type::String]
        .into_iter()
        .enumerate()
        .flat_map(|(index, payload)| {
            let suffix = index + 1;
            [
                declared(
                    &format!("Record{suffix}"),
                    artifact_struct(vec![(
                        "hidden".into(),
                        absent(artifact_type(payload.clone())),
                    )]),
                ),
                declared(
                    &format!("Cases{suffix}"),
                    artifact_type(a::Type::Sum(a::Row {
                        labels: vec![("Hidden".into(), absent(artifact_type(payload)))],
                        rest: a::Rest::Closed,
                    })),
                ),
            ]
        })
        .collect();
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types,
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "module A =\n  effect Record = { get: dep::Record1 -> () }\n  effect Cases = { get: dep::Cases1 -> () }\nend\n\
               module B =\n  effect Record = { get: dep::Record2 -> () }\n  effect Cases = { get: dep::Cases2 -> () }\nend";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    for name in ["Record", "Cases"] {
        let ids: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ids[1], "absent {name} payload leaked into identity");
    }
}

#[test]
fn absent_imported_payloads_do_not_create_visible_quantifiers_or_effect_counts() {
    let mut dependency = effect_artifact("dep", "absent-payload-walks");
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::ghost".into(),
        scheme: a::Scheme {
            count: 1,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Struct(a::Row {
                labels: vec![(
                    "hidden".into(),
                    a::RowField {
                        presence: a::Presence::Absent,
                        ty: a::Type::Arrow(
                            Box::new(a::Type::Bound(0)),
                            Box::new(a::Type::Bound(0)),
                            a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Bound(0),
                            },
                        ),
                    },
                )],
                rest: a::Rest::Closed,
            }),
        },
    });
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::nested".into(),
        scheme: a::Scheme {
            count: 1,
            presences: 1,
            existentials: Vec::new(),
            formula: a::Formula::Bound(0),
            body: a::Type::Struct(a::Row {
                labels: vec![
                    (
                        "outer".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: a::Type::Struct(a::Row {
                                labels: vec![(
                                    "inner".into(),
                                    a::RowField {
                                        presence: a::Presence::Bound(0),
                                        ty: a::Type::Nat,
                                    },
                                )],
                                rest: a::Rest::Closed,
                            }),
                        },
                    ),
                    (
                        "gone".into(),
                        a::RowField {
                            presence: a::Presence::Absent,
                            ty: a::Type::Struct(a::Row {
                                labels: vec![(
                                    "invisible".into(),
                                    a::RowField {
                                        presence: a::Presence::Bound(0),
                                        ty: a::Type::Nat,
                                    },
                                )],
                                rest: a::Rest::Closed,
                            }),
                        },
                    ),
                ],
                rest: a::Rest::Closed,
            }),
        },
    });
    let parsed = parse::parse(
        lex(
            "let ghost = dep::ghost\nlet nested = dep::nested",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    let scheme = inferred
        .semantics()
        .schemes()
        .values()
        .next()
        .expect("ghost scheme");
    assert_eq!(scheme.count(), 0);
    assert_eq!(scheme.presences(), 0);
}

#[test]
fn local_and_imported_structural_types_share_one_canonical_encoding() {
    let unit = || artifact_type(artifact_unit());
    let present = |ty| a::RowField {
        presence: a::Presence::Present,
        ty,
    };
    let record = artifact_struct(vec![("x".into(), present(artifact_type(a::Type::Nat)))]);
    let sum = artifact_type(a::Type::Sum(a::Row {
        labels: vec![("X".into(), present(unit()))],
        rest: a::Rest::Closed,
    }));
    let declared = |name: &str, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(body),
    };
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: vec![
                declared("Record", record),
                declared("Cases", sum),
                declared(
                    "Alias",
                    artifact_type(a::Type::Named {
                        name: "dep@1.0.0::Record".into(),
                        args: Vec::new(),
                    }),
                ),
                declared(
                    "Different",
                    artifact_struct(vec![("z".into(), present(artifact_type(a::Type::Nat)))]),
                ),
            ],
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "type LocalRecord = { x: Nat }\n\
               type LocalCases = #X\n\
               type LocalAlias = LocalRecord\n\
               module L =\n  effect Record = { get: LocalRecord -> () }\n  effect Cases = { get: LocalCases -> () }\n  effect Alias = { get: LocalAlias -> () }\nend\n\
               module I =\n  effect Record = { get: dep::Record -> () }\n  effect Cases = { get: dep::Cases -> () }\n  effect Alias = { get: dep::Alias -> () }\nend\n\
               module D =\n  effect Record = { get: dep::Different -> () }\nend";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    for name in ["Cases", "Alias"] {
        let ids: Vec<_> = out
            .program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            ids[0], ids[1],
            "{name} differs across syntax/semantic boundary"
        );
    }
    let records: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Record")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0], records[1]);
    assert_ne!(records[0], records[2]);
}

#[test]
fn imported_interfaces_keep_applied_types_effects_and_alias_overlap_structural() {
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: vec![a::DeclaredType {
                name: "dep@1.0.0::Box".into(),
                params: vec![a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: true,
                }],
                scheme: a::Scheme {
                    count: 1,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: a::Formula::True,
                    body: artifact_type(a::Type::Bound(0)),
                },
            }],
            effects: [
                ("IO", "read:{}->Nat"),
                ("Net", "send:String->{}"),
                ("Log", "write:Nat->{}"),
            ]
            .into_iter()
            .map(|(name, interface)| a::DeclaredEffect {
                name: format!("dep@1.0.0::{name}"),
                identity: Some(a::EffectIdentity {
                    name: name.into(),
                    interface: interface.into(),
                }),
                kind: a::EffectKind::Operations(Vec::new()),
            })
            .chain(["A", "B"].into_iter().map(|name| a::DeclaredEffect {
                name: format!("dep@1.0.0::{name}"),
                identity: None,
                kind: a::EffectKind::Alias(vec!["dep@1.0.0::Log".into()]),
            }))
            .collect(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "type Wrap 'a = dep::Box 'a\n\
               type Deep 'a = Wrap (Wrap 'a)\n\
               module N =\n  effect Pick = { get: Wrap Nat -> () }\n  effect DeepPick = { get: Deep Nat -> () }\n  effect Recover = { run: (() -> () + dep::!IO) -> () }\nend\n\
               module S =\n  effect Pick = { get: Wrap String -> () }\n  effect DeepPick = { get: Deep String -> () }\n  effect Recover = { run: (() -> () + dep::!Net) -> () }\nend\n\
               effect Both = dep::!A + dep::!B";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-case"]
    );
    let identities = |name: &str| {
        out.program
            .effect_ids
            .iter()
            .filter(|(symbol, _)| mint.name(**symbol) == name)
            .map(|(_, identity)| identity)
            .collect::<Vec<_>>()
    };
    let picks = identities("Pick");
    assert_ne!(picks[0], picks[1], "Box Nat and Box String stay distinct");
    let deep_picks = identities("DeepPick");
    assert_ne!(
        deep_picks[0], deep_picks[1],
        "nested local aliases preserve imported application arguments"
    );
    let recovered = identities("Recover");
    assert_ne!(
        recovered[0], recovered[1],
        "imported effect ids stay distinct"
    );
    assert_eq!(out.errors[0].span.start, src.rfind("dep::!B").unwrap());
}

#[test]
fn imported_effects_in_recovery_signatures_compare_structurally() {
    let first = checked(effect_artifact("first", "read:{}->Nat"));
    let second = checked(effect_artifact("second", "read:{}->Nat"));
    let distinct = checked(effect_artifact("distinct", "read:{}->String"));
    let imports = [
        DependencyImport {
            alias: "first",
            artifact: &first,
        },
        DependencyImport {
            alias: "second",
            artifact: &second,
        },
        DependencyImport {
            alias: "distinct",
            artifact: &distinct,
        },
    ];
    let src = "module A =\n  effect Recover = { run: (() -> () + first::!IO) -> () }\nend\n\
               module B =\n  effect Recover = { run: (() -> () + second::!IO) -> () }\nend\n\
               module C =\n  effect Recover = { run: (() -> () + distinct::!IO) -> () }\nend";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependency_imports(&mut mint, parsed.stmts, &imports, &[]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let recover: Vec<_> = out
        .program
        .effect_ids
        .iter()
        .filter(|(symbol, _)| mint.name(**symbol) == "Recover")
        .map(|(_, identity)| identity)
        .collect();
    assert_eq!(recover.len(), 3);
    assert_eq!(recover[0], recover[1]);
    assert_ne!(recover[0], recover[2]);
}

#[test]
fn imported_declared_types_exercise_every_semantic_identity_form() {
    let field = |presence, ty| a::RowField { presence, ty };
    let unit = || artifact_type(artifact_unit());
    let row_type = |name: &str, labels, rest| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(artifact_type(a::Type::Sum(a::Row { labels, rest }))),
    };
    let declared = |name: &str, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(body),
    };
    // A bound position has to sit inside its scheme's quantifier space, so
    // the forms that use one quantify enough positions to hold it.
    let quantified = |name: &str, count, presences, body| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: a::Scheme {
            count,
            presences,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body,
        },
    };
    let mut types = vec![
        declared("Int", artifact_type(a::Type::Int)),
        declared("Real", artifact_type(a::Type::Real)),
        declared("Boolean", artifact_type(a::Type::Boolean)),
        declared("Var", artifact_type(a::Type::Var(7))),
        declared(
            "Rigid",
            artifact_type(a::Type::Rigid {
                id: 8,
                name: "rigid".into(),
            }),
        ),
        declared("Undecided", artifact_type(a::Type::Undecided)),
        quantified("Bound", 11, 0, artifact_type(a::Type::Bound(10))),
        declared(
            "Arrow",
            artifact_type(a::Type::Arrow(
                Box::new(artifact_type(a::Type::Int)),
                Box::new(artifact_type(a::Type::Boolean)),
                a::Row {
                    labels: vec![(
                        "IO\u{1f}run:{}->{}".into(),
                        field(a::Presence::Present, unit()),
                    )],
                    rest: a::Rest::Closed,
                },
            )),
        ),
        quantified(
            "Fields",
            3,
            3,
            artifact_struct(vec![
                ("variable".into(), field(a::Presence::Var(1), unit())),
                ("bound".into(), field(a::Presence::Bound(2), unit())),
                ("unknown".into(), field(a::Presence::Undecided, unit())),
            ]),
        ),
        quantified(
            "RowPresences",
            5,
            5,
            artifact_type(a::Type::Sum(a::Row {
                labels: vec![
                    ("Variable".into(), field(a::Presence::Var(3), unit())),
                    ("Bound".into(), field(a::Presence::Bound(4), unit())),
                    ("Unknown".into(), field(a::Presence::Undecided, unit())),
                ],
                rest: a::Rest::Closed,
            })),
        ),
        row_type("RowVar", Vec::new(), a::Rest::Var(5)),
        quantified(
            "RowBound",
            7,
            0,
            artifact_type(a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Bound(6),
            })),
        ),
        row_type(
            "RowRigid",
            Vec::new(),
            a::Rest::Rigid {
                id: 9,
                name: "tail".into(),
            },
        ),
        row_type("RowUnknown", Vec::new(), a::Rest::Undecided),
    ];
    // A malformed semantic row-composition cycle is still finite: flattening
    // visits the self edge once rather than recursing forever.
    types.push(declared(
        "RowCycle",
        artifact_struct(vec![("x".into(), field(a::Presence::Present, unit()))]),
    ));
    types.push(declared(
        "BareCycle",
        artifact_type(a::Type::Named {
            name: "dep@1.0.0::BareCycle".into(),
            args: Vec::new(),
        }),
    ));
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types,
            effects: Vec::new(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "effect Probe = { inspect: {\n\
                 int: dep::Int, real: dep::Real, boolean: dep::Boolean,\n\
                 variable: dep::Var, rigid: dep::Rigid, unknown: dep::Undecided,\n\
                 bound: dep::Bound, arrow: dep::Arrow,\n\
                 fields: dep::Fields, presences: dep::RowPresences,\n\
                 row_var: dep::RowVar, row_bound: dep::RowBound,\n\
                 row_rigid: dep::RowRigid, row_unknown: dep::RowUnknown,\n\
                 cycle: dep::RowCycle, bare_cycle: dep::BareCycle\n\
               } -> () }";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    assert_eq!(out.program.effect_ids.len(), 1);
}

#[test]
fn malformed_dependency_declarations_are_ignored_without_shadow_symbols() {
    let mut dependency = effect_artifact("dep", "read:{}->Nat");
    let value = a::Value {
        name: "dep@1.0.0::answer".into(),
        scheme: artifact_scheme(artifact_type(a::Type::Nat)),
    };
    dependency.header.values = vec![value.clone(), value];
    dependency.header.types.push(a::DeclaredType {
        name: "dep@1.0.0::".into(),
        params: Vec::new(),
        scheme: artifact_scheme(artifact_type(a::Type::Nat)),
    });
    dependency.header.types.push(a::DeclaredType {
        name: "other@1.0.0::Ignored".into(),
        params: Vec::new(),
        scheme: artifact_scheme(artifact_type(a::Type::Nat)),
    });
    dependency.header.effects.push(a::DeclaredEffect {
        name: "other@1.0.0::Ignored".into(),
        identity: Some(a::EffectIdentity {
            name: "Ignored".into(),
            interface: String::new(),
        }),
        kind: a::EffectKind::Operations(Vec::new()),
    });
    let parsed = parse::parse(lex("let value = dep::answer", FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(out.program.external_schemes.len(), 1);
    assert!(
        out.program
            .external_names
            .values()
            .all(|name| !name.contains("Ignored"))
    );
}

#[test]
fn imported_alias_cycles_recover_without_a_spurious_structural_identity() {
    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".into(),
                version: "1.0.0".into(),
            },
            dependencies: Vec::new(),
            values: Vec::new(),
            types: Vec::new(),
            effects: [("A", "dep@1.0.0::B"), ("B", "dep@1.0.0::A")]
                .into_iter()
                .map(|(name, target)| a::DeclaredEffect {
                    name: format!("dep@1.0.0::{name}"),
                    identity: None,
                    kind: a::EffectKind::Alias(vec![target.into()]),
                })
                .collect(),
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    let src = "type Runner = () -> () + dep::!A\n\
               effect Probe = { op: Runner -> () }\n\
               effect Local = dep::!A";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(out.program.effect_ids.len(), 1);
    assert!(matches!(effect_of(&mint, &out, "Local"), Effect::Alias(_)));
}

#[test]
fn ir_dependency_aliases_reject_reserved_identifiers() {
    let dependency = checked(effect_artifact("dep", "read:{}->Nat"));
    let imports = ["let", "1dep", "a-b"].map(|alias| DependencyImport {
        alias,
        artifact: &dependency,
    });
    let mut mint = dummy_mint();
    let out = build_with_dependency_imports(&mut mint, Vec::new(), &imports, &[]);
    assert_eq!(out.errors.len(), 3);
    for (error, alias) in out.errors.iter().zip(["let", "1dep", "a-b"]) {
        assert!(matches!(
            error,
            ruddy::ir::Error {
                kind: ErrorKind::InvalidDependencyAlias { alias: found },
                ..
            } if found == alias
        ));
    }
}

#[test]
fn duplicate_dependency_aliases_are_rejected_without_merging_roots() {
    let first = checked(effect_artifact("first", "read:{}->Nat"));
    let mut second = effect_artifact("second", "send:String->{}");
    second.header.effects[0].name = "second@1.0.0::Net".to_string();
    second.header.effects[0].identity.as_mut().unwrap().name = "Net".to_string();
    let second = checked(second);
    let imports = [
        DependencyImport {
            alias: "shared",
            artifact: &first,
        },
        DependencyImport {
            alias: "shared",
            artifact: &second,
        },
    ];
    let src = "effect Both = shared::!IO + shared::!Net";
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = dummy_mint();
    let out = build_with_dependency_imports(&mut mint, parsed.stmts, &imports, &[]);

    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-dependency-alias", "undefined-effect"]
    );
    assert!(
        out.program
            .external_names
            .values()
            .any(|name| name == "first@1.0.0::IO")
    );
    assert!(
        !out.program
            .external_names
            .values()
            .any(|name| name.starts_with("second@"))
    );
}

#[test]
fn dependency_interfaces_import_every_semantic_form() {
    let unit = || artifact_type(artifact_unit());
    let rich = artifact_struct(vec![(
        "x".to_string(),
        a::RowField {
            presence: a::Presence::Present,
            ty: artifact_type(a::Type::Named {
                name: "other@2.0.0::Remote".into(),
                args: Vec::new(),
            }),
        },
    )]);
    let formula = a::Formula::And(
        Box::new(a::Formula::Not(Box::new(a::Formula::Var(0)))),
        Box::new(a::Formula::Or(
            Box::new(a::Formula::Bound(1)),
            Box::new(a::Formula::Iff(
                Box::new(a::Formula::False),
                Box::new(a::Formula::Xor(
                    Box::new(a::Formula::True),
                    Box::new(a::Formula::Bound(0)),
                )),
            )),
        )),
    );
    let mut values = vec![a::Value {
        name: "dep@1.0.0::M::rich".to_string(),
        scheme: a::Scheme {
            count: 3,
            presences: 2,
            existentials: Vec::new(),
            formula,
            body: rich,
        },
    }];
    for (name, core) in [
        ("unit", artifact_unit()),
        ("nat", a::Type::Nat),
        ("int", a::Type::Int),
        ("real", a::Type::Real),
        ("string", a::Type::String),
        ("boolean", a::Type::Boolean),
        ("var", a::Type::Var(0)),
        ("bound", a::Type::Bound(0)),
        ("undecided", a::Type::Undecided),
        (
            "struct-absent-more",
            a::Type::Struct(a::Row {
                labels: vec![(
                    "gone".into(),
                    a::RowField {
                        presence: a::Presence::Absent,
                        ty: artifact_type(a::Type::Nat),
                    },
                )],
                rest: a::Rest::More(Box::new(a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Closed,
                })),
            }),
        ),
        (
            "struct-var",
            a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Var(0),
            }),
        ),
        (
            "struct-bound",
            a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Bound(0),
            }),
        ),
        (
            "struct-rigid",
            a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Rigid {
                    id: 7,
                    name: "r".into(),
                },
            }),
        ),
        (
            "struct-undecided",
            a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Undecided,
            }),
        ),
        (
            "sum-closed",
            a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Closed,
            }),
        ),
        (
            "sum-var",
            a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Var(0),
            }),
        ),
        (
            "sum-bound",
            a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Bound(0),
            }),
        ),
        (
            "sum-undecided",
            a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Undecided,
            }),
        ),
    ] {
        // A bound position needs a quantifier to stand in; the other forms
        // quantify nothing.
        let count = u32::from(name.contains("bound"));
        values.push(a::Value {
            name: format!("dep@1.0.0::{name}"),
            scheme: a::Scheme {
                count,
                ..artifact_scheme(artifact_type(core))
            },
        });
    }
    values.push(a::Value {
        name: "wrong@1.0.0::ignored".to_string(),
        scheme: artifact_scheme(unit()),
    });

    let dependency = a::UncheckedArtifact {
        header: a::Header {
            identity: a::Identity {
                name: "dep".to_string(),
                version: "1.0.0".to_string(),
            },
            dependencies: Vec::new(),
            values,
            types: vec![
                a::DeclaredType {
                    name: "dep@1.0.0::M::T".to_string(),
                    params: vec![
                        a::Parameter {
                            sense: a::Sense::Type,
                            lacks: vec!["x".to_string()],
                            relevant: true,
                        },
                        a::Parameter {
                            sense: a::Sense::Cases,
                            lacks: Vec::new(),
                            relevant: false,
                        },
                        a::Parameter {
                            sense: a::Sense::Effects,
                            lacks: Vec::new(),
                            relevant: true,
                        },
                    ],
                    scheme: artifact_scheme(artifact_type(a::Type::Named {
                        name: "dep@1.0.0::M::T".to_string(),
                        args: Vec::new(),
                    })),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructMore".into(),
                    params: Vec::new(),
                    scheme: a::Scheme {
                        count: 3,
                        presences: 3,
                        ..artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                            labels: vec![
                                (
                                    "present".into(),
                                    a::RowField {
                                        presence: a::Presence::Present,
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                ),
                                (
                                    "absent".into(),
                                    a::RowField {
                                        presence: a::Presence::Absent,
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                ),
                                (
                                    "var".into(),
                                    a::RowField {
                                        presence: a::Presence::Var(1),
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                ),
                                (
                                    "bound".into(),
                                    a::RowField {
                                        presence: a::Presence::Bound(2),
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                ),
                                (
                                    "unknown".into(),
                                    a::RowField {
                                        presence: a::Presence::Undecided,
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                ),
                            ],
                            rest: a::Rest::More(Box::new(a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Closed,
                            })),
                        })))
                    },
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructVar".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::Var(3),
                    }))),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructBound".into(),
                    params: vec![a::Parameter {
                        sense: a::Sense::Fields,
                        lacks: Vec::new(),
                        relevant: true,
                    }],
                    scheme: a::Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: artifact_type(a::Type::Struct(a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Bound(0),
                        })),
                    },
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructMissing".into(),
                    params: Vec::new(),
                    // A quantified tail no parameter of the declaration
                    // supplies: in range for the scheme, missing for the use.
                    scheme: a::Scheme {
                        count: 10,
                        ..artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Bound(9),
                        })))
                    },
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructRigid".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::Rigid {
                            id: 4,
                            name: "r".into(),
                        },
                    }))),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::StructUnknown".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::Undecided,
                    }))),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::RowLoop".into(),
                    params: vec![a::Parameter {
                        sense: a::Sense::Fields,
                        lacks: Vec::new(),
                        relevant: true,
                    }],
                    scheme: a::Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: artifact_type(a::Type::Struct(a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Bound(0),
                        })),
                    },
                },
                a::DeclaredType {
                    name: "dep@1.0.0::RowCycle".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_type(a::Type::Named {
                        name: "dep@1.0.0::RowLoop".into(),
                        args: vec![artifact_type(a::Type::Named {
                            name: "dep@1.0.0::RowCycle".into(),
                            args: Vec::new(),
                        })],
                    })),
                },
            ],
            effects: vec![
                a::DeclaredEffect {
                    name: "dep@1.0.0::M::Read".to_string(),
                    identity: Some(a::EffectIdentity {
                        name: "Read".to_string(),
                        interface: "get:{}->Nat".to_string(),
                    }),
                    kind: a::EffectKind::Operations(vec![
                        a::Operation {
                            selector: a::OperationSelector::Named("get".to_string()),
                            from: unit(),
                            to: artifact_type(a::Type::Nat),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("absent_more".into()),
                            from: artifact_type(a::Type::Struct(a::Row {
                                labels: vec![(
                                    "gone".into(),
                                    a::RowField {
                                        presence: a::Presence::Absent,
                                        ty: artifact_type(a::Type::Nat),
                                    },
                                )],
                                rest: a::Rest::More(Box::new(a::Row {
                                    labels: Vec::new(),
                                    rest: a::Rest::Closed,
                                })),
                            })),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("var".into()),
                            from: artifact_type(a::Type::Struct(a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Var(4),
                            })),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("bound".into()),
                            from: artifact_type(a::Type::Struct(a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Bound(4),
                            })),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("rigid".into()),
                            from: artifact_type(a::Type::Struct(a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Rigid {
                                    id: 4,
                                    name: "r".into(),
                                },
                            })),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("undecided".into()),
                            from: artifact_type(a::Type::Struct(a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Undecided,
                            })),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("named_more".into()),
                            from: artifact_type(a::Type::Named {
                                name: "dep@1.0.0::StructMore".into(),
                                args: Vec::new(),
                            }),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("named_var".into()),
                            from: artifact_type(a::Type::Named {
                                name: "dep@1.0.0::StructVar".into(),
                                args: Vec::new(),
                            }),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("named_bound".into()),
                            from: artifact_type(a::Type::Named {
                                name: "dep@1.0.0::StructBound".into(),
                                args: vec![artifact_type(a::Type::Struct(a::Row {
                                    labels: Vec::new(),
                                    rest: a::Rest::Closed,
                                }))],
                            }),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("named_rigid".into()),
                            from: artifact_type(a::Type::Named {
                                name: "dep@1.0.0::StructRigid".into(),
                                args: Vec::new(),
                            }),
                            to: unit(),
                        },
                        a::Operation {
                            selector: a::OperationSelector::Named("named_unknown".into()),
                            from: artifact_type(a::Type::Named {
                                name: "dep@1.0.0::StructUnknown".into(),
                                args: Vec::new(),
                            }),
                            to: unit(),
                        },
                    ]),
                },
                a::DeclaredEffect {
                    name: "dep@1.0.0::Alias".to_string(),
                    identity: None,
                    kind: a::EffectKind::Alias(vec![
                        "dep@1.0.0::M::Read".to_string(),
                        "other@2.0.0::IO".to_string(),
                    ]),
                },
            ],
        },
        lir: a::Lir {
            externs: Vec::new(),
            functions: Vec::new(),
            globals: Vec::new(),
        },
    };
    // A second header with the same root/name covers deterministic duplicate
    // handling; the first declaration remains the source-visible one.
    let mut duplicate = dependency.clone();
    duplicate.header.values.clear();
    duplicate.header.types.clear();
    duplicate.header.effects.clear();
    let parsed = parse::parse(
        lex(
            "effect Probe = {\n\
               more: dep::StructMore -> (),\n\
               var: dep::StructVar -> (),\n\
               bound: (dep::StructBound {}) -> (),\n\
               missing: dep::StructMissing -> (),\n\
               rigid: dep::StructRigid -> (),\n\
               unknown: dep::StructUnknown -> (),\n\
               cycle: dep::RowCycle -> (),\n\
             }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(
        &mut mint,
        parsed.stmts,
        &[checked(dependency), checked(duplicate)],
    );

    assert_eq!(out.errors.len(), 1);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::DuplicateDependency { .. }
    ));
    // The declared type plus a recovery interface for the transitive `Remote`
    // reference whose header was deliberately not supplied.
    assert_eq!(out.program.external_types.len(), 10);
    assert_eq!(out.program.external_operations.len(), 11);
    assert!(
        out.program
            .external_names
            .values()
            .any(|name| name == "other@2.0.0::IO")
    );
    assert!(
        !out.program
            .external_names
            .values()
            .any(|name| name.contains("ignored"))
    );
}

#[test]
fn externs_bind_terms_without_becoming_initializer_groups() {
    let source = "extern log : String -> {} = \"console.log\"\nlet written = log \"hello\"";
    let (mint, output) = built(source);
    assert_eq!(output.program.externs.len(), 1);
    assert_eq!(output.program.terms.len(), 1);
    assert_eq!(groups(&mint, &output), vec![vec!["written"]]);
    let (symbol, external) = output
        .program
        .externs
        .first()
        .expect("the extern is lowered");
    assert_eq!(mint.name(*symbol), "log");
    assert_eq!(external.value.target.tracked, "console.log");
    assert_eq!(
        display_program(source),
        "extern log : String -> () = \"console.log\"\nlet written = log \"hello\""
    );

    for source in [
        "extern log : String -> () = \"console.log\"\nlet log = fn x => x",
        "let log = fn x => x\nextern log : String -> () = \"console.log\"",
    ] {
        let (_, output) = build_src(source);
        assert!(matches!(
            output.errors.first().map(|error| &error.kind),
            Some(ErrorKind::Duplicate {
                namespace: Namespace::Terms,
                ..
            })
        ));
    }
}

#[test]
fn extern_abi_retains_resolved_arity_callback_nesting_and_effects() {
    let (_, output) = build_src(
        "effect Log = { write: () -> () }\n\
         extern schedule : fn((fn(Nat, String) -> Boolean + !Log), Nat) -> (fn() -> String) + !Log = \"host.schedule\"",
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let external = &output.program.externs.first().unwrap().1.value;
    let ExternTypeKind::Function {
        parameters, result, ..
    } = &external.abi.tracked
    else {
        panic!("outer ABI function was not retained: {:#?}", external.abi);
    };
    assert_eq!(parameters.len(), 2);
    let ExternTypeKind::Group(callback) = &parameters[0].tracked else {
        panic!("callback grouping was not retained");
    };
    let ExternTypeKind::Function {
        parameters: callback_parameters,
        effects,
        ..
    } = &callback.tracked
    else {
        panic!("marked callback was not retained");
    };
    assert_eq!(callback_parameters.len(), 2);
    assert_eq!(
        effects.effects.len(),
        1,
        "callback effects were not resolved"
    );
    assert!(matches!(
        callback_parameters[0].tracked,
        ExternTypeKind::Ordinary(ruddy::tracking::Tracked {
            tracked: TypeKind::Prim(Prim::Nat),
            ..
        })
    ));
    let ExternTypeKind::Group(result) = &result.tracked else {
        panic!("marked result grouping was not retained");
    };
    let ExternTypeKind::Function {
        parameters: result_parameters,
        result: callback_result,
        ..
    } = &result.tracked
    else {
        panic!("marked result function was not retained");
    };
    assert!(
        result_parameters.is_empty(),
        "nullary ABI gained a host argument"
    );
    assert!(matches!(
        callback_result.tracked,
        ExternTypeKind::Ordinary(ruddy::tracking::Tracked {
            tracked: TypeKind::Prim(Prim::String),
            ..
        })
    ));
}

#[test]
fn invalid_names_inside_an_extern_abi_are_diagnosed_once_and_shape_is_retained() {
    let (_, output) =
        build_src("extern broken : fn(Missing) -> fn() -> Other + !Absent = \"host.broken\"");
    assert_eq!(output.errors.len(), 3, "{:#?}", output.errors);
    let abi = &output.program.externs.first().unwrap().1.value.abi;
    let ExternTypeKind::Function { result, .. } = &abi.tracked else {
        panic!("outer ABI shape was lost after an error");
    };
    assert!(matches!(result.tracked, ExternTypeKind::Function { .. }));
}

#[test]
fn externs_accept_every_annotation_type() {
    let (_, output) = build_src(
        "effect Log = { write: () -> () }\n\
         extern count : Nat = \"host.count\"\n\
         extern point : { x: Nat } = \"host.point\"\n\
         extern log : String -> () + !Log = \"console.log\"",
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    assert_eq!(output.program.externs.len(), 3);
}

#[test]
fn struct_tails_are_field_rows_and_only_accept_struct_rows() {
    let (mint, out) = built(
        "type WithX 'r = { x: Nat, ..'r }\n\
         type Plain = { y: Nat }\n\
         type Chain = Plain\n\
         type Box 'a = { value: 'a }\n\
         type Identity 'a = 'a\n\
         type Recursive = { next: Recursive }\n\
         type A = WithX { y: Nat }\n\
         type C = WithX {}\n\
         type D = WithX Plain\n\
         type E = WithX Chain\n\
         type F = WithX (Box Nat)\n\
         type G = WithX Recursive\n\
         type H = WithX (Identity Plain)\n\
         let open : WithX { y: Nat, .. } -> Nat = fn p => p.x",
    );
    let with_x = type_symbol(&mint, &out, "WithX");
    assert!(matches!(
        out.program.types[&with_x].params[0].kind,
        ParamKind::Fields { .. }
    ));

    for argument in ["Nat", "(Nat -> Nat)", "(#A | #B)"] {
        let (_, out) = build_src(&format!(
            "type WithX 'r = {{ x: Nat, ..'r }}\ntype Bad = WithX {argument}"
        ));
        let [error] = out.errors.as_slice() else {
            panic!("{argument}: {:#?}", out.errors);
        };
        assert!(matches!(
            error.kind,
            ErrorKind::NotARow {
                sense: Sense::Fields
            }
        ));
    }

    for source in [
        "type WithX 'r = { x: Nat, ..'r } type A = B type B = A type Bad = WithX A",
        "type WithX 'r = { x: Nat, ..'r } type A 'r = B 'r type B 'r = A 'r type Bad = WithX (A {})",
    ] {
        let (_, out) = build_src(source);
        assert!(!out.errors.is_empty());
    }

    let (_, out) = build_src("type Mixed 'a = { value: 'a, ..'a }");
    let [error] = out.errors.as_slice() else {
        panic!("{:#?}", out.errors);
    };
    assert!(matches!(
        error.kind,
        ErrorKind::MixedParameter {
            first: Sense::Type,
            second: Sense::Fields
        }
    ));
}

#[test]
fn sum_row_aliases_are_normalized_for_repetition_and_lacks() {
    for argument in ["Alias", "Transitive", "(AddB (#C | #A))"] {
        let source = format!(
            "type Cases 'r = #A | ..'r\n\
             type Alias = #B | #A\n\
             type Transitive = Alias\n\
             type AddB 'r = #B | ..'r\n\
             type Bad = Cases {argument}"
        );
        let (_, out) = build_src(&source);
        assert!(
            matches!(
                out.errors.as_slice(),
                [error]
                    if matches!(&error.kind, ErrorKind::RepeatedRowField { shape: Shape::Sum, field } if field == "A")
            ),
            "{argument}: {:#?}",
            out.errors
        );
    }

    // `Forward`'s parameter reaches `Cases` through another row constructor;
    // it inherits A as well as AddB's own B, in source order.
    let (_, out) = build_src(
        "type Cases 'r = #A | ..'r\n\
         type AddB 'r = #B | ..'r\n\
         type Forward 'r = Cases (AddB 'r)\n\
         type Bad = Forward (#C | #A | #B)",
    );
    assert!(
        matches!(
            out.errors.as_slice(),
            [error]
                if matches!(&error.kind, ErrorKind::RepeatedRowField { shape: Shape::Sum, field } if field == "A")
        ),
        "{:#?}",
        out.errors
    );

    // Effect rows use the same normalization machinery without borrowing the
    // sum or struct sense.
    let (_, out) = build_src(
        "effect Log = { write: () -> () }\n\
         type Eff 'e = Nat -> Nat + ..'e\n\
         type Forward 'e = Eff (!Log + ..'e)",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
}

fn forwarding_rows_artifact(include_cycle: bool) -> a::UncheckedArtifact {
    let mut dependency = effect_artifact("dep", "forwarding");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Id".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Struct(a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Bound(0),
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Identity".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Bound(0)),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::First".into(),
            params: vec![
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: true,
                },
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: false,
                },
            ],
            scheme: a::Scheme {
                count: 2,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Bound(0)),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::MissingSelect".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Named {
                name: "dep@1.0.0::Identity".into(),
                args: Vec::new(),
            })),
        },
        a::DeclaredType {
            name: "dep@1.0.0::GenericUnknown".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: false,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Undecided),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Twice".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::Identity".into(),
                    args: vec![artifact_type(a::Type::Named {
                        name: "dep@1.0.0::Identity".into(),
                        args: vec![artifact_type(a::Type::Bound(0))],
                    })],
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Chain".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::Id".into(),
                    args: vec![artifact_type(a::Type::Bound(0))],
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Second".into(),
            params: vec![
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: false,
                },
                a::Parameter {
                    sense: a::Sense::Fields,
                    lacks: Vec::new(),
                    relevant: true,
                },
            ],
            scheme: a::Scheme {
                count: 2,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::Chain".into(),
                    args: vec![artifact_type(a::Type::Bound(1))],
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::WithX".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: vec!["x".into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Struct(a::Row {
                    labels: vec![(
                        "x".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: artifact_type(a::Type::Nat),
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::MaybeX".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: vec!["x".into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Struct(a::Row {
                    labels: vec![(
                        "x".into(),
                        a::RowField {
                            presence: a::Presence::Undecided,
                            ty: artifact_type(a::Type::Nat),
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::AbsentX".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: vec!["x".into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Struct(a::Row {
                    labels: vec![(
                        "x".into(),
                        a::RowField {
                            presence: a::Presence::Absent,
                            ty: artifact_type(a::Type::Undecided),
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })),
            },
        },
    ];
    if include_cycle {
        dependency.header.types.extend([
            a::DeclaredType {
                name: "dep@1.0.0::Cycle".into(),
                params: Vec::new(),
                scheme: artifact_scheme(artifact_type(a::Type::Named {
                    name: "dep@1.0.0::Cycle".into(),
                    args: Vec::new(),
                })),
            },
            a::DeclaredType {
                name: "dep@1.0.0::BrokenSlot".into(),
                params: vec![a::Parameter {
                    sense: a::Sense::Fields,
                    lacks: Vec::new(),
                    relevant: true,
                }],
                scheme: a::Scheme {
                    count: 1,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: a::Formula::True,
                    body: a::Type::Struct(a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::Bound(9),
                    }),
                },
            },
        ]);
    }
    dependency
}

fn case_rows_artifact() -> a::UncheckedArtifact {
    let mut dependency = forwarding_rows_artifact(false);
    dependency.header.types.extend([
        a::DeclaredType {
            name: "dep@1.0.0::CaseAlias".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Sum(a::Row {
                labels: ["B", "A"]
                    .into_iter()
                    .map(|name| {
                        (
                            name.into(),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty: artifact_type(a::Type::Nat),
                            },
                        )
                    })
                    .collect(),
                rest: a::Rest::Closed,
            }))),
        },
        a::DeclaredType {
            name: "dep@1.0.0::CaseTransitive".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Named {
                name: "dep@1.0.0::CaseAlias".into(),
                args: Vec::new(),
            })),
        },
        a::DeclaredType {
            name: "dep@1.0.0::AddB".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Cases,
                lacks: vec!["B".into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Sum(a::Row {
                    labels: vec![(
                        "B".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: artifact_type(a::Type::Nat),
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::SelectSecond".into(),
            params: vec![
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: false,
                },
                a::Parameter {
                    sense: a::Sense::Cases,
                    lacks: vec!["B".into()],
                    relevant: true,
                },
            ],
            scheme: a::Scheme {
                count: 2,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::AddB".into(),
                    args: vec![artifact_type(a::Type::Bound(1))],
                }),
            },
        },
        // No parameters, despite the malformed semantic rest. The importer
        // must absorb slot zero rather than assigning it to a local parameter.
        a::DeclaredType {
            name: "dep@1.0.0::BrokenBare".into(),
            params: Vec::new(),
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Sum(a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Bound(0),
                })),
            },
        },
    ]);
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Named("cases".into()),
        from: artifact_type(a::Type::Named {
            name: "dep@1.0.0::AddB".into(),
            args: vec![artifact_type(a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Closed,
            }))],
        }),
        to: artifact_type(a::Type::Nat),
    });
    dependency
}

#[test]
fn imported_case_rows_are_normalized_and_bad_slots_do_not_escape() {
    let dependency = case_rows_artifact();
    let parsed = parse::parse(
        lex(
            "type Cases 'r = #A | ..'r\n\
             type BadDirect = Cases dep::CaseAlias\n\
             type BadTransitive = Cases dep::CaseTransitive\n\
             type BadParameterized = Cases (dep::AddB (#C | #A))\n\
             type BadSelected = Cases (dep::SelectSecond Nat (#C | #A))\n\
             type CasesB 'r = #B | ..'r\n\
             type Local 'r = { one: Cases 'r, two: CasesB dep::BrokenBare }\n\
             type Good = Local (#B)\n\
             effect UsesImportedCases = { run: dep::AddB (#C) -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert_eq!(out.errors.len(), 4, "{:#?}", out.errors);
    assert!(out.errors.iter().all(|error| matches!(
        &error.kind,
        ErrorKind::RepeatedRowField {
            shape: Shape::Sum,
            field,
        } if field == "A"
    )));
}

#[test]
fn imported_forwarding_aliases_classify_local_recursion() {
    let dependency = forwarding_rows_artifact(false);
    let parsed = parse::parse(
        lex(
            "type ViaBound = dep::Identity ViaBound\n\
             type ViaRow = dep::Id ViaRow\n\
             type ViaSelection = dep::Second Nat ViaSelection\n\
             type Endless = dep::WithX Endless\n\
             type Optional = dep::MaybeX Optional\n\
             type Absent = dep::AbsentX Absent\n\
             type ValidExit = dep::WithX (dep::Twice { y: Nat })\n\
             type ValidUnknown = dep::WithX (dep::GenericUnknown {})\n\
             type ValidMissing = dep::MissingSelect\n\
             type ValidSelection = dep::First Nat ValidSelection",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert_eq!(out.errors.len(), 6, "{:#?}", out.errors);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        [
            "circular-type",
            "circular-type",
            "circular-type",
            "endless-fields",
            "endless-fields",
            "circular-type",
        ]
    );
}

#[test]
fn malformed_named_applications_of_unequal_arity_are_not_congruent() {
    let mut dependency = effect_artifact("dep", "bad-arity");
    dependency.header.types = vec![a::DeclaredType {
        name: "dep@1.0.0::Box".into(),
        params: vec![a::Parameter {
            sense: a::Sense::Type,
            lacks: Vec::new(),
            relevant: true,
        }],
        scheme: a::Scheme {
            count: 1,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: artifact_struct(vec![(
                "value".into(),
                a::RowField {
                    presence: a::Presence::Present,
                    ty: a::Type::Bound(0),
                },
            )]),
        },
    }];
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::short".into(),
        scheme: artifact_scheme(a::Type::Named {
            name: "dep@1.0.0::Box".into(),
            args: Vec::new(),
        }),
    });
    let parsed = parse::parse(lex("let use : dep::Box Nat = dep::short", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(
        inferred
            .diagnostics()
            .steps()
            .iter()
            .all(|step| !matches!(step.rule, inference::Rule::Congruent)),
        "{:#?}",
        inferred.diagnostics().steps()
    );
    assert!(
        inferred
            .diagnostics()
            .steps()
            .iter()
            .any(|step| matches!(step.rule, inference::Rule::Unfold)),
        "{:#?}",
        inferred.diagnostics().steps()
    );
}

#[test]
fn malformed_nested_imported_applications_recover_during_effect_identity() {
    let mut dependency = effect_artifact("dep", "bad-nested-arity");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Box".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Bound(0),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Broken".into(),
            params: Vec::new(),
            scheme: artifact_scheme(a::Type::Named {
                name: "dep@1.0.0::Box".into(),
                args: Vec::new(),
            }),
        },
    ];
    let parsed = parse::parse(
        lex(
            "effect Local = { op: dep::Broken -> () }\nlet value = 1n",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(out.program.effect_ids.values().any(|identity| {
        matches!(identity, ruddy::types::EffectId::Structural { name, .. } if name == "Local")
    }));
}

#[test]
fn deep_acyclic_operation_effect_dependencies_use_a_bounded_stack() {
    std::thread::Builder::new()
        .name("deep-operation-effect-identity".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 512;

            let mut source = String::new();
            for index in 0..DEPTH - 1 {
                source.push_str(&format!(
                    "effect E{index} = {{ op: (() -> () + !E{}) -> () }}\n",
                    index + 1
                ));
            }
            source.push_str(&format!("effect E{} = {{ op: () -> () }}", DEPTH - 1));

            let parsed = parse::parse(lex(&source, FileID::GENERATED).tokens);
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let out = build(&mut mint, parsed.stmts);
            assert!(
                out.errors
                    .iter()
                    .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
                "{:#?}",
                out.errors
            );
            assert_eq!(out.program.effect_ids.len(), DEPTH);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("operation effect dependencies are iterative");
}

#[test]
fn deep_local_type_and_effect_identity_dependencies_use_a_bounded_stack() {
    std::thread::Builder::new()
        .name("deep-local-effect-identity".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 4_096;

            let mut source = String::new();
            for index in 0..DEPTH - 1 {
                source.push_str(&format!("type E{index} = E{}\n", index + 1));
            }
            source.push_str(&format!(
                "type E{} = () -> () + !Leaf\n\
                 effect Probe = {{ inspect: E0 -> () }}\n\
                 effect Leaf = {{ touch: () -> () }}",
                DEPTH - 1
            ));

            let parsed = parse::parse(lex(&source, FileID::GENERATED).tokens);
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let out = build(&mut mint, parsed.stmts);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
            assert!(out.program.effect_ids.iter().any(|(symbol, identity)| {
                mint.name(*symbol) == "Probe"
                    && matches!(identity, ruddy::types::EffectId::Structural { interface, .. } if interface.contains("Leaf"))
            }));
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("source type/effect identity dependencies are iterative");
}

#[test]
fn imported_effect_identity_graph_is_stack_safe_and_absorbs_growing_types() {
    std::thread::Builder::new()
        .name("imported-effect-identity-graph".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 4_096;

            let mut deep = a::Type::Nat;
            for _ in 0..DEPTH {
                deep = a::Type::Arrow(
                    Box::new(a::Type::Nat),
                    Box::new(deep),
                    // No structural separator: malformed artifact effect keys
                    // are recovery input, not a reason to panic.
                    a::Row {
                        labels: vec![(
                            "legacy".into(),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty: artifact_unit(),
                            },
                        )],
                        rest: a::Rest::Closed,
                    },
                );
            }
            let mut dependency = effect_artifact("dep", "identity-graph");
            dependency.header.types = vec![
                a::DeclaredType {
                    name: "dep@1.0.0::Deep".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(deep),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::Grow".into(),
                    params: vec![a::Parameter {
                        sense: a::Sense::Type,
                        lacks: Vec::new(),
                        relevant: true,
                    }],
                    scheme: a::Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: a::Type::Named {
                            name: "dep@1.0.0::Grow".into(),
                            args: vec![a::Type::Struct(a::Row {
                                labels: vec![(
                                    "next".into(),
                                    a::RowField {
                                        presence: a::Presence::Present,
                                        ty: a::Type::Bound(0),
                                    },
                                )],
                                rest: a::Rest::Closed,
                            })],
                        },
                    },
                },
            ];

            let parsed = parse::parse(
                lex(
                    "effect DeepProbe = { inspect: dep::Deep -> () }\n\
                     effect GrowProbe = { inspect: dep::Grow Nat -> () }",
                    FileID::GENERATED,
                )
                .tokens,
            );
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(
                out.errors
                    .iter()
                    .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
                "{:#?}",
                out.errors
            );
            for name in ["DeepProbe", "GrowProbe"] {
                assert!(
                    out.program
                        .effect_ids
                        .keys()
                        .any(|symbol| mint.name(*symbol) == name),
                    "{name} retained no recovery identity"
                );
            }
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("imported effect identity graph building is iterative and total");
}

#[test]
fn imported_recursive_instantiations_close_through_forwarding_aliases() {
    let mut dependency = effect_artifact("dep", "forwarding-loop");
    let parameter = || a::Parameter {
        sense: a::Sense::Type,
        lacks: Vec::new(),
        relevant: true,
    };
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Id".into(),
            params: vec![parameter()],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Bound(0),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Loop".into(),
            params: vec![parameter()],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Named {
                    name: "dep@1.0.0::Loop".into(),
                    args: vec![a::Type::Named {
                        name: "dep@1.0.0::Id".into(),
                        args: vec![a::Type::Bound(0)],
                    }],
                },
            },
        },
    ];
    let parsed = parse::parse(
        lex(
            "effect Probe = { inspect: dep::Loop Nat -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let interface = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| {
            (mint.name(*symbol) == "Probe").then(|| match identity {
                ruddy::types::EffectId::Structural { interface, .. } => interface,
                ruddy::types::EffectId::Pending(_) => panic!("pending identity"),
            })
        })
        .expect("Probe identity");
    assert!(interface.len() < 500, "{interface}");
}

#[test]
fn a_finite_imported_rotation_longer_than_256_states_remains_exact() {
    const PARAMETERS: usize = 257;

    let mut dependency = effect_artifact("dep", "long-rotation");
    let parameters = (0..PARAMETERS)
        .map(|_| a::Parameter {
            sense: a::Sense::Type,
            lacks: Vec::new(),
            relevant: true,
        })
        .collect::<Vec<_>>();
    let rotated = (1..PARAMETERS)
        .chain(std::iter::once(0))
        .map(|index| a::Type::Bound(index as u32))
        .collect();
    let rotate = a::DeclaredType {
        name: "dep@1.0.0::Rotate".into(),
        params: parameters,
        scheme: a::Scheme {
            count: PARAMETERS as u32,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Arrow(
                Box::new(a::Type::Bound(0)),
                Box::new(a::Type::Named {
                    name: "dep@1.0.0::Rotate".into(),
                    args: rotated,
                }),
                a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Closed,
                },
            ),
        },
    };
    // Every argument root is a different state of one 257-node recursive SCC.
    // Rotating those roots is not constructor growth: all states mutually
    // retain each other. The final state's marker still has to remain semantic.
    let ring = |prefix: &str, marker: a::Type| {
        let prefix = prefix.to_string();
        (0..PARAMETERS)
            .map(move |index| {
                let mut fields = vec![(
                    "next".into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: a::Type::Named {
                            name: format!("dep@1.0.0::{prefix}{}", (index + 1) % PARAMETERS),
                            args: Vec::new(),
                        },
                    },
                )];
                if index + 1 == PARAMETERS {
                    fields.push((
                        "marker".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: marker.clone(),
                        },
                    ));
                }
                a::DeclaredType {
                    name: format!("dep@1.0.0::{prefix}{index}"),
                    params: Vec::new(),
                    scheme: artifact_scheme(artifact_struct(fields)),
                }
            })
            .collect::<Vec<_>>()
    };
    let alias = |name: &str, prefix: &str| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(a::Type::Named {
            name: "dep@1.0.0::Rotate".into(),
            args: (0..PARAMETERS)
                .map(|index| a::Type::Named {
                    name: format!("dep@1.0.0::{prefix}{index}"),
                    args: Vec::new(),
                })
                .collect(),
        }),
    };
    dependency.header.types = std::iter::once(rotate)
        .chain(ring("StringRing", a::Type::String))
        .chain(ring("BooleanRing", a::Type::Boolean))
        .chain([
            alias("Strings", "StringRing"),
            alias("Booleans", "BooleanRing"),
        ])
        .collect();

    let parsed = parse::parse(
        lex(
            "effect StringProbe = { inspect: dep::Strings -> () }\n\
             effect BooleanProbe = { inspect: dep::Booleans -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let interface = |name: &str| {
        out.program
            .effect_ids
            .iter()
            .find_map(|(symbol, identity)| {
                (mint.name(*symbol) == name).then(|| match identity {
                    ruddy::types::EffectId::Structural { interface, .. } => interface,
                    ruddy::types::EffectId::Pending(_) => panic!("pending effect identity"),
                })
            })
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    assert_ne!(
        interface("StringProbe"),
        interface("BooleanProbe"),
        "the differing 257th rotation state is semantic"
    );
}

#[test]
fn structurally_growing_imported_rotation_recovers_without_a_depth_cap() {
    let mut dependency = effect_artifact("dep", "growing-rotation");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Grow".into(),
            params: vec![
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: true,
                },
                a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: true,
                },
            ],
            scheme: a::Scheme {
                count: 2,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Named {
                    name: "dep@1.0.0::Grow".into(),
                    args: vec![
                        a::Type::Bound(1),
                        artifact_struct(vec![(
                            "next".into(),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty: a::Type::Bound(0),
                            },
                        )]),
                    ],
                },
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Loop".into(),
            params: Vec::new(),
            scheme: artifact_scheme(a::Type::Arrow(
                Box::new(a::Type::Nat),
                Box::new(a::Type::Named {
                    name: "dep@1.0.0::Loop".into(),
                    args: Vec::new(),
                }),
                a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Closed,
                },
            )),
        },
        // A malformed changing arity is finite here and must not be mistaken
        // for constructor growth merely because its active vectors differ.
        a::DeclaredType {
            name: "dep@1.0.0::Odd".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Named {
                    name: "dep@1.0.0::Odd".into(),
                    args: vec![a::Type::Bound(0), a::Type::Bound(0)],
                },
            },
        },
    ];
    let parsed = parse::parse(
        lex(
            "effect Probe = { inspect: dep::Grow String dep::Loop -> () }\n\
             effect OddProbe = { inspect: dep::Odd Nat -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let interface = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| {
            (mint.name(*symbol) == "Probe").then(|| match identity {
                ruddy::types::EffectId::Structural { interface, .. } => interface,
                ruddy::types::EffectId::Pending(_) => panic!("pending effect identity"),
            })
        })
        .expect("Probe has a recovery identity");
    assert!(interface.contains(":?"), "{interface}");
    assert!(interface.len() < 1_000, "{interface}");
}

#[test]
fn sequential_imported_instantiations_do_not_exhaust_the_active_recursion_limit() {
    const APPLICATIONS: usize = 300;

    let mut dependency = effect_artifact("dep", "wide-instantiations");
    let fields = (0..APPLICATIONS)
        .map(|index| {
            (
                format!("slot{index}"),
                a::RowField {
                    presence: a::Presence::Present,
                    ty: a::Type::Named {
                        name: "dep@1.0.0::Box".into(),
                        args: vec![artifact_struct(vec![(
                            format!("marker{index}"),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty: a::Type::Nat,
                            },
                        )])],
                    },
                },
            )
        })
        .collect();
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Box".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Bound(0),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Wide".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_struct(fields)),
        },
    ];

    let parsed = parse::parse(
        lex(
            "effect Probe = { inspect: dep::Wide -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let interface = out
        .program
        .effect_ids
        .iter()
        .find_map(|(symbol, identity)| {
            (mint.name(*symbol) == "Probe").then(|| match identity {
                ruddy::types::EffectId::Structural { interface, .. } => interface,
                ruddy::types::EffectId::Pending(_) => panic!("effect identity was not finalized"),
            })
        })
        .expect("Probe has a structural identity");
    assert!(interface.contains("marker0"), "{interface}");
    assert!(
        interface.contains(&format!("marker{}", APPLICATIONS - 1)),
        "late sequential applications must not recover as undecided: {interface}"
    );
}

#[test]
fn canonical_unknown_effect_interfaces_decode_as_graph_nodes() {
    let dependency = effect_artifact("dep", "0#u;");
    let parsed = parse::parse(lex("type Use = Nat", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.program.effect_ids.values().any(|identity| matches!(
        identity,
        ruddy::types::EffectId::Structural { interface, .. } if interface == "0#u;"
    )));
}

#[test]
fn malformed_interfaces_cannot_collide_with_encoded_ordinary_atoms() {
    let raw = effect_artifact("raw", "x");
    let ordinary = effect_artifact("ordinary", "0#18:opaque-interface:x;");
    let round_trip = effect_artifact("round", "0#o1:x;");
    let parsed = parse::parse(
        lex(
            "effect RawProbe = { op: (() -> () + raw::!IO) -> () }\n\
             effect OrdinaryProbe = { op: (() -> () + ordinary::!IO) -> () }\n\
             effect RoundProbe = { op: (() -> () + round::!IO) -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(
        &mut mint,
        parsed.stmts,
        &[checked(raw), checked(ordinary), checked(round_trip)],
    );
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let interface = |name: &str| {
        out.program
            .effect_ids
            .iter()
            .find_map(|(symbol, identity)| {
                (mint.name(*symbol) == name).then(|| match identity {
                    ruddy::types::EffectId::Structural { interface, .. } => interface,
                    ruddy::types::EffectId::Pending(_) => panic!("pending effect identity"),
                })
            })
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    assert_eq!(interface("RawProbe"), interface("RoundProbe"));
    assert_ne!(interface("RawProbe"), interface("OrdinaryProbe"));
}

#[test]
fn imported_effect_ids_are_normalized_before_alias_duplicate_checks() {
    let raw = effect_artifact("raw", "x");
    let encoded = effect_artifact("encoded", "0#o1:x;");
    let parsed =
        parse::parse(lex("effect Both = raw::!IO + encoded::!IO", FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(raw), checked(encoded)]);
    assert_eq!(
        out.errors
            .iter()
            .map(|error| error.kind.code())
            .collect::<Vec<_>>(),
        ["duplicate-case"]
    );
    let imported: Vec<_> = out
        .program
        .effect_ids
        .values()
        .filter_map(|identity| match identity {
            ruddy::types::EffectId::Structural { name, interface } if name == "IO" => {
                Some(interface.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(imported, ["0#o1:x;", "0#o1:x;"]);
}

#[test]
fn noncanonical_graph_encodings_are_opaque_without_identity_collisions() {
    for noncanonical in [
        "00#1:a;",                        // padded node number
        "0#01:a;",                        // padded label length
        "0#1:a;1#1:b;",                   // unreachable record
        "0#1:r|1:b>0|1:a>0;",             // noncanonical edge order
        "0#1:r|1:a>1|1:b>2;1#1:x;2#1:x;", // nonminimal duplicate states
        "0#1:r|1:a>2|1:b>1;1#1:y;2#1:x;", // construction numbering
        "0#1:a|1:x>00;",                  // padded backreference
    ] {
        let bad = effect_artifact("bad", noncanonical);
        let opaque = effect_artifact(
            "opaque",
            &format!("0#o{}:{noncanonical};", noncanonical.len()),
        );
        let parsed =
            parse::parse(lex("effect Both = bad::!IO + opaque::!IO", FileID::GENERATED).tokens);
        assert!(
            parsed.errors.is_empty(),
            "{noncanonical}: {:#?}",
            parsed.errors
        );
        let mut mint = dummy_mint();
        let out =
            build_with_dependencies(&mut mint, parsed.stmts, &[checked(bad), checked(opaque)]);
        assert_eq!(
            out.errors
                .iter()
                .map(|error| error.kind.code())
                .collect::<Vec<_>>(),
            ["duplicate-case"],
            "{noncanonical} was accepted as a graph rather than opaque text"
        );
    }
}

#[test]
fn malformed_effect_keys_cannot_collide_with_encoded_unknown_atoms() {
    let mut dependency = effect_artifact("dep", "key-collisions");
    let effectful = |key: &str| {
        a::Type::Arrow(
            Box::new(a::Type::Nat),
            Box::new(a::Type::Nat),
            a::Row {
                labels: vec![(
                    key.into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: artifact_unit(),
                    },
                )],
                rest: a::Rest::Closed,
            },
        )
    };
    dependency.header.types = [
        ("Raw", "IO"),
        ("Ordinary", "IO\u{1f}0#17:unknown-interface;"),
        ("Round", "IO\u{1f}0#u;"),
    ]
    .into_iter()
    .map(|(name, key)| a::DeclaredType {
        name: format!("dep@1.0.0::{name}"),
        params: Vec::new(),
        scheme: artifact_scheme(effectful(key)),
    })
    .collect();
    let parsed = parse::parse(
        lex(
            "effect RawProbe = { op: dep::Raw -> () }\n\
             effect OrdinaryProbe = { op: dep::Ordinary -> () }\n\
             effect RoundProbe = { op: dep::Round -> () }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        out.errors
            .iter()
            .all(|error| matches!(error.kind, ErrorKind::ImpureOperation { .. })),
        "{:#?}",
        out.errors
    );
    let interface = |name: &str| {
        out.program
            .effect_ids
            .iter()
            .find_map(|(symbol, identity)| {
                (mint.name(*symbol) == name).then(|| match identity {
                    ruddy::types::EffectId::Structural { interface, .. } => interface,
                    ruddy::types::EffectId::Pending(_) => panic!("pending effect identity"),
                })
            })
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    assert_ne!(interface("RawProbe"), interface("RoundProbe"));
    assert_eq!(interface("RoundProbe"), interface("OrdinaryProbe"));
}

#[test]
fn forwarded_field_addition_respects_outer_absent_shadowing() {
    let mut dependency = effect_artifact("dep", "shadow");
    dependency.header.types = vec![a::DeclaredType {
        name: "dep@1.0.0::Shadow".into(),
        params: vec![a::Parameter {
            sense: a::Sense::Fields,
            lacks: Vec::new(),
            relevant: true,
        }],
        scheme: a::Scheme {
            count: 1,
            presences: 0,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Struct(a::Row {
                labels: vec![(
                    "x".into(),
                    a::RowField {
                        presence: a::Presence::Absent,
                        ty: a::Type::Undecided,
                    },
                )],
                rest: a::Rest::More(Box::new(a::Row {
                    labels: vec![(
                        "x".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: a::Type::Nat,
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })),
            }),
        },
    }];
    let parsed = parse::parse(lex("type Loop = dep::Shadow Loop", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        matches!(
            out.errors.as_slice(),
            [error] if error.kind.code() == "circular-type"
        ),
        "{:#?}",
        out.errors
    );
}

#[test]
fn imported_field_rows_forward_outer_lacks_constraints() {
    let dependency = forwarding_rows_artifact(false);
    let parsed = parse::parse(
        lex(
            "type WithXY 'r = { x: Nat, y: Nat, ..'r }\n\
             type Direct 'r = WithXY (dep::Id { ..'r })\n\
             type Chained 'r = WithXY (dep::Chain { ..'r })\n\
             type Parameterized 'r = WithXY (dep::Second Nat { ..'r })\n\
             type BadDirect = Direct { y: String, x: String }\n\
             type BadChained = Chained { y: String, x: String }\n\
             type BadParameterized = Parameterized { y: String, x: String }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert_eq!(out.errors.len(), 3, "{:#?}", out.errors);
    assert!(
        out.errors.iter().all(|error| matches!(
            &error.kind,
            ErrorKind::RepeatedRowField {
                shape: Shape::Struct,
                field,
            } if field == "y"
        )),
        "{:#?}",
        out.errors
    );
}

fn deep_field_summary_artifact(depth: usize, cycle: bool) -> a::UncheckedArtifact {
    let mut dependency = effect_artifact("dep", "empty");
    dependency.header.types = (0..depth)
        .map(|index| {
            let stem = if cycle { "Cycle" } else { "Link" };
            let name = format!("dep@1.0.0::{stem}{index}");
            let params = if cycle {
                Vec::new()
            } else {
                vec![a::Parameter {
                    sense: a::Sense::Fields,
                    lacks: vec!["z".into()],
                    relevant: true,
                }]
            };
            let body = if !cycle && index + 1 == depth {
                a::Type::Struct(a::Row {
                    labels: vec![(
                        "z".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: artifact_type(a::Type::Nat),
                        },
                    )],
                    rest: a::Rest::Bound(0),
                })
            } else {
                let next = if index + 1 == depth { 0 } else { index + 1 };
                a::Type::Named {
                    name: format!("dep@1.0.0::{stem}{next}"),
                    args: if cycle {
                        Vec::new()
                    } else {
                        vec![artifact_type(a::Type::Bound(0))]
                    },
                }
            };
            a::DeclaredType {
                name,
                params,
                scheme: a::Scheme {
                    count: u32::from(!cycle),
                    presences: 0,
                    existentials: Vec::new(),
                    formula: a::Formula::True,
                    body: artifact_type(body),
                },
            }
        })
        .collect();
    dependency
}

#[test]
fn deeply_nested_imported_more_rows_are_imported_and_clamped_iteratively() {
    std::thread::Builder::new()
        .name("deep-imported-more-row".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            let mut row = a::Row {
                labels: Vec::new(),
                rest: a::Rest::Undecided,
            };
            for index in (0..DEPTH).rev() {
                row = a::Row {
                    labels: vec![(
                        format!("field{index:05}"),
                        a::RowField {
                            presence: if index == 0 {
                                a::Presence::Absent
                            } else {
                                a::Presence::Present
                            },
                            ty: a::Type::Nat,
                        },
                    )],
                    rest: a::Rest::More(Box::new(row)),
                };
            }
            let mut dependency = effect_artifact("dep", "deep-more");
            dependency.header.types = vec![a::DeclaredType {
                name: "dep@1.0.0::Deep".into(),
                params: Vec::new(),
                scheme: artifact_scheme(a::Type::Struct(row)),
            }];
            let parsed = parse::parse(lex("type Use = dep::Deep", FileID::GENERATED).tokens);
            let mut mint = dummy_mint();
            let out =
                build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency.clone())]);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
            let imported = out
                .program
                .external_types
                .values()
                .find(|declaration| matches!(&**declaration.scheme.body(), Ty::Struct(_)))
                .expect("the imported deep row");
            let Ty::Struct(row) = &**imported.scheme.body() else {
                unreachable!()
            };
            let Rest::More(row) = &row.rest else {
                panic!("the normalized composition marker was lost")
            };
            assert_eq!(row.labels.len(), DEPTH);
            assert_eq!(row.labels.first().unwrap().0, "field00000");
            assert_eq!(row.labels.last().unwrap().0, "field29999");
            assert!(matches!(
                row.labels["field00000"].presence,
                Presence::Absent
            ));
            assert!(matches!(row.rest, Rest::Undecided));
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep Rest::More import and clamping are stack safe and linear");
}

#[test]
fn deeply_nested_imported_semantics_are_preserved_on_a_small_stack() {
    std::thread::Builder::new()
        .name("deep-imported-semantics".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            const TYPE_DEPTH: usize = 30_000;
            const NAMED_DEPTH: usize = 30_000;

            let mut named = a::Type::Nat;
            for _ in 0..NAMED_DEPTH {
                named = a::Type::Named {
                    name: "dep@1.0.0::Phantom".into(),
                    args: vec![named],
                };
            }
            let mut payload = a::Type::Nat;
            for _ in 0..TYPE_DEPTH {
                payload = a::Type::Arrow(
                    Box::new(a::Type::Nat),
                    Box::new(payload),
                    a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::Closed,
                    },
                );
            }
            let mut tail = a::Row {
                labels: Vec::new(),
                rest: a::Rest::Closed,
            };
            for _ in 0..TYPE_DEPTH {
                tail = a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::More(Box::new(tail)),
                };
            }
            let body = a::Type::Arrow(
                Box::new(named),
                Box::new(a::Type::Struct(a::Row {
                    labels: vec![(
                        "deep".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: payload,
                        },
                    )],
                    rest: a::Rest::Closed,
                })),
                tail,
            );

            let mut formula = a::Formula::Bound(0);
            for depth in 0..DEPTH {
                formula = match depth % 3 {
                    0 => a::Formula::Not(Box::new(formula)),
                    1 => a::Formula::And(Box::new(formula), Box::new(a::Formula::True)),
                    _ => a::Formula::Or(Box::new(a::Formula::False), Box::new(formula)),
                };
            }

            let mut malformed_formula = a::Formula::Var(999);
            for _ in 0..DEPTH {
                malformed_formula = a::Formula::Not(Box::new(malformed_formula));
            }

            let mut dependency = effect_artifact("dep", "deep-semantics");
            dependency.header.types.push(a::DeclaredType {
                name: "dep@1.0.0::Deep".into(),
                params: Vec::new(),
                scheme: artifact_scheme(a::Type::Nat),
            });
            dependency.header.values.push(a::Value {
                name: "dep@1.0.0::deep_type".into(),
                scheme: artifact_scheme(body),
            });
            dependency.header.values.push(a::Value {
                name: "dep@1.0.0::deep_formula".into(),
                scheme: a::Scheme {
                    count: 1,
                    presences: 1,
                    existentials: Vec::new(),
                    formula,
                    body: a::Type::Nat,
                },
            });
            dependency.header.values.push(a::Value {
                name: "dep@1.0.0::malformed_formula".into(),
                scheme: a::Scheme {
                    count: 0,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: malformed_formula,
                    body: a::Type::Nat,
                },
            });

            // Importing alone only clamps the tree. Naming the value also
            // instantiates, substitutes, reads, and solves the deep formula.
            let parsed = parse::parse(
                lex(
                    "let answer = dep::deep_formula\n\
                     let typed = dep::deep_type\n\
                     let ignore = fn value => 0n\n\
                     let guarded = fn choice => match choice with\n\
                     | {left, ..} => ignore dep::deep_type\n\
                     | {right, ..} => 0n end\n\
                     let bad : Nat = dep::deep_type",
                    FileID::GENERATED,
                )
                .tokens,
            );
            let mut mint = dummy_mint();
            let dependencies = vec![checked(dependency)];
            let out = build_with_dependencies(&mut mint, parsed.stmts, &dependencies);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
            assert!(out.program.external_types.len() >= 2);
            assert_eq!(out.program.external_schemes.len(), 3);
            assert_eq!(
                out.program
                    .external_schemes
                    .values()
                    .filter(|scheme| scheme.formula().is_true())
                    .count(),
                2
            );

            // Naming both values semantically instantiates the deep formula
            // and the deep arrow/name/field-payload type on this small stack.
            let mut program = out.program;
            let inferred = inference::infer(&mint, &mut program, inference::Trace::Complete);
            assert_eq!(inferred.errors().len(), 1);
            assert_eq!(inferred.errors()[0].kind.code(), "type-mismatch");
            let guarded_step = inferred
                .diagnostics()
                .steps()
                .iter()
                .any(|step| matches!(step.effect, inference::Effect::Guarded { .. }));
            assert!(guarded_step);

            // Build both readings of the Types stage. Node meanings use
            // semantic Display and the raw dump deliberately does not use the
            // recursively derived Debug implementation.
            let dependency_declarations = IndexMap::new();
            let symbols = std::collections::HashMap::new();
            let cx = ruddy_debug::stage::Cx {
                files: &[],
                sources: &[],
                diagnostics: &[],
                bundle: None,
                program: Some(&program),
                inference: Some(&inferred),
                patterns: None,
                lir: None,
                artifact: None,
                linked: None,
                js: None,
                js_error: None,
                js_panicked: false,
                standard_library: &ruddy_debug::wire::StdConfig::Disabled,
                dependency_declarations: &dependency_declarations,
                dependency_aliases: &[],
                dependencies: &[],
                dependency_interfaces: &[],
                dependencies_valid: true,
                artifact_panicked: false,
                link_error: None,
                link_panicked: false,
                mint: Some(&mint),
                symbols: &symbols,
                micros: ruddy_debug::stage::Phases::default(),
                errored: true,
            };
            let spec = ruddy_debug::stage::REGISTRY
                .iter()
                .find(|spec| spec.id == "types")
                .expect("the Types stage spec");
            let stage = ruddy_debug::stage::types::build(spec, &cx);
            assert!(stage.nodes.iter().any(|node| node.text.contains("Phantom")));
            assert!(stage.debug.contains("externs:"));
            assert!(stage.debug.contains("Phantom"));
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep semantic imports and clamping do not recurse");
}

#[test]
fn malformed_imported_scheme_bounds_recover_for_types_and_values() {
    let malformed = a::Scheme {
        count: 1,
        presences: 2,
        existentials: Vec::new(),
        formula: a::Formula::And(Box::new(a::Formula::Bound(9)), Box::new(a::Formula::True)),
        body: a::Type::Struct(a::Row {
            labels: vec![(
                "x".into(),
                a::RowField {
                    presence: a::Presence::Bound(9),
                    ty: a::Type::Bound(0),
                },
            )],
            rest: a::Rest::Bound(9),
        }),
    };
    let mut dependency = effect_artifact("dep", "bad-bounds");
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::bad".into(),
        scheme: malformed.clone(),
    });
    let valid_body = a::Type::Struct(a::Row {
        labels: vec![(
            "x".into(),
            a::RowField {
                presence: a::Presence::Bound(0),
                ty: a::Type::Bound(1),
            },
        )],
        rest: a::Rest::Bound(1),
    });
    for (name, formula) in [
        ("not", a::Formula::Not(Box::new(a::Formula::Bound(9)))),
        (
            "and",
            a::Formula::And(Box::new(a::Formula::Bound(0)), Box::new(a::Formula::True)),
        ),
        (
            "or",
            a::Formula::Or(Box::new(a::Formula::True), Box::new(a::Formula::Bound(9))),
        ),
        (
            "iff",
            a::Formula::Iff(
                Box::new(a::Formula::Bound(0)),
                Box::new(a::Formula::Bound(0)),
            ),
        ),
        (
            "xor",
            a::Formula::Xor(
                Box::new(a::Formula::Bound(0)),
                Box::new(a::Formula::Bound(9)),
            ),
        ),
    ] {
        dependency.header.values.push(a::Value {
            name: format!("dep@1.0.0::{name}"),
            scheme: a::Scheme {
                count: 2,
                presences: 1,
                existentials: Vec::new(),
                formula,
                body: valid_body.clone(),
            },
        });
    }
    dependency.header.values.push(a::Value {
        name: "dep@1.0.0::presence_as_row".into(),
        scheme: a::Scheme {
            count: 2,
            presences: 1,
            existentials: Vec::new(),
            formula: a::Formula::True,
            body: a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Bound(0),
            }),
        },
    });
    dependency.header.types.push(a::DeclaredType {
        name: "dep@1.0.0::Broken".into(),
        params: Vec::new(),
        scheme: malformed,
    });
    // Malformed bounds never reach the IR: the tolerant boundary drops each
    // declaration that fails on its own and says why, and the ones whose
    // bounds are in range import unchanged.
    let (dependency, facts) = recovered(dependency);
    let discarded: Vec<&str> = facts
        .iter()
        .map(|fact| match fact {
            a::RecoveryFact::TypeDiscarded { name, .. }
            | a::RecoveryFact::ValueDiscarded { name, .. } => name.as_str(),
            other => panic!("{other:#?}"),
        })
        .collect();
    assert_eq!(
        discarded,
        [
            "dep@1.0.0::Broken",
            "dep@1.0.0::bad",
            "dep@1.0.0::not",
            "dep@1.0.0::or",
            "dep@1.0.0::xor",
            "dep@1.0.0::presence_as_row",
        ]
    );
    let parsed = parse::parse(
        lex(
            "let kept = dep::and\nlet also = dep::iff",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[dependency]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(out.program.external_schemes.len(), 2);
    assert!(
        out.program
            .external_schemes
            .values()
            .all(|scheme| scheme.count() == 2 && scheme.presences() == 1)
    );
}

#[test]
fn imported_interfaces_discard_foreign_solver_local_ids_before_inference() {
    let row = |presence, rest, ty| {
        a::Type::Struct(a::Row {
            labels: vec![("x".into(), a::RowField { presence, ty })],
            rest,
        })
    };
    let mut dependency = effect_artifact("dep", "foreign-locals");
    for (name, body) in [
        ("TypeVar", a::Type::Var(999)),
        (
            "TypeRigid",
            a::Type::Rigid {
                id: 999,
                name: "foreign".into(),
            },
        ),
        ("TypeUnknown", a::Type::Undecided),
        (
            "RestVar",
            row(a::Presence::Present, a::Rest::Var(999), a::Type::Nat),
        ),
        (
            "RestRigid",
            row(
                a::Presence::Present,
                a::Rest::Rigid {
                    id: 999,
                    name: "foreign".into(),
                },
                a::Type::Nat,
            ),
        ),
        (
            "PresenceVar",
            row(a::Presence::Var(999), a::Rest::Closed, a::Type::Nat),
        ),
    ] {
        dependency.header.types.push(a::DeclaredType {
            name: format!("dep@1.0.0::{name}"),
            params: Vec::new(),
            scheme: artifact_scheme(body),
        });
    }
    for (name, formula) in [
        ("formula_var", a::Formula::Var(999)),
        (
            "nested_formula_var",
            a::Formula::And(
                Box::new(a::Formula::True),
                Box::new(a::Formula::Not(Box::new(a::Formula::Var(999)))),
            ),
        ),
    ] {
        dependency.header.values.push(a::Value {
            name: format!("dep@1.0.0::{name}"),
            scheme: a::Scheme {
                count: 0,
                presences: 0,
                existentials: Vec::new(),
                formula,
                body: a::Type::Nat,
            },
        });
    }
    let a::EffectKind::Operations(operations) = &mut dependency.header.effects[0].kind else {
        unreachable!()
    };
    operations.push(a::Operation {
        selector: a::OperationSelector::Named("foreign".into()),
        from: a::Type::Arrow(
            Box::new(row(
                a::Presence::Var(999),
                a::Rest::Var(999),
                a::Type::Var(999),
            )),
            Box::new(row(
                a::Presence::Undecided,
                a::Rest::Rigid {
                    id: 999,
                    name: "foreign".into(),
                },
                a::Type::Rigid {
                    id: 999,
                    name: "foreign".into(),
                },
            )),
            a::Row {
                labels: Vec::new(),
                rest: a::Rest::Var(999),
            },
        ),
        to: a::Type::Rigid {
            id: 999,
            name: "foreign".into(),
        },
    });

    let parsed = parse::parse(
        lex(
            "type A = dep::TypeVar\n\
             type B = dep::TypeRigid\n\
             type C = dep::RestVar\n\
             type D = dep::RestRigid\n\
             type E = dep::PresenceVar\n\
             let a = dep::formula_var\n\
             let b = dep::nested_formula_var\n\
             let operation = dep::!IO.foreign (fn x => x)",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(
        out.program
            .external_schemes
            .values()
            .all(|scheme| scheme.formula().is_true())
    );
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(matches!(
        inferred.errors(),
        [error] if matches!(error.kind, inference::ErrorKind::Unhandled { .. })
    ));
}

#[test]
fn arrow_effect_more_rows_use_one_canonical_form_in_both_directions() {
    let effect = |name: &str| {
        (
            format!("{name}\u{1f}interface"),
            a::RowField {
                presence: a::Presence::Present,
                ty: artifact_unit(),
            },
        )
    };
    let plain = || {
        (
            "plain".into(),
            a::RowField {
                presence: a::Presence::Present,
                ty: artifact_unit(),
            },
        )
    };
    let arrow = |labels, rest| {
        a::Type::Arrow(
            Box::new(a::Type::Nat),
            Box::new(a::Type::Nat),
            a::Row { labels, rest },
        )
    };
    let mut dependency = effect_artifact("dep", "effect-more-equality");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Effectful".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Effects,
                lacks: vec!["A\u{1f}interface".into()],
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: arrow(vec![effect("A"), plain()], a::Rest::Bound(0)),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Composed".into(),
            params: Vec::new(),
            scheme: artifact_scheme(a::Type::Named {
                name: "dep@1.0.0::Effectful".into(),
                args: vec![a::Type::Sum(a::Row {
                    labels: vec![effect("B")],
                    rest: a::Rest::Closed,
                })],
            }),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Flat".into(),
            params: Vec::new(),
            scheme: artifact_scheme(arrow(
                vec![effect("A"), plain(), effect("B")],
                a::Rest::Closed,
            )),
        },
    ];
    dependency.header.values.extend([
        a::Value {
            name: "dep@1.0.0::composed".into(),
            scheme: artifact_scheme(a::Type::Named {
                name: "dep@1.0.0::Composed".into(),
                args: Vec::new(),
            }),
        },
        a::Value {
            name: "dep@1.0.0::flat".into(),
            scheme: artifact_scheme(a::Type::Named {
                name: "dep@1.0.0::Flat".into(),
                args: Vec::new(),
            }),
        },
    ]);
    let parsed = parse::parse(
        lex(
            "let forward : dep::Flat = dep::composed\n\
             let reverse : dep::Composed = dep::flat",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
}

#[test]
fn deep_equal_imported_types_unify_on_a_bounded_stack() {
    std::thread::Builder::new()
        .name("deep-equal-imported-solve".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            let deep = |hidden, bottom| {
                let mut named = bottom;
                let mut payload = a::Type::Nat;
                let mut effects = a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Closed,
                };
                for index in 0..DEPTH {
                    named = a::Type::Named {
                        name: "dep@1.0.0::Wrap".into(),
                        args: vec![named],
                    };
                    payload = a::Type::Struct(a::Row {
                        labels: vec![(
                            format!("field{index}"),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty: payload,
                            },
                        )],
                        rest: a::Rest::Closed,
                    });
                    effects = a::Row {
                        labels: Vec::new(),
                        rest: a::Rest::More(Box::new(effects)),
                    };
                }
                let mut ty = a::Type::Arrow(Box::new(named), Box::new(payload), effects);
                for _ in 1..DEPTH {
                    ty = a::Type::Arrow(
                        Box::new(a::Type::Nat),
                        Box::new(ty),
                        a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Closed,
                        },
                    );
                }
                a::Type::Struct(a::Row {
                    labels: vec![
                        (
                            "value".into(),
                            a::RowField {
                                presence: a::Presence::Present,
                                ty,
                            },
                        ),
                        (
                            "hidden".into(),
                            a::RowField {
                                presence: a::Presence::Absent,
                                ty: hidden,
                            },
                        ),
                    ],
                    rest: a::Rest::Closed,
                })
            };

            let mut dependency = effect_artifact("dep", "deep-equal");
            dependency.header.types = vec![
                a::DeclaredType {
                    name: "dep@1.0.0::Wrap".into(),
                    params: vec![a::Parameter {
                        sense: a::Sense::Type,
                        lacks: Vec::new(),
                        relevant: true,
                    }],
                    scheme: a::Scheme {
                        count: 1,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: a::Type::Bound(0),
                    },
                },
                a::DeclaredType {
                    name: "dep@1.0.0::A".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(deep(a::Type::Nat, a::Type::Nat)),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::B".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(deep(a::Type::String, a::Type::Nat)),
                },
                a::DeclaredType {
                    name: "dep@1.0.0::C".into(),
                    params: Vec::new(),
                    scheme: artifact_scheme(deep(a::Type::Nat, a::Type::String)),
                },
            ];
            dependency.header.values.extend([
                a::Value {
                    name: "dep@1.0.0::value".into(),
                    scheme: artifact_scheme(a::Type::Named {
                        name: "dep@1.0.0::B".into(),
                        args: Vec::new(),
                    }),
                },
                a::Value {
                    name: "dep@1.0.0::bad".into(),
                    scheme: artifact_scheme(a::Type::Named {
                        name: "dep@1.0.0::C".into(),
                        args: Vec::new(),
                    }),
                },
                a::Value {
                    name: "dep@1.0.0::accept".into(),
                    scheme: artifact_scheme(a::Type::Arrow(
                        Box::new(a::Type::Named {
                            name: "dep@1.0.0::A".into(),
                            args: Vec::new(),
                        }),
                        Box::new(a::Type::Nat),
                        a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Closed,
                        },
                    )),
                },
                // Grow the table before the 30,000 nested `Wrap`s disagree.
                // Per-depth snapshots would retain DEPTH × 5,000 slots; the
                // single outer transaction remains O(DEPTH + variables).
                a::Value {
                    name: "dep@1.0.0::padding".into(),
                    scheme: a::Scheme {
                        count: 5_000,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: a::Type::Nat,
                    },
                },
            ]);
            let parsed = parse::parse(
                lex(
                    "let padding = dep::padding\n\
                     let imported = dep::accept dep::value\n\
                     let mismatch = dep::accept dep::bad",
                    FileID::GENERATED,
                )
                .tokens,
            );
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let mut out =
                build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency.clone())]);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
            let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
            assert_eq!(inferred.errors().len(), 1, "{:#?}", inferred.errors());
            assert!(matches!(
                inferred.errors()[0].kind,
                inference::ErrorKind::Mismatch { .. }
            ));
            assert!(
                inferred
                    .diagnostics()
                    .steps()
                    .iter()
                    .any(|step| matches!(step.rule, inference::Rule::Arrow)),
                "the deep bodies were decomposed"
            );
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("successful deep Arrow/Named/Row unification is iterative");
}

#[test]
fn deep_alias_reentry_shares_one_congruence_transaction() {
    std::thread::Builder::new()
        .name("deep-alias-congruence-transaction".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            let named = |name: String| a::Type::Named {
                name,
                args: Vec::new(),
            };
            let wrapped = |name: String| a::Type::Named {
                name: "dep@1.0.0::Wrap".into(),
                args: vec![named(name)],
            };
            let mut dependency = effect_artifact("dep", "deep-alias-transaction");
            dependency.header.types.push(a::DeclaredType {
                name: "dep@1.0.0::Wrap".into(),
                params: vec![a::Parameter {
                    sense: a::Sense::Type,
                    lacks: Vec::new(),
                    relevant: true,
                }],
                scheme: a::Scheme {
                    count: 1,
                    presences: 0,
                    existentials: Vec::new(),
                    formula: a::Formula::True,
                    body: a::Type::Bound(0),
                },
            });
            for side in ["X", "Y"] {
                for index in 0..DEPTH {
                    let body = match index + 1 == DEPTH {
                        true if side == "X" => a::Type::Nat,
                        true => a::Type::String,
                        false => wrapped(format!("dep@1.0.0::{side}{}", index + 1)),
                    };
                    dependency.header.types.push(a::DeclaredType {
                        name: format!("dep@1.0.0::{side}{index}"),
                        params: Vec::new(),
                        scheme: artifact_scheme(body),
                    });
                }
            }
            dependency.header.values.extend([
                a::Value {
                    name: "dep@1.0.0::value".into(),
                    scheme: artifact_scheme(named("dep@1.0.0::Y0".into())),
                },
                a::Value {
                    name: "dep@1.0.0::accept".into(),
                    scheme: artifact_scheme(a::Type::Arrow(
                        Box::new(named("dep@1.0.0::X0".into())),
                        Box::new(a::Type::Nat),
                        a::Row {
                            labels: Vec::new(),
                            rest: a::Rest::Closed,
                        },
                    )),
                },
                a::Value {
                    name: "dep@1.0.0::padding".into(),
                    scheme: a::Scheme {
                        count: 5_000,
                        presences: 0,
                        existentials: Vec::new(),
                        formula: a::Formula::True,
                        body: a::Type::Nat,
                    },
                },
            ]);
            let parsed = parse::parse(
                lex(
                    "let padding = dep::padding\nlet mismatch = dep::accept dep::value",
                    FileID::GENERATED,
                )
                .tokens,
            );
            let mut mint = dummy_mint();
            let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
            let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
            assert_eq!(inferred.errors().len(), 1, "{:#?}", inferred.errors());
            assert!(matches!(
                inferred.errors()[0].kind,
                inference::ErrorKind::Mismatch { .. }
            ));
            assert!(
                inferred
                    .diagnostics()
                    .steps()
                    .iter()
                    .any(|step| matches!(step.rule, inference::Rule::Unfold))
            );
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("alias unfolding shares the explicit congruence transaction");
}

#[test]
fn recursive_imported_alias_reentry_compares_malformed_arities() {
    let recursive = |name: &str, extra: bool| {
        let mut args = vec![a::Type::Bound(0)];
        if extra {
            args.push(a::Type::Nat);
        }
        a::Type::Struct(a::Row {
            labels: vec![(
                "next".into(),
                a::RowField {
                    presence: a::Presence::Present,
                    ty: a::Type::Named {
                        name: name.into(),
                        args,
                    },
                },
            )],
            rest: a::Rest::Closed,
        })
    };
    let parameter = || a::Parameter {
        sense: a::Sense::Type,
        lacks: Vec::new(),
        relevant: false,
    };
    let mut dependency = effect_artifact("dep", "malformed-recursive-arity");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::A".into(),
            params: vec![parameter()],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: recursive("dep@1.0.0::A", true),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::B".into(),
            params: vec![parameter()],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: recursive("dep@1.0.0::B", false),
            },
        },
    ];
    let applied = |name: &str| a::Type::Named {
        name: name.into(),
        args: vec![a::Type::String],
    };
    dependency.header.values.extend([
        a::Value {
            name: "dep@1.0.0::value".into(),
            scheme: artifact_scheme(applied("dep@1.0.0::A")),
        },
        a::Value {
            name: "dep@1.0.0::accept".into(),
            scheme: artifact_scheme(a::Type::Arrow(
                Box::new(applied("dep@1.0.0::B")),
                Box::new(a::Type::Nat),
                a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Closed,
                },
            )),
        },
    ]);
    let parsed =
        parse::parse(lex("let compared = dep::accept dep::value", FileID::GENERATED).tokens);
    let mut mint = dummy_mint();
    let mut out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let inferred = inference::infer(&mint, &mut out.program, inference::Trace::Complete);
    assert!(inferred.errors().is_empty(), "{:#?}", inferred.errors());
    assert!(
        inferred
            .diagnostics()
            .steps()
            .iter()
            .any(|step| matches!(step.rule, inference::Rule::Assume))
    );
}

#[test]
fn deep_unequal_imported_struct_rows_unify_on_a_bounded_stack() {
    std::thread::Builder::new()
        .name("deep-unequal-imported-rows".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 30_000;
            for (actual_label, actual_presence, extra_absent, expected_errors) in [
                ("other", a::Presence::Present, false, 2),
                ("bottom", a::Presence::Absent, false, 1),
                ("bottom", a::Presence::Present, true, 0),
            ] {
                let nested = |label: &str, presence, extra_absent| {
                    let mut labels = vec![(
                        label.into(),
                        a::RowField {
                            presence,
                            ty: a::Type::Nat,
                        },
                    )];
                    if extra_absent {
                        labels.push((
                            "explicitly-absent".into(),
                            a::RowField {
                                presence: a::Presence::Absent,
                                ty: a::Type::String,
                            },
                        ));
                    }
                    let mut ty = a::Type::Struct(a::Row {
                        labels,
                        rest: a::Rest::Closed,
                    });
                    for _ in 0..DEPTH {
                        ty = a::Type::Struct(a::Row {
                            labels: vec![(
                                "payload".into(),
                                a::RowField {
                                    presence: a::Presence::Present,
                                    ty,
                                },
                            )],
                            rest: a::Rest::Closed,
                        });
                    }
                    ty
                };

                let mut dependency = effect_artifact("dep", "deep-unequal-rows");
                dependency.header.types = vec![
                    a::DeclaredType {
                        name: "dep@1.0.0::Expected".into(),
                        params: Vec::new(),
                        scheme: artifact_scheme(nested("bottom", a::Presence::Present, false)),
                    },
                    a::DeclaredType {
                        name: "dep@1.0.0::Actual".into(),
                        params: Vec::new(),
                        scheme: artifact_scheme(nested(
                            actual_label,
                            actual_presence,
                            extra_absent,
                        )),
                    },
                ];
                dependency.header.values.extend([
                    a::Value {
                        name: "dep@1.0.0::bad".into(),
                        scheme: artifact_scheme(a::Type::Named {
                            name: "dep@1.0.0::Actual".into(),
                            args: Vec::new(),
                        }),
                    },
                    a::Value {
                        name: "dep@1.0.0::accept".into(),
                        scheme: artifact_scheme(a::Type::Arrow(
                            Box::new(a::Type::Named {
                                name: "dep@1.0.0::Expected".into(),
                                args: Vec::new(),
                            }),
                            Box::new(a::Type::Nat),
                            a::Row {
                                labels: Vec::new(),
                                rest: a::Rest::Closed,
                            },
                        )),
                    },
                ]);
                let parsed = parse::parse(
                    lex("let mismatch = dep::accept dep::bad", FileID::GENERATED).tokens,
                );
                let mut mint = dummy_mint();
                let mut out =
                    build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
                assert!(out.errors.is_empty(), "{:#?}", out.errors);
                let inferred =
                    inference::infer(&mint, &mut out.program, inference::Trace::Complete);
                let messages: Vec<String> = inferred
                    .errors()
                    .iter()
                    .map(|error| error.kind.to_string())
                    .collect();
                assert_eq!(messages.len(), expected_errors, "{messages:#?}");
                assert!(
                    messages.iter().all(|message| message.contains("bottom"))
                        && (actual_label != "other"
                            || messages.iter().all(|message| message.contains("other"))),
                    "direction was lost: {messages:#?}"
                );
                assert!(
                    inferred
                        .diagnostics()
                        .steps()
                        .iter()
                        .any(|step| matches!(step.rule, inference::Rule::Struct)),
                    "the deep rows did not use the struct decomposition path"
                );
            }
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep unequal row payloads use one explicit continuation stack");
}

#[test]
fn deep_imported_field_summaries_are_stack_safe_when_unused_and_used() {
    std::thread::Builder::new()
        .name("deep-imported-field-summaries".into())
        .stack_size(1024 * 1024)
        .spawn(|| {
            const DEPTH: usize = 512;

            let dependency = deep_field_summary_artifact(DEPTH, false);
            let parsed = parse::parse(lex("let answer = 1n", FileID::GENERATED).tokens);
            let mut mint = dummy_mint();
            let unused = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(unused.errors.is_empty(), "{:#?}", unused.errors);

            let dependency = deep_field_summary_artifact(DEPTH, false);
            let parsed = parse::parse(
                lex(
                    "type WithZ 'r = { z: Nat, ..'r }\n\
                     type Bad = WithZ (dep::Link0 {})",
                    FileID::GENERATED,
                )
                .tokens,
            );
            let mut mint = dummy_mint();
            let used = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(
                used.errors.iter().any(|error| matches!(
                    &error.kind,
                    ErrorKind::RepeatedRowField {
                        shape: Shape::Struct,
                        field,
                    } if field == "z"
                )),
                "{:#?}",
                used.errors
            );

            let dependency = deep_field_summary_artifact(DEPTH, true);
            let parsed = parse::parse(lex("let answer = 1n", FileID::GENERATED).tokens);
            let mut mint = dummy_mint();
            let unused = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(unused.errors.is_empty(), "{:#?}", unused.errors);

            let dependency = deep_field_summary_artifact(DEPTH, true);
            let parsed = parse::parse(
                lex(
                    "type WithX 'r = { x: Nat, ..'r }\n\
                     type Safe = WithX dep::Cycle0",
                    FileID::GENERATED,
                )
                .tokens,
            );
            let mut mint = dummy_mint();
            let used = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(
                used.errors.iter().any(|error| matches!(
                    error.kind,
                    ErrorKind::NotARow {
                        sense: Sense::Fields
                    }
                )),
                "{:#?}",
                used.errors
            );
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep imported field summaries terminate without overflowing");
}

#[test]
fn deep_used_imported_forwarding_is_stack_safe_for_recursion_classification() {
    std::thread::Builder::new()
        .name("deep-used-imported-forwarding".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let dependency = deep_field_summary_artifact(2_048, false);
            let parsed = parse::parse(lex("type Used = dep::Link0 {}", FileID::GENERATED).tokens);
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
            assert!(out.errors.is_empty(), "{:#?}", out.errors);
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("a deep used forwarding chain is classified without overflowing");
}

#[test]
fn malformed_imported_alias_cycles_are_absorbed_without_recursing() {
    std::thread::Builder::new()
        .name("imported-field-cycle".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let dependency = forwarding_rows_artifact(true);
            let parsed = parse::parse(
                lex(
                    "type WithX 'r = { x: Nat, ..'r }\n\
                     type SafeCycle = WithX (dep::Id dep::Cycle)\n\
                     type SafeBrokenSlot = WithX (dep::BrokenSlot {})",
                    FileID::GENERATED,
                )
                .tokens,
            );
            assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
            let mut mint = dummy_mint();
            let (dependency, facts) = recovered(dependency);
            assert!(
                matches!(
                    facts.as_slice(),
                    [a::RecoveryFact::TypeDiscarded { name, .. }] if name == "dep@1.0.0::BrokenSlot"
                ),
                "{facts:#?}"
            );
            let out = build_with_dependencies(&mut mint, parsed.stmts, &[dependency]);
            // The cycle is refused as the non-row it is, and the slot the
            // boundary discarded is simply a name the dependency no longer
            // declares.
            assert!(
                matches!(
                    out.errors.as_slice(),
                    [cycle, missing]
                        if matches!(
                            cycle.kind,
                            ErrorKind::NotARow {
                                sense: Sense::Fields
                            }
                        ) && matches!(
                            &missing.kind,
                            ErrorKind::Undefined { name, namespace: Namespace::Types } if name == "BrokenSlot"
                        )
                ),
                "{:#?}",
                out.errors
            );
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("an imported alias cycle terminates without crashing");
}

#[test]
fn imported_struct_aliases_are_valid_field_row_arguments() {
    let mut dependency = effect_artifact("dep", "empty");
    dependency.header.types = vec![
        a::DeclaredType {
            name: "dep@1.0.0::Record".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_struct(vec![(
                "y".into(),
                a::RowField {
                    presence: a::Presence::Present,
                    ty: artifact_type(a::Type::Nat),
                },
            )])),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Fields".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: a::Type::Struct(a::Row {
                    labels: Vec::new(),
                    rest: a::Rest::Bound(0),
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::IdentityRow".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Bound(0)),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::ForwardRow".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::IdentityRow".into(),
                    args: vec![artifact_type(a::Type::Bound(0))],
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Cases".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Sum(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Closed,
            }))),
        },
        a::DeclaredType {
            name: "dep@1.0.0::MissingTail".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::Undecided,
            }))),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Nat".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Nat)),
        },
        a::DeclaredType {
            name: "dep@1.0.0::More".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Struct(a::Row {
                labels: Vec::new(),
                rest: a::Rest::More(Box::new(a::Row {
                    labels: vec![(
                        "x".into(),
                        a::RowField {
                            presence: a::Presence::Present,
                            ty: artifact_type(a::Type::Nat),
                        },
                    )],
                    rest: a::Rest::Closed,
                })),
            }))),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Cycle".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Named {
                name: "dep@1.0.0::Cycle".into(),
                args: Vec::new(),
            })),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Unknown".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Undecided)),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Pass".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Fields,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_type(a::Type::Named {
                    name: "dep@1.0.0::Fields".into(),
                    args: vec![artifact_type(a::Type::Bound(0))],
                }),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Box".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: true,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_struct(vec![(
                    "value".into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: artifact_type(a::Type::Bound(0)),
                    },
                )]),
            },
        },
        a::DeclaredType {
            name: "dep@1.0.0::Alias".into(),
            params: Vec::new(),
            scheme: artifact_scheme(artifact_type(a::Type::Named {
                name: "dep@1.0.0::Box".into(),
                args: vec![artifact_struct(vec![(
                    "x".into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: artifact_type(a::Type::Nat),
                    },
                )])],
            })),
        },
        a::DeclaredType {
            name: "dep@1.0.0::Phantom".into(),
            params: vec![a::Parameter {
                sense: a::Sense::Type,
                lacks: Vec::new(),
                relevant: false,
            }],
            scheme: a::Scheme {
                count: 1,
                presences: 0,
                existentials: Vec::new(),
                formula: a::Formula::True,
                body: artifact_struct(vec![(
                    "phantom".into(),
                    a::RowField {
                        presence: a::Presence::Present,
                        ty: artifact_type(a::Type::Nat),
                    },
                )]),
            },
        },
    ];
    let parsed = parse::parse(
        lex(
            "type WithX 'r = { x: Nat, ..'r }\n\
             type A = WithX dep::Record\n\
             type B = WithX (dep::Box Nat)\n\
             type Payload = WithX dep::Alias\n\
             type Phantom = WithX (dep::Phantom { x: Nat })\n\
             type Rows = WithX (dep::Pass { y: Nat })\n\
             type Direct = WithX (dep::IdentityRow { y: Nat })\n\
             type Local = { y: Nat }\n\
             type Forward = WithX (dep::ForwardRow Local)\n\
             type Unknown = WithX dep::Unknown\n\
             type MissingTail = WithX dep::MissingTail\n\
             type SumCases 'r = #A | ..'r\n\
             type ImportedCases = SumCases dep::Cases",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency.clone())]);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);

    for argument in ["dep::Cases", "dep::Cycle", "dep::Nat"] {
        let parsed = parse::parse(
            lex(
                &format!("type WithX 'r = {{ x: Nat, ..'r }}\ntype Bad = WithX {argument}"),
                FileID::GENERATED,
            )
            .tokens,
        );
        let mut mint = dummy_mint();
        let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency.clone())]);
        assert!(
            matches!(
                out.errors.as_slice(),
                [error] if matches!(error.kind, ErrorKind::NotARow { sense: Sense::Fields })
            ),
            "{argument}: {:#?}",
            out.errors
        );
    }

    let parsed = parse::parse(
        lex(
            "type WithX 'r = { x: Nat, ..'r }\n\
             type Bad = WithX (dep::Pass { x: Nat })\n\
             type AlsoBad = WithX dep::More",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        matches!(
            out.errors.as_slice(),
            [first, second]
                if [first, second].iter().all(|error| matches!(&error.kind, ErrorKind::RepeatedRowField { shape: Shape::Struct, field } if field == "x"))
        ),
        "{:#?}",
        out.errors
    );

    let mut dependency = effect_artifact("dep", "empty");
    dependency.header.types = vec![a::DeclaredType {
        name: "dep@1.0.0::Record".into(),
        params: Vec::new(),
        scheme: artifact_scheme(artifact_struct(vec![(
            "x".into(),
            a::RowField {
                presence: a::Presence::Present,
                ty: artifact_type(a::Type::Nat),
            },
        )])),
    }];
    let parsed = parse::parse(
        lex(
            "type WithX 'r = { x: Nat, ..'r }\ntype Bad = WithX dep::Record",
            FileID::GENERATED,
        )
        .tokens,
    );
    let mut mint = dummy_mint();
    let out = build_with_dependencies(&mut mint, parsed.stmts, &[checked(dependency)]);
    assert!(
        matches!(
            out.errors.as_slice(),
            [error] if matches!(&error.kind, ErrorKind::RepeatedRowField { shape: Shape::Struct, field } if field == "x")
        ),
        "{:#?}",
        out.errors
    );
}
