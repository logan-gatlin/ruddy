//! Tests for [`ruddy::inference`].

use std::rc::Rc;

use ruddy::{
    inference::{self, Effect, ErrorKind, Rule},
    ir::{self, Decl, Term, TermKind},
    parse,
    symbol::{Bundle, Mint, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::{Ty, TyVar},
};

fn dummy_mint() -> Mint {
    Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
}

/// Parse, lower and infer. Only the parse is required to be clean: some tests
/// are exactly about what inference does with a program lowering complained
/// about.
fn infer_src(src: &str) -> (Mint, ir::Output, inference::Output) {
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(
        parsed.errors.is_empty(),
        "unexpected parse errors: {:#?}",
        parsed.errors
    );
    let mut mint = dummy_mint();
    let mut out = ir::build(&mut mint, parsed.stmts);
    let inferred = inference::infer(&mint, &mut out.program);
    (mint, out, inferred)
}

/// [`infer_src`] for a program that should be entirely error-free.
fn inferred(src: &str) -> (Mint, ir::Output, inference::Output) {
    let (mint, out, output) = infer_src(src);
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert!(
        output.errors.is_empty(),
        "inference errors: {:#?}",
        output.errors
    );
    (mint, out, output)
}

fn symbol_named(mint: &Mint, symbols: impl IntoIterator<Item = Symbol>, name: &str) -> Symbol {
    symbols
        .into_iter()
        .find(|symbol| mint.name(*symbol) == name)
        .unwrap_or_else(|| panic!("nothing named {name}"))
}

/// How the scheme of a top-level definition prints — which is the shape the
/// debugger shows and the reader compares against, so it is what the tests
/// assert on.
fn scheme(mint: &Mint, inferred: &inference::Output, name: &str) -> String {
    let symbol = symbol_named(mint, inferred.schemes.keys().copied(), name);
    inferred.schemes[&symbol].to_string()
}

/// Every constraint generation emitted for one definition, rendered as
/// `code: constraint`, in the order it emitted them.
fn constraints(mint: &Mint, inferred: &inference::Output, name: &str) -> Vec<String> {
    let symbol = symbol_named(mint, inferred.constraints.keys().copied(), name);
    inferred.constraints[&symbol]
        .iter()
        .map(|constraint| format!("{}: {constraint}", constraint.kind.code()))
        .collect()
}

/// Every step the solver took for one definition, rendered as
/// `rule  goal => effect` and indented by how deep in a decomposition it was.
fn steps(mint: &Mint, inferred: &inference::Output, name: &str) -> Vec<String> {
    let symbol = symbol_named(
        mint,
        inferred.steps.iter().map(|step| step.definition),
        name,
    );
    inferred
        .steps
        .iter()
        .filter(|step| step.definition == symbol)
        .map(|step| {
            format!(
                "{}{}  {} => {}",
                "  ".repeat(step.depth as usize),
                step.rule.code(),
                step.goal,
                step.effect
            )
        })
        .collect()
}

fn term_decl<'a>(mint: &Mint, out: &'a ir::Output, name: &str) -> &'a Decl<Term> {
    let symbol = symbol_named(mint, out.program.terms.keys().copied(), name);
    &out.program.terms[&symbol]
}

/// The type of every term in a definition's body, outermost first — the set
/// the debugger paints onto the IR tab as badges.
fn body_tys(term: &Term) -> Vec<Rc<Ty>> {
    let mut out = vec![term.ty.clone()];
    match &term.kind {
        TermKind::Apply { func, arg } => {
            out.extend(body_tys(func));
            out.extend(body_tys(arg));
        }
        TermKind::Fn { body, .. } => out.extend(body_tys(body)),
        TermKind::Struct(fields) => {
            for field in fields.values() {
                out.extend(body_tys(&field.value));
            }
        }
        TermKind::Project { base, .. } => out.extend(body_tys(base)),
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
    }
    out
}

fn body_types(term: &Term) -> Vec<String> {
    body_tys(term).iter().map(Rc::<Ty>::to_string).collect()
}

/// Whether a type still names one of the solver's variables anywhere inside
/// it. `Ty::Undecided` does not count: it is a type, and it prints as one.
fn mentions_a_variable(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Arrow(from, to) => mentions_a_variable(from) || mentions_a_variable(to),
        Ty::Struct { fields, rest } => {
            fields
                .values()
                .any(|field| mentions_a_variable(&field.presence) || mentions_a_variable(&field.ty))
                || mentions_a_variable(rest)
        }
        // A declared type's body is not looked inside, for the reason the
        // compiler's own walks do not: what one stands for was lowered from
        // what the user wrote, and no solver variable can reach it. Its
        // arguments are another matter — they were written at the use site and
        // hold whatever it held, so this has to descend or it would pass while
        // the leak it exists to catch is right there.
        Ty::Named { args, .. } => args.iter().any(|arg| mentions_a_variable(arg)),
        Ty::Nat | Ty::Bound(_) | Ty::Undecided | Ty::Present | Ty::Absent | Ty::Empty => false,
    }
}

#[test]
fn a_literal_is_a_nat() {
    let (mint, out, output) = inferred("let n = 1");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    // The type is written into the term itself, not just the scheme table.
    assert_eq!(term_decl(&mint, &out, "n").value.ty.to_string(), "Nat");
}

#[test]
fn the_identity_generalizes() {
    let (mint, out, output) = inferred("let id = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "'a -> 'a");

    // Zonking spells the body's types with the same letters as the scheme:
    // the lambda is 'a -> 'a and the variable inside it is 'a.
    let decl = term_decl(&mint, &out, "id");
    assert_eq!(decl.value.ty.to_string(), "'a -> 'a");
    let TermKind::Fn { body, .. } = &decl.value.kind else {
        panic!("expected a lambda");
    };
    assert_eq!(body.ty.to_string(), "'a");
}

/// Nothing downstream is given the solver's variable table, so nothing
/// downstream can read a `?N`. Resolving the body against the substitution
/// generalization happened to build covered only the variables the
/// definition's own type mentions, and a body is larger than that — so a
/// subterm came out spelled in a notation only the solver could read.
#[test]
fn no_subterm_keeps_a_solver_variable() {
    // `a : Nat` mentions nothing of the argument's type, so the substitution
    // generalization built for it is empty; the argument is still a lambda
    // with a type of its own.
    let (mint, out, output) = inferred("let k = fn x => fn y => x\nlet a = k 1 (fn z => z)");
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(
        body_types(&term_decl(&mint, &out, "a").value),
        [
            "Nat",
            "('a -> 'a) -> Nat",
            "Nat -> ('a -> 'a) -> Nat",
            "Nat",
            "'a -> 'a",
            "'a",
        ]
    );

    // The promise stated as the property it is, over a program that reaches
    // every arm of the walk — errors, which recover, included.
    let (_, out, _) = infer_src(
        "type Endo = Nat -> Nat\n\
         let id : Endo = fn x => x\n\
         let k = fn x => fn y => x\n\
         let a = k { p: id } (fn z => z)\n\
         let b = a.p 1\n\
         let bad = 1 1\n\
         let worse = fn q => q.nope\n",
    );
    for decl in out.program.terms.values() {
        for ty in body_tys(&decl.value) {
            assert!(!mentions_a_variable(&ty), "{ty}");
        }
    }
}

#[test]
fn quantifiers_are_named_in_first_occurrence_order() {
    let (mint, _, output) = inferred("let apply = fn f x => f x");
    assert_eq!(scheme(&mint, &output, "apply"), "('a -> 'b) -> 'a -> 'b");

    let (mint, _, output) = inferred("let compose = fn f g x => f (g x)");
    assert_eq!(
        scheme(&mint, &output, "compose"),
        "('a -> 'b) -> ('c -> 'a) -> 'c -> 'b"
    );
}

#[test]
fn each_use_instantiates_afresh() {
    // One polymorphic definition used at two different types: the uses must
    // not constrain each other, which is the whole point of the scheme.
    let (mint, _, output) = inferred("let id = fn x => x\nlet n = id 1\nlet u = id {}");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    assert_eq!(scheme(&mint, &output, "u"), "{}");
}

#[test]
fn a_lambda_argument_stays_monomorphic() {
    // `f` is a lambda argument, so its two uses share one type — using it at
    // Nat and at {} in one body has to be a mismatch.
    let (_, _, output) = infer_src("let both = fn f => { a: f 1, b: f {} }");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Mismatch { .. })),
        "expected a mismatch: {:#?}",
        output.errors
    );
}

