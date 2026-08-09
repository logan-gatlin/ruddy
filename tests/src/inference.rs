//! Tests for [`ruddy::inference`].

use std::rc::Rc;

use ruddy::{
    inference::{self, ConstraintKind, Effect, ErrorKind, Rule},
    ir::{self, Decl, Term, TermKind},
    parse,
    symbol::{Bundle, Mint, Symbol, Version},
    token::lex,
    tracking::FileID,
    types::{Core, Presence, Rest, Shape, Ty, TyVar},
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
        TermKind::Let { value, body, .. } => {
            out.extend(body_tys(value));
            out.extend(body_tys(body));
        }
        TermKind::Struct(fields) => {
            for field in fields.values() {
                out.extend(body_tys(&field.value));
            }
        }
        TermKind::Project { base, .. } => out.extend(body_tys(base)),
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                out.extend(body_tys(payload));
            }
        }
        TermKind::Match {
            scrutinee,
            arms,
            default,
        } => {
            out.extend(body_tys(scrutinee));
            for arm in arms {
                out.extend(body_tys(&arm.body));
            }
            if let Some((_, body)) = default {
                out.extend(body_tys(body));
            }
        }
        TermKind::Ident(_) | TermKind::Natural(_) | TermKind::Error => {}
    }
    out
}

fn body_types(term: &Term) -> Vec<String> {
    body_tys(term).iter().map(Rc::<Ty>::to_string).collect()
}

/// Whether a type still names one of the solver's variables anywhere inside
/// it. An undecided core does not count: it is a type, and it prints as one.
fn mentions_a_variable(ty: &Ty) -> bool {
    let core = match &ty.core {
        Core::Var(_) => true,
        Core::Arrow(from, to) => mentions_a_variable(from) || mentions_a_variable(to),
        Core::Sum(cases) => row_mentions_a_variable(cases),
        // A declared type's body is not looked inside, for the reason the
        // compiler's own walks do not: what one stands for was lowered from
        // what the user wrote, and no solver variable can reach it. Its
        // arguments are another matter — they were written at the use site and
        // hold whatever it held, so this has to descend or it would pass while
        // the leak it exists to catch is right there.
        Core::Named { args, .. } => args.iter().any(|arg| mentions_a_variable(arg)),
        Core::Unit | Core::Nat | Core::Bound(_) | Core::Undecided => false,
    };
    core || labels_mention_a_variable(&ty.fields)
}

/// [`mentions_a_variable`] over a sum's cases: every label, and the tail —
/// both are places a variable can survive into a scheme. A type's fields have
/// no tail of their own; their tail is the core beside them, which
/// [`mentions_a_variable`] reaches where it sits.
fn row_mentions_a_variable(row: &ruddy::types::Row) -> bool {
    matches!(row.rest, Rest::Var(_))
        || matches!(&row.rest, Rest::More(more) if row_mentions_a_variable(more))
        || labels_mention_a_variable(&row.labels)
}

