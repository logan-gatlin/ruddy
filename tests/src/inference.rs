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
    let inferred = inference::infer(&mut out.program);
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
        Ty::Struct(fields) => fields.values().any(|ty| mentions_a_variable(ty)),
        Ty::Nat | Ty::Bound(_) | Ty::Undecided => false,
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

    let (_, _, output) =
        infer_src("let f : { x: Nat } -> Nat = fn p => p.x\nlet y = f { x: 1, z: 2 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `{ x: Nat }`, found `{ x: Nat, z: Nat }`"
    );

    // The annotation path words its own mismatch the same way round, and did
    // before: fixing applications must not have got there by swapping both.
    let (_, _, output) = infer_src("let p : { x: Nat } = { x: 1, y: 2 }");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "type mismatch: expected `{ x: Nat }`, found `{ x: Nat, y: Nat }`"
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

    // The same for a definition abandoned by a projection rather than by a
    // mismatch: neither the base it never learned nor the result it never
    // produced may come back quantified.
    let (mint, _, output) = infer_src("let f = fn x => x.y\nlet g = f { y: 1 }");
    assert_eq!(output.errors.len(), 1, "{:#?}", output.errors);
    assert_eq!(scheme(&mint, &output, "f"), "? -> ?");
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
/// `Rule::Bind` above an effect reading "recursive type" tells whoever is
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
            "occurs  ?1 ~ ?1 -> ?2 => recursive type",
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
fn an_unannotated_projection_asks_for_an_annotation() {
    // Unification can only equate a variable with a type it is given, and
    // nothing here gives it one: `p.x` says the base has an `x`, not what the
    // base is. So this is an error — and a different one from projecting out
    // of a type that is not a struct, because there is no type to name and the
    // fix is an annotation rather than a different base.
    let (_, _, output) = infer_src("let f = fn p => p.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert!(
        matches!(error.kind, ErrorKind::UnknownBase),
        "expected an unknown base: {:#?}",
        error.kind
    );
    assert_eq!(
        error.kind.to_string(),
        "cannot infer the type being projected from; annotate it"
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
    let (_, _, output) = infer_src(&format!("let f = fn a b c => {{ p: a, q: b, r: c }}\n{src}"));
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

/// Which of the two projection complaints an error is, is settled where the
/// solver gives up — because giving up is itself what points the base at
/// `Ty::Undecided`. Deriving the wording from the payload afterwards read the
/// solver's own recovery as knowledge and told the reader about a type `?`
/// they never wrote.
#[test]
fn a_projection_complaint_keeps_the_wording_it_was_reported_with() {
    // Two stuck projections, and the first one recovered before the second was
    // reported: both are still the actionable complaint, at both spans.
    let src = "let r = fn x => (fn y => y.q) x.a";
    let (_, _, output) = infer_src(src);
    let wordings: Vec<String> = output
        .errors
        .iter()
        .map(|error| format!("{}: {}", error.span.start, error.kind))
        .collect();
    assert_eq!(
        wordings,
        [
            format!(
                "{}: cannot infer the type being projected from; annotate it",
                src.find("y.q").expect("the inner base")
            ),
            format!(
                "{}: cannot infer the type being projected from; annotate it",
                src.find("x.a").expect("the outer base")
            ),
        ]
    );

    // The other wording is for a base that really is a type, and it still says
    // which type.
    let (_, _, output) = infer_src("let n = 1\nlet bad = n.x");
    let [error] = output.errors.as_slice() else {
        panic!("expected exactly one error: {:#?}", output.errors);
    };
    assert_eq!(
        error.kind.to_string(),
        "cannot project a field out of `Nat`"
    );
}

/// A chain of projections off one unknown base is one complaint. Abandoning
/// the first link points the second link's base at `Ty::Undecided`, and every
/// link after that has nothing of its own to say: the type it would name is
/// one the solver invented while recovering.
#[test]
fn a_projection_chain_complains_once() {
    let (_, _, output) = infer_src("let r = fn x => x.a.b.c");
    let messages: Vec<String> = output
        .errors
        .iter()
        .map(|error| error.kind.to_string())
        .collect();
    assert_eq!(
        messages,
        ["cannot infer the type being projected from; annotate it"]
    );
}

#[test]
fn generation_records_what_it_asked_for_without_solving_it() {
    let (mint, _, output) = inferred("let fst : { x: Nat } -> Nat = fn p => p.x");
    // In the order the walk emitted them, and unsolved: the projection comes
    // before the annotation's demand on its result, and both still spell that
    // result as the variable generation minted for it. The solver runs the
    // other way round — every equality first, then the projections those
    // equalities made answerable — which is why the list is worth keeping.
    assert_eq!(
        constraints(&mint, &output, "fst"),
        ["field: { x: Nat }.x ~ ?0", "equal: Nat ~ ?0"]
    );

    // A definition can demand nothing at all, and still be one the pass ran
    // over: the entry is there, empty, rather than missing.
    let (mint, _, output) = inferred("let n = 1");
    assert!(constraints(&mint, &output, "n").is_empty());
}

#[test]
fn a_projection_waits_for_a_base_the_rest_of_the_definition_explains() {
    // Nothing has said what `p` is when `p.x` is walked — the argument that
    // settles it comes later. Generation cannot read a field out of a type it
    // does not have, so it emits the projection as a constraint and the solver
    // comes back to it once the equalities have bound the base.
    let (mint, _, output) = inferred("let h : Nat = (fn p => p.x) { x: 1 }");
    assert_eq!(scheme(&mint, &output, "h"), "Nat");
}

#[test]
fn a_projection_waits_for_a_base_another_projection_supplies() {
    // Two projections, the first of which cannot be solved until the second
    // has been: one pass over the deferred constraints is not enough, so the
    // solver keeps going until a round learns nothing new.
    let (mint, _, output) =
        inferred("let deep : Nat = (fn q => (fn p => p.x) q.inner) { inner: { x: 1 } }");
    assert_eq!(scheme(&mint, &output, "deep"), "Nat");
}

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
}

#[test]
fn aliases_unfold_to_their_meaning() {
    let (mint, _, output) =
        inferred("type Endo = Nat -> Nat\ntype Pair = { f: Endo }\nlet id : Endo = fn x => x");
    assert_eq!(scheme(&mint, &output, "id"), "Nat -> Nat");

    let pair = symbol_named(&mint, output.aliases.keys().copied(), "Pair");
    assert_eq!(output.aliases[&pair].to_string(), "{ f: Nat -> Nat }");
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
    // The annotation's demand is solved first, which is what gives the
    // projection a result to match; the projection then becomes an equality,
    // one level deeper, between the field's type and that result.
    assert_eq!(
        steps(&mint, &output, "fst"),
        [
            "bind  Nat ~ ?0 => ?0 := Nat",
            "project  { x: Nat }.x ~ Nat => broken into smaller goals",
            "  prim  Nat ~ Nat => no change",
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
fn a_deferred_projection_is_deferred_then_solved() {
    let (_, _, output) =
        inferred("let deep : Nat = (fn q => (fn p => p.x) q.inner) { inner: { x: 1 } }");
    let rules: Vec<&str> = output
        .steps
        .iter()
        .filter(|step| matches!(step.rule, Rule::Defer | Rule::Project))
        .map(|step| step.rule.code())
        .collect();
    // The inner projection waits a round, the outer one explains its base, and
    // only then does it go through: the fixpoint, seen from outside.
    assert_eq!(rules, ["defer", "project", "project"]);
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
    // Nothing ever says what `p` is, so the projection is deferred, gives up,
    // and points both the base it never learned and the result it cannot
    // produce at `?`. That last part changes the solution, so it has to be a
    // step: otherwise a reader watching the state would see `?1` acquire a
    // value that no rule they were shown gave it.
    let (mint, _, output) = infer_src("let f = fn p => p.x");
    assert_eq!(
        steps(&mint, &output, "f"),
        [
            "defer  ?0.x ~ ?1 => no change",
            "stuck  ?0.x ~ ?1 => cannot infer the type being projected from; annotate it",
            "recover  ?0 ~ ? => ?0 := ?",
            "recover  ?1 ~ ? => ?1 := ?",
        ]
    );
}

/// Two structs line up when they carry exactly the same field names, in
/// whatever order — and it is one rule, applied by both passes. Generation
/// decides it when it checks a literal against an annotation and the solver
/// decides it again when it unifies two struct types, and the two had spelled
/// it out separately: a pair of maps one arm would take apart field by field
/// while the other refused to equate them at all was a difference nothing
/// would have reported.
///
/// Exact field sets are the current choice rather than an oversight. The two
/// halves below are what would change if the language ever adopted width
/// subtyping, which is why they are pinned here as well as commented there.
#[test]
fn a_struct_matches_on_its_field_names_in_both_passes() {
    // Order is not part of a record's identity. The annotated definition is
    // decided by checking; the argument at the call site can only be equated,
    // so it is decided by unification.
    inferred("let p : { a: Nat, b: Nat } = { b: 2, a: 1 }");
    inferred("let f : { a: Nat, b: Nat } -> Nat = fn r => r.a\nlet y = f { b: 2, a: 1 }");

    // A field too many, and a field too few. Both are refused, by both passes,
    // and refused as one mismatch of whole types rather than as a complaint
    // per field.
    for (src, expected, actual) in [
        (
            "let p : { a: Nat } = { a: 1, b: 2 }",
            "{ a: Nat }",
            "{ a: Nat, b: Nat }",
        ),
        (
            "let f : { a: Nat } -> Nat = fn r => r.a\nlet y = f { a: 1, b: 2 }",
            "{ a: Nat }",
            "{ a: Nat, b: Nat }",
        ),
        (
            "let p : { a: Nat, b: Nat } = { a: 1 }",
            "{ a: Nat, b: Nat }",
            "{ a: Nat }",
        ),
        (
            "let f : { a: Nat, b: Nat } -> Nat = fn r => r.a\nlet y = f { a: 1 }",
            "{ a: Nat, b: Nat }",
            "{ a: Nat }",
        ),
    ] {
        let (_, _, output) = infer_src(src);
        let [error] = output.errors.as_slice() else {
            panic!("expected exactly one error for {src}: {:#?}", output.errors);
        };
        assert_eq!(
            error.kind.to_string(),
            format!("type mismatch: expected `{expected}`, found `{actual}`"),
            "{src}"
        );
    }
}

/// A projection whose base nothing has explained yet is set aside and retried,
/// and what is set aside is the projection — its two spans, its name and its
/// result — rather than a constraint every round has to re-establish is a field
/// one. Here `q.b` is emitted before the projection that decides what `q` is,
/// so the first round can only defer it and a later one is what reads it.
#[test]
fn a_projection_is_deferred_until_a_later_round_knows_its_base() {
    let (_, _, output) = inferred("let f : { a: { b: Nat } } -> Nat = fn p => (fn q => q.b) p.a");
    let rules: Vec<Rule> = output.steps.iter().map(|step| step.rule).collect();
    let deferred = rules
        .iter()
        .position(|rule| *rule == Rule::Defer)
        .unwrap_or_else(|| panic!("nothing was deferred: {rules:?}"));
    assert!(
        rules[deferred..].contains(&Rule::Project),
        "the deferred projection was never retried: {rules:?}"
    );
}