#[test]
fn an_annotation_is_checked_and_kept() {
    let (mint, out, output) =
        inferred("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x\nlet n = fst { x: 1, y: 2 }");
    assert_eq!(scheme(&mint, &output, "fst"), "{ x: Nat, y: Nat } -> Nat");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");

    // Checking pushed the annotation into the binder: the projection knows it
    // produced a Nat even though nothing after it demanded one.
    let decl = term_decl(&mint, &out, "fst");
    let TermKind::Fn { body, .. } = &decl.value.kind else {
        panic!("expected a lambda");
    };
    assert_eq!(body.ty.to_string(), "Nat");
}

#[test]
fn checking_reaches_into_struct_literals() {
    // The lambda sits inside a struct literal; only checking mode can hand it
    // the arrow it needs before its body projects nothing.
    inferred("let s : { f: { n: Nat } -> Nat } = { f: fn r => r.n }");
}

#[test]
fn struct_fields_unify_by_name_not_position() {
    inferred("let p : { a: Nat, b: {} } = { b: {}, a: 1 }");
}

#[test]
fn an_annotation_mismatch_is_one_error() {
    let (_, _, output) = infer_src("let n : Nat = fn x => x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let ErrorKind::Mismatch { expected, .. } = &error.kind else {
        panic!("expected a mismatch: {:#?}", error.kind);
    };
    assert_eq!(expected.to_string(), "Nat");
}

/// An application's mismatch is worded from the function's side: what the
/// parameter demands is the expectation and what was passed is the finding.
/// Emission is the only place that can get this right — the solver decomposes
/// structurally and swaps nothing — and getting it backwards named the user's
/// annotation as the mistake and their mistake as the expectation.
#[test]
fn an_argument_mismatch_names_the_parameter_as_the_expectation() {
    let (_, _, output) = infer_src("let f : Nat -> Nat = fn x => x\nlet y = f {}");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `Nat`, found `{}`"
    );

    // A struct with a field the closed parameter type does not allow is the
    // same direction one field at a time: the annotation is what says what is
    // allowed, and the extra field is the finding.
    let (_, _, output) =
        infer_src("let f : { x: Nat } -> Nat = fn p => p.x\nlet y = f { x: 1, z: 2 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "extra field `z`: the type `{ x: Nat }` lists every field it allows"
    );

    // The annotation path words its own complaint the same way round, and did
    // before: fixing applications must not have got there by swapping both.
    let (_, _, output) = infer_src("let p : { x: Nat } = { x: 1, y: 2 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "extra field `y`: the type `{ x: Nat }` lists every field it allows"
    );

    // The same wording when the function is not an arrow until the solver
    // makes it one. Generation cannot see that coming, so this is the case
    // that came out backwards — ``expected `{}`, found `Nat` `` — naming the
    // parameter the first call had established as what the second call found.
    let src = "let f = fn g => { a: g 1, b: g {} }";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `Nat`, found `{}`"
    );
    // And reported at the argument, not at the whole application: the
    // argument is the part the reader can change.
    assert_eq!(error.span.start, src.find("g {}").expect("the call") + 2);

    // Applying something that is no function at all is the other complaint,
    // and it reads the other way round: the call site is what demands an
    // arrow, and the callee is what turned out not to be one.
    //
    // The demanded parameter prints as `?` rather than as the argument's type
    // because it is a variable of its own. Writing the argument into the
    // demanded arrow is what worded the mismatch above backwards, and it made
    // this arrow's own specificity an accident of the argument being a
    // literal: for `fn x => 1 x` the same mistake read `Nat -> ?` or `? -> ?`
    // depending only on where else in the definition `x` was mentioned.
    let (_, _, output) = infer_src("let bad = 1 1");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `? -> ?`, found `Nat`"
    );
}