/// [`mentions_a_variable`] over a label map.
fn labels_mention_a_variable(labels: &indexmap::IndexMap<String, ruddy::types::RowField>) -> bool {
    labels
        .values()
        .any(|field| matches!(field.presence, Presence::Var(_)) || mentions_a_variable(&field.ty))
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

/// A definition whose fields could not be made to agree publishes a type
/// something can satisfy, so the mistake is reported where it was made and not
/// again at every use.
///
/// The core is why this needs saying. A projection demands a fresh core carrying
/// the field, and asking the same base for something else — to be a function,
/// say — binds that core to the something else. If the fields then turn out not
/// to fit on it, the binding was part of the goal that failed: left standing it
/// makes the parameter a function that also carries an `x`, which no term can
/// be and no annotation can write, and every call is then refused for a mistake
/// already reported one line up.
#[test]
fn a_definition_whose_fields_failed_is_not_refused_again_at_every_use() {
    let src = "let f = fn p => { a: p.x, b: p 1 }\nlet g = f { x: 1 }\nlet h = f (fn n => n)";
    let (mint, _, output) = infer_src(src);
    let messages: Vec<String> = output
        .errors
        .iter()
        .map(|error| error.kind.to_string())
        .collect();
    assert_eq!(
        messages,
        ["extra field `x`: the type `Nat -> 'a` lists every field it allows"],
        "{:#?}",
        output.errors
    );
    // What `f` takes is abandoned rather than published: the core is undecided,
    // which unifies with anything, so both calls go through.
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ x: ?, .. } -> { a: ?, b: 'a }"
    );

    // The same shape one binder further out, where the two projections share a
    // core and the failure is about the other binder's.
    let src = "let z = fn a => fn b => { l: a.m, r: b.m, e: a b }\nlet w = z { m: 1 } { m: 2 }";
    let (_, _, output) = infer_src(src);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
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
            // `?0` is the definition's own variable, minted before the body was
            // walked so that the body could have named it; `?1` is `f`.
            "bind  ?2 -> ?3 ~ ?1 => ?1 := ?2 -> ?3",
            // Only the argument asks `f` to be its own parameter.
            "occurs  ?2 ~ ?2 -> ?3 => this type would have to contain itself",
            "recover  ?2 ~ ? => ?2 := ?",
            "recover  ?3 ~ ? => ?3 := ?",
            // And the definition is what its body turned out to be, which is
            // the last thing every unannotated definition says.
            "bind  ?0 ~ ?1 -> ?3 => ?0 := ?1 -> ?3",
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
    let ErrorKind::MissingField { base, field, .. } = &error.kind else {
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
    // `p.x` says everything a projection demands of its base: some type at all,
    // carrying an `x`, and whatever else it may also have. That is a type, so
    // nothing here needs an annotation — the definition is polymorphic in the
    // base's core, in the field's type and in the rest of the row.
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
/// decides — recovery points it at the undecided value of its own sort, which
/// is a binding like
/// any other — so a definition that already said something has its annotation
/// left alone rather than blamed for the fallout.
#[test]
fn a_definition_that_already_failed_is_not_also_told_off_for_its_annotation() {
    // A complaint from another phase entirely: nothing inference reported, so
    // the suppression above cannot be what saves this one. The annotation's
    // variables were bound — to the undecided row, by the recovery that follows
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
    assert_eq!(scheme(&mint, &output, "h"), "{ x: { y: Nat, .. } } -> ?");

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

/// One contradiction, one sentence: a row handed to a `..` parameter that
/// repeats two of the fields the declaration already names is one mistake with
/// one thing to change, and `Solve::assign` already reports it that way.
#[test]
fn a_row_repeating_two_fields_is_one_complaint() {
    // A row written out where a row parameter goes is refused where it is
    // written: what the declaration already names is part of what its parameter
    // stands for, so lowering knows it without anything having to unfold. The
    // first label is the one named, and the argument absorbs, so the second is
    // not a second complaint.
    let (_, out, _) = infer_src(
        "type W r = { x: Nat, y: Nat, ..r }\n\
         let p : W { x: Nat, y: Nat } = { x: 1, y: 2 }",
    );
    assert_eq!(out.errors.len(), 1, "ir errors: {:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ir::ErrorKind::RepeatedRowField { field, .. } if field == "x"
    ));

    // The same thing reached through a variable, where only the solve can know
    // it: the tail `r` stands for the rest of a row that already names `x` and
    // `y`, and the second argument is what says what that rest is.
    let (_, out, output) = infer_src(
        "let h : { x: Nat, y: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let k = fn p q => h p { x: 1, y: 2 }",
    );
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(
        &output.errors[0].kind,
        ErrorKind::RepeatedField { field, .. } if field == "x"
    ));
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

/// Reading a field off something that has none is a missing field, whatever
/// the thing is. A projection demands a fresh core carrying the field, so it
/// fits any type at all — and what fails is the field row, which is the one
/// complaint that reads correctly for every base. There is no second wording
/// for a base that "is not a struct", because every type has fields.
#[test]
fn a_projection_off_a_non_struct_names_the_type_with_no_fields() {
    let (_, _, output) = infer_src("let n = 1\nlet bad = n.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
    assert_eq!(error.kind.to_string(), "no field `x` on `Nat`");

    // The other side of the goal, and the other type with no fields: here the
    // demand is the `actual` half, because what the base turned out to be is
    // an arrow the call site had already demanded of it.
    let (_, _, output) = infer_src("let h = fn g => (fn z => g z).b");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
    assert_eq!(error.kind.to_string(), "no field `b` on `'a -> 'b`");

    // And a sum-cored base, which is the case that decides the noun: the row
    // that went wrong is the type's *fields*, so the reader is told about a
    // field even though the type beside it is written in cases.
    let (_, _, output) = infer_src("let bad = (`A 1).x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.to_string(), "no field `x` on ``A Nat | ..\'a`");

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

/// The same projection, refused from the other side of the goal, is an extra
/// field rather than a missing one. `g` demands a base carrying an `x`, and the
/// annotation on `b` is what the demand is checked against — so the `x` is a
/// label the checked-against type does not allow, and the closed empty field
/// row of `Nat` pushes it out as an extra. This used to be `not-a-struct`, and
/// which of the two complaints it becomes is decided by where the annotation
/// sits, not by anything the projection did. Pinned so the direction stays
/// deliberate.
#[test]
fn a_projection_checked_against_a_fieldless_annotation_is_an_extra_field() {
    let src = "let g = fn p => p.x\nlet b : Nat -> Nat = g";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "extra-field");
    assert_eq!(
        error.kind.to_string(),
        "extra field `x`: the type `Nat` lists every field it allows"
    );

    // Underlining the definition, which is the side the demand came from.
    assert_eq!(error.span.start, 41);
    assert_eq!(error.span.width, 1);

    // And the same thing written as one definition is the *other* complaint: the
    // annotation pushes the `Nat` down onto `p`, so the demand is the expected
    // side and the `Nat` the actual one. Two doc comments used to disagree about
    // this program; the compiler never did.
    let (_, _, output) = infer_src("let b : Nat -> Nat = fn p => p.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
    assert_eq!(error.kind.to_string(), "no field `x` on `Nat`");
}

/// Every projection complaint underlines the field name, whatever the base is.
/// There is one way for a projection to be wrong now — the base has not got the
/// field — and one place to say it: the name that was read. A base that used to
/// be "not a struct" is a base carrying no such field, and the field is still
/// the thing on the page to change.
#[test]
fn a_projection_complaint_underlines_the_field() {
    let src = "let n = 1\nlet bad = n.x";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
    assert_eq!(error.span.start, src.find(".x").expect("the field") + 1);
    assert_eq!(error.span.width, 1);

    // Including a projection straight off a literal, which is where the base
    // and the field used to be told apart.
    let src = "let bad = 1.x";
    let (_, _, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.span.start, src.find(".x").expect("the field") + 1);

    // And one whose base is a whole expression rather than a name: the span is
    // the field's either way, so the base's own span is never needed.
    let src = "let h = fn g => (fn z => g z).b";
    let (mint, out, output) = infer_src(src);
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    let TermKind::Fn { body, .. } = &term_decl(&mint, &out, "h").value.kind else {
        panic!("expected a function");
    };
    let TermKind::Project { base, field } = &body.kind else {
        panic!("expected a projection");
    };
    assert_ne!(error.span, base.span);
    assert_eq!(error.span, field.span);
}

/// A chain of projections off a base that failed is one complaint. The
/// complaint recovers the field's type to the undecided type, and every link
/// after the first absorbs rather than echoes.
#[test]
fn a_projection_chain_complains_once() {
    let (_, _, output) = infer_src("let n = 1\nlet r = n.a.b.c");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.to_string(), "no field `a` on `Nat`");
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

    // The one demand every unannotated definition makes, and in `let n = 1`
    // the only one: that the definition is what its body turned out to be.
    // `?0` is the variable the definition was put in scope as before its body
    // was walked, which is what a body naming the definition would have found.
    let (mint, _, output) = inferred("let n = 1");
    assert_eq!(constraints(&mint, &output, "n"), ["equal: ?0 ~ Nat"]);
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
                .any(|error| matches!(error.kind, ir::ErrorKind::Circular { .. })),
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
            // not five: the goal is remembered while what it broke into is
            // open, so the second `b` and `a` are recognized as the first ones
            // and not as two more copies to unfold.
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

/// An argument that cannot stand for a set of cases is refused and absorbed.
/// Lowering said it once; inference must not say it again in words about the
/// empty row, which is the failure `lowering_errors_are_absorbed_not_echoed`
/// was written to prevent.
#[test]
fn a_bad_row_argument_is_absorbed_not_echoed() {
    let (_, out, output) = infer_src(
        "type Or r = `A | ..r\n\
         let f : Or Nat -> Nat = fn p => 1",
    );
    assert_eq!(out.errors.len(), 1, "ir errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "not-a-row");
    assert!(
        output.errors.is_empty(),
        "inference echoed it: {:#?}",
        output.errors
    );
}

#[test]
fn the_solve_is_recorded_rule_by_rule() {
    let (mint, _, output) = inferred("let fst : { x: Nat } -> Nat = fn p => p.x");
    // The projection's demand meets the annotation's closed struct. The
    // labels go first, since the demand's core is what the annotation's extras
    // would have to be absorbed into — there are none here — and the core step
    // follows: the demand's `..` takes the unit the annotation says it is, and
    // the field's type is bound. The annotation's demand on the result is then
    // already answered.
    assert_eq!(
        steps(&mint, &output, "fst"),
        [
            "struct  { x: ?0, ..?1 } ~ { x: Nat } => replaced by the goals below",
            "  bind  ?1 ~ {} => ?1 := {}",
            "  bind  ?0 ~ Nat => ?0 := Nat",
            "prim  Nat ~ Nat => no change",
        ]
    );

    // And two types that are nothing but their fields record the struct rule
    // and nothing else, which is what a struct against a struct has always
    // recorded: no core step above it, because there is no core to speak of.
    let (mint, _, output) = inferred("let v : { x: Nat } -> { x: Nat } = fn p => p");
    assert_eq!(
        steps(&mint, &output, "v"),
        [
            "struct  { x: Nat } ~ { x: Nat } => replaced by the goals below",
            "  prim  Nat ~ Nat => no change",
        ]
    );

    // A `Nat` against a `Nat` is one step, for the same reason: neither
    // carries a field, so there is no second row to decide.
    let (mint, _, output) = inferred("let w : Nat = 1");
    assert_eq!(steps(&mint, &output, "w"), ["prim  Nat ~ Nat => no change"]);
}

/// Two closed sets of labels that each name one the other does not still
/// continue as one, and what they continue as allows nothing — twice, once from
/// each side. The second time is the one place the solver is asked whether a
/// tail that allows nothing equals itself, and it is a rule of its own rather
/// than a mismatch: the first side settled the shared continuation, and the
/// second finds it already there.
#[test]
fn closing_a_row_from_both_sides_agrees_with_itself() {
    // For a struct the continuation is a core, and what closes it is unit.
    let (mint, _, output) = infer_src("let p : { a: Nat } = { b: 1 }");
    assert_eq!(
        steps(&mint, &output, "p"),
        [
            "struct  { a: Nat } ~ { b: Nat } => replaced by the goals below",
            "  presence  absent ~ present => extra field `b`: the type `{ a: Nat }` lists every field it allows",
            "  bind  {} ~ ?0 => ?0 := {}",
            "  presence  present ~ absent => no field `a` on `{ b: Nat }`",
        ]
    );

    // For a sum it is still a tail, and both halves of the closing show: the
    // first side binds the shared continuation to `∅`, and the second finds it
    // already there and says so.
    let (mint, _, output) = infer_src(
        "let f : (`A Nat) -> Nat = fn p => 1\n\
         let g : (`B Nat) -> Nat = fn p => 1\n\
         let h = fn v => { one: f v, two: g v }",
    );
    let taken = steps(&mint, &output, "h");
    assert!(
        taken.contains(&"  bind  ∅ ~ ?2 => ?2 := ∅".to_string()),
        "{taken:#?}"
    );
    assert!(
        taken.contains(&"  same  ∅ ~ ∅ => no change".to_string()),
        "{taken:#?}"
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
            // `?0` is the definition itself, minted before its body was walked;
            // `?1` is `p`. The first projection's demand is a bare variable's
            // whole value, fields and all.
            "bind  { x: ?2, ..?3 } ~ ?1 => ?1 := { x: ?2, ..?3 }",
            // The next two each decide the labels first and let the cores take
            // what is left, since a core variable is what the extras go into.
            "struct  { y: ?4, ..?5 } ~ { x: ?2, ..?3 } => replaced by the goals below",
            "  bind  ?5 ~ { x: ?2, ..?8 } => ?5 := { x: ?2, ..?8 }",
            "  bind  { y: ?4, ..?8 } ~ ?3 => ?3 := { y: ?4, ..?8 }",
            // The base carries `x` and, behind `?3`, `y`; the goal says so
            // rather than leaving the reader to remember it.
            "struct  { z: ?6, ..?7 } ~ { x: ?2, y: ?4, ..?8 } => replaced by the goals below",
            "  bind  ?7 ~ { x: ?2, y: ?4, ..?9 } => ?7 := { x: ?2, y: ?4, ..?9 }",
            "  bind  { z: ?6, ..?9 } ~ ?8 => ?8 := { z: ?6, ..?9 }",
            "bind  ?0 ~ ?1 -> { a: ?2, b: ?4, c: ?6 } => ?0 := ?1 -> { a: ?2, b: ?4, c: ?6 }",
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
            "bind  { x: Nat, ..?3 } ~ ?2 => ?2 := { x: Nat, ..?3 }",
            "struct  { y: Nat, ..?3 } ~ { x: Nat, ..?3 } => replaced by the goals below",
            "  occurs  { y: Nat, ..?3 } ~ { x: Nat, ..?3 } => this type would have to contain itself",
            "  recover  ?3 ~ ? => ?3 := ?",
            "bind  ?1 ~ ?2 -> Nat => ?1 := ?2 -> Nat",
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
            "  bind  ?5 ~ {} => ?5 := {}",
            "  struct  { x: ?2, ..?3 } ~ { x: Nat } => replaced by the goals below",
            "    bind  ?3 ~ {} => ?3 := {}",
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
    // The labels go first, because the demand's core is what a `Nat`'s extras
    // would have to be absorbed into — and the labels are where it fails: a
    // `Nat` carries no fields and allows none. The goal is abandoned and the
    // field's type is pointed at `?`, while the demand's own core takes the
    // `Nat`, since that is what the other side said it allows. Each change to
    // the solution has to be a step: otherwise a reader watching the state
    // would see a variable acquire a value that no rule they were shown gave
    // it.
    let (mint, _, output) = infer_src("let n = 1\nlet bad = n.x");
    assert_eq!(
        steps(&mint, &output, "bad"),
        [
            "struct  { x: ?2, ..?3 } ~ Nat => replaced by the goals below",
            "  presence  present ~ absent => no field `x` on `Nat`",
            "  recover  ?2 ~ ? => ?2 := ?",
            "  bind  ?3 ~ Nat => ?3 := Nat",
            // And then the core, which the step above bound to the `Nat` and
            // the failure took back: a type is its core and its fields, one
            // goal decided both, and a core standing for a type that carries a
            // field it cannot carry is a type nothing satisfies. Said as a step
            // like every other change to the solution.
            "  recover  ?3 ~ ? => ?3 := ?",
            // And the definition is what its abandoned body is, which absorbs
            // — and abandons `bad`'s own variable with it, so nothing later
            // reads it as still open.
            "absorb  ?1 ~ ? => no change",
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

/// A declaration stands for its body with its arguments put in, so a term
/// checked against one is checked against the shape that substitution reaches.
#[test]
fn a_constructor_stands_for_its_body_with_its_arguments_put_in() {
    let (mint, _, output) = inferred(
        "type Pair A B = { first: A, second: B }\n\
         let p : Pair Nat Nat = { first: 1, second: 2 }",
    );
    assert_eq!(scheme(&mint, &output, "p"), "Pair Nat Nat");

    // The argument reaches the field it was substituted into.
    let (_, _, output) = infer_src(
        "type Pair A B = { first: A, second: B }\n\
         let p : Pair Nat Nat = { first: 1, second: {} }",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
}

/// Two applications of one declaration are equal when their arguments are, and
/// that is decided without unfolding either. Deciding it here is what keeps a
/// declaration that leads back to itself from unfolding forever.
#[test]
fn applications_of_one_declaration_are_equal_by_their_arguments() {
    let (_, _, output) = inferred(
        "type Pair A B = { first: A, second: B }\n\
         let same : Pair Nat Nat -> Pair Nat Nat = fn p => p",
    );
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    assert!(rules.contains(&Rule::Congruent), "rules: {rules:?}");
    assert!(!rules.contains(&Rule::Unfold), "rules: {rules:?}");

    // A congruence that fails is a failure of the two applications, so that is
    // what the reader is shown: the types they wrote, not a field of a body
    // neither of them put on the page.
    let (_, _, output) = infer_src(
        "type Pair A B = { first: A, second: B }\n\
         let f : Pair Nat Nat -> Nat = fn p => p.first\n\
         let g : Pair Nat {} -> Nat = fn p => f p",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "type-mismatch");
    assert_eq!(
        output.errors[0].kind.to_string(),
        "type mismatch: expected `Pair Nat Nat`, found `Pair Nat {}`"
    );

    // And the attempt leaves nothing behind: the steps a reader is shown are
    // the ones that decided the answer, and the state they build is the state
    // the solve ended in.
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    assert!(!rules.contains(&Rule::Congruent), "rules: {rules:?}");
    assert!(rules.contains(&Rule::Mismatch), "rules: {rules:?}");
}

/// Two *different* declarations are still equal by what they unfold to, which
/// is what keeps a name a barrier to unfolding rather than to equality.
#[test]
fn applications_of_different_declarations_unfold() {
    let (_, _, output) = inferred(
        "type Box A = { it: A }\n\
         type Crate A = { it: A }\n\
         let f : Box Nat -> Crate Nat = fn b => b",
    );
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    assert!(rules.contains(&Rule::Unfold), "rules: {rules:?}");
}

/// A parameter a declaration discards buys no distinction. `type Ptr a = Nat`
/// stands for `Nat` whatever it is applied to, so `Ptr A` and `Ptr B` are one
/// type — the language has no phantom types, and congruence is refused here
/// precisely because taking it would say otherwise.
///
/// This is the transitivity the old rule broke: each of `Ptr A` and `Ptr B` was
/// `Nat` by unfolding while the two were kept apart from each other by their
/// name, so equality depended on which pair you asked about.
#[test]
fn a_constructor_that_ignores_a_parameter_is_transparent() {
    let (mint, _, output) = inferred(
        "type Ptr a = Nat\n\
         type A = { a: Nat }\n\
         type B = { b: Nat }\n\
         let x : Ptr A = 1\n\
         let y : Ptr B = x\n\
         let z : Nat = x",
    );
    // It still prints as what the reader wrote: a name is a barrier to
    // unfolding, not to equality.
    assert_eq!(scheme(&mint, &output, "y"), "Ptr B");

    // And it is decided by unfolding rather than by the name, so no congruence
    // was taken.
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    assert!(!rules.contains(&Rule::Congruent), "rules: {rules:?}");

    // A parameter reaching the body only through something that discards it is
    // discarded too, however many hand-offs there are in between.
    let (_, _, output) = inferred(
        "type Ptr a = Nat\n\
         type Alias b = Ptr b\n\
         type A = { a: Nat }\n\
         type B = { b: Nat }\n\
         let x : Alias A = 1\n\
         let y : Alias B = x",
    );
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    assert!(!rules.contains(&Rule::Congruent), "rules: {rules:?}");

    // What is still refused is a difference the declaration keeps.
    let (_, _, output) = infer_src(
        "type Box a = { it: a }\n\
         let x : Box Nat = { it: 1 }\n\
         let y : Box {} = x",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
}

/// A group whose members mention each other at a fixed argument is an ordinary
/// recursive type, and two such groups of the same shape are one type.
///
/// The goal comes back round at the same arguments it went in with, so the
/// assumption fires and the solve ends — which is the half of the finer key
/// that could have been lost: a key that never repeats answers nothing.
#[test]
fn a_recursion_at_a_fixed_argument_still_comes_back_round() {
    let (mint, _, output) = inferred(
        "type Tree a = { value: a, kids: Forest }\n\
         type Forest = { head: Tree Nat, tail: Forest }\n\
         type Wood a = { value: a, kids: Grove }\n\
         type Grove = { head: Wood Nat, tail: Grove }\n\
         let f : Forest -> Grove = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "Forest -> Grove");

    // And the step a reader is shown says which goal was assumed, arguments
    // and all — which is the key itself, so the solve reads back as what the
    // solver actually did.
    assert_eq!(
        steps(&mint, &output, "f"),
        [
            "unfold  Grove ~ Forest => replaced by the goals below",
            "  struct  { head: Wood Nat, tail: Grove } ~ { head: Tree Nat, tail: Forest } \
             => replaced by the goals below",
            "    unfold  Wood Nat ~ Tree Nat => replaced by the goals below",
            "      struct  { value: Nat, kids: Grove } ~ { value: Nat, kids: Forest } \
             => replaced by the goals below",
            "        prim  Nat ~ Nat => no change",
            "        assume  Grove ~ Forest => no change",
            "    assume  Grove ~ Forest => no change",
        ]
    );
}

/// An argument may hold a variable, and the variable may be bound while the
/// goal that carries it is still open. The assumption is then read against what
/// the solver has since decided rather than against what it believed when the
/// goal was pushed.
///
/// `peek` is generalized over whether its argument has an `x`, so the use in `q`
/// asks `Tree { x?: Nat }` against `Wood { x: Nat }` with that question still
/// open. Comparing the two `value` fields settles it, and by the time the goal
/// comes back round one field further in it is the same goal — which the step
/// list below shows being assumed rather than unfolded a second time.
#[test]
fn an_assumption_is_read_against_what_has_since_been_decided() {
    let (mint, _, output) = inferred(
        "type Tree a = { value: a, kids: Forest }\n\
         type Forest = { head: Tree { x: Nat }, tail: Forest }\n\
         type Wood a = { value: a, kids: Grove }\n\
         type Grove = { head: Wood { x: Nat }, tail: Grove }\n\
         let peek : Tree { x?: Nat } -> Forest = fn t => t.kids\n\
         let q : Wood { x: Nat } -> Grove = fn w => peek w",
    );
    assert_eq!(scheme(&mint, &output, "peek"), "Tree { x?: Nat } -> Forest");
    assert_eq!(
        steps(&mint, &output, "q")[..9],
        [
            "unfold  Tree { x?: Nat } ~ Wood { x: Nat } => replaced by the goals below",
            "  struct  { value: { x?: Nat }, kids: Forest } ~ { value: { x: Nat }, kids: Grove } \
         => replaced by the goals below",
            "    struct  { x?: Nat } ~ { x: Nat } => replaced by the goals below",
            // Here is where the argument stops being a question.
            "      bind  ?3 ~ present => ?3 := present",
            "      prim  Nat ~ Nat => no change",
            "    unfold  Forest ~ Grove => replaced by the goals below",
            "      struct  { head: Tree { x: Nat }, tail: Forest } ~ \
         { head: Wood { x: Nat }, tail: Grove } => replaced by the goals below",
            // And here is the goal on the stack, recognized through the binding
            // that has happened since it was pushed.
            "        assume  Tree { x: Nat } ~ Wood { x: Nat } => no change",
            "        assume  Forest ~ Grove => no change",
        ]
    );
}

/// The assumption that two names already being compared are equal has to be
/// keyed on the whole goal, arguments and all. This program is the reason.
///
/// `Forest` against `Forest2` unfolds to `Tree Nat` against `Tree2 Nat`, whose
/// unfolding asks `Bush` against `Bush2`, whose unfolding asks `Tree {}`
/// against `Tree2 Nat`. Those are the same two *declarations* as the pair
/// already open, and a different question: one stands for `{ v: {}, .. }` and
/// the other for `{ v: Nat, .. }`. Keyed on the names alone the solver would
/// take them as equal and accept `f`; keyed on the goal it asks the question
/// and finds the contradiction.
#[test]
fn a_pair_of_declarations_is_not_assumed_equal_at_other_arguments() {
    let (_, out, output) = infer_src(
        "type Tree x = { v: x, k: Bush }\n\
         type Bush = { t: Tree {}, f: Forest }\n\
         type Forest = { t: Tree Nat }\n\
         type Tree2 x = { v: x, k: Bush2 }\n\
         type Bush2 = { t: Tree2 Nat, f: Forest2 }\n\
         type Forest2 = { t: Tree2 Nat }\n\
         let f : Forest -> Forest2 = fn x => x",
    );
    // Nothing here grows, so lowering has nothing to say: the whole question is
    // the solver's.
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "type-mismatch");
}

/// A recursive constructor unfolds against another of the same shape and comes
/// back round rather than running away. The point of the test is that it
/// returns at all.
#[test]
fn a_recursive_constructor_does_not_unfold_forever() {
    let (mint, _, output) = inferred(
        "type List a = { head: a, tail: List a }\n\
         type Seq a = { head: a, tail: Seq a }\n\
         let f : List Nat -> Seq Nat = fn x => x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "List Nat -> Seq Nat");
}

/// The declaration binds one parameter, so the annotation hands it one, and
/// opening the scheme reaches every `A` in the body.
#[test]
fn a_repeated_parameter_leaves_one_argument_to_supply() {
    let (mint, _, output) = infer_src("type P A A = { x: A }\nlet v : P Nat = { x: 1 }");
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    assert_eq!(scheme(&mint, &output, "v"), "P Nat");
}

/// A declaration standing for its own argument may be nested as deep as the
/// writer likes: unfolding follows one declaration once per level, which is
/// not following it round.
#[test]
fn a_pass_through_declaration_may_be_nested() {
    let (mint, _, output) = inferred("type Id a = a\nlet x : Id (Id Nat) = 1");
    assert_eq!(scheme(&mint, &output, "x"), "Id (Id Nat)");

    // Deeper than there are declarations, which is what the old bound could
    // not survive.
    inferred("type Id a = a\nlet z : Id (Id (Id (Id Nat))) = 1");

    // And it still unfolds to something: the literal is checked against `Nat`.
    let (_, _, output) = infer_src("type Id a = a\nlet y : Id (Id Nat) = {}");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "type-mismatch");
}

/// A row written inside an argument still says what its tail may not stand for.
/// The condition lives beside the variables rather than inside the type, so it
/// is only recorded by a walk that reaches in — and if this one stops at the
/// name, the tail is quietly bound to a row repeating `x` and the definition
/// comes out with a type it was never shown to have.
#[test]
fn a_row_inside_an_argument_still_lacks_what_it_names() {
    let (_, _, output) = infer_src(
        "type Box A = { it: A }\n\
         let f : Box { x: Nat, ..r } -> { ..r } = fn b => { x: 1 }",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "repeated-field");
}

/// A quantified argument is the definition's to generalize, the same as one
/// written anywhere else. This is what fails if `quantify` or `zonk` stops at a
/// name instead of descending into what it was applied to.
#[test]
fn an_open_argument_is_generalized_with_its_definition() {
    let (mint, _, output) = inferred(
        "type Box A = { it: A }\n\
         let wrap : Box { x: Nat, .. } -> Nat = fn b => b.it.x",
    );
    assert_eq!(
        scheme(&mint, &output, "wrap"),
        "Box { x: Nat, ..'a } -> Nat"
    );
}

/// Nothing downstream of inference resolves a solver variable, so none may
/// survive inside an argument either.
#[test]
fn no_solver_variable_survives_inside_an_argument() {
    let (_, _, output) = inferred(
        "type Box A = { it: A }\n\
         let wrap : Box { x: Nat, .. } -> Nat = fn b => b.it.x",
    );
    for scheme in output.schemes.values() {
        assert!(
            !mentions_a_variable(scheme.body()),
            "a variable survived: {scheme}"
        );
    }
}

/// A row argument's fields are spliced in where the parameter's tail sat. None
/// of that is new machinery: substitution puts the row into the tail, and the
/// flattening that already existed for a tail bound to a row does the rest.
#[test]
fn a_row_argument_is_spliced_into_the_tail() {
    let (mint, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { y: Nat } -> Nat = fn p => p.x\n\
         let n = f { x: 1, y: 2 }",
    );
    assert_eq!(scheme(&mint, &output, "n"), "Nat");

    // Given nothing, the declaration is just its own fields.
    let (_, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         let g : WithX {} -> Nat = fn p => p.x\n\
         let m = g { x: 1 }",
    );
    assert!(output.errors.is_empty(), "errors: {:#?}", output.errors);

    // And the spliced field really is required: the row said `y` is there.
    let (_, _, output) = infer_src(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { y: Nat } -> Nat = fn p => p.x\n\
         let n = f { x: 1 }",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
}

/// A declaration's body says `r` has no `x`, and that condition has to be said
/// again of every use — it lives beside the variables a use site mints, not
/// inside the type. Handing the row a field the declaration already names is
/// refused, and refused the same way whichever route it arrives by.
///
/// A row written out is refused where it is written, because what the
/// declaration names is part of what its parameter stands for and lowering has
/// both. A row that only turns out to name the field once the solve says what
/// it is has nowhere earlier to be caught, and is caught at the binding.
#[test]
fn a_row_argument_may_not_repeat_a_field_the_type_names() {
    let (_, out, _) = infer_src(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { x: Nat } -> Nat = fn p => p.x",
    );
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "repeated-row-field");

    // The same complaint reached the other way: not a written row spliced over
    // a field, but a tail variable the definition later tries to bind to one.
    // The condition is only on the variable once something has asked for the
    // declaration's shape, which the projection here is what does.
    let (_, _, output) = infer_src(
        "type WithX r = { x: Nat, ..r }\n\
         let g : WithX { ..s } -> { ..s } = fn p => { x: p.x }",
    );
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "repeated-field");

    // Naming the field rather than covering it is not the same thing, and is
    // fine: the row still stands for what the declaration does not name.
    let (_, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         let ok : WithX { ..s } -> { x: Nat, ..s } = fn p => p",
    );
    assert!(output.errors.is_empty(), "errors: {:#?}", output.errors);
}

/// The condition a declaration imposes on its row argument is recorded at the
/// application, not on the way out of an unfolding — because there may not be
/// one. Two applications of one declaration are compared argument against
/// argument, so a goal about `WithX` can be decided from end to end without
/// either side's body ever being built, and a condition only an unfolding
/// recorded would simply be lost.
///
/// This program is what that cost. `f` says its argument is `WithX` of some
/// rest, and `g` hands that rest a row naming `x` — which `WithX` names
/// already. Nothing here unfolds `WithX`: the two `WithX`es meet each other and
/// agree by their arguments. The definition used to come out typed
/// `WithX { x: Nat } -> Nat`, a type naming one field twice, with nothing said.
#[test]
fn a_row_condition_survives_a_goal_that_never_unfolds() {
    let (_, out, output) = infer_src(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { ..s } -> { ..s } -> Nat = fn a => fn b => 1\n\
         let g : WithX { ..t } -> Nat = fn q => f q { x: 1 }",
    );
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "repeated-field");

    // A label `WithX` does not name is what the `..` was for, and travels the
    // same route with nothing to say.
    let (_, _, output) = infer_src(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { ..s } -> { ..s } -> Nat = fn a => fn b => 1\n\
         let g : WithX { y: Nat } -> Nat = fn q => f q { y: 1 }",
    );
    assert!(output.errors.is_empty(), "errors: {:#?}", output.errors);
}

/// Refusing a row is a failure like any other, so the goal it was about is
/// over: [`Solve::fail`] has already abandoned every copy of the repeated
/// field, and lining the two rows up afterwards would compare whichever copy
/// flattening happened to keep and complain a second time about a type nobody
/// wrote.
///
/// The whole route is now caught a phase earlier — what a declaration names is
/// part of what its row parameter stands for, so lowering refuses the argument
/// where it is written — which is why this asserts the outcome rather than the
/// rule: one complaint about the row, at the row, and nothing said twice.
#[test]
fn a_refused_row_is_not_compared_afterwards() {
    let src = "type W r = { x: Nat, ..r }\nlet p : W { x: {} } = { x: {} }";
    let (_, out, output) = infer_src(src);
    assert_eq!(out.errors.len(), 1, "ir errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "repeated-row-field");
    assert_eq!(
        out.errors[0].span.start,
        src.find("{ x: {} }").expect("the argument")
    );

    // Nothing says it again where it was said: what inference is left with is
    // the definition's own body against the fields `W` declares, which is a
    // complaint about somewhere else or none at all.
    for error in &output.errors {
        assert_ne!(error.span.start, out.errors[0].span.start, "{:#?}", error);
        assert_ne!(error.kind.code(), "repeated-field", "{error:#?}");
    }
}

/// The condition is re-imposed per use rather than recorded once, so one use
/// being wrong says nothing about another. Two annotations of the same
/// declaration, one legal and one not, is what tells the two apart.
#[test]
fn the_lacks_condition_is_said_again_at_each_use() {
    let src = "type WithX r = { x: Nat, ..r }\n\
               let fine : WithX { y: Nat } -> Nat = fn p => p.x\n\
               let bad : WithX { x: Nat } -> Nat = fn p => p.x\n\
               let also : WithX { z: Nat } -> Nat = fn p => p.x";
    let (_, out, output) = infer_src(src);
    assert_eq!(out.errors.len(), 1, "ir errors: {:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "repeated-row-field");
    // At the one argument that names it, and not at either of the two beside it
    // that do not.
    assert_eq!(
        out.errors[0].span.start,
        src.find("{ x: Nat }").expect("the offending argument")
    );
    // And the argument absorbed, so nothing downstream says it again.
    assert!(output.errors.is_empty(), "errors: {:#?}", output.errors);
}

/// A declaration left open through a parameter is still a declaration: its body
/// mentions no solver variable, so it generalizes and prints as itself.
#[test]
fn a_row_parameter_leaves_the_declaration_closed_to_the_solver() {
    let (mint, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX { y: Nat } -> Nat = fn p => p.x",
    );
    assert_eq!(scheme(&mint, &output, "f"), "WithX { y: Nat } -> Nat");
    for scheme in output.schemes.values() {
        assert!(
            !mentions_a_variable(scheme.body()),
            "a variable survived: {scheme}"
        );
    }
}

/// Every shape the growth rule allows comes back round. Each of these is a
/// solve that only ends because a goal it has already taken on is one it
/// recognizes, and reaching the assertion at all is most of what is checked.
#[test]
fn a_recursion_that_cannot_grow_always_comes_back_round() {
    // A group member mentioned at a fixed argument that is itself a member of
    // the group.
    inferred(
        "type A a = { x: A B, v: a }\n\
         type B = { y: A Nat }\n\
         type C a = { x: C D, v: a }\n\
         type D = { y: C Nat }\n\
         let f : A Nat -> C Nat = fn z => z",
    );

    // Asked at an argument the declarations never mention, so the goal has to
    // travel one round before it repeats.
    inferred(
        "type T a = { next: T Nat }\n\
         type U a = { next: U Nat }\n\
         let f : T {} -> U {} = fn x => x",
    );

    // A permutation: the arguments only ever swap places, so the lists they
    // make are finitely many.
    inferred(
        "type A a b = { x: B b a, v: a }\n\
         type B c d = { x: A d c, v: c }\n\
         type P a b = { x: Q b a, v: a }\n\
         type Q c d = { x: P d c, v: c }\n\
         let f : A Nat {} -> P Nat {} = fn x => x",
    );
}

/// A tag literal is one case of however many the context allows, so its type
/// names that case and leaves the tail open. That is the exact opposite of a
/// struct literal, which has every field it will ever have — and it is the
/// whole of what makes a sum row-polymorphic.
#[test]
fn a_tag_is_open_where_a_struct_literal_is_closed() {
    let (mint, _, out) = inferred("let v = `Some 1");
    assert_eq!(scheme(&mint, &out, "v"), "`Some Nat | ..'a");

    // A case written bare carries unit, which is the same type `()` has.
    let (mint, _, out) = inferred("let n = `None");
    assert_eq!(scheme(&mint, &out, "n"), "`None | ..'a");

    // The payload is inferred like anything else, so a function over one is
    // polymorphic in what it wraps as well as in the cases it allows.
    let (mint, _, out) = inferred("let wrap = fn x => `Some x");
    assert_eq!(scheme(&mint, &out, "wrap"), "'a -> `Some 'a | ..'b");
}

/// A declared sum lists every case there is, so a literal naming one of them
/// fits: the literal's own tail absorbs the cases it did not name.
#[test]
fn a_tag_fits_a_declaration_that_allows_its_case() {
    let (mint, _, out) = inferred(
        "type Option T = `Some T | `None\n\
         let some : Option Nat = `Some 1\n\
         let none : Option Nat = `None",
    );
    assert_eq!(scheme(&mint, &out, "some"), "Option Nat");
    assert_eq!(scheme(&mint, &out, "none"), "Option Nat");

    // And the annotation is what the definition means downstream, so a second
    // definition can take it as the declared type.
    let (mint, _, out) = inferred(
        "type Option T = `Some T | `None\n\
         let some : Option Nat = `Some 1\n\
         let again : Option Nat = some",
    );
    assert_eq!(scheme(&mint, &out, "again"), "Option Nat");
}

/// A case the type does not allow is the sum's version of an extra field, and
/// a case it requires that the value cannot be is a missing one. Both are
/// worded as cases, because the type they are about says so.
#[test]
fn a_case_a_sum_does_not_allow_is_reported_as_a_case() {
    let (_, _, out) = infer_src(
        "type Flag = `On | `Off\n\
         let bad : Flag = `Maybe",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "extra-field");
    assert_eq!(
        out.errors[0].kind.to_string(),
        "extra case ``Maybe`: the type ``On | `Off` lists every case it allows"
    );

    // The other direction: a closed sum on the value's side, against a type
    // that requires a case it has not got.
    let (_, _, out) = infer_src(
        "let f : (`A Nat | `B) -> Nat = fn x => 1\n\
         let g : (`A Nat) -> Nat = fn y => f y",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "missing-field");
    assert_eq!(out.errors[0].kind.to_string(), "no case ``B` on ``A Nat`");
}

/// A struct and a sum are two types, however much their rows look alike. The
/// solver refuses the pair rather than lining their labels up, and says so as
/// the ordinary mismatch it is.
#[test]
fn a_struct_and_a_sum_are_never_the_same_type() {
    let (_, _, out) = infer_src("let bad : { a: Nat } = `a 1");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "type-mismatch");

    // And a sum carries no fields of its own, so a projection against one is
    // a missing *field* — the noun follows the row that went wrong, which is
    // the type's field row, not the cases the type prints as.
    let (_, _, out) = infer_src("let f : (`A Nat) -> Nat = fn s => s.a");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "missing-field");
    assert_eq!(out.errors[0].kind.to_string(), "no field `a` on ``A Nat`");
}

/// The row machinery is one machinery, so a declaration takes the rest of a
/// sum's cases exactly as it takes the rest of a struct's fields.
#[test]
fn a_declaration_may_take_the_rest_of_a_sum() {
    let (mint, _, out) = inferred(
        "type Fallible r = `Err Nat | ..r\n\
         let ok : Fallible (`Ok Nat) = `Ok 1",
    );
    assert_eq!(scheme(&mint, &out, "ok"), "Fallible (`Ok Nat)");

    // What the tail may not name is a fact about the declaration, so a use
    // site repeating one of its cases is refused where the argument is
    // written — before anything unfolds.
    let (_, out, _) = infer_src(
        "type Fallible r = `Err Nat | ..r\n\
         let bad : Fallible (`Err Nat) = `Err 1",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        &out.errors[0].kind,
        ir::ErrorKind::RepeatedRowField { field, .. } if field == "Err"
    ));
}

/// The solve reads as what it is about: a goal between two sums fires the sum
/// rule where one between two structs fires the struct rule, so a reader
/// stepping through it is never told about a struct they did not write.
#[test]
fn the_solve_names_the_shape_it_took_apart() {
    let (mint, _, out) = inferred(
        "type Flag = `On Nat | `Off Nat\n\
         let v = `On 1\n\
         let f : Flag = v",
    );
    let taken = steps(&mint, &out, "f");
    assert!(taken.iter().any(|step| step.contains("sum")), "{taken:#?}");
    assert!(
        !taken.iter().any(|step| step.contains("struct")),
        "{taken:#?}"
    );

    // A case carrying nothing carries unit, and two units are already the same
    // thing — no labels on either side, and a core that is unit both ways — so
    // the payload records no step at all. Which is the whole of why a struct
    // against a struct reads as it always has: the core step under a
    // [`Rule::Struct`] is silent whenever the two cores are unit.
    let (mint, _, out) = inferred(
        "type Flag = `On | `Off\n\
         let v = `On\n\
         let g : Flag = v",
    );
    let carrying_unit = steps(&mint, &out, "g");
    assert!(
        !carrying_unit.iter().any(|step| step.contains("struct")),
        "{carrying_unit:#?}"
    );
    assert!(
        carrying_unit.iter().any(|step| step.contains("sum")),
        "{carrying_unit:#?}"
    );
}

/// Checking pushes an expected sum into a tag, so a literal named by the type
/// it is being checked against asks nothing of the row at all: what is left is
/// the payload against what the case carries.
#[test]
fn checking_pushes_a_sum_into_a_tag() {
    let (mint, _, out) = inferred(
        "type Flag = `On Nat | `Off Nat\n\
         let f : Flag = `On 1",
    );
    assert_eq!(
        steps(&mint, &out, "f"),
        vec!["prim  Nat ~ Nat => no change"]
    );
    // And the term keeps the name the annotation gave it rather than the row
    // the literal would have been inferred.
    assert_eq!(scheme(&mint, &out, "f"), "Flag");
}

/// A case that may or may not be allowed is the dual of a field that may or
/// may not be there, and it is not something a tail can say: a tail allows
/// every other case, and a `?` allows exactly one more.
#[test]
fn a_case_may_be_marked_optional() {
    let (mint, _, out) = inferred("let m : `A? Nat | `B = `B");
    assert_eq!(scheme(&mint, &out, "m"), "`A? Nat | `B");

    // Written open, the definition would have to decide it, and an annotation
    // the definition narrows is one the reader was promised more by than the
    // definition keeps.
    let (_, _, out) = infer_src("let m : `A? Nat | `B = `A 1");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind.code(), "annotation-too-open");
}

/// A sum leads back to itself the way a struct does, and unfolding it stops at
/// the name — so a recursive sum is a finite type with a finite scheme.
#[test]
fn a_sum_may_name_itself() {
    let (mint, _, out) = inferred(
        "type List a = `Nil | `Cons { head: a, tail: List a }\n\
         let empty : List Nat = `Nil\n\
         let one : List Nat = `Cons { head: 1, tail: empty }",
    );
    assert_eq!(scheme(&mint, &out, "empty"), "List Nat");
    assert_eq!(scheme(&mint, &out, "one"), "List Nat");
    let symbol = symbol_named(&mint, out.aliases.keys().copied(), "List");
    assert_eq!(
        out.aliases[&symbol].to_string(),
        "`Nil | `Cons { head: 'a, tail: List 'a }"
    );
}

/// The lacks condition is one condition, so it reaches a sum's tail through a
/// variable exactly as it reaches a struct's — and the complaint that comes
/// back is worded as the cases it is about.
#[test]
fn a_sum_tail_may_not_come_back_naming_its_own_case() {
    let (_, out, output) = infer_src(
        "let h : (`A Nat | ..r) -> (| ..r) -> Nat = fn a => fn b => 1\n\
         let k = fn p => h p (`A 1)",
    );
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(
        matches!(&output.errors[0].kind, ErrorKind::RepeatedField { field, .. } if field == "A"),
        "{:#?}",
        output.errors
    );
    assert_eq!(
        output.errors[0].kind.to_string(),
        "`..` covers only the cases a type does not already name, \
         and here it would have to cover ``A`"
    );
}

/// A label both rows name and disagree about is reported as the label, never
/// as `present` against `absent`: the name is known here, and "no field `x`"
/// is what a reader can act on where "expected present, found absent" is a
/// sentence about the solver.
///
/// Getting a row to hold a settled `absent` takes three uses of one parameter:
/// one to give it the optional field, one to close it — which settles the
/// field away — and one that then asks for the field back. A `?` field is the
/// only thing that can be settled either way, so it is the only thing that can
/// disagree with a constant on the other side.
#[test]
fn a_settled_presence_is_reported_as_the_field_it_is_about() {
    // The annotation demands the field; the row it is given no longer has it.
    let (_, _, output) = infer_src(
        "let optional : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let demands : { x: Nat } -> Nat = fn p => p.x\n\
         let f = fn r => { a: optional r, b: closed r, c: demands r }",
    );
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "missing-field");
    assert_eq!(
        error.kind.to_string(),
        "no field `x` on `{}`",
        "the row is quoted as it stands, and a field settled away is not part of it"
    );

    // And the other way round: the parameter settled the field away, so a
    // caller writing it is writing one the function does not take.
    let (_, _, output) = infer_src(
        "let optional : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let f = fn r => { a: optional r, b: closed r }\n\
         let v = f { x: 1 }",
    );
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "extra-field");
    assert!(
        error.kind.to_string().contains("extra field `x`"),
        "{}",
        error.kind
    );
}

/// A field already known to be absent is nothing for a closed row to refuse:
/// it is not part of what the type says, so closing a row that has settled it
/// away is closing a row that does not name it.
#[test]
fn a_field_settled_absent_passes_a_closed_row_without_a_word() {
    inferred(
        "let optional : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let f = fn r => { a: optional r, b: closed r, c: closed r }",
    );

    // And an open row asking for it back settles its own `?` the same way,
    // without going near the field's type: an absent field's type slot means
    // nothing, and constraining it would refuse rows that agree.
    inferred(
        "let optional : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let f = fn r => { a: optional r, b: closed r, c: optional r }",
    );
}

/// A field already known to be absent is nothing for a *lacking tail* to
/// refuse either, for the reason the closed row above lets it pass: it is not
/// part of what the type says. `q` here is `{}` — its optional `x` was settled
/// away by `closed` — and `g` asks `q` to be exactly what `p` has beyond `x`,
/// which `{}` satisfies. The lacks check rules on presence, not on the bare
/// name: only a label certainly there is a copy the tail could repeat, so a
/// row the solver holds equal to `{}` binds wherever `{}` would.
#[test]
fn a_field_settled_absent_is_no_repeat_for_a_tail_that_lacks_it() {
    inferred(
        "let opt : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let g : { x: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn p q => { a: opt q, b: closed q, c: g p q }",
    );

    // The same row met the other way round — the lacking tail arriving on the
    // actual side — reaches the same check at the same binding.
    inferred(
        "let opt : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let closed : {} -> Nat = fn p => 1\n\
         let g : { ..r } -> { x: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn p q => { a: opt q, b: closed q, c: g q p }",
    );
}

/// The sum's spelling of the test above: a case settled absent is not one of
/// the cases a value can be, so a tail forbidden the case can still take the
/// row. `q` can only be `` `B `` — `only` settled its optional `` `A `` away —
/// and `g` asks `q` to be exactly the cases `p` allows beyond `` `A ``.
#[test]
fn a_case_settled_absent_is_no_repeat_for_a_tail_that_lacks_it() {
    inferred(
        "let opt : (`A? Nat | ..) -> Nat = fn p => 1\n\
         let only : (`B Nat) -> Nat = fn p => 1\n\
         let g : (`A Nat | ..r) -> (| ..r) -> Nat = fn a => fn b => 1\n\
         let h = fn p q => { a: opt q, b: only q, c: g p q }",
    );
}

/// A field whose presence is still being decided, meeting a tail that lacks
/// it, is the presence being decided: absent is the one answer both sides
/// allow, and it is the answer the same field gets against a closed row —
/// `closed r` settles an undecided `x?` away without a word. A tail that
/// lacks `x` says no more about `x` than a closed row does, so meeting one
/// settles the presence the same way rather than refusing a program that has
/// a type.
#[test]
fn an_undecided_field_meeting_a_tail_that_lacks_it_settles_absent() {
    inferred(
        "let opt : { x?: Nat, .. } -> Nat = fn p => 1\n\
         let g : { x: Nat, ..r } -> { ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn p q => { a: opt q, b: g p q }",
    );
}

/// Nothing is decided twice, in every sort: the value a failure leaves behind
/// absorbs whatever a later goal puts it against, and abandons it. A presence
/// and a sum's tail are the two sorts that were previously only ever abandoned
/// at the end of a solve, so this is where they are asked to do the absorbing —
/// and a struct's open end is now a *core*, which absorbs at the top of the
/// rule rather than under it.
#[test]
fn an_abandoned_presence_and_an_abandoned_tail_absorb_what_meets_them() {
    // `nope` is a name lowering complained about, so its type says nothing and
    // the application abandons the whole type the annotation wrote — the `?`
    // and the core its `..` lowered to. The recursive call then puts that type
    // against a struct with a field it does not name, and the abandoned core
    // takes the whole of it. One mistake, and inference adds nothing to it.
    let (mint, out, output) = infer_src("let f : { x?: Nat, .. } -> Nat = fn p => f { y: nope p }");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.contains(&"recover  ?1 ~ ? => ?1 := ?".to_string()),
        "the core the `..` lowered to is abandoned: {taken:#?}"
    );
    assert!(
        taken.contains(&"recover  ?0 ~ ? => ?0 := ?".to_string()),
        "and the `?` beside it: {taken:#?}"
    );
    assert!(
        taken.contains(&"absorb  { x?: Nat, .. } ~ { y: ?3 } => no change".to_string()),
        "an abandoned core takes whatever meets it: {taken:#?}"
    );

    // A sum's tail is still a tail, and an abandoned one still absorbs where it
    // stands: the annotation's `..r` is thrown away by the same bad
    // application, and the closed sum it is then asked to be meets it there.
    let (mint, out, output) = infer_src(
        "let g : Nat -> (`A Nat) -> Nat = fn a => fn b => 1\n\
         let f : (`A Nat | ..r) -> Nat = fn p => g (nope p) p",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.iter().any(|step| step.contains("absorb  ∅ ~ ?")),
        "{taken:#?}"
    );

    // And with the undecided side on the right, which is the same rule read
    // from the other end: here the closed struct is what the annotation is
    // asked to be, so `{}` is the expected side and the abandoned core the
    // actual.
    let (mint, out, output) = infer_src(
        "let g : Nat -> { } -> Nat = fn a => fn b => 1\n\
         let f : { x?: Nat, .. } -> Nat = fn p => g (nope p) p",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.contains(&"absorb  {} ~ { x?: Nat, .. } => no change".to_string()),
        "{taken:#?}"
    );
}

/// Two presences that are one variable are already the same thing, and saying
/// so is [`Rule::Same`] rather than a binding of a variable to itself — which
/// is the presence sort's version of what a tail and a type each get.
#[test]
fn a_presence_against_itself_is_already_the_same_presence() {
    // A recursive call is monomorphic, so the parameter's row meets itself:
    // one `?` on both sides, and one tail on both sides.
    let (mint, _, output) = inferred("let f : { x?: Nat, .. } -> Nat = fn p => f p");
    assert_eq!(scheme(&mint, &output, "f"), "{ x?: Nat, ..\'a } -> Nat");
    assert_eq!(
        steps(&mint, &output, "f"),
        [
            "struct  { x?: Nat, ..?1 } ~ { x?: Nat, ..?1 } => replaced by the goals below",
            "  same  ?1 ~ ?1 => no change",
            "  same  ?0 ~ ?0 => no change",
            "  prim  Nat ~ Nat => no change",
            "prim  Nat ~ Nat => no change",
        ]
    );
}

/// Two presences already decided the same way agree without a word about the
/// label, because there is no label in sight to word it about: a clash of the
/// constants is intercepted where the name is known, so what is left here is a
/// pair that already agrees.
#[test]
fn two_presences_that_already_agree_need_no_rule_of_their_own() {
    // `closed` settles the `?` away, and then `both` puts the row against
    // itself through a tail it shares — so the field settled absent is named on
    // both sides of one goal, absent on both.
    let (mint, _, output) = inferred(
        "let optional : { x?: Nat, y: Nat, .. } -> Nat = fn p => 1\n\
         let closed : { y: Nat } -> Nat = fn p => 1\n\
         let both : { y: Nat, ..s } -> { y: Nat, ..s } -> Nat = fn p => fn q => 1\n\
         let f = fn r => { a: optional r, b: closed r, c: both r r }",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ y: Nat } -> { a: Nat, b: Nat, c: Nat }"
    );
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.contains(&"  same  absent ~ absent => no change".to_string()),
        "{taken:#?}"
    );
}

/// A declaration whose parameter is its whole rest names nothing beside it, so
/// there is no label an argument could repeat and nothing to forbid. The
/// condition is recorded only where there is one.
#[test]
fn a_rest_that_stands_for_everything_forbids_nothing() {
    let (mint, _, output) = inferred("type Only r = { ..r }\nlet v : Only { x: Nat } = { x: 1 }");
    assert_eq!(scheme(&mint, &output, "v"), "Only { x: Nat }");
}

/// Two applications of one recursive declaration are compared argument by
/// argument when the goal comes back round, and an argument is any type at
/// all: an arrow is compared as an arrow, side by side, rather than by whether
/// the two were written the same way.
#[test]
fn a_recursive_constructor_carrying_an_arrow_comes_back_round() {
    let (mint, _, output) = inferred(
        "type List a = { head: a, tail: List a }\n\
         type Seq a = { head: a, tail: Seq a }\n\
         let f : List (Nat -> Nat) -> Seq (Nat -> Nat) = fn x => x",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "List (Nat -> Nat) -> Seq (Nat -> Nat)"
    );
}

/// The demand a projection makes can land on either side of a goal, because
/// which side it lands on is decided by whoever emitted the constraint rather
/// than by the projection. Reading a field off a definition the annotation
/// says is a `Nat` puts the demand on the `actual` half — and it is still the
/// `Nat` that has no such field, said as the extra field it would need rather
/// than as the one it is missing, since it is the annotation that says what is
/// allowed.
#[test]
fn a_demand_on_the_actual_side_still_names_the_type_with_no_fields() {
    let (_, _, output) = infer_src("let g = fn p => p.x\nlet v : Nat -> Nat = g");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(error.kind.code(), "extra-field");
    assert_eq!(
        error.kind.to_string(),
        "extra field `x`: the type `Nat` lists every field it allows"
    );
}

/// An assumption is compared to the goal in front of it argument by argument,
/// and an argument still holding a variable is compared as that variable: two
/// `?` fields unified with each other are one question, so the goal that comes
/// back round is the goal already on the stack.
#[test]
fn an_assumption_matches_through_a_variable_the_two_sides_share() {
    let (mint, _, output) = inferred(
        "type Tree a = { value: a, kids: Nest a }\n\
         type Nest a = { head: Tree a, tail: Nest a }\n\
         type Wood a = { value: a, kids: Grove a }\n\
         type Grove a = { head: Wood a, tail: Grove a }\n\
         let peek : Tree { x?: Nat } -> Nat = fn t => 1\n\
         let take : Wood { x?: Nat } -> Nat = fn w => 2\n\
         let q = fn w => { a: peek w, b: take w }",
    );
    assert_eq!(
        scheme(&mint, &output, "q"),
        "Tree { x?: Nat } -> { a: Nat, b: Nat }"
    );
}

/// Recognizing a goal already open compares whatever an argument can be, and an
/// argument is any type: a variable the solve has not decided, a sum, a row
/// still open at its tail, and a row whose tail is not the other's. Each of
/// these programs reaches one pair of declarations twice, so the comparison
/// runs — and each is a program that types, so the comparison is right.
#[test]
fn an_assumption_compares_arguments_of_every_shape() {
    // An argument still holding a variable. `use` pins the two declarations to
    // one row, and the row picked up a field whose type nothing decides, so the
    // comparison reaches a variable against itself.
    let (mint, _, output) = inferred(
        "type A r = { next: B r, ..r }\n\
         type B r = { next: A r, ..r }\n\
         let use : { ..s } -> A { ..s } -> B { ..s } -> Nat = fn c => fn x => fn y => 1\n\
         let n = fn z => fn w => use { j: w } z z",
    );
    assert_eq!(scheme(&mint, &output, "n"), "A { j: \'a } -> \'a -> Nat");

    // An argument that is a sum, compared case by case.
    let (mint, _, output) = inferred(
        "type A a = { v: a, next: B a }\n\
         type B a = { v: a, next: A a }\n\
         let f : A (`X | `Y) -> Nat = fn p => 1\n\
         let g : B (`X | `Y) -> Nat = fn p => 1\n\
         let h = fn z => { a: f z, b: g z }",
    );
    assert_eq!(
        scheme(&mint, &output, "h"),
        "A (`X | `Y) -> { a: Nat, b: Nat }"
    );

    // Two rows still open, at the same tail: the same row, so the goal is the
    // one already on the stack.
    let (mint, _, output) = inferred(
        "type A r = { next: B r, ..r }\n\
         type B r = { next: A r, ..r }\n\
         let use : A { p: Nat, ..s } -> B { p: Nat, ..s } -> Nat = fn x => fn y => 1\n\
         let n = fn z => use z z",
    );
    assert_eq!(scheme(&mint, &output, "n"), "A { p: Nat, ..\'a } -> Nat");

    // And two rows whose tails are not each other, which is the answer that has
    // to be no: the declarations name themselves at a closed argument as well
    // as at the open one they were reached with, and assuming the one for the
    // other would answer a question nobody asked.
    let (mint, _, output) = inferred(
        "type A r = { next: B r, alt: B { p: Nat }, ..r }\n\
         type B r = { next: A r, alt: A { p: Nat }, ..r }\n\
         let use : A { p: Nat, ..s } -> B { p: Nat, ..s } -> Nat = fn x => fn y => 1\n\
         let n = fn z => use z z",
    );
    assert_eq!(scheme(&mint, &output, "n"), "A { p: Nat, ..\'a } -> Nat");
}

/// One declaration against itself is the same declaration however it was
/// applied, and the shortcut that says so has to stop short of applications
/// that were given something: `List Nat` against `List Nat` is decided by
/// comparing the arguments, not by the names agreeing.
#[test]
fn one_declaration_applied_to_something_is_not_settled_by_its_name() {
    let (mint, _, output) = inferred(
        "type List a = { head: a, tail: List a }\n\
         let f : List Nat -> Nat = fn x => 1\n\
         let g : List Nat -> Nat = fn x => f x",
    );
    assert_eq!(scheme(&mint, &output, "g"), "List Nat -> Nat");
}

/// Two rows sharing one tail cannot differ in fields — but two that share a
/// tail and name the same fields are simply the same row, and refusing that
/// pair would refuse a function applied twice to one argument.
#[test]
fn two_rows_sharing_a_tail_and_naming_the_same_fields_agree() {
    let (mint, _, output) = inferred(
        "let f : { x: Nat, ..r } -> { x: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let g = fn p => f p p",
    );
    assert_eq!(scheme(&mint, &output, "g"), "{ x: Nat, ..\'a } -> Nat");

    // One more field on one side and the pair is the cycle again: whatever the
    // shared tail absorbed from one row it would grow on the other.
    for src in [
        "let f : { x: Nat, ..r } -> { x: Nat, y: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let g = fn p => f p p",
        // Whichever side is the longer one: the extras are the same cycle from
        // either end.
        "let f : { x: Nat, y: Nat, ..r } -> { x: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let g = fn p => f p p",
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected exactly one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(error.kind.code(), "recursive-type", "{src}");
    }
}

/// An assumption is only about the goal it was pushed for. Reaching the same
/// two declarations at a different argument is a different question, and the
/// comparison that decides so has to look at whatever an argument can be: a
/// row with other labels, a row with more of them, a row of the other shape, an
/// arrow whose halves differ, and a `?` nothing has decided yet.
///
/// Each of these programs reaches `Tree ? ~ Tree2 ?` twice with the two
/// arguments apart, so the second question is asked rather than answered by
/// the first — and every one of them is true, so the answer is the same either
/// way. What differs is only how much work it took, which is exactly what an
/// assumption is for.
#[test]
fn an_assumption_is_not_reused_at_an_argument_it_was_not_pushed_for() {
    for src in [
        // Other labels, and more of them.
        "type Bush = { t: Tree { b: Nat } }\n\
         type Bush2 = { t: Tree2 { b: Nat } }\n\
         let f : Tree { a: Nat } -> Tree2 { a: Nat } = fn x => x",
        "type Bush = { t: Tree { a: Nat, b: Nat } }\n\
         type Bush2 = { t: Tree2 { a: Nat, b: Nat } }\n\
         let f : Tree { a: Nat } -> Tree2 { a: Nat } = fn x => x",
        // The other shape: a sum and a struct are two types however much their
        // insides look alike.
        "type Bush = { t: Tree { b: Nat } }\n\
         type Bush2 = { t: Tree2 { b: Nat } }\n\
         let f : Tree (`a Nat) -> Tree2 (`a Nat) = fn x => x",
        // An arrow, compared side by side rather than whole.
        "type Bush = { t: Tree (Nat -> Nat) }\n\
         type Bush2 = { t: Tree2 (Nat -> Nat) }\n\
         let f : Tree ({} -> Nat) -> Tree2 ({} -> Nat) = fn x => x",
    ] {
        let src = format!(
            "type Tree x = {{ v: x, k: Bush }}\n\
             type Tree2 x = {{ v: x, k: Bush2 }}\n{src}"
        );
        let (_, out, output) = infer_src(&src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
        assert!(output.errors.is_empty(), "{src}: {:#?}", output.errors);
    }

    // And a presence still open, which is the one part of an argument that can
    // be a variable while the goal is being asked. The definition is left
    // unannotated so that unifying the two `?`s is not the annotation being
    // narrowed.
    let (mint, _, output) = inferred(
        "type Tree x = { v: x, k: Bush }\n\
         type Bush = { t: Tree { a: Nat } }\n\
         type Tree2 x = { v: x, k: Bush2 }\n\
         type Bush2 = { t: Tree2 { a: Nat } }\n\
         let g : Tree { a?: Nat } -> Nat = fn x => 1\n\
         let h : Tree2 { a?: Nat } -> Nat = fn x => 2\n\
         let f = fn y => { p: g y, q: h y }",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "Tree { a?: Nat } -> { p: Nat, q: Nat }"
    );
}

/// A definition may name itself. Nothing constrains the recursive call here, so
/// the argument and the result stay whatever the caller wants.
#[test]
fn a_definition_may_name_itself() {
    let (mint, _, output) = inferred("let loop = fn x => loop x");
    assert_eq!(scheme(&mint, &output, "loop"), "'a -> 'b");
}

/// And the use is monomorphic, which is what a recursive use inside its own
/// group means: passing a literal there pins the parameter for the definition
/// as a whole.
#[test]
fn a_recursive_use_is_monomorphic() {
    let (mint, _, output) = inferred("let self = fn x => self 1");
    assert_eq!(scheme(&mint, &output, "self"), "Nat -> 'a");
}

/// Two definitions can name each other, and both are typed at once.
#[test]
fn a_mutual_pair_is_typed_together() {
    let (mint, _, output) = inferred("let even = fn n => odd n  let odd = fn n => even n");
    assert_eq!(scheme(&mint, &output, "even"), "'a -> 'b");
    assert_eq!(scheme(&mint, &output, "odd"), "'a -> 'b");
}

/// The regression test for the whole design. Grouping is what keeps a
/// definition that is not recursive polymorphic: treating the file as one
/// mutually recursive block would make `id` monomorphic and refuse the second
/// use of it.
#[test]
fn a_definition_outside_a_group_stays_polymorphic() {
    let (mint, _, output) = inferred("let id = fn x => x  let a = id 1  let b = id { x: 1 }");
    assert_eq!(scheme(&mint, &output, "id"), "'a -> 'a");
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "{ x: Nat }");
}

/// And it is still polymorphic when it is written below the use, since the
/// group it is in is solved first however the file is laid out.
#[test]
fn a_forward_reference_instantiates_a_scheme() {
    let (mint, _, output) = inferred("let a = id 1  let b = id { x: 1 }  let id = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "'a -> 'a");
    assert_eq!(scheme(&mint, &output, "a"), "Nat");
    assert_eq!(scheme(&mint, &output, "b"), "{ x: Nat }");
}

/// An annotated definition is checked against its annotation, and so are its
/// recursive uses: the definition is put in scope as what it says it is, so a
/// use inside it is checked against that rather than against nothing.
#[test]
fn a_recursive_use_is_checked_against_the_annotation() {
    let (mint, _, output) = inferred("let count : Nat -> Nat = fn n => count n");
    assert_eq!(scheme(&mint, &output, "count"), "Nat -> Nat");

    let (_, _, output) = infer_src("let bad : Nat -> Nat = fn n => bad { x: n }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    let ErrorKind::Mismatch { expected, actual } = &output.errors[0].kind else {
        panic!("expected a mismatch, got {:#?}", output.errors[0].kind);
    };
    assert_eq!(expected.to_string(), "Nat");
    assert_eq!(actual.to_string(), "{ x: Nat }");
}

/// A definition naming itself through a shape reaches inference, which is where
/// a value that would have to contain itself is refused.
#[test]
fn a_definition_that_would_contain_itself_is_refused() {
    for src in ["let s = { a: s }", "let f = fn x => f x x"] {
        let (_, out, output) = infer_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
        assert!(
            output
                .errors
                .iter()
                .any(|error| matches!(error.kind, ErrorKind::Recursive)),
            "{src}: {:#?}",
            output.errors
        );
    }
}

/// Two members of one group are solved together, and each still owns its own
/// complaints: the span is the member's own, and the types in it are spelled
/// the way that member's scheme spells them.
#[test]
fn a_complaint_belongs_to_the_member_that_made_it() {
    let (mint, out, output) = infer_src(
        "let one = fn x => two (fn p => ({ a: p }).missing)\n\
         let two = fn y => one (fn q => ({ b: q }).missing)",
    );
    assert_eq!(output.errors.len(), 2, "{:#?}", output.errors);

    // Each complaint lands inside the definition that made it.
    let span_of = |name| term_decl(&mint, &out, name).value.span;
    let (one, two) = (span_of("one"), span_of("two"));
    assert!(output.errors[0].span.start >= one.start && output.errors[0].span.end() <= one.end());
    assert!(output.errors[1].span.start >= two.start && output.errors[1].span.end() <= two.end());

    // And each is spelled in its own definition's letters. The struct named
    // holds a variable the solve never decided, so a complaint zonked against
    // some other member's substitution would name it `?7` — the solver's
    // bookkeeping, and a number the reader cannot count to.
    assert_eq!(
        output.errors[0].kind.to_string(),
        "no field `missing` on `{ a: 'c }`"
    );
    assert_eq!(
        output.errors[1].kind.to_string(),
        "no field `missing` on `{ b: 'c }`"
    );
    assert!(
        output
            .errors
            .iter()
            .all(|error| !names_a_solver_index(&error.kind.to_string())),
        "{:#?}",
        output.errors
    );
}

/// Both maps are keyed in source order however the groups were solved. The file
/// below is written back to front, so a map keyed in solve order would come out
/// reversed.
#[test]
fn the_schemes_are_keyed_in_source_order() {
    let (mint, out, output) = inferred("let a = b 1  let b = fn n => c n  let c = fn n => n");

    let source: Vec<&str> = out
        .program
        .terms
        .keys()
        .map(|symbol| mint.name(*symbol))
        .collect();
    assert_eq!(source, ["a", "b", "c"]);

    let keys = |map: Vec<Symbol>| -> Vec<&str> {
        map.into_iter().map(|symbol| mint.name(symbol)).collect()
    };
    assert_eq!(keys(output.schemes.keys().copied().collect()), source);
    assert_eq!(keys(output.constraints.keys().copied().collect()), source);

    // The solve ran the other way round, which is what makes the above a
    // statement rather than a coincidence.
    let mut solved: Vec<&str> = Vec::new();
    for step in &output.steps {
        let name = mint.name(step.definition);
        if solved.last() != Some(&name) {
            solved.push(name);
        }
    }
    assert_eq!(solved, ["c", "b", "a"]);
}

/// An annotation that leaves something open and gets it decided is still the
/// complaint it always was, recursive definition or not — and is still held
/// back when that definition already said something else.
#[test]
fn a_recursive_definition_may_still_promise_too_much() {
    let (_, _, output) = infer_src("let f : { x?: Nat } -> Nat = fn r => f { x: r.x }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(
        output.errors[0].kind,
        ErrorKind::AnnotationTooOpen
    ));

    // And not at all when the definition already failed: the annotation is not
    // the mistake to send the reader to.
    let (_, _, output) = infer_src("let f : { x?: Nat } -> Nat = fn r => f { x: r.y }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "missing-field");
}

/// Every scheme the change is meant to produce, pinned. Most of these are the
/// schemes the language already had: a type carrying no fields prints as its
/// core, so nothing wears a `with` unless a projection left a base unexplained.
#[test]
fn fields_on_every_type_change_only_the_schemes_a_projection_reaches() {
    let (mint, _, output) = inferred(
        "let id = fn x => x\n\
         let getx = fn p => p.x\n\
         let both = fn p => { a: p.x, b: p.y }\n\
         let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x\n\
         let n = getx { x: 1, y: 2 }\n\
         let u = {}\n\
         let opened : { small: Nat, ..extra } -> Nat = fn s => s.small\n",
    );
    // Unchanged: a bare variable still takes a whole type, fields and all.
    assert_eq!(scheme(&mint, &output, "id"), "'a -> 'a");
    // Changed: the base is polymorphic in its core rather than fixed to a
    // struct, which is the whole of what the change buys.
    assert_eq!(scheme(&mint, &output, "getx"), "{ x: 'a, ..'b } -> 'a");
    assert_eq!(
        scheme(&mint, &output, "both"),
        "{ x: 'a, y: 'b, ..'c } -> { a: 'a, b: 'b }"
    );
    // Unchanged: the annotation pins the core to unit, so nothing prints with
    // a `with`.
    assert_eq!(scheme(&mint, &output, "fst"), "{ x: Nat, y: Nat } -> Nat");
    // The core variable binds to unit, `x` matches, `y` flows into the tail.
    assert_eq!(scheme(&mint, &output, "n"), "Nat");
    // Unit is one type with one spelling.
    assert_eq!(scheme(&mint, &output, "u"), "{}");
    assert_eq!(
        scheme(&mint, &output, "opened"),
        "{ small: Nat, ..'a } -> Nat"
    );

    // A sum's own field row is empty and closed, so nothing about sums prints
    // with a `with` either — and a declaration taking a row is untouched.
    let (mint, _, output) = inferred(
        "type Option T = `Some T | `None\n\
         let some = `Some 1\n\
         type Tagged r = { tag: Nat, ..r }\n\
         let tag : Tagged { note: Nat } -> Nat = fn t => t.tag\n",
    );
    assert_eq!(scheme(&mint, &output, "some"), "`Some Nat | ..'a");
    assert_eq!(scheme(&mint, &output, "tag"), "Tagged { note: Nat } -> Nat");
}

/// Two projections on one binder are two demands about one base, so they share
/// the core variable as well as the row: `p` is one type, and both readings of
/// it have to be about the same one.
#[test]
fn two_projections_on_one_binder_share_a_core_variable() {
    let (mint, _, output) = inferred("let both = fn p => { a: p.x, b: p.y }");
    let scheme = scheme(&mint, &output, "both");
    assert_eq!(scheme, "{ x: 'a, y: 'b, ..'c } -> { a: 'a, b: 'b }");
    // One open end, not two: a second would print as its own `..`, and the
    // argument would read as two unrelated bases.
    assert_eq!(scheme.matches("..").count(), 1, "{scheme}");

    // The demand each projection makes is what generation asked for, and the
    // two name different variables *there* — sharing is what the solve
    // decides, not what generation writes down.
    let asked = constraints(&mint, &output, "both");
    assert_eq!(
        asked[0..2],
        [
            "equal: { x: ?2, ..?3 } ~ ?1".to_string(),
            "equal: { y: ?4, ..?5 } ~ ?1".to_string(),
        ]
    );
}

/// Generalization numbers variables in plain first-occurrence order, reading a
/// type's labels and then the core beneath them — which is the order they print
/// in, since the core is what a `..` spells and a tail is written last. No
/// special case for a core: the field's type of an accessor is `'a` because it
/// is the first variable a reader meets, and the base's core is `'b` because it
/// is the second.
#[test]
fn variables_are_numbered_in_the_order_they_are_read() {
    let (mint, _, output) = inferred("let getx = fn p => p.x");
    assert_eq!(scheme(&mint, &output, "getx"), "{ x: 'a, ..'b } -> 'a");

    // Presences keep their second pass, so a `?` never takes a letter another
    // variable would have had: `f`'s one letter is its core, and `g`'s are the
    // field's type and then the core it sits on.
    let (mint, _, output) = inferred(
        "let f : { x?: Nat, .. } -> Nat = fn p => 1\nlet g = fn p => { got: p.y, more: p }",
    );
    assert_eq!(scheme(&mint, &output, "f"), "{ x?: Nat, ..'a } -> Nat");
    assert_eq!(
        scheme(&mint, &output, "g"),
        "{ y: 'a, ..'b } -> { got: 'a, more: { y: 'a, ..'b } }"
    );
}

/// A projection demands a fresh core carrying the field, and that core is
/// forbidden from naming it: the core stands for a type the field is already on,
/// so a second copy could disagree with the first. One variable says what two
/// used to, and one condition sits on it.
///
/// The refusal below is the core's, and it is now the only one a struct has:
/// there is no tail variable left to put the condition on, so `Table::repeated`
/// finds the label by reading the fields of whatever the core is being bound to.
/// That is what makes `let bad : { ..r } -> { x: Nat, ..r }` a complaint about
/// the annotation's own core rather than about a tail nobody wrote.
#[test]
fn a_projection_demands_a_core_that_may_not_name_the_field() {
    let (mint, _, output) = inferred("let getx = fn p => p.x");
    // `?1` is `p`, `?2` the field's type and `?3` the demanded core, which is
    // the whole of what the base may also have. There is no fourth.
    assert_eq!(
        constraints(&mint, &output, "getx"),
        ["equal: { x: ?2, ..?3 } ~ ?1", "equal: ?0 ~ ?1 -> ?2"]
    );

    // The core may not come back naming the field, which is the condition a
    // projection has always imposed: the accessor's own open end is forbidden
    // the field it reads, so a scheme whose `..` is asked to cover it again is
    // refused. `?3` above is the variable refused here — the annotation's `..r`
    // reaches it, and the binding that would give it an `x` is a whole type
    // pushed into a core rather than a row pushed into a tail.
    let (_, _, output) = infer_src(
        "let getx = fn p => p.x\n\
         let bad : { ..r } -> { x: Nat, ..r } = fn q => { x: getx q }",
    );
    assert!(
        output
            .errors
            .iter()
            .any(|error| error.kind.code() == "repeated-field"),
        "{:#?}",
        output.errors
    );
}

/// The occurs check reaches a core variable inside the fields beside it. A
/// base that would have to carry itself is the row's version of `fn x => x x`,
/// and it is refused for the same reason.
#[test]
fn the_occurs_check_reaches_through_a_core_variable() {
    let (_, _, output) = infer_src("let bad = fn p => p.x p");
    assert!(
        output
            .errors
            .iter()
            .any(|error| error.kind.code() == "recursive-type"),
        "{:#?}",
        output.errors
    );
}

/// Every error the change touches, with the span it is reported at. The
/// projection complaints all underline the field name now — there is one way a
/// projection can be wrong, so there is one place to say it.
#[test]
fn every_projection_complaint_underlines_the_field_it_read() {
    for (src, message) in [
        ("let a = 1.x", "no field `x` on `Nat`"),
        ("let a = (fn x => x).y", "no field `y` on `'a -> 'a`"),
        ("let a = (`A 1).x", "no field `x` on ``A Nat | ..'a`"),
        ("let a = { y: 1 }.x", "no field `x` on `{ y: Nat }`"),
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(error.kind.to_string(), message, "{src}");
        assert_eq!(error.kind.code(), "missing-field", "{src}");
        let dot = src.rfind('.').expect("the projection");
        assert_eq!(error.span.start, dot + 1, "{src}");
    }

    // And the complaints a projection is not involved in are exactly as they
    // were, including the core mismatch that names two whole types.
    for (src, code, message) in [
        (
            "let a : { x: Nat } = { y: 1 }",
            "missing-field",
            "no field `x` on `{ y: Nat }`",
        ),
        (
            "let a : Nat = { x: 1 }",
            "type-mismatch",
            "type mismatch: expected `Nat`, found `{ x: Nat }`",
        ),
        (
            "let a = fn x => x x",
            "recursive-type",
            "this type would have to contain itself",
        ),
    ] {
        let (_, _, output) = infer_src(src);
        let found = output
            .errors
            .iter()
            .find(|error| error.kind.code() == code)
            .unwrap_or_else(|| panic!("no {code} for {src}: {:#?}", output.errors));
        assert_eq!(found.kind.to_string(), message, "{src}");
    }
}

/// A row variable and a presence variable are abandoned the way a type
/// variable is: pointed at the nothing of their own sort, each as a step, so a
/// reader replaying the solve is never shown a variable acquiring a value no
/// rule gave it.
#[test]
fn a_row_and_a_presence_are_abandoned_in_their_own_sorts() {
    // The projection's field row fails against a `Nat`, which abandons the
    // field's type and closes the tail — and the presence that the annotation
    // left open is settled in the same decomposition.
    let (mint, _, output) = infer_src("let bad : { y?: Nat } = { y: 1 }.x");
    let recovered: Vec<String> = output
        .steps
        .iter()
        .filter(|step| matches!(step.rule, Rule::Recover))
        .map(|step| step.goal.to_string())
        .collect();
    assert!(!recovered.is_empty(), "{:#?}", output.steps);
    let _ = mint;

    // A presence variable the solve never decided, abandoned by the failure
    // beside it.
    let (_, _, output) = infer_src("let bad : { y?: Nat, .. } -> Nat = fn p => p.y.q");
    let sorts: Vec<String> = output
        .steps
        .iter()
        .filter_map(|step| match &step.effect {
            Effect::Bound { value, .. } => Some(value.to_string()),
            _ => None,
        })
        .collect();
    assert!(sorts.iter().any(|value| value == "?"), "{sorts:?}");
}

/// Lowering a written type builds the new shapes: a struct is unit carrying
/// the fields that were written, and a sum is a sum core carrying its cases
/// with a field row of its own that is empty and closed.
#[test]
fn a_written_struct_lowers_to_unit_and_a_written_sum_to_a_sum_core() {
    let (mint, _, output) = inferred(
        "type Rec = { x: Nat }\n\
         type Sum = `A Nat | `B\n",
    );
    let rec = symbol_named(&mint, output.aliases.keys().copied(), "Rec");
    let body = output.aliases[&rec].body();
    assert!(matches!(body.core, Core::Unit), "{body:?}");
    assert_eq!(body.fields.len(), 1);

    let sum = symbol_named(&mint, output.aliases.keys().copied(), "Sum");
    let body = output.aliases[&sum].body();
    let Core::Sum(cases) = &body.core else {
        panic!("a written sum lowers to a sum core: {body:?}");
    };
    assert_eq!(cases.labels.len(), 2);
    // The sum carries no fields of its own, which is what keeps it printing as
    // its cases alone.
    assert!(body.fields.is_empty(), "{body:?}");
}

/// Deciding the labels of two types is the row rule it has always been, with
/// the tail read off the core. One test per row of it.
#[test]
fn the_labels_of_two_types_are_decided_by_what_their_cores_allow() {
    // Two different core variables, extras only one way: the variable takes the
    // other side's core with the labels only that side named.
    let (mint, _, output) = inferred(
        "let one : { x: Nat, ..r } -> Nat = fn p => 1\n\
         let two : { x: Nat, y: Nat, ..s } -> Nat = fn p => 1\n\
         let both = fn v => { a: one v, b: two v }",
    );
    assert_eq!(
        scheme(&mint, &output, "both"),
        "{ x: Nat, y: Nat, ..'a } -> { a: Nat, b: Nat }"
    );

    // Extras both ways: the two continue as one fresh core, so what comes out
    // names every field either side did.
    let (mint, _, output) = inferred(
        "let one : { x: Nat, ..r } -> Nat = fn p => 1\n\
         let two : { y: Nat, ..s } -> Nat = fn p => 1\n\
         let both = fn v => { a: one v, b: two v }",
    );
    assert_eq!(
        scheme(&mint, &output, "both"),
        "{ x: Nat, y: Nat, ..'a } -> { a: Nat, b: Nat }"
    );

    // A variable against a core that allows nothing more: every label only the
    // variable's side names must be absent, and the variable takes the closed
    // core with the labels only *it* named.
    let (mint, _, output) = inferred(
        "let one : { x: Nat, ..r } -> Nat = fn p => 1\n\
         let two : { x: Nat, y: Nat } -> Nat = fn p => 1\n\
         let both = fn v => { a: one v, b: two v }",
    );
    assert_eq!(
        scheme(&mint, &output, "both"),
        "{ x: Nat, y: Nat } -> { a: Nat, b: Nat }"
    );

    // Two cores that both allow nothing more: every label only one side names
    // must be absent, by side, and then the cores decide each other.
    let (_, _, output) = infer_src("let a : { x: Nat } = { x: 1, y: 2 }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "extra-field");

    // One variable on both sides, with extras either way: no finite type fits,
    // so it is the occurs check rather than a binding that grows forever.
    let (_, _, output) = infer_src(
        "let g : { x: Nat, ..r } -> { y: Nat, ..r } -> Nat = fn a => fn b => 1\n\
         let h = fn c => g c c",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "recursive-type");

    // And the same variable with no extras either way is already the same
    // thing, which is what a recursive call asks.
    let (mint, _, output) = inferred("let f : { x: Nat, ..r } -> Nat = fn p => f p");
    assert_eq!(scheme(&mint, &output, "f"), "{ x: Nat, ..'a } -> Nat");
}

/// A struct's `..` handed a type that is not a struct is a type inference can
/// build and no source syntax can write, and the solve shows it: the name is
/// unfolded where the labels beside it have to be decided, and what it stands
/// for is the argument carrying the declaration's own fields.
#[test]
fn a_struct_tail_handed_a_known_type_unfolds_to_a_with() {
    let (mint, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         let f : WithX Nat -> Nat = fn p => p.x",
    );
    // The scheme reads as the name the reader wrote.
    assert_eq!(scheme(&mint, &output, "f"), "WithX Nat -> Nat");
    // And the solve shows what that name stands for, once, where the
    // projection's demand had to be decided against it.
    let taken = steps(&mint, &output, "f");
    assert!(
        taken
            .iter()
            .any(|step| step.contains("Nat with { x: Nat }")),
        "{taken:#?}"
    );

    // A declared type carrying no fields of its own is the ordinary case and
    // wears no `with` at all.
    let (mint, _, output) = inferred(
        "type WithX r = { x: Nat, ..r }\n\
         type Foo = { y: Nat }\n\
         let g : WithX Foo -> Nat = fn p => p.y\n\
         let a : WithX { y: Nat } = { x: 1, y: 2 }\n\
         let n = (fn p => p.x) a",
    );
    assert_eq!(scheme(&mint, &output, "g"), "WithX Foo -> Nat");
    assert_eq!(scheme(&mint, &output, "a"), "WithX { y: Nat }");
    assert_eq!(scheme(&mint, &output, "n"), "Nat");

    // A parameter read as a field's type and as the `..` is one reading twice,
    // so the declaration means what both said and unfolds to it.
    let (mint, _, output) = inferred(
        "type W r = { f: r, ..r }\n\
         let w : W { y: Nat } = { f: { y: 3 }, y: 4 }",
    );
    assert_eq!(scheme(&mint, &output, "w"), "W { y: Nat }");
}

/// A sum's tail is still a tail, so every act about one is still an act about a
/// row: taking the other side's, being already the same variable, and absorbing
/// a set of cases where the tail was abandoned.
#[test]
fn a_sums_tail_is_decided_as_a_row() {
    // Two open sums naming the same cases: one tail takes the other.
    let (mint, _, output) = inferred(
        "let f : (`A Nat | ..r) -> Nat = fn p => 1\n\
         let g : (`A Nat | ..s) -> Nat = fn p => 1\n\
         let h = fn v => { one: f v, two: g v }",
    );
    assert_eq!(
        scheme(&mint, &output, "h"),
        "`A Nat | ..'a -> { one: Nat, two: Nat }"
    );

    // A recursive call meets its own tail, which is already the same thing.
    let (mint, _, output) = inferred("let f : (`A Nat | ..r) -> Nat = fn p => f p");
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.contains(&"  same  ?0 ~ ?0 => no change".to_string()),
        "{taken:#?}"
    );

    // An abandoned tail on the expected side absorbs what it is asked to be.
    let (mint, out, output) = infer_src(
        "let mk : Nat -> (`A Nat) = fn n => `A n\n\
         let f : (`A Nat | ..r) -> Nat = fn p => f (mk (nope p))",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken.iter().any(|step| step.contains("absorb  ? ~ ∅")),
        "{taken:#?}"
    );

    // And an abandoned tail takes a case only the other side names, then
    // abandons it in turn rather than deciding anything with it.
    let (mint, out, output) = infer_src(
        "let g : Nat -> (`A Nat | `B) -> Nat = fn a => fn b => 1\n\
         let f : (`A Nat | ..r) -> Nat = fn p => g (nope p) p",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken
            .iter()
            .any(|step| step.contains("absorb  { B: {} } ~ ?")),
        "{taken:#?}"
    );
}

/// A `?` the solve abandoned absorbs whatever a later goal puts it against,
/// from either side, and abandons it in turn.
#[test]
fn an_abandoned_presence_absorbs_from_either_side() {
    let (mint, out, output) = infer_src(
        "let mk : Nat -> (`A Nat) = fn n => `A n\n\
         let f : (`A? Nat | ..r) -> Nat = fn p => f (mk (nope p))",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken
            .iter()
            .any(|step| step.contains("absorb  ? ~ present")),
        "{taken:#?}"
    );

    let (mint, out, output) = infer_src(
        "let g : Nat -> (`A Nat) -> Nat = fn a => fn b => 1\n\
         let f : (`A? Nat | ..r) -> Nat = fn p => g (nope p) p",
    );
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let taken = steps(&mint, &output, "f");
    assert!(
        taken
            .iter()
            .any(|step| step.contains("absorb  present ~ ?")),
        "{taken:#?}"
    );
}

/// A sum's `..` in an annotation is a variable this definition may decide, and
/// deciding it is the annotation promising more than the definition keeps.
#[test]
fn a_sums_open_tail_is_the_definitions_to_leave_alone() {
    let (mint, _, output) = inferred("let f : (`A Nat | ..) -> Nat = fn p => 1");
    assert_eq!(scheme(&mint, &output, "f"), "`A Nat | ..'a -> Nat");

    // Narrowed, and told about: the tail the annotation left open came back
    // naming a case, so the contract it looked like was never checked.
    let (_, _, output) = infer_src("let f : (`A Nat | ..r) -> Nat = fn p => f (`B 1)");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "annotation-too-open");

    // And narrowed the other way: the tail came back allowing nothing more,
    // which names no case at all and is still not what the annotation said.
    let (_, _, output) = infer_src(
        "let g : (`A Nat) -> Nat = fn p => 1\n\
         let f : (`A Nat | ..r) -> Nat = fn p => g p",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "annotation-too-open");
}

/// What a sum's row parameter may not name travels to whatever is written at
/// it, including a row that is itself left open: the tail of the argument ends
/// up where the declaration's own tail sat, so it inherits the condition.
#[test]
fn a_sum_argument_carries_the_declarations_condition_into_its_own_tail() {
    let (_, out, output) = infer_src(
        "type Or r = `A | ..r\n\
         let f : Or (`B Nat | ..s) -> Nat = fn p => 1",
    );
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);

    // And the condition bites where the argument's own tail would have to take
    // the case the declaration already names.
    let (_, _, output) = infer_src(
        "type Or r = `A | ..r\n\
         let mk : (`A) -> Nat = fn p => 1\n\
         let f : Or (`B Nat | ..s) -> Nat = fn p => f p",
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
}

/// An assumption is compared to the goal in front of it argument by argument,
/// and an argument that is a *sum* is compared as one: the cases it names, and
/// what it allows past them.
///
/// Every way that can answer is here. `other` hands on a sum naming a case the
/// open annotation does not, so the labels settle it. `head` hands on one whose
/// cases agree but whose tail does not — closed against a variable — so the
/// tails do. And `tail` hands the parameter straight on, so both halves agree
/// and the goal is assumed rather than asked again.
#[test]
fn an_assumption_compares_a_sum_argument_as_a_sum() {
    let (mint, _, output) = inferred(
        "type Tree a = { value: a, kids: Nest a }\n\
         type Nest a = { head: Tree (`A Nat), other: Tree (`C Nat), tail: Nest a }\n\
         type Wood a = { value: a, kids: Grove a }\n\
         type Grove a = { head: Wood (`A Nat), other: Wood (`C Nat), tail: Grove a }\n\
         let f : Tree (`A Nat | ..r) -> Nat = fn t => 1\n\
         let g : Wood (`A Nat | ..r) -> Nat = fn w => 1\n\
         let h = fn v => { one: f v, two: g v }",
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let taken = steps(&mint, &output, "h");
    assert!(
        taken.iter().any(|step| step.contains("assume")),
        "{taken:#?}"
    );
}

/// A nested binding's value is generalized once it is solved, and each use of
/// the name in the body instantiates a fresh copy — which is the whole point of
/// keeping [`TermKind::Let`] a node of its own rather than desugaring it into a
/// lambda applied to its value, where the two uses would have to agree.
#[test]
fn a_nested_let_is_generalized() {
    let (mint, _, output) = inferred("let pair = let id = fn x => x in { a: id 1, b: id {} }");
    assert_eq!(scheme(&mint, &output, "pair"), "{ a: Nat, b: {} }");
}

/// There is no value restriction: a binding whose value is not a function
/// generalizes like any other. This language has nothing mutable for the
/// restriction to protect.
#[test]
fn a_nested_let_generalizes_whatever_its_value_is() {
    let (mint, _, output) =
        inferred("let e = let box = { it: fn x => x } in { a: box.it 1, b: box.it {} }");
    assert_eq!(scheme(&mint, &output, "e"), "{ a: Nat, b: {} }");
}

/// Generalization quantifies only what the value is entitled to. A variable an
/// enclosing binder owns is not quantified even when it was first minted inside
/// the let, which is what levels decide and what a range of minted variables
/// could never have.
#[test]
fn a_nested_let_quantifies_only_what_it_owns() {
    // The field's variable is minted inside the let, by the projection, and
    // then dropped out of the let's range when `p`'s own variable is bound to a
    // type carrying it. So the argument and the result are one type.
    let (mint, _, output) = inferred("let projected = fn p => let q = p.x in q");
    assert_eq!(scheme(&mint, &output, "projected"), "{ x: 'a, ..'b } -> 'a");

    // The same, without the projection in the way.
    let (mint, _, output) = inferred("let held = fn p => let q = p in q");
    assert_eq!(scheme(&mint, &output, "held"), "'a -> 'a");

    // And both halves at once: `dup` generalizes, so it is used at two types,
    // while `p` — the lambda's — does not, so the two uses share it.
    let (mint, _, output) =
        inferred("let shared = fn p => let dup = fn x => x in { a: dup p, b: dup 1 }");
    assert_eq!(scheme(&mint, &output, "shared"), "'a -> { a: 'a, b: Nat }");
}

/// Inside its own value the name is bound monomorphically, the same rule a
/// binding group follows: a recursive use is the one type being decided rather
/// than a copy of a scheme that does not exist yet. So the binding is not
/// polymorphically recursive.
#[test]
fn a_nested_let_names_itself_monomorphically() {
    let (mint, _, output) = inferred("let looping = let f = fn n => f n in f");
    assert_eq!(scheme(&mint, &output, "looping"), "'a -> 'b");
}

/// Every shape of nesting and every position a `let` can be written in, by the
/// type it comes out as.
#[test]
fn nested_lets_nest_and_sit_wherever_an_expression_does() {
    for (src, name, printed) in [
        (
            "let chained = let one = 1 in let two = { v: one } in two",
            "chained",
            "{ v: Nat }",
        ),
        (
            "let inside = fn p => let x = p.x in { first: x, second: x }",
            "inside",
            "{ x: 'a, ..'b } -> { first: 'a, second: 'a }",
        ),
        (
            "let in_field = { v: let n = 1 in n }",
            "in_field",
            "{ v: Nat }",
        ),
        (
            "let id = fn x => x\nlet applied = id (let n = 1 in n)",
            "applied",
            "Nat",
        ),
        (
            "let n = 1\nlet shadowed = let n = { v: 2 } in n",
            "shadowed",
            "{ v: Nat }",
        ),
    ] {
        let (mint, _, output) = inferred(src);
        assert_eq!(scheme(&mint, &output, name), printed, "{src}");
    }
}

/// An annotated nested binding is checked the way an annotated definition is:
/// the annotation is the contract, the value is checked against it, and the
/// scheme published is the annotation's.
#[test]
fn an_annotated_nested_let_is_checked_against_its_annotation() {
    let (mint, _, output) = inferred("let annotated = let n : Nat = 1 in n");
    assert_eq!(scheme(&mint, &output, "annotated"), "Nat");

    // A value that does not match is refused at the value, which is the term
    // the reader can change.
    let src = "let e = let n : Nat = {} in n";
    let (_, _, output) = infer_src(src);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(
        output.errors[0].kind.to_string(),
        "type mismatch: expected `Nat`, found `{}`"
    );
    assert_eq!(output.errors[0].span.start, src.find("{}").expect("value"));

    // And the annotation is what the name means inside its own value, so a
    // recursive use is checked against it.
    let (mint, _, output) = inferred("let e = let f : Nat -> Nat = fn n => f n in f 1");
    assert_eq!(scheme(&mint, &output, "e"), "Nat");
}

/// A nested annotation is held to the same promise a definition's is: what it
/// left open with a `..` or a `?` the value may not decide. Reported once, at
/// the annotation, and held back when the binding already complained.
#[test]
fn a_nested_annotation_may_not_be_narrowed_by_its_value() {
    let src = "let e = let n : { a?: Nat } = { a: 1 } in n";
    let (_, _, output) = infer_src(src);
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(
        output.errors[0].kind,
        ErrorKind::AnnotationTooOpen
    ));
    assert_eq!(
        output.errors[0].span.start,
        src.find("{ a?: Nat }").expect("the annotation")
    );

    // An annotation the value keeps open is no complaint.
    let (_, _, output) = infer_src("let e = let f : { x: Nat, .. } -> Nat = fn p => p.x in f");
    assert!(output.errors.is_empty(), "{:#?}", output.errors);

    // And nothing is said when the definition already failed: a second
    // complaint about the fallout of the first is one mistake said twice.
    let (_, _, output) = infer_src("let e = let n : { a?: Nat } = { a: 1 } in n.b");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(output.errors[0].kind.code(), "missing-field");
}

/// One scheme per nested binding, in the order the lets were walked, beside the
/// definitions rather than among them: `Output::schemes` keeps meaning the
/// top-level definitions and nothing else.
#[test]
fn every_nested_let_publishes_a_scheme() {
    let (mint, _, output) = inferred(
        "let a = let id = fn x => x in { one: id 1, two: id {} }\n\
         let b = fn p => let q = p.x in q\n",
    );
    let locals: Vec<(&str, String)> = output
        .locals
        .iter()
        .map(|(symbol, scheme)| (mint.name(*symbol), scheme.to_string()))
        .collect();
    assert_eq!(
        locals,
        [("id", "'a -> 'a".to_string()), ("q", "'a".to_string())]
    );

    // The definitions' own map is untouched by any of it.
    let named: Vec<&str> = output
        .schemes
        .keys()
        .map(|symbol| mint.name(*symbol))
        .collect();
    assert_eq!(named, ["a", "b"]);
}

/// Every term inside a nested `let` carries its resolved type when inference is
/// done, the way every other subterm does — and the `Let` node's own type is
/// its body's, which is what the expression evaluates to.
#[test]
fn every_term_inside_a_nested_let_is_typed() {
    let (mint, out, _) = inferred("let a = let n = 1 in { v: n }");
    assert_eq!(
        body_types(&term_decl(&mint, &out, "a").value),
        [
            // The `let` itself, which is its body's type.
            "{ v: Nat }",
            // Its value, then its body and the use inside it.
            "Nat",
            "{ v: Nat }",
            "Nat",
        ]
    );

    // And no solver variable survives anywhere in one, including the parts
    // generalization never numbered.
    let (_, out, _) = infer_src("let a = fn p => let q = fn z => z in { held: p, made: q }");
    for decl in out.program.terms.values() {
        for ty in body_tys(&decl.value) {
            assert!(!mentions_a_variable(&ty), "{ty}");
        }
    }
}

/// The scoping a nested `let` needs lives in the constraint language, so the
/// walk still reads nothing out of the variable table: what has to happen in
/// what order is written down as a constraint that carries its two lists.
#[test]
fn a_nested_let_is_said_as_two_constraint_kinds() {
    let (mint, _, output) = inferred("let a = let id = fn x => x in id 1");
    // One `let` carrying everything the expression asked for, and then the
    // definition's own equation tying it to what `a` is.
    let generated = constraints(&mint, &output, "a");
    assert_eq!(generated.len(), 2, "{generated:#?}");
    assert!(generated[0].starts_with("let: "), "{generated:#?}");
    assert!(generated[1].starts_with("equal: "), "{generated:#?}");

    // The two lists are the `let`'s own, in the order the solver runs them.
    let symbol = output
        .constraints
        .keys()
        .copied()
        .find(|symbol| mint.name(*symbol) == "a")
        .expect("the definition");
    let ConstraintKind::Let {
        level, value, body, ..
    } = &output.constraints[&symbol][0].kind
    else {
        panic!("expected a let constraint");
    };
    assert_eq!(*level, 1);
    assert!(
        value
            .iter()
            .all(|constraint| constraint.kind.code() == "equal"),
        "{value:#?}"
    );
    let codes: Vec<&str> = body
        .iter()
        .map(|constraint| constraint.kind.code())
        .collect();
    assert!(codes.contains(&"instance"), "{codes:?}");
}

/// A local's scheme may leave more than a bare core free. A sum's tail and a
/// field's presence are variables like any other, so one an enclosing binder
/// owns stays where it stands rather than being quantified — and is numbered
/// only when the scheme is published, past the quantifiers of its own.
#[test]
fn a_local_leaves_every_sort_of_outer_variable_alone() {
    for (src, printed) in [
        (
            "let f : (`A | ..r) -> Nat = fn s => let q = s in 1",
            "`A | ..'a",
        ),
        (
            "let f : { a?: Nat } -> Nat = fn r => let q = r in 1",
            "{ a?: Nat }",
        ),
    ] {
        let (mint, _, output) = inferred(src);
        let locals: Vec<(&str, String)> = output
            .locals
            .iter()
            .map(|(symbol, scheme)| (mint.name(*symbol), scheme.to_string()))
            .collect();
        assert_eq!(locals, [("q", printed.to_string())], "{src}");
    }
}

/// The row entry an explicitly absent label lowers to: `Presence::Absent`,
/// with its type deliberately unconstrained — a field that is not there has
/// nothing to have a type.
#[test]
fn a_written_absence_lowers_to_an_absent_presence() {
    let (mint, _, output) = inferred("let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x");
    let symbol = symbol_named(&mint, output.schemes.keys().copied(), "f");
    let body = output.schemes[&symbol].body().clone();
    let Core::Arrow(from, _) = &body.core else {
        panic!("expected an arrow, got {body:?}");
    };
    let field = &from.fields["y"];
    assert!(matches!(field.presence, Presence::Absent), "{field:?}");
    assert!(matches!(field.ty.core, Core::Undecided), "{field:?}");

    // The sum counterpart, through the row inside the core.
    let (mint, _, output) = inferred("let s : `Ok Nat | \\`Err | .. = `Ok 1");
    let symbol = symbol_named(&mint, output.schemes.keys().copied(), "s");
    let body = output.schemes[&symbol].body().clone();
    let Core::Sum(row) = &body.core else {
        panic!("expected a sum, got {body:?}");
    };
    let case = &row.labels["Err"];
    assert!(matches!(case.presence, Presence::Absent), "{case:?}");
    assert!(matches!(case.ty.core, Core::Undecided), "{case:?}");
}

/// Ascribing an explicitly absent field behaves as absence: an application
/// supplying the field is refused with the existing present-vs-absent
/// complaint, and one supplying anything else is welcome.
#[test]
fn an_ascribed_absence_refuses_the_field_and_nothing_else() {
    let (mint, _, output) =
        inferred("let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x\nlet good = f { x: 1, z: 2 }");
    assert_eq!(scheme(&mint, &output, "good"), "Nat");

    let (_, out, output) =
        infer_src("let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x\nlet bad = f { x: 1, y: 2 }");
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert_eq!(output.errors.len(), 1, "errors: {:#?}", output.errors);
    assert!(
        matches!(
            &output.errors[0].kind,
            ErrorKind::ExtraField {
                shape: Shape::Struct,
                field,
                ..
            } if field == "y"
        ),
        "{:#?}",
        output.errors
    );
}

/// The sum counterpart: a value of the case a declaration writes absent is
/// refused, and any other case the row parameter allows is welcome.
#[test]
fn an_ascribed_absence_refuses_the_case_and_nothing_else() {
    let (mint, _, output) =
        inferred("type NoErr r = `Ok Nat | \\`Err | ..r\nlet ok : NoErr (`Warn Nat) = `Ok 1");
    assert_eq!(scheme(&mint, &output, "ok"), "NoErr (`Warn Nat)");

    let (_, out, output) =
        infer_src("type NoErr r = `Ok Nat | \\`Err | ..r\nlet bad : NoErr (`Warn Nat) = `Err 1");
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert!(!output.errors.is_empty(), "the absent case was allowed");
    assert!(
        matches!(
            &output.errors[0].kind,
            ErrorKind::ExtraField {
                shape: Shape::Sum,
                field,
                ..
            } if field == "Err"
        ),
        "{:#?}",
        output.errors
    );
}

/// The shared-rest example: both `..r` are one rest, and it lacks `y`. The
/// definition types — the absence costs it nothing — and the condition
/// survives instantiation, so a use that would put a `y` into the rest is
/// refused.
#[test]
fn a_shared_rest_lacks_the_absent_label() {
    let (mint, _, output) = inferred("let g : { \\y, ..r } -> { ..r } = fn a => a");
    // The from side keeps the absent `y` in its label map, so it prints in
    // braces with the absence dropped; the result is the bare rest, which has
    // always printed as its core alone.
    assert_eq!(scheme(&mint, &output, "g"), "{ ..'a } -> 'a");

    let (_, out, output) =
        infer_src("let g : { \\y, ..r } -> { ..r } = fn a => a\nlet bad = fn s => (g s).y");
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert!(!output.errors.is_empty(), "the projection was allowed");
    assert!(
        matches!(
            &output.errors[0].kind,
            ErrorKind::RepeatedField { field, .. } if field == "y"
        ),
        "{:#?}",
        output.errors
    );
}

/// A match with no default closes the row: the listed cases are the whole
/// sum, and the annotation's own spelling unfolds to exactly that.
#[test]
fn a_match_without_a_default_closes_the_sum() {
    let (mint, _, output) = inferred(
        "type Option T = `Some T | `None\n\
         let get = fn opt => match opt with | `Some x => x | `None => 0 end",
    );
    assert_eq!(scheme(&mint, &output, "get"), "`Some Nat | `None -> Nat");
}

/// With a default the row stays open — the rest a fresh tail lacking the
/// listed cases — and the default's binder sees the sum with every handled
/// case ruled out.
#[test]
fn a_default_keeps_the_sum_open_and_is_typed_minus_the_handled_cases() {
    let (mint, _, output) = inferred(
        "let handle = fn r => 0\n\
         let first = fn v => match v with | `Some x => x | rest => handle rest end",
    );
    // The scrutinee allows `Some` and whatever else; the binder's sum has
    // `Some` absent — not part of what the type says, so it prints as the
    // bare rest — over the same tail.
    assert_eq!(scheme(&mint, &output, "first"), "`Some Nat | ..'a -> Nat");
    let (mint, _, output) =
        inferred("let rest_of = fn v => match v with | `Some x => x | rest => rest end");
    // Binding the result to both the payload and the leftover sum ties them
    // together: what `Some` carries is the sum without `Some`.
    assert_eq!(
        scheme(&mint, &output, "rest_of"),
        "`Some (| ..'a) | ..'a -> | ..'a"
    );
}

/// R7's nuance: a tag only partially handled — a refutable payload — reaches
/// the catch-all through a join point applied to the whole scrutinee, so the
/// binder's type keeps that case present rather than ruling it out.
#[test]
fn a_partially_handled_case_stays_present_for_the_catch_all() {
    let (mint, _, output) = inferred(
        "let keep = fn r => 0\n\
         let f = fn e => match e with | `A `X x => 1 | r => keep r end",
    );
    // `A` is still in the sum the binder sees: an `A`-tagged value whose
    // payload is not `X` reaches it.
    assert_eq!(
        scheme(&mint, &output, "f"),
        "`A (`X 'a | ..'b) | ..'c -> Nat"
    );
    let (mint, _, output) =
        inferred("let g = fn e => match e with | `A `X x => `Nothing | r => r end");
    // Handing the binder back out shows the same nuance from the result side:
    // the sum the match evaluates to still allows `A`.
    assert_eq!(
        scheme(&mint, &output, "g"),
        "`A (`X 'a | ..'b) | `Nothing | ..'c -> `Nothing | `A (`X 'a | ..'b) | ..'c"
    );
}

/// What a case carries flows to the arm's binder, nesting included, and a
/// bare tag arm constrains its payload to unit.
#[test]
fn payloads_flow_to_binders_and_a_bare_tag_means_unit() {
    let (mint, _, output) = inferred(
        "let pick = fn l => match l with \
         | `Cons { head: `Some x, tail: t } => x \
         | `Cons { head: `None, tail: t } => 0 \
         | `Nil => 0 \
         end",
    );
    assert_eq!(
        scheme(&mint, &output, "pick"),
        "`Cons { head: `Some Nat | `None, tail: 'a, ..'b } | `Nil -> Nat"
    );
    // The bare `` `None `` and `` `Nil `` both carry unit: supplying a
    // payload to one is the ordinary mismatch.
    let (_, _, output) = infer_src(
        "let f = fn v => match v with | `None => 0 | r => 1 end\n\
         let bad = f (`None 5)",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(output.errors[0].kind, ErrorKind::Mismatch { .. }));
}

/// Natural arms constrain the scrutinee — and the default's binder — to
/// `Nat`; the result is whatever the bodies agree on.
#[test]
fn a_natural_match_is_about_numbers() {
    let (mint, _, output) =
        inferred("let describe = fn n => match n with | 0 => `Zero | 1 => `One | k => `Many end");
    assert_eq!(
        scheme(&mint, &output, "describe"),
        "Nat -> `Zero | `One | `Many | ..'a"
    );
    let (mint, _, output) = inferred("let same = fn n => match n with | 0 => 1 | k => k end");
    assert_eq!(scheme(&mint, &output, "same"), "Nat -> Nat");
}

/// The empty match eliminates the empty sum: the scrutinee is `|`, and the
/// match's own type is anything at all.
#[test]
fn an_empty_match_eliminates_the_empty_sum() {
    let (mint, _, output) = inferred("let absurd = fn v => match v with end");
    assert_eq!(scheme(&mint, &output, "absurd"), "| -> 'a");
}

/// Every arm's body — the default's included — unifies with the match's own
/// type, so bodies that cannot agree are the mismatch, reported at the body.
#[test]
fn arm_bodies_unify_with_the_match() {
    let (mint, _, output) = inferred("let sole = fn v => match v with w => w end");
    assert_eq!(scheme(&mint, &output, "sole"), "'a -> 'a");

    let (_, _, output) = infer_src("let f = fn v => match v with | `A x => 0 | `B y => {} end");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(output.errors[0].kind, ErrorKind::Mismatch { .. }));
}

/// A case the arms do not list is refused at the use site that supplies it —
/// the ordinary mismatch against the closed sum, not a rule of the match's
/// own.
#[test]
fn supplying_an_unhandled_case_is_a_use_site_mismatch() {
    let (_, _, output) = infer_src(
        "let f = fn opt => match opt with | `Some x => x end\n\
         let bad = f `None",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    match &output.errors[0].kind {
        ErrorKind::ExtraField { shape, base, field } => {
            assert_eq!(*shape, Shape::Sum);
            assert_eq!(base.to_string(), "`Some 'a");
            assert_eq!(field, "None");
        }
        other => panic!("expected an extra case, got {other:?}"),
    }
}

/// A body two failure paths share is bound once as a join point, so a type
/// error inside it is reported exactly once.
#[test]
fn an_error_in_a_join_pointed_body_is_reported_once() {
    let (_, _, output) = infer_src(
        "let wants_nat : Nat -> Nat = fn n => n\n\
         let f = fn e => match e with | `A `X x => 1 | r => wants_nat {} end",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(output.errors[0].kind, ErrorKind::Mismatch { .. }));
}

/// A natural match with no default still types — the scrutinee and the arms
/// are numbers — even though lowering already complained that the numbers not
/// listed are unhandled; one mistake, one complaint, and no echo from here.
#[test]
fn a_natural_match_without_a_default_still_types() {
    let (mint, out, output) = infer_src("let f = fn n => match n with | 0 => 1 end");
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    assert_eq!(scheme(&mint, &output, "f"), "Nat -> Nat");
}

/// Three levels of nesting keep the written catch-all reachable: every inner
/// row stays open, so a case none of the inner arms list is the catch-all's
/// to take, not an extra case.
#[test]
fn a_deeply_nested_fallthrough_keeps_the_rows_open() {
    let (mint, _, output) = inferred(
        "let f = fn e => match e with | `A `B `C 0 => 1 | `A `B r => 2 | q => 3 end\n\
         let user = f (`A `Z)",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "`A (`B (`C Nat | ..'a) | ..'b) | ..'c -> Nat"
    );
}

/// The same through a struct payload: the leftover rows carry the enclosing
/// jump as a copy, so the tag above the struct stays partially handled and
/// open rather than closing.
#[test]
fn a_struct_fallthrough_keeps_the_enclosing_rows_open() {
    let (mint, _, output) = inferred(
        "let f = fn e => match e with | `P `A { x: `X } => 1 | `P `A w => 2 | q => 3 end\n\
         let user = f (`P `Z)",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "`P (`A { x: `X | ..'a, ..'b } | ..'c) | ..'d -> Nat"
    );
}

/// A struct row broken out of the merge — its column test overlaps an earlier
/// row that can still fail on another field — is live: the field it leaves
/// unconstrained stays open, so a case neither row lists is the use site's to
/// supply, not an extra case.
#[test]
fn a_broken_off_struct_row_keeps_its_field_open() {
    let (mint, _, output) = inferred(
        "let f = fn e => match e with | { a: `A, b: `X } => 1 | { a: `A, b: k } => 2 end\n\
         let u = f { a: `A, b: `Z }",
    );
    assert_eq!(scheme(&mint, &output, "u"), "Nat");

    // A type error in the broken-off fallthrough's body — bound once as the
    // join — is reported exactly once.
    let (_, _, output) = infer_src(
        "let wants_nat : Nat -> Nat = fn n => n\n\
         let f = fn e => match e with | { a: `A, b: 0 } => 1 | { a: `A, b: k } => wants_nat {} end",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(output.errors[0].kind, ErrorKind::Mismatch { .. }));
}

/// A run whose later row rides an earlier one — same column test, the
/// earlier row still able to fail on another field — stays one match over
/// the column: with no catch-all, the column's cases close over every tag
/// the run lists, and every arm is reachable. The round-3 regression, in
/// each of its three orderings.
#[test]
fn a_ridden_run_closes_the_column_over_every_listed_case() {
    let (mint, _, output) = inferred(
        "let f = fn e => match e with \
         | { a: `A, b: x } => 1 | { a: `B, b: 0 } => 2 | { a: `B, b: k } => 3 end",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ a: `A | `B, b: Nat, ..'a } -> Nat"
    );

    // Mirror ordering: the riding row lands past a different tag and the
    // column still closes over both.
    let (mint, _, output) = inferred(
        "let f = fn e => match e with \
         | { a: `A, b: 0 } => 1 | { a: `B, b: x } => 2 | { a: `A, b: k } => 3 end",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "{ a: `A | `B, b: Nat, ..'a } -> Nat"
    );

    // Nested in a tag's payload: the outer sum closes over the one tag, the
    // payload's column over both of its own.
    let (mint, _, output) = inferred(
        "let f = fn e => match e with \
         | `T { a: `A, b: x } => 1 | `T { a: `B, b: 0 } => 2 | `T { a: `B, b: k } => 3 end",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "`T { a: `A | `B, b: Nat, ..'a } -> Nat"
    );

    // The same three shapes with a trailing catch-all still build clean.
    for src in [
        "let f = fn e => match e with \
         | { a: `A, b: x } => 1 | { a: `B, b: 0 } => 2 | { a: `B, b: k } => 3 | w => 4 end",
        "let f = fn e => match e with \
         | { a: `A, b: 0 } => 1 | { a: `B, b: x } => 2 | { a: `A, b: k } => 3 | w => 4 end",
        "let f = fn e => match e with \
         | `T { a: `A, b: x } => 1 | `T { a: `B, b: 0 } => 2 | `T { a: `B, b: k } => 3 | w => 4 end",
    ] {
        let (_, out, output) = infer_src(src);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
        assert!(output.errors.is_empty(), "{src}: {:#?}", output.errors);
    }

    // A type error in a body shared across the riding row's failure paths is
    // reported exactly once: the row rides as a join point when the ridden
    // row fails more than one way.
    let (_, _, output) = infer_src(
        "let wants_nat : Nat -> Nat = fn n => n\n\
         let f = fn e => match e with \
         | { a: `A, b: `X `Y } => 1 | { a: `A, b: k } => wants_nat {} end",
    );
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert!(matches!(output.errors[0].kind, ErrorKind::Mismatch { .. }));
}

/// A sibling tag group after one whose nesting borrowed the fallthrough: the
/// sibling's inner level still falls through to the catch-all, so an unlisted
/// inner case type-checks instead of closing the row.
#[test]
fn a_sibling_group_still_falls_through_to_the_catch_all() {
    let (mint, _, output) = inferred(
        "let f = fn e => match e with \
         | `A `B `C 0 => 1 | `A `B r => 2 | `D `E 0 => 4 | q => 5 end\n\
         let user = f (`D `F)\n\
         let deep = f (`D (`E 7))",
    );
    assert_eq!(
        scheme(&mint, &output, "f"),
        "`A (`B (`C Nat | ..'a) | ..'b) | `D (`E Nat | ..'c) | ..'d -> Nat"
    );
}