/// A term that failed to type is undecided, not polymorphic. Reporting without
/// recovering left the abandoned variable unbound, generalization quantified
/// it, and the scheme that came out fitted every later use — so one mistake
/// silently licensed everything written on top of it.
#[test]
fn a_failed_definition_is_undecided_rather_than_polymorphic() {
    let (mint, _, output) = infer_src("let bad = 1 1\nlet ok : Nat = bad\nlet ok2 = bad 5");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(scheme(&mint, &output, "bad"), "?");
    assert_eq!(scheme(&mint, &output, "ok2"), "?");

    // The same for a definition abandoned by a missing field: the result the
    // projection never produced may not come back quantified, while the
    // argument's own type — which nothing failed about — still may.
    let (mint, _, output) = infer_src("let f = fn x => (fn r => r.a) { b: x }\nlet g = f { b: 1 }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(scheme(&mint, &output, "f"), "'a -> ?");
    assert_eq!(scheme(&mint, &output, "g"), "?");

    // And for one abandoned by the occurs check. The call site says `f` is a
    // function before the argument asks it to be its own argument, so that
    // much of the shape survives what the occurs check abandons — what must
    // not survive is a quantifier, which is the whole point.
    let (mint, _, output) = infer_src("let t = fn f => f f");
    assert_eq!(scheme(&mint, &output, "t"), "(? -> ?) -> ?");
    assert!(
        !scheme(&mint, &output, "t").contains('\''),
        "a definition that failed to type came back quantified"
    );
}

/// The occurs check failing is not a variable taking a type. A step naming
/// `Rule::Bind` above an effect reading "this type would have to contain itself" tells whoever is
/// stepping through the solve the opposite of what happened.
#[test]
fn a_failed_occurs_check_is_a_rule_of_its_own() {
    let (mint, _, output) = infer_src("let t = fn f => f f");
    assert_eq!(
        steps(&mint, &output, "t"),
        [
            // Being applied makes `f` a function, which is true and is bound.
            "bind  ?1 -> ?2 ~ ?0 => ?0 := ?1 -> ?2",
            // Only the argument asks `f` to be its own parameter.
            "occurs  ?1 ~ ?1 -> ?2 => this type would have to contain itself",
            "recover  ?1 ~ ? => ?1 := ?",
            "recover  ?2 ~ ? => ?2 := ?",
        ]
    );
    // No step claims to have bound anything by the rule that failed. The bind
    // above is a different step, which succeeded and says so; what must never
    // appear is a failure labelled as one.
    assert!(
        output
            .steps
            .iter()
            .filter(|step| matches!(step.effect, Effect::Failed(_)))
            .all(|step| step.rule == Rule::Occurs),
        "{:#?}",
        output.steps
    );
}

#[test]
fn a_missing_field_names_the_struct() {
    let src = "let f : { x: Nat } -> Nat = fn p => p.y";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let ErrorKind::MissingField { base, field } = &error.kind else {
        panic!("expected a missing field: {:#?}", error.kind);
    };
    assert_eq!(base.to_string(), "{ x: Nat }");
    // The name travels in the payload, so a reporter needs no source to say
    // which field was asked for.
    assert_eq!(field, "y");
    assert_eq!(error.kind.to_string(), "no field `y` on `{ x: Nat }`");
    // Reported at the field name, which is the only thing the user can fix.
    assert_eq!(error.span.start, src.find(".y").expect("the field") + 1);
}

#[test]
fn an_unannotated_projection_infers_an_open_row() {
    // `p.x` says everything a projection demands of its base: a struct with
    // an `x`, whatever else it may also have. That is a type, so nothing here
    // needs an annotation — the definition is polymorphic in the field's type
    // and in the rest of the row.
    let (mint, _, output) = inferred("let f = fn p => p.x");
    assert_eq!(scheme(&mint, &output, "f"), "{ x: 'a, ..'b } -> 'a");

    // A chain demands a nested shape, each link open past the field it reads.
    let (mint, _, output) = inferred("let r = fn x => x.a.b.c");
    assert_eq!(
        scheme(&mint, &output, "r"),
        "{ a: { b: { c: 'a, ..'b }, ..'c }, ..'d } -> 'a"
    );

    // Two projections composed off one base still name one row: the demands
    // meet at `x` and merge, extra fields flowing into a shared tail.
    let (mint, _, output) = inferred("let r = fn x => (fn y => y.q) x.a");
    assert_eq!(
        scheme(&mint, &output, "r"),
        "{ a: { q: 'a, ..'b }, ..'c } -> 'a"
    );

    // And the open row closes at a use: applying the accessor to a literal
    // decides the whole struct, without the literal having been annotated.
    let (mint, _, output) = inferred("let f = fn p => p.x\nlet n = f { x: 1, y: {} }");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");

    // One polymorphic accessor, used at two shapes that share nothing but the
    // field: each use instantiates its own row.
    let (mint, _, output) = inferred(
        "let getx = fn p => p.x\n\
         let a = getx { x: 1 }\n\
         let b = getx { x: {}, y: 2 }",
    );
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "{}");
}

#[test]
fn an_open_annotation_generalizes_its_tail() {
    let (mint, _, output) = inferred("let f : { x: Nat, .. } -> Nat = fn p => p.x");
    assert_eq!(scheme(&mint, &output, "f"), "{ x: Nat, ..'a } -> Nat");

    // The tail instantiates per use, so one accessor serves a narrower and a
    // wider struct alike — which is what `..` is for.
    let (mint, _, output) = inferred(
        "let f : { x: Nat, .. } -> Nat = fn p => p.x\n\
         let a = f { x: 1 }\n\
         let b = f { x: 1, y: {} }",
    );
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "Nat");

    // A closed annotation still means closed: the same wider call is refused.
    let (_, _, output) =
        infer_src("let f : { x: Nat } -> Nat = fn p => p.x\nlet b = f { x: 1, y: {} }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
}

#[test]
fn a_named_tail_is_shared_within_one_annotation() {
    // The same `..r` on both sides of the arrow: whatever extra fields the
    // argument carries, the result carries too — and the scheme says so with
    // one letter.
    let (mint, _, output) = inferred(
        "let keep : { x: Nat, ..r } -> { x: Nat, ..r } = fn p => p\n\
         let q = keep { x: 1, y: {} }",
    );
    assert_eq!(
        scheme(&mint, &output, "keep"),
        "{ x: Nat, ..'a } -> { x: Nat, ..'a }"
    );
    assert_eq!(scheme(&mint, &output, "q"), "{ x: Nat, y: {} }");
}

#[test]
fn a_tail_name_is_scoped_to_its_annotation() {
    // Two annotations both naming `r`: neither can see the other's, so the
    // two definitions stay independently polymorphic and each prints its own
    // first letter.
    let (mint, _, output) = inferred(
        "let f : { x: Nat, ..r } -> Nat = fn p => p.x\n\
         let g : { y: Nat, ..r } -> Nat = fn p => p.y\n\
         let n = f { x: 1, extra: 2 }\n\
         let m = g { y: 1, other: 2 }",
    );
    assert_eq!(scheme(&mint, &output, "f"), "{ x: Nat, ..'a } -> Nat");
    assert_eq!(scheme(&mint, &output, "g"), "{ y: Nat, ..'a } -> Nat");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    assert_eq!(scheme(&mint, &output, "m"), "Nat");
}

#[test]
fn an_optional_field_may_be_absent_present_or_wrong() {
    let (mint, _, output) = inferred(
        "let f : { x?: Nat, y: Nat, .. } -> Nat = fn r => r.y\n\
         let a = f { y: 1 }\n\
         let b = f { x: 5, y: 1 }",
    );
    // The presence variable is quantified too, but it prints as the `?` on
    // its field rather than as a letter — the tail is still `'a`.
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ x?: Nat, y: Nat, ..'a } -> Nat"
    );
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "Nat");

    // Optional is not untyped: when the field is there, it is a Nat.
    let (_, _, output) =
        infer_src("let f : { x?: Nat, y: Nat } -> Nat = fn r => r.y\nlet c = f { x: {}, y: 1 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `Nat`, found `{}`"
    );
}

/// An annotation is the contract, and a contract the definition rewrites is
/// not one. Every `..` and every `?` in a written type says the definition
/// works for more than one thing; where the body settles one of them, the
/// annotation the reader is shown promises something nobody checked — so the
/// annotation is refused rather than quietly replaced by what was solved.
///
/// This used to be silent, and the silence was the whole problem: `f` below
/// was exported as `{ x: Nat, ..'a } -> Nat` off an annotation saying `x` may
/// be absent, and the reader had no way to tell which of the two the compiler
/// meant.
#[test]
fn an_annotation_may_not_be_narrowed_by_its_definition() {
    // The body projects `x`, and a projection needs the field to be there —
    // so the `?` cannot stay. Reported at the annotation, which is the line
    // that has to change.
    let src = "let f : { x?: Nat, .. } -> Nat = fn p => p.x";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "annotation-too-open");
    assert_eq!(
        error.kind.to_string(),
        "this type promises a `..` or a `?` that the definition does not leave open: \
         write the type it actually has"
    );
    assert_eq!(error.span.start, src.find('{').expect("the annotation"));

    // A tail is the same promise: reading a field off `p` says the rest of
    // the row is not anything at all, it is a row with an `x` in it.
    let (_, _, output) = infer_src("let f : { .. } -> Nat = fn p => p.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "annotation-too-open");

    // And a tail shared across an arrow, which is where the silence was
    // worst: the annotation said the result carries whatever the argument
    // carried, and the body returns one fixed struct.
    let (_, _, output) = infer_src("let f : { ..r } -> { x: Nat, ..r } = fn p => { x: 0 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "annotation-too-open");

    // A value binding is a definition too, and a concrete value decides every
    // `?` and every `..` above it — so an open annotation on one is a
    // contradiction in terms rather than a niche mistake. The wording has to
    // read as well here as it does over a function, since there is no body to
    // blame and nothing else to write.
    for src in [
        "let x : { a?: Nat } = { a: 1 }",
        "let x : { a: Nat, .. } = { a: 1 }",
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected exactly one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(error.kind.code(), "annotation-too-open", "{src}");
    }
}

/// What the check must not catch. Each of these leaves the annotation's own
/// variables unbound — the demand a body makes is a fresh row of its own, and
/// unification binds that side rather than the annotation's — so the type the
/// reader wrote is the type that was checked.
#[test]
fn an_annotation_the_definition_keeps_open_is_no_complaint() {
    // A field read off an otherwise open row: the projection's demand takes
    // the annotation's tail, not the other way round.
    let (mint, _, output) = inferred("let f : { x: Nat, .. } -> Nat = fn p => p.x");
    assert_eq!(scheme(&mint, &output, "f"), "{ x: Nat, ..'a } -> Nat");

    // A tail carried straight through, and used at a wider struct.
    let (mint, _, output) = inferred(
        "let keep : { x: Nat, ..r } -> { x: Nat, ..r } = fn p => p\n\
         let q = keep { x: 1, y: {} }",
    );
    assert_eq!(
        scheme(&mint, &output, "keep"),
        "{ x: Nat, ..'a } -> { x: Nat, ..'a }"
    );
    assert_eq!(scheme(&mint, &output, "q"), "{ x: Nat, y: {} }");

    // An optional field the body never reads stays optional, whether a caller
    // supplies it or not.
    let (mint, _, output) = inferred(
        "let f : { x?: Nat, y: Nat, .. } -> Nat = fn r => r.y\n\
         let a = f { y: 1 }\n\
         let b = f { x: 5, y: 1 }",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ x?: Nat, y: Nat, ..'a } -> Nat"
    );
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "Nat");
}

/// One mistake is one complaint. Everything a failure abandons it also
/// decides — recovery points it at `Ty::Undecided`, which is a binding like
/// any other — so a definition that already said something has its annotation
/// left alone rather than blamed for the fallout.
#[test]
fn a_definition_that_already_failed_is_not_also_told_off_for_its_annotation() {
    // A complaint from another phase entirely: nothing inference reported, so
    // the suppression above cannot be what saves this one. The annotation's
    // variables were bound — to `Ty::Undecided`, by the recovery that follows
    // absorbing the error term — and that is not a narrowing.
    let (_, out, output) = infer_src("let f : { x?: Nat, .. } -> Nat = fn p => nope p");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);

    // And one of inference's own: the field the body asks for is not the one
    // the type has, which is the thing to fix. That it also pinned the tail
    // on the way is not a second mistake.
    let (_, _, output) = infer_src("let f : { x?: Nat, y: Nat } -> Nat = fn p => p.q");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
}

/// The one cycle rows add beyond the occurs check: two structs sharing a
/// tail cannot differ in fields, because whatever the tail absorbed from one
/// side it would grow on the other, forever.
#[test]
fn two_rows_sharing_a_tail_cannot_differ() {
    let (mint, _, output) = infer_src(
        "let g : { x: Nat, ..r } -> { y: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn c => g c c",
    );
    assert_eq!(
        scheme(&mint, &output, "g"),
        "{ x: Nat, ..'a } -> { y: Nat, ..'a } -> Nat"
    );
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Recursive)),
        "expected a recursive-type error: {:#?}",
        output.errors
    );
}

/// A `..` stands for the fields its row does not write out, so it may never
/// be solved to a row that writes one of them out. Nothing said so, and the
/// consequence was not a bad message but a wrong answer: whether the
/// contradiction was noticed at all depended on whether the program happened
/// to bring the two rows back together afterwards.
#[test]
fn a_tail_may_not_take_a_field_its_own_row_names() {
    // The tail `r` is named beside a `y` in the argument and stands alone in
    // the result, so returning a struct with a `y` in it asks `r` for a `y`.
    // This used to typecheck: the solve bound `r` to `{ y: {} }`, the two
    // copies of `y` never met again, and zonking dropped one of them without
    // a word — so the definition was exported as
    // `{ x: { y: Nat } } -> { y: {} }`, a type nothing had checked.
    let src = "let h : { x: { y: Nat, ..r } } -> { ..r } = fn p => { y: {} }";
    let (mint, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "repeated-field");
    assert_eq!(
        error.kind.to_string(),
        "`..` covers only the fields a type does not already name, and here it would have to cover `y`"
    );
    // And the scheme no longer claims the narrowing it was never entitled to:
    // the abandoned tail is undecided, which is what `..` with nothing after
    // it means.
    assert_eq!(
        scheme(&mint, &output, "h"),
        "{ x: { y: Nat, .. } } -> { .. }"
    );

    // The same contradiction reached through a call rather than through a
    // body. The scheme quantifies the shared tail, so instantiating it has to
    // say again what the copy may not stand for — losing that on the way out
    // of a scheme is how this one used to be reported: as an incidental
    // ``Nat ~ {}`` mismatch, and only because the two rows happened to meet a
    // second time.
    let (_, _, output) = infer_src(
        "let h : { ..r } -> { x: { y: Nat, ..r } } -> Nat = fn a => fn b => 1\n\
         let z = h { y: {} } { x: { y: 1 } }",
    );
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "repeated-field");
    assert_eq!(
        error.kind.to_string(),
        "`..` covers only the fields a type does not already name, and here it would have to cover `y`"
    );

    // One complaint per binding rather than one per label: a tail asked for
    // two fields it may not have is one row gone wrong, and it names the
    // first of them in the order the row writes them.
    let (_, _, output) =
        infer_src("let h : { x: { a: Nat, b: Nat, ..r } } -> { ..r } = fn p => { a: 1, b: 2 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "`..` covers only the fields a type does not already name, and here it would have to cover `a`"
    );
}

/// The condition survives every binding it passes through. A tail that
/// absorbs a field continues as a fresh tail, and that fresh tail is where the
/// next conflicting field would arrive — so what the first one could not be,
/// the second cannot be either.
#[test]
fn what_a_tail_may_not_be_is_carried_across_a_binding() {
    // `h` says `q` is exactly the rest of a row that already names `y`, so
    // `q` has no `y`. Reading `q.w` first splits that rest into a `w` and a
    // remainder, and it is the remainder — a variable minted by the solve,
    // which no annotation ever mentioned — that has to refuse the `y` the
    // last projection asks for.
    let (_, _, output) = infer_src(
        "let h : { y: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let k = fn p q => { m: h p q, n: q.w, o: q.y }",
    );
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "repeated-field");

    // And a tail that never conflicts is still free: sharing one `r` across
    // rows that all agree costs nothing.
    let (mint, _, output) = inferred(
        "let h : { y: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let z = h { y: 1, q: 2 } { q: 2 }",
    );
    assert_eq!(scheme(&mint, &output, "z"), "Nat");
    let (mint, _, output) = inferred(
        "let h : { y: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let k = fn p q => { m: h p q, n: q.w }",
    );
    assert_eq!(
        scheme(&mint, &output, "k"),
        "{ y: Nat, w: 'a, ..'b } -> { w: 'a, ..'b } -> { m: Nat, n: 'a }"
    );
}

/// The occurs check reaches through a row: a struct that would have to
/// contain itself through one of its own fields is the same cycle
/// `fn x => x x` asks for, one level of braces in.
#[test]
fn the_occurs_check_reaches_through_a_row() {
    let (_, _, output) = infer_src("let w = fn x => x.f x");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Recursive)),
        "expected a recursive-type error: {:#?}",
        output.errors
    );
}

/// Whether a message names one of the solver's own variables.
fn names_a_solver_index(message: &str) -> bool {
    message
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'?' && pair[1].is_ascii_digit())
}

/// A diagnostic spells a type the way the scheme beside it spells the same
/// type. An unsolved variable that reaches the reader as `?7` names the
/// solver's bookkeeping rather than anything they wrote — and the index is a
/// counter over the whole program, so it is not even a number they could count
/// to. Resolving error payloads against an empty substitution left every one
/// of them like that, beside a scheme calling the same type `'a`.
#[test]
fn a_diagnostic_spells_a_variable_the_way_the_scheme_does() {
    let src = "let g = fn x y => ({ p: x, q: y }).missing";
    let (mint, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "no field `missing` on `{ p: 'a, q: 'b }`"
    );
    assert_eq!(scheme(&mint, &output, "g"), "'a -> 'b -> ?");

    // The counter runs over the whole program, so a definition solved after
    // others is where an index shows through as something unplaceable.
    let (_, _, output) = infer_src(&format!(
        "let f = fn a b c => {{ p: a, q: b, r: c }}\n{src}"
    ));
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "no field `missing` on `{ p: 'a, q: 'b }`"
    );

    // Every payload that carries a type, over every complaint that has one.
    for src in [
        "let h = fn g => (fn z => g z).b",
        "let m = fn x y => ({ p: x, q: y }).missing",
        "let n = fn x => x.a",
        "let o = fn x => { a: x 1, b: x {} }",
        "let p : Nat = fn a => a",
        "let q = fn f => f f",
    ] {
        let (_, _, output) = infer_src(src);
        for error in &output.errors {
            let message = error.kind.to_string();
            assert!(!names_a_solver_index(&message), "{src}: {message}");
        }
    }
}

/// Reading a field off something that has none names the thing that has none,
/// and stops there. What was asked of it — a struct with that field and an
/// open tail — is a shape the solver made up to ask the question with, so
/// quoting it back as the expectation reads as though the user had written
/// `{ x: ?, .. }` somewhere and got it wrong. An open row is how a demand is
/// told apart from a written type: no closed type has a tail.
#[test]
fn a_projection_off_a_non_struct_names_the_type_with_no_fields() {
    let (_, _, output) = infer_src("let n = 1\nlet bad = n.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "not-a-struct");
    assert_eq!(
        error.kind.to_string(),
        "`Nat` is not a struct, so it has no fields to read"
    );

    // The other side of the goal, and the other type with no fields: here the
    // demand is the `actual` half, because what the base turned out to be is
    // an arrow the call site had already demanded of it.
    let (_, _, output) = infer_src("let h = fn g => (fn z => g z).b");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "not-a-struct");
    assert_eq!(
        error.kind.to_string(),
        "`? -> ?` is not a struct, so it has no fields to read"
    );

    // A closed struct against a `Nat` stays an ordinary mismatch, both ways
    // round: the reader wrote both of those types, and naming both is more
    // use than naming one.
    for (src, message) in [
        (
            "let p : { x: Nat } = 1",
            "type mismatch: expected `{ x: Nat }`, found `Nat`",
        ),
        (
            "let p : Nat = { x: 1 }",
            "type mismatch: expected `Nat`, found `{ x: Nat }`",
        ),
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected exactly one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(error.kind.to_string(), message, "{src}");
    }
}

/// The two ways a projection can be wrong are wrong in two different places.
/// A struct that lacks the field is about the name that was read — see
/// [`a_missing_field_names_the_struct`] — but something that is not a struct
/// at all is about the base: renaming the field would leave it just as wrong,
/// so the base is the term the reader has to change.
#[test]
fn a_projection_off_a_non_struct_is_reported_at_the_base() {
    let src = "let n = 1\nlet bad = n.x";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "not-a-struct");
    assert_eq!(error.span.start, src.find("n.x").expect("the base"));
    assert_eq!(error.span.width, 1);

    // Whatever stands to the left of the dot, not just a name: the complaint
    // takes the base term's own span, so it points at an expression as
    // readily as at a name.
    let src = "let h = fn g => (fn z => g z).b";
    let (mint, out, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "not-a-struct");
    let TermKind::Fn { body, .. } = &term_decl(&mint, &out, "h").value.kind else {
        panic!("expected a function");
    };
    let TermKind::Project { base, .. } = &body.kind else {
        panic!("expected a projection");
    };
    assert_eq!(error.span, base.span);
}

/// A chain of projections off a base that failed is one complaint. The
/// complaint recovers the field's type to `Ty::Undecided`, and every link
/// after the first absorbs rather than echoes.
#[test]
fn a_projection_chain_complains_once() {
    let (_, _, output) = infer_src("let n = 1\nlet r = n.a.b.c");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(
        output.errors[0].kind.to_string(),
        "`Nat` is not a struct, so it has no fields to read"
    );
}

#[test]
fn generation_records_what_it_asked_for_without_solving_it() {
    let (mint, _, output) = inferred("let fst : { x: Nat } -> Nat = fn p => p.x");
    // In the order the walk emitted them, and unsolved: the projection's
    // demand comes before the annotation's demand on its result, and both
    // still spell that result as the variable generation minted for it. The
    // projection is an ordinary equality against an open row — a struct with
    // the field, whatever else the base may have.
    assert_eq!(
        constraints(&mint, &output, "fst"),
        ["equal: { x: ?0, ..?1 } ~ { x: Nat }", "equal: Nat ~ ?0"]
    );

    // A definition can demand nothing at all, and still be one the pass ran
    // over: the entry is there, empty, rather than missing.
    let (mint, _, output) = inferred("let n = 1");
    assert!(constraints(&mint, &output, "n").is_empty());
}

#[test]
fn a_projection_needs_nothing_said_about_its_base() {
    // Nothing has said what `p` is when `p.x` is walked — the argument that
    // settles it comes later in the constraint list. The projection's demand
    // is an open row, which unifies now and closes when the argument arrives,
    // so no constraint waits on another.
    let (mint, _, output) = inferred("let h : Nat = (fn p => p.x) { x: 1 }");
    assert_eq!(scheme(&mint, &output, "h"), "Nat");
}

#[test]
fn nested_projections_solve_in_one_pass() {
    // Two projections, the inner one demanded before anything explains its
    // base. Each demand is an equality against an open row, so the solve
    // takes them in order and never has to come back.
    let (mint, _, output) =
        inferred("let deep : Nat = (fn q => (fn p => p.x) q.inner) { inner: { x: 1 } }");
    assert_eq!(scheme(&mint, &output, "deep"), "Nat");
}

/// The line recursive types are not allowed to move. A declaration may name
/// itself, because a person wrote down what it is; a term may not ask the
/// solver to invent a type that contains itself, because nothing they could
/// write is what it would be.
#[test]
fn self_application_is_recursive_not_divergent() {
    let (_, _, output) = infer_src("let w = fn x => x x");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Recursive)),
        "expected a recursive-type error: {:#?}",
        output.errors
    );

    // And it stays that way in a program that also declares a recursive type,
    // so the two are decided separately rather than by one switch.
    let (_, _, output) = infer_src("type list = { val: Nat, next: list }\nlet w = fn x => x x");
    assert!(
        output
            .errors
            .iter()
            .any(|error| matches!(error.kind, ErrorKind::Recursive)),
        "expected a recursive-type error: {:#?}",
        output.errors
    );
}

/// A type may name itself, and reading a field off one gives the type back.
#[test]
fn a_type_may_name_itself() {
    let (mint, _, output) = inferred(
        "type list = { val: Nat, next: list }\n\
         let head : list -> Nat = fn l => l.val\n\
         let rest : list -> list = fn l => l.next\n\
         let third : list -> Nat = fn l => l.next.next.val\n\
         let second = fn l => head (rest l)",
    );
    // The annotation is answered in the words it was written in.
    assert_eq!(scheme(&mint, &output, "head"), "list -> Nat");
    assert_eq!(scheme(&mint, &output, "rest"), "list -> list");
    // A chain of projections walks the type as far as it is asked to, each
    // link reading a field off a name the link before it produced.
    assert_eq!(scheme(&mint, &output, "third"), "list -> Nat");
    // And an unannotated definition works the name back out of the two it
    // called, having never been told it.
    assert_eq!(scheme(&mint, &output, "second"), "list -> Nat");

    // What the declaration means is one step deep: the name inside it is still
    // a name, which is why this is a finite value at all.
    let list = symbol_named(&mint, output.aliases.keys().copied(), "list");
    assert_eq!(
        output.aliases[&list].to_string(),
        "{ val: Nat, next: list }"
    );
}

/// Equality is structural through a name, so two declarations of the same
/// shape are one type however differently they are spelled — and neither is
/// equal to a shape that differs.
#[test]
fn two_recursive_types_of_the_same_shape_are_one_type() {
    let (mint, _, output) = inferred(
        "type a = { n: a }\n\
         type b = { n: b }\n\
         let f : a -> b = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "a -> b");

    let (_, _, output) = infer_src(
        "type a = { n: a }\n\
         type c = { n: Nat }\n\
         let f : a -> c = fn x => x",
    );
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert!(matches!(error.kind, ErrorKind::Mismatch { .. }));
}

/// How far a type happens to be unrolled is not part of what it is. Deciding
/// this is what the solver's assumption stack is for: without it the two sides
/// unfold against each other forever, because neither ever runs out.
#[test]
fn equality_looks_past_how_far_a_type_is_unrolled() {
    let (mint, _, output) = inferred(
        "type a = { n: a }\n\
         type b = { n: { n: b } }\n\
         let f : a -> b = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "a -> b");

    // Every other way two of them can be out of step: through arrows, through
    // a third declaration, and against a shape written out by hand. Each of
    // these is a solve that only ends because a pair it has already taken on
    // is a pair it refuses to take on again — reaching the assertion at all is
    // most of what is being checked.
    let (mint, _, output) = inferred(
        "type f1 = Nat -> f1\n\
         type f2 = Nat -> Nat -> f2\n\
         let f : f1 -> f2 = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "f1 -> f2");

    let (mint, _, output) = inferred(
        "type a = { n: b }\n\
         type b = { n: a }\n\
         type c = { n: c }\n\
         let f : a -> c = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "a -> c");

    let (mint, _, output) = inferred(
        "type c = { n: c }\n\
         let f : { n: c } -> c = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "{ n: c } -> c");
}

/// Two declarations may name each other, since every type's name is bound
/// before any type's body is read.
#[test]
fn recursive_types_may_be_written_in_terms_of_each_other() {
    let (mint, _, output) = inferred(
        "type forest = { head: tree, tail: forest }\n\
         type tree = { val: Nat, kids: forest }\n\
         let first : forest -> Nat = fn f => f.head.val",
    );
    assert_eq!(scheme(&mint, &output, "first"), "forest -> Nat");

    // Written below it, and reached anyway.
    let tree = symbol_named(&mint, output.aliases.keys().copied(), "tree");
    assert_eq!(
        output.aliases[&tree].to_string(),
        "{ val: Nat, kids: forest }"
    );
}

/// The one loop that is not a recursive type: a name standing for a name
/// standing for the first says nothing, so it is refused where it is written
/// rather than unfolded forever — or, worse, taken as equal to everything by
/// the assumption that makes the real ones work.
#[test]
fn a_type_declared_as_only_a_name_is_rejected() {
    for src in [
        "type t = t\nlet n : t = 1",
        "type a = b\ntype b = a\nlet n : a = 1",
    ] {
        let (_, out, output) = infer_src(src);
        assert!(
            out.errors
                .iter()
                .any(|error| matches!(error.kind, ir::ErrorKind::Circular)),
            "{src}: expected a circular-type error: {:#?}",
            out.errors
        );
        // The declaration is undecided from there on, which absorbs: the one
        // complaint is not echoed by everything that named it.
        assert!(
            output.errors.is_empty(),
            "{src}: inference errors: {:#?}",
            output.errors
        );
    }

    // A loop with any shape in it is a type, not a mistake.
    inferred("type t = { next: t }\nlet f : t -> t = fn x => x.next");
    inferred("type t = t -> Nat\nlet f : t -> t = fn x => x");
}

/// Unfolding is a rule of the solve like any other, so a reader stepping
/// through it is shown where a name was replaced and where a pair came back
/// round.
#[test]
fn unfolding_a_declared_type_is_a_rule_of_its_own() {
    let (mint, _, output) = inferred(
        "type a = { n: a }\n\
         type b = { n: b }\n\
         let f : a -> b = fn x => x",
    );
    assert_eq!(
        steps(&mint, &output, "f"),
        [
            // The body is an `a` where the annotation wants a `b`. Neither is
            // a variable, so nothing binds: the whole question is whether two
            // declarations mean the same thing.
            "unfold  b ~ a => replaced by the goals below",
            "  struct  { n: b } ~ { n: a } => replaced by the goals below",
            // Which is the first question again, one field in. Three steps,
            // not five: the pair is remembered by which declarations it names,
            // so the second `b` and `a` are recognized as the first ones and
            // not as two more copies to unfold.
            "    assume  b ~ a => no change",
        ]
    );
}

/// A declared type keeps its name everywhere it is used, and the alias table
/// is what says what a name means — one step, not all the way down. Unfolding
/// used to happen at lowering, which spelled `Endo` back at the user as
/// `Nat -> Nat` and made a type that names itself impossible to lower at all.
#[test]
fn a_declared_type_stays_the_name_it_was_written_as() {
    let (mint, _, output) =
        inferred("type Endo = Nat -> Nat\ntype Pair = { f: Endo }\nlet id : Endo = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "Endo");

    let pair = symbol_named(&mint, output.aliases.keys().copied(), "Pair");
    assert_eq!(output.aliases[&pair].to_string(), "{ f: Endo }");

    // And the name is a barrier to unfolding only: `id` is still the function
    // the alias stands for, so it takes a `Nat` and gives one back.
    let (mint, _, output) =
        inferred("type Endo = Nat -> Nat\nlet id : Endo = fn x => x\nlet n = id 1");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
}

#[test]
fn lowering_errors_are_absorbed_not_echoed() {
    // `origin` is undefined: lowering reported it, and the error term must
    // sail through inference without a second complaint.
    let (mint, out, output) = infer_src("let point = { x: origin }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        output.errors.is_empty(),
        "inference echoed a lowering error: {:#?}",
        output.errors
    );
    assert_eq!(scheme(&mint, &output, "point"), "{ x: ? }");
}

#[test]
fn the_solve_is_recorded_rule_by_rule() {
    let (mint, _, output) = inferred("let fst : { x: Nat } -> Nat = fn p => p.x");
    // The projection's open row meets the annotation's closed struct: the
    // struct rule decomposes the pair, the open tail closes — the annotation
    // allows nothing more — and the field's type is bound. The annotation's
    // demand on the result is then already answered.
    assert_eq!(
        steps(&mint, &output, "fst"),
        [
            "struct  { x: ?0, ..?1 } ~ { x: Nat } => replaced by the goals below",
            "  bind  ?1 ~ ∅ => ?1 := ∅",
            "  bind  ?0 ~ Nat => ?0 := Nat",
            "prim  Nat ~ Nat => no change",
        ]
    );
}

/// Two closed rows that each name a field the other does not still continue
/// as one row, and that row is closed — twice, once from each side. The second
/// time is the one place the solver is asked whether the closed tail equals
/// itself, and it is a rule of its own rather than a mismatch: the first side
/// bound the shared continuation to `∅`, and the second finds it already
/// there.
#[test]
fn closing_a_row_from_both_sides_agrees_with_itself() {
    let (mint, _, output) = infer_src("let p : { a: Nat } = { b: 1 }");
    assert_eq!(
        steps(&mint, &output, "p"),
        [
            "struct  { a: Nat } ~ { b: Nat } => replaced by the goals below",
            "  presence  absent ~ present => extra field `b`: the type `{ a: Nat }` lists every field it allows",
            "  bind  ∅ ~ ?0 => ?0 := ∅",
            "  presence  present ~ absent => no field `a` on `{ b: Nat }`",
            "  same  ∅ ~ ∅ => no change",
        ]
    );
}

/// A rule owns everything it does, and the trace has to show that: the struct
/// step comes first and every act the rule performs is indented under it.
/// Flattening the two rows used to happen before the step and could decide
/// things while it went, so those decisions were recorded above the rule they
/// belonged to and at the same depth as it.
///
/// The goal is recorded flattened for the same reason. A tail already solved
/// to a row is the case where it matters: the third projection below meets a
/// base whose `x` and `y` live behind a variable, and a goal still naming that
/// variable could not account for the fields decided under it.
#[test]
fn a_struct_rule_owns_the_steps_beneath_it() {
    let (mint, _, output) = inferred("let f = fn p => { a: p.x, b: p.y, c: p.z }");
    assert_eq!(
        steps(&mint, &output, "f"),
        [
            "bind  { x: ?1, ..?2 } ~ ?0 => ?0 := { x: ?1, ..?2 }",
            "struct  { y: ?3, ..?4 } ~ { x: ?1, ..?2 } => replaced by the goals below",
            "  bind  ?4 ~ { x: ?1, ..?7 } => ?4 := { x: ?1, ..?7 }",
            "  bind  { y: ?3, ..?7 } ~ ?2 => ?2 := { y: ?3, ..?7 }",
            // The base is `{ x: ?1, ..?2 }` and `?2` is `{ y: ?3, ..?7 }`; the
            // goal says so rather than leaving the reader to remember it.
            "struct  { z: ?5, ..?6 } ~ { x: ?1, y: ?3, ..?7 } => replaced by the goals below",
            "  bind  ?6 ~ { x: ?1, y: ?3, ..?8 } => ?6 := { x: ?1, y: ?3, ..?8 }",
            "  bind  { z: ?5, ..?8 } ~ ?7 => ?7 := { z: ?5, ..?8 }",
        ]
    );

    // The guard against two rows sharing one tail is part of the rule too, so
    // it fails under the step rather than instead of it.
    let (mint, _, output) = infer_src(
        "let g : { x: Nat, ..r } -> { y: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn c => g c c",
    );
    assert_eq!(
        steps(&mint, &output, "h"),
        [
            "bind  { x: Nat, ..?2 } ~ ?1 => ?1 := { x: Nat, ..?2 }",
            "struct  { y: Nat, ..?2 } ~ { x: Nat, ..?2 } => replaced by the goals below",
            "  occurs  { y: Nat, ..?2 } ~ { x: Nat, ..?2 } => this type would have to contain itself",
            "  recover  ?2 ~ ? => ?2 := ?",
        ]
    );
}

#[test]
fn replaying_the_steps_rebuilds_the_solution() {
    let (_, _, output) = inferred(
        "let fst : { x: Nat } -> Nat = fn p => p.x\nlet n = fst { x: 1 }\nlet c = fn f g x => f (g x)",
    );

    // Every change to the solution is a step's effect and nothing else, so
    // taking them in order is the substitution the solver ended with — which
    // is what lets a reader see the state at step k without re-running it.
    let bound: Vec<TyVar> = output
        .steps
        .iter()
        .filter_map(|step| match step.effect {
            Effect::Bound { var, .. } => Some(var),
            _ => None,
        })
        .collect();
    assert!(!bound.is_empty(), "the solve bound nothing");

    // No variable is bound twice: replaying is an append, never a rewrite,
    // which is what lets stepping backwards be dropping the tail.
    let mut once = bound.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(once.len(), bound.len(), "{bound:?}");
}

#[test]
fn no_goal_is_ever_put_back_for_later() {
    // The program that used to need the solver's retry loop: the inner
    // projection's base is explained only by the outer one. Open rows are why
    // every goal now resolves the first time it is looked at — nothing waits,
    // and the trace is one decomposition after another.
    let src = "let deep : Nat = (fn q => (fn p => p.x) q.inner) { inner: { x: 1 } }";
    let (mint, _, output) = inferred(src);

    // The whole solve, pinned: five constraints, each decided where it stands
    // and each in the order generation asked. Nothing repeats, nothing is
    // revisited, and the two struct rules nest rather than following one
    // another — the inner row is decided as part of deciding the outer one.
    assert_eq!(
        steps(&mint, &output, "deep"),
        [
            "bind  { x: ?2, ..?3 } ~ ?1 => ?1 := { x: ?2, ..?3 }",
            "bind  { inner: ?4, ..?5 } ~ ?0 => ?0 := { inner: ?4, ..?5 }",
            "bind  { x: ?2, ..?3 } ~ ?4 => ?4 := { x: ?2, ..?3 }",
            "struct  { inner: ?4, ..?5 } ~ { inner: { x: Nat } } => replaced by the goals below",
            "  bind  ?5 ~ ∅ => ?5 := ∅",
            "  struct  { x: ?2, ..?3 } ~ { x: Nat } => replaced by the goals below",
            "    bind  ?3 ~ ∅ => ?3 := ∅",
            "    bind  ?2 ~ Nat => ?2 := Nat",
            "prim  Nat ~ Nat => no change",
        ]
    );

    // Every constraint generation emitted got a step of its own at the top
    // level, which is what "nothing waits" means: none was looked at, put
    // back, and looked at again.
    let asked = constraints(&mint, &output, "deep").len();
    let started = output.steps.iter().filter(|step| step.depth == 0).count();
    assert_eq!(started, asked, "{:#?}", output.steps);

    assert!(
        output
            .steps
            .iter()
            .all(|step| !matches!(step.effect, Effect::Failed(_))),
        "{:#?}",
        output.steps
    );
}

#[test]
fn a_failure_is_a_step_that_failed() {
    let src = "let miss : { x: Nat } -> Nat = fn p => p.y";
    let (_, _, output) = infer_src(src);
    let failures: Vec<String> = output
        .steps
        .iter()
        .filter_map(|step| match &step.effect {
            Effect::Failed(kind) => Some(kind.to_string()),
            _ => None,
        })
        .collect();
    // Every complaint reaches a reader stepping through the solve, worded the
    // same way the reporter words it.
    assert_eq!(failures, ["no field `y` on `{ x: Nat }`"]);
    assert_eq!(output.errors.len(), failures.len());
    assert_eq!(output.errors[0].kind.to_string(), failures[0]);
}

#[test]
fn giving_up_on_a_goal_is_a_step_of_its_own() {
    // The projection demands a struct of something that is a `Nat`, so the
    // goal is abandoned and everything it would have decided — the field's
    // type and the row's tail — is pointed at `?`. That changes the solution,
    // so each one has to be a step: otherwise a reader watching the state
    // would see a variable acquire a value that no rule they were shown gave
    // it.
    let (mint, _, output) = infer_src("let n = 1\nlet bad = n.x");
    assert_eq!(
        steps(&mint, &output, "bad"),
        [
            "mismatch  { x: ?0, ..?1 } ~ Nat => `Nat` is not a struct, so it has no fields to read",
            "recover  ?0 ~ ? => ?0 := ?",
            "recover  ?1 ~ ? => ?1 := ?",
        ]
    );
}

/// Two structs line up by their field names, in whatever order — and a
/// written struct type is closed: it lists every field it allows, so a field
/// too many is refused as firmly as a field too few. Both passes agree —
/// generation when it checks a literal against an annotation, the solver when
/// it unifies two struct types — and the complaint names the field rather
/// than mismatching the whole types, because the field is what the reader has
/// to add or remove.
#[test]
fn a_struct_matches_on_its_field_names_in_both_passes() {
    // Order is not part of a record's identity. The annotated definition is
    // decided by checking; the argument at the call site can only be equated,
    // so it is decided by unification.
    inferred("let p : { a: Nat, b: Nat } = { b: 2, a: 1 }");
    inferred("let f : { a: Nat, b: Nat } -> Nat = fn r => r.a\nlet y = f { b: 2, a: 1 }");

    // A field too many, and a field too few, by both passes.
    for (src, message) in [
        (
            "let p : { a: Nat } = { a: 1, b: 2 }",
            "extra field `b`: the type `{ a: Nat }` lists every field it allows",
        ),
        (
            "let f : { a: Nat } -> Nat = fn r => r.a\nlet y = f { a: 1, b: 2 }",
            "extra field `b`: the type `{ a: Nat }` lists every field it allows",
        ),
        (
            "let p : { a: Nat, b: Nat } = { a: 1 }",
            "no field `b` on `{ a: Nat }`",
        ),
        (
            "let f : { a: Nat, b: Nat } -> Nat = fn r => r.a\nlet y = f { a: 1 }",
            "no field `b` on `{ a: Nat }`",
        ),
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected exactly one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(error.kind.to_string(), message, "{src}");
    }

    // One complaint per field that is wrong, not one per struct: each is its
    // own mistake, and fixing one should not re-reveal the next.
    let (_, _, output) = infer_src("let p : { a: Nat } = { a: 1, b: 2, c: 3 }");
    let messages: Vec<String> = output
        .errors
        .iter()
        .map(|error| error.kind.to_string())
        .collect();
    assert_eq!(
        messages,
        [
            "extra field `b`: the type `{ a: Nat }` lists every field it allows",
            "extra field `c`: the type `{ a: Nat }` lists every field it allows",
        ]
    );
}

/// A complaint names the type it was made against, not that type as its
/// siblings went on to leave it. Resolving a payload at the end of the
/// definition is what makes a variable in one read as what it turned out to
/// be — but a presence is decided by the very decomposition the failure is
/// part of, and an absent field prints as no field at all, so the reader was
/// shown a type with the optional fields quietly deleted.
#[test]
fn a_field_complaint_keeps_the_optional_fields_of_the_type_it_names() {
    // `a` is optional and the call omits it, so the sibling goal settles it
    // absent — in the same struct rule that refuses `c`. The type in the
    // message is the annotation, and the annotation says `a?`.
    let src = "let g : { a?: Nat, b: Nat } -> Nat = fn r => r.b\nlet z = g { b: 1, c: 2 }";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "extra field `c`: the type `{ a?: Nat, b: Nat }` lists every field it allows"
    );

    // The step that failed says the same thing, which is the point: the
    // reader replaying the solve and the reader reading the report are being
    // shown one type. They agreed before this only because the step is taken
    // at the moment of the failure — the report is what had drifted.
    let failures: Vec<String> = output
        .steps
        .iter()
        .filter_map(|step| match &step.effect {
            Effect::Failed(kind) => Some(kind.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, [error.kind.to_string()]);

    // Both complaints of the pair at once. Each names the side that is wrong
    // about it — the type, for a field it does not allow; the value, for one
    // it does not have — and the optional field survives in the one that
    // mentions it.
    let (_, _, output) =
        infer_src("let g : { a?: Nat, b: Nat } -> Nat = fn r => r.b\nlet z = g { c: 2 }");
    let messages: Vec<String> = output
        .errors
        .iter()
        .map(|error| error.kind.to_string())
        .collect();
    assert_eq!(
        messages,
        [
            "extra field `c`: the type `{ a?: Nat, b: Nat }` lists every field it allows",
            "no field `b` on `{ c: Nat }`",
        ]
    );
}

/// A projection emitted before anything explains its base solves where it
/// stands: `q.b` demands an open row of `q`, and when `p.a` later says what
/// `q` is, the two demands meet as ordinary structs. The emission order of
/// the constraints is not an order the solver has to overcome.
#[test]
fn a_projection_solves_wherever_it_falls_in_the_list() {
    let (mint, _, output) =
        inferred("let f : { a: { b: Nat } } -> Nat = fn p => (fn q => q.b) p.a");
    assert_eq!(scheme(&mint, &output, "f"), "{ a: { b: Nat } } -> Nat");
}
