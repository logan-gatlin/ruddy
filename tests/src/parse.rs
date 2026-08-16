//! Tests for [`ruddy::parse`].

use ruddy::{
    parse::{ErrorKind, Place, StmtKind, SumCase, Type, TypeField, TypeKind, parse},
    token::lex,
    tracking::FileID,
};
use ruddy_debug::print;

#[test]
fn parses_let_and_type() {
    // `with` can neither extend the preceding type nor start a statement,
    // so it is the sole parse error and recovery resumes at `let z`.
    let src = "let x = y  type T = U  with  let z = w";
    let toks = lex(src, FileID::GENERATED).tokens;
    let out = parse(toks);
    assert_eq!(out.stmts.len(), 3, "stmts: {:#?}", out.stmts);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(matches!(out.stmts[0].tracked, StmtKind::Let { .. }));
    assert!(matches!(out.stmts[1].tracked, StmtKind::Type { .. }));
    assert!(matches!(out.stmts[2].tracked, StmtKind::Let { .. }));
}

/// Parse a single statement and render it back. Since grouping is dropped
/// rather than recorded, the rendered source is re-parsed to confirm the
/// printer emits enough parentheses to reproduce the same tree.
fn parse_one(src: &str) -> String {
    let printed = parse_print(src);
    assert_eq!(
        parse_print(&printed),
        printed,
        "printing {src:?} did not round-trip"
    );
    printed
}

fn parse_print(src: &str) -> String {
    let toks = lex(src, FileID::GENERATED).tokens;
    let out = parse(toks);
    assert!(
        out.errors.is_empty(),
        "unexpected errors for {src:?}: {:#?}",
        out.errors
    );
    assert_eq!(out.stmts.len(), 1, "stmts: {:#?}", out.stmts);
    print::ast::stmt(&out.stmts[0].tracked).to_string()
}

#[test]
fn application_is_left_associative() {
    assert_eq!(parse_one("let a = f x y"), "let a = f x y");
}

#[test]
fn struct_types() {
    assert_eq!(
        parse_one("type Point = { x: Int, y: Int }"),
        "type Point = { x: Int, y: Int }"
    );
    // Trailing comma is allowed.
    assert_eq!(parse_one("type T = { a: A, }"), "type T = { a: A }");
}

#[test]
fn open_struct_types() {
    // The `..` tail, anonymous and named, and with or without fields before
    // it. The parser accepts the syntax anywhere a struct type can be
    // written; that a `type` declaration must be closed is lowering's rule.
    assert_eq!(
        parse_one("let x : { a: A, .. } = y"),
        "let x : { a: A, .. } = y"
    );
    assert_eq!(
        parse_one("let x : { a: A, ..'r } = y"),
        "let x : { a: A, ..'r } = y"
    );
    assert_eq!(parse_one("let x : { .. } = y"), "let x : { .. } = y");
    assert_eq!(parse_one("let x : { ..'r } = y"), "let x : { ..'r } = y");
}

/// A struct field writes its `when` clause bare between the label and the
/// colon: the colon is what ends the clause, so there is nothing for
/// parentheses to disambiguate.
#[test]
fn a_struct_field_may_name_its_presence() {
    for src in [
        "let x : { a when 'a: A } = y",
        "let x : { a when 'a: A, b: B, ..'r } = y",
        // `when _` is the anonymous presence: a variable this definition
        // decides that no formula can name.
        "let x : { a when _: A } = y",
        // Two labels of one name share one presence, which is how a type says
        // two fields are there together.
        "let x : { a when 'p: A, b when 'p: B } = y",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// One token of lookahead is the whole disambiguation: a field *called* `when`
/// has already spent its `when` on the name, so the next token is the colon.
#[test]
fn a_label_may_be_called_when() {
    assert_eq!(
        parse_one("let x : { when: A } = y"),
        "let x : { when: A } = y"
    );
    assert_eq!(
        parse_one("let x : { when when 'a: A } = y"),
        "let x : { when when 'a: A } = y"
    );
}

/// The contextual keywords are ordinary identifiers everywhere 'but the type
/// positions that read them, so terms and labels of those names still work.
#[test]
fn the_clause_keywords_are_contextual() {
    for src in [
        "let when = 1",
        "let where = 1",
        "let or = 1",
        "let and = 1",
        "let not = 1",
        "let x = fn when => when",
        "let x = { when: 1, where: 2, or: 3, and: 4, not: 5 }",
        "let x : { when: A, where: B, or: C, and: D, not: E } = y",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// A sum case has no colon to end a bare clause, so its `when` takes
/// parentheses — and two tokens of lookahead tell one from a parenthesized
/// payload, since only the clause has `when` inside it.
#[test]
fn a_sum_case_parenthesizes_its_presence() {
    for src in [
        "let x : #A (when 'a) Nat | #B = y",
        "let x : #A (when 'a) | #B (when 'b) Nat = y",
        "let x : #A (when _) Nat = y",
        // A parenthesized payload is still a payload: nothing has changed
        // about a case carrying a type that needed brackets.
        "let x : #A (Nat -> Nat) = y",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// A `when` promises a name or the discard, and a case's clause promises its
/// closing parenthesis; anything else is reported where it was written.
#[test]
fn a_when_clause_must_name_something() {
    for (src, at) in [
        ("let x : { a when: A } = y", ": A } = y"),
        ("let x : { a when 1: A } = y", "1: A } = y"),
        ("let x : #A (when 'a Nat = y", "Nat = y"),
        // Unparenthesized in a sum, the `when` is read as the payload type it
        // looks like and the variable after it is nobody's token.
        ("let x : #A when 'a = y", "'a = y"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert_eq!(
            out.errors[0].span.start,
            src.len() - at.len(),
            "{src:?}: {:#?}",
            out.errors
        );
    }
}

/// The `where` grammar, at every level and with the parentheses re-parsing
/// needs. Printing goes through the shared writer, so a clause that comes back
/// out of the printer and in again is the clause it started as.
#[test]
fn where_clauses_read_at_every_level() {
    for src in [
        "let x : { a when 'a: A } where 'a = y",
        "let x : { a when 'a: A } where not 'a = y",
        "let x : { a when 'a: A } where not not 'a = y",
        "let x : { a when 'a: A, b when 'b: B } where 'a and 'b = y",
        "let x : { a when 'a: A, b when 'b: B } where 'a or 'b = y",
        "let x : { a when 'a: A, b when 'b: B } where 'a = 'b = y",
        "let x : { a when 'a: A, b when 'b: B } where 'a != 'b = y",
        // `and` binds tighter than `or`, which binds tighter than the
        // comparison — so these need no parentheses at all.
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a or 'b and 'c = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a and 'b or 'c = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a = 'b or 'c = y",
        // And these do, because the grammar reads them the other way round
        // without them.
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where ('a or 'b) and 'c = y",
        "let x : { a when 'a: A, b when 'b: B } where not ('a and 'b) = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a and ('b or 'c) = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where ('a = 'b) or 'c = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a != ('b = 'c) = y",
        // A clause ends a whole written type, so it sits outside the arrow.
        "let x : { a when 'a: A } -> B where 'a = y",
        // And a declaration's body reads one too, for lowering to refuse.
        "type T = { a when 'a: A } where 'a",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// The comparison is non-associative: a chain has no reading, so it is refused
/// rather than folded one way or the other.
///
/// A definition's own `=` is the exception, and not the grammar bending: in
/// `where a != 'b = v` the `=` is what introduces the value, and the clause has
/// already ended. What tells the clause's `=` from that one is that the
/// clause's is followed by it.
#[test]
fn a_comparison_does_not_chain() {
    for src in [
        "type T = { a when 'a: A, b when 'b: B, c when 'c: C } where 'a = 'b = 'c",
        "type T = { a when 'a: A, b when 'b: B, c when 'c: C } where 'a != 'b != 'c",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where 'a != 'b != 'c = y",
        "let x : { a when 'a: A, b when 'b: B, c when 'c: C } where ('a = 'b = 'c) = y",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    }

    // A single comparison over a definition still reads, with the `=` after it
    // introducing the value.
    assert_eq!(
        parse_one("let x : { a when 'a: A, b when 'b: B } where 'a = 'b = y"),
        "let x : { a when 'a: A, b when 'b: B } where 'a = 'b = y"
    );
    // And so does a clause of one name, whose `=` is the definition's alone.
    assert_eq!(
        parse_one("let x : { a when 'a: A } where 'a = y"),
        "let x : { a when 'a: A } where 'a = y"
    );
}

/// A `where` clause is a `;`-separated list of constraints, conjoined in
/// written order. Nothing declares anything: a variable is introduced where it
/// is used, so what is left here is what a formula was always for.
#[test]
fn a_where_clause_is_a_statement_list() {
    for src in [
        // No clause at all, which is what an annotation with no presence to
        // constrain writes now.
        "let id : 'a -> 'a = fn x => x",
        "let h : 'a -> 'b = fn x => x",
        // One constraint, and several.
        "let f : { x when 'a: A } where 'a = z",
        "let f : { x when 'a: A, y when 'b: B } where 'a = 'b = z",
        "let f : { x when 'a: A, y when 'b: B } where 'a; 'b = z",
        "let f : { x when 'a: A, y when 'b: B, z when 'c: C } where 'a; 'b; 'c = z",
        // The last one still tells its `=` from the definition's.
        "let f : { x when 'a: A, y when 'b: B } where 'b; 'a = 'b = z",
        // And a declaration's body reads one too, for lowering to refuse.
        "type T = { x when 'a: A } where 'a",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// A `;` promises another statement, so a trailing one is the unexpected token
/// it looks like — the treatment every other separator in the language gets.
#[test]
fn a_trailing_separator_is_unexpected() {
    for src in [
        "type T = { x when 'a: A } where 'a;",
        "type T = { x when 'a: A } where ",
        "let f : { x when 'a: A } where 'a; = z",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{src:?}: expected one error: {:#?}", out.errors);
        };
        assert_eq!(error.kind, ErrorKind::Unexpected, "{src}");
    }
}

/// `_` in a type is the hole — a position left for inference — so it parses
/// wherever a type does, as often as one likes, and prints back as itself.
#[test]
fn a_type_hole_parses_as_a_type() {
    for src in [
        "let k : _ -> Nat = fn x => 0",
        "let k : _ -> _ = fn x => x",
        "let k : { x: _, y: _ } -> Nat = fn p => 0",
        "let k : #Some _ | #None = nothing",
        "let k : List _ = nil",
    ] {
        assert_eq!(parse_one(src), src);
    }

    // And it is the node it says it is, rather than a name spelled `_`.
    let out = parse(lex("let k : _ = 1", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Let { ty, .. } = &out.stmts[0].tracked else {
        panic!("expected a definition");
    };
    let ty = ty.as_ref().expect("it is annotated");
    assert!(matches!(ty.ty.tracked, TypeKind::Hole), "{:#?}", ty.ty);
}

/// `_` names nothing, so a formula cannot be about it: `when _` mints a
/// presence no clause may mention, and the `_` written in one gets the
/// wildcard's own complaint worded for the type position it sits in.
#[test]
fn a_clause_may_not_name_the_discard() {
    let src = "let x : { a when _: A } where _ = y";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert_eq!(error.kind, ErrorKind::Wildcard { place: Place::Type });
    assert_eq!(error.span.start, src.rfind('_').expect("the discard"));
}

/// `\name` is accepted wherever a field may be written — first, middle, last,
/// beside ordinary and `when` fields, before either kind of tail — and prints
/// back as written. The parser judges nothing: a closed struct with a `\` in
/// it parses too, and refusing it is lowering's rule.
#[test]
fn a_struct_field_may_be_written_absent() {
    for src in [
        "let x : { a: A, \\y, .. } = z",
        "let x : { a: A, \\y, ..'r } = z",
        "let x : { \\y } = z",
        "let x : { \\y, a: A, .. } = z",
        "let x : { a: A, \\y, b when 'a: B, .. } = z",
        "let x : { a: A, b when 'a: B, \\y, ..'r } = z",
        // Two absences of one name are both recorded, the way two fields of
        // one name are: rejecting a repeat is the IR's job.
        "let x : { \\y, \\y, .. } = z",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// The absent entry claims the whole `\name` as its span, so a diagnostic
/// about it underlines the mark along with the name it marks.
#[test]
fn an_absent_label_spans_its_mark() {
    let ascribed = |src: &str| -> Type {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(out.errors.is_empty(), "{src:?}: {:#?}", out.errors);
        match &out.stmts[0].tracked {
            StmtKind::Let { ty: Some(ty), .. } => ty.ty.clone(),
            other => panic!("expected an ascribed let, got {other:?}"),
        }
    };

    let src = "let x : { a: A, \\y, .. } = z";
    let TypeKind::Struct { fields, .. } = ascribed(src).tracked else {
        panic!("expected a struct type");
    };
    let (name, field) = fields.get_index(1).expect("the absent field");
    assert!(matches!(field, TypeField::Absent));
    assert_eq!(name.tracked, "y");
    assert_eq!(name.span.start, src.find("\\y").expect("the mark"));
    assert_eq!(name.span.width, 2);

    let src = "let x : #Ok Nat | \\#Err | ..'r = z";
    let TypeKind::Sum { cases, .. } = ascribed(src).tracked else {
        panic!("expected a sum type");
    };
    let (name, case) = cases.get_index(1).expect("the absent case");
    assert!(matches!(case, SumCase::Absent));
    assert_eq!(name.tracked, "Err");
    assert_eq!(name.span.start, src.find("\\#Err").expect("the mark"));
    assert_eq!(name.span.width, 5);
}

#[test]
fn a_tail_ends_the_field_list() {
    // The tail stands for the fields not named, which have no order among
    // the named ones to claim: nothing may follow it, not even a comma.
    for src in [
        "let x : { .., a: A } = y",
        "let x : { a: A, ..'r, } = y",
        "let x : { a: A .. } = y",
        "let x : { ..1 } = y",
        "let x : { a? A } = y",
        "let x : { a: A, ... } = y",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src} parsed without complaint");
    }
}

#[test]
fn functions() {
    assert_eq!(parse_one("let id = fn x => x"), "let id = fn x => x");
    // Body extends rightward over application.
    assert_eq!(
        parse_one("let k = fn a b => f a b"),
        "let k = fn a b => f a b"
    );
    // A function can be an application argument. The printer parenthesizes
    // it so the lambda body can't swallow whatever follows.
    assert_eq!(
        parse_one("let m = map fn x => x"),
        "let m = map (fn x => x)"
    );
}

#[test]
fn zero_arg_functions_are_rejected() {
    // A function binding nothing is an error, reported at the arrow.
    let out = parse(lex("let z = fn => y", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn struct_exprs() {
    assert_eq!(
        parse_one("let p = { x: a, y: b }"),
        "let p = { x: a, y: b }"
    );
    // Trailing comma, application values, and nesting.
    assert_eq!(
        parse_one("let q = { f: g x, inner: { z: w }, }"),
        "let q = { f: g x, inner: { z: w } }"
    );
    // A struct literal can be an application argument.
    assert_eq!(parse_one("let r = use { k: v }"), "let r = use { k: v }");
}

#[test]
fn duplicate_fields_are_allowed() {
    // The surface syntax records both; rejecting a repeated name is the
    // IR's job, so nothing is dropped or merged here.
    assert_eq!(
        parse_one("let p = { x: a, x: b }"),
        "let p = { x: a, x: b }"
    );
    assert_eq!(
        parse_one("type T = { a: A, a: B }"),
        "type T = { a: A, a: B }"
    );
}

#[test]
fn functions_are_terms_only() {
    // The type language has no binder, so `fn` in type position is the
    // unexpected token it looks like — just as a natural literal there is.
    let out = parse(lex("type F = fn a => a", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert!(out.stmts.is_empty(), "stmts: {:#?}", out.stmts);

    // Including in a nested type position, where the `fn` is all that is
    // wrong with the statement around it.
    let out = parse(lex("let x : fn a => a = ()", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn parens_group_application() {
    // Grouping survives as tree shape, and the printer re-inserts the
    // parentheses it needs to round-trip.
    assert_eq!(parse_one("let a = f (g x)"), "let a = f (g x)");
    assert_eq!(parse_one("let b = (f g) x"), "let b = f g x");
    assert_eq!(parse_one("let c = f (g x) y"), "let c = f (g x) y");
    // Redundant parentheses leave no trace.
    assert_eq!(parse_one("let d = ((x))"), "let d = x");
    assert_eq!(parse_one("let e = (f) (x)"), "let e = f x");
}

#[test]
fn parens_group_lambdas() {
    // A lambda body extends rightward, so parenthesizing it is the only way
    // to apply one directly — and the only way to print it back.
    assert_eq!(parse_one("let a = (fn x => x) y"), "let a = (fn x => x) y");
    assert_eq!(
        parse_one("let b = f (fn x => x) y"),
        "let b = f (fn x => x) y"
    );
    // Without parentheses the lambda still swallows the rest, which the
    // printer then makes explicit.
    assert_eq!(
        parse_one("let c = f fn x => x y"),
        "let c = f (fn x => x y)"
    );
}

#[test]
fn naturals_are_atoms() {
    assert_eq!(parse_one("let n = 0"), "let n = 0");
    // An atom, so it takes part in application on either side and never
    // needs parentheses of its own.
    assert_eq!(parse_one("let a = f 1 2"), "let a = f 1 2");
    assert_eq!(parse_one("let b = 1 f"), "let b = 1 f");
    assert_eq!(parse_one("let c = f (g 3)"), "let c = f (g 3)");
    assert_eq!(parse_one("let d = fn x => 7"), "let d = fn x => 7");
    assert_eq!(
        parse_one("let e = { width: 3, height: 4 }"),
        "let e = { width: 3, height: 4 }"
    );
}

#[test]
fn naturals_are_terms_only() {
    // There is no type-level natural, so a literal in type position is the
    // unexpected token it looks like rather than a silently accepted one.
    let out = parse(lex("type T = 42", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
}

#[test]
fn empty_parens_are_unit() {
    assert_eq!(parse_one("let a = ()"), "let a = ()");
    assert_eq!(parse_one("let b = f ()"), "let b = f ()");
    assert_eq!(parse_one("type T = ()"), "type T = ()");
}

#[test]
fn parens_group_types() {
    // Redundant parentheses leave no trace, as in expressions.
    assert_eq!(parse_one("type T = ((A))"), "type T = A");
    assert_eq!(parse_one("type U = (A -> B)"), "type U = A -> B");
    assert_eq!(
        parse_one("type R = { items: (A -> B) }"),
        "type R = { items: A -> B }"
    );
}

#[test]
fn unmatched_closing_paren_is_an_error() {
    let out = parse(lex("let x = y)  let z = w", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    // Recovery resumes at the next statement keyword.
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);
}

#[test]
fn arrows_are_right_associative() {
    assert_eq!(parse_one("type F = A -> B"), "type F = A -> B");
    // Right-associative, so the grouping on the right leaves no trace...
    assert_eq!(parse_one("type G = A -> (B -> C)"), "type G = A -> B -> C");
    assert_eq!(parse_one("type H = A -> B -> C"), "type H = A -> B -> C");
    // ...and the one on the left is reconstructed, since without it the
    // printed source would re-parse as the right-nested tree.
    assert_eq!(
        parse_one("type I = (A -> B) -> C"),
        "type I = (A -> B) -> C"
    );
    // Everything else is an atom, so it stands on either side unparenthesized.
    assert_eq!(
        parse_one("type M = { f: A -> B }"),
        "type M = { f: A -> B }"
    );
    assert_eq!(parse_one("type N = () -> ()"), "type N = () -> ()");
    assert_eq!(
        parse_one("type O = { x: A } -> B"),
        "type O = { x: A } -> B"
    );
}

#[test]
fn projection_binds_tighter_than_application() {
    assert_eq!(parse_one("let a = p.x"), "let a = p.x");
    // Left-associative, so a chain needs no parentheses.
    assert_eq!(parse_one("let b = p.x.y"), "let b = p.x.y");
    // `f p.x` is `f (p.x)`, not `(f p).x` — which is why the second form
    // has to be written, and printed, with parentheses.
    assert_eq!(parse_one("let c = f p.x"), "let c = f p.x");
    assert_eq!(parse_one("let d = (f p).x"), "let d = (f p).x");
    assert_eq!(parse_one("let e = f p.x q.y"), "let e = f p.x q.y");
    // Any atom can be projected out of.
    assert_eq!(parse_one("let g = { x: a }.x"), "let g = { x: a }.x");
    assert_eq!(parse_one("let h = (fn p => p).x"), "let h = (fn p => p).x");
}

#[test]
fn a_projection_needs_a_field_name() {
    let out = parse(lex("let a = p.  let b = ()", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(out.stmts.len(), 1, "stmts: {:#?}", out.stmts);
}

/// `..` belongs to a struct type and nowhere 'else, so meeting one where 'an
/// expression is being read is an error rather than a place to stop.
///
/// Stopping was the bug: the projection ended cleanly at the `..`, so did the
/// application, and so did the `let` — leaving a definition of `p` that
/// parsed, and one "unexpected token" pointing at whatever followed. A
/// truncated edit came out as a program that meant something else, and the
/// complaint was about the wrong line.
#[test]
fn a_dot_dot_cannot_follow_an_expression() {
    for src in [
        "let a = p..x",
        "let a = p...'x",
        "let a = p..",
        "let a = 1 ..'x",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without error");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }

    // And the `..` a struct type is entitled to is untouched.
    assert_eq!(parse_one("let x : { ..'r } = y"), "let x : { ..'r } = y");
}

/// Nothing is silently stood in for a missing expression or type: a position
/// with nothing usable in it is reported where it was written, so a truncated
/// edit cannot pass for a program that happens to mean something else.
#[test]
fn a_missing_expression_or_type_is_reported() {
    let errors = |src: &str| {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without error");
        out
    };

    // A stray dot is not a projection out of unit.
    errors("let a = .x");
    // An ascription with no type in it is not an ascription of `()`.
    errors("let b : = ()");
    // Nor is a dangling arrow an arrow to `()`.
    errors("type T = A ->  let y = ()");
}

/// Running out of input is reported too. A production that failed quietly at
/// EOF would drop the definition it was parsing and leave the run looking
/// successful; the error points at the end of the input, which is where the
/// missing piece would have gone.
#[test]
fn end_of_input_is_reported_where_it_runs_out() {
    for src in ["let a = p.", "let b :", "type T = A ->", "let c ="] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(out.errors[0].span.start, src.len(), "{src:?}");
        assert_eq!(out.errors[0].span.width, 0, "{src:?}");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }
}

#[test]
fn a_let_may_be_ascribed_a_type() {
    assert_eq!(parse_one("let x : A = y"), "let x : A = y");
    assert_eq!(
        parse_one("let f : A -> B = fn a => a"),
        "let f : A -> B = fn a => a"
    );
    // The plan's witness for width subtyping, in one line.
    assert_eq!(
        parse_one("let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"),
        "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x"
    );
    // Without an ascription nothing is printed where one would go.
    assert_eq!(parse_one("let z = ()"), "let z = ()");
}

#[test]
fn only_a_let_may_be_ascribed() {
    // A `type` declaration is a type already; there is nothing to check it
    // against, so the colon is the unexpected token it looks like.
    let out = parse(lex("type T : K = ()", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "stmts: {:#?}", out.stmts);
}

/// Juxtaposition in a type is an application, the way it is in a term. It was
/// once an error — there was no type-level binder for it to mean anything
/// against — and the two cases below are the ones that used to be reported.
#[test]
fn types_apply_by_juxtaposition() {
    assert_eq!(parse_one("type M = Map K"), "type M = Map K");
    assert_eq!(
        parse_one("type R = { items: List T }"),
        "type R = { items: List T }"
    );
}

/// Gathered flat and printed flat: `Pair A B` is one application of two
/// arguments, so an argument that is itself an application needs parentheses to
/// survive a round trip.
#[test]
fn type_application_is_flat() {
    assert_eq!(parse_one("type P = Pair A B"), "type P = Pair A B");
    assert_eq!(
        parse_one("type P = Pair (Pair A B) C"),
        "type P = Pair (Pair A B) C"
    );
    // An arrow is looser than an application, so an argument that is one keeps
    // its parentheses and a result that is one does not need any.
    assert_eq!(
        parse_one("type F = Pair (A -> B) C"),
        "type F = Pair (A -> B) C"
    );
}

/// Application binds tighter than the arrow, so neither side of an arrow needs
/// parentheses to hold an application.
#[test]
fn type_application_binds_tighter_than_the_arrow() {
    assert_eq!(
        parse_one("type F = Pair A B -> Nat"),
        "type F = Pair A B -> Nat"
    );
    assert_eq!(
        parse_one("type F = Nat -> Pair A B"),
        "type F = Nat -> Pair A B"
    );
}

/// A declaration binds its parameters at its head, and prints them back there.
#[test]
fn a_declaration_binds_parameters_at_its_head() {
    assert_eq!(
        parse_one("type Pair A B = { a: A, b: B }"),
        "type Pair A B = { a: A, b: B }"
    );
    // Taking none is still the ordinary case, and prints with no room left for
    // a list that is not there.
    assert_eq!(parse_one("type T = Nat"), "type T = Nat");
}

/// The comma is what tells a field's type from the struct's tail. Without one,
/// a `..` after a type would be an argument to it; with one, it ends the field
/// list. Nothing today can write a `..` in argument position, but the grammar
/// only stays unambiguous while that is true, so it is pinned here.
#[test]
fn a_comma_tells_a_field_from_a_tail() {
    assert_eq!(
        parse_one("let x : { f: List T, ..'r } = y"),
        "let x : { f: List T, ..'r } = y"
    );
    assert_eq!(
        parse_one("let x : { f: List, ..'r } = y"),
        "let x : { f: List, ..'r } = y"
    );
}

/// A sum type is its cases, separated by `|`, and the leading one is optional.
/// Both spellings are one tree, and it prints back without the leading bar —
/// so the round-trip through `parse_one` is what pins that the printer's
/// choice is one the parser reads back.
#[test]
fn sum_types() {
    assert_eq!(
        parse_one("type Option T = #Some T | #None"),
        "type Option T = #Some T | #None"
    );
    // The leading `|` is accepted and dropped.
    assert_eq!(
        parse_one("type Option T = | #Some T | #None"),
        "type Option T = #Some T | #None"
    );
    // One case is a sum like any other.
    assert_eq!(parse_one("type Just = #It Nat"), "type Just = #It Nat");
    // And a case may be marked with a `when`, which no tail can say for it.
    assert_eq!(
        parse_one("let x : #A (when 'a) Nat | #B = y"),
        "let x : #A (when 'a) Nat | #B = y"
    );
}

/// The two forms that write no case at all. Neither would be read back as a
/// sum without the leading `|`, which is why the printer writes one there and
/// nowhere 'else.
#[test]
fn a_sum_with_no_cases_is_a_bare_bar() {
    assert_eq!(parse_one("type Void = |"), "type Void = |");
    assert_eq!(parse_one("type Only r = | ..r"), "type Only r = | ..r");
}

/// A `|` between two cases promises a second one, so one with nothing after it
/// is reported. Left silent, the annotation in `let x : #A Nat | = 1` read
/// as `#A Nat` and the bar the writer meant something by vanished without a
/// word.
///
/// The leading bar is the exception, and the only one: `|` alone is the sum
/// with no cases, so there is nothing it promised. The cases already read
/// stand either way — the reader has one thing to delete, and the sum they
/// wrote is still there to be checked.
#[test]
fn a_bar_promising_a_case_that_never_comes_is_reported() {
    for (src, at) in [
        ("let x : #A Nat | = 1", 15),
        ("let x : #A Nat | | #B = 1", 15),
        ("type T = #A | ", 12),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let first = out.errors.first().unwrap_or_else(|| panic!("{src}"));
        assert_eq!(first.span.start, at, "{src}: {:#?}", out.errors);
        assert_eq!(first.span.width, 1, "{src}: {:#?}", out.errors);
    }

    // The leading bar promises nothing, and neither does a case with no bar
    // after it.
    for src in ["type Void = |", "type Only r = | ..r", "type T = #A | #B"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    }
}

/// A sum's tail is the struct's `..`, standing for the cases not written out.
#[test]
fn a_sum_may_be_left_open() {
    assert_eq!(
        parse_one("let x : #A Nat | .. = y"),
        "let x : #A Nat | .. = y"
    );
    assert_eq!(
        parse_one("type Tagged r = #Err Nat | ..r"),
        "type Tagged r = #Err Nat | ..r"
    );
}

/// `\#Tag` is accepted wherever a case may be written, and a leading `\`
/// begins a sum the way a leading tag or `|` does — with or without the
/// leading bar. The case keeps its `#`, so it is spelled the same way
/// present or absent, and a closed sum with one parses here: refusing it is
/// lowering's rule.
#[test]
fn a_sum_case_may_be_written_absent() {
    for src in [
        "let x : #Ok Nat | \\#Err | ..'r = y",
        "let x : \\#B | .. = y",
        "let x : #A | \\#B = y",
        "let x : \\#A | #B Nat | \\#C | .. = y",
        "type NoErr r = #Ok Nat | \\#Err | ..r",
    ] {
        assert_eq!(parse_one(src), src);
    }
    // The leading `|` is accepted and dropped, exactly as it is before a tag.
    assert_eq!(
        parse_one("let x : | \\#B | .. = y"),
        "let x : \\#B | .. = y"
    );
}

/// An absent label is bare: `\y` takes no `:` type and no `when`, `\#B` no
/// payload and no `when`, a `\` needs a name after it, and a `\` begins no
/// expression at all. Each is the ordinary unexpected-token report, at the
/// token that has no place — and the statement is dropped, as for any other
/// malformed one.
#[test]
fn an_absent_label_is_bare() {
    // The tail of each source is where the complaint lands.
    for (src, from) in [
        ("let x : { \\y: Nat, .. } = 1", ": Nat, .. } = 1"),
        ("let x : { \\y when a, .. } = 1", "when a, .. } = 1"),
        ("let x : \\#B (when 'a) | .. = 1", "(when 'a) | .. = 1"),
        ("let x : #A | \\#B Nat | .. = 1", "Nat | .. = 1"),
        ("let x : { \\, .. } = 1", ", .. } = 1"),
        // In a type, what follows a `\` must be a tag or a field name.
        ("let x : \\= 1", "= 1"),
        // In an expression, the `\` itself begins nothing.
        ("let x = \\y", "\\y"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}: {:#?}", out.errors);
        assert!(out.stmts.is_empty(), "{src:?} kept: {:#?}", out.stmts);
    }
}

/// A case carries one atom, the same as a tag in a term does, so anything with
/// a space in it takes parentheses — and an arrow is looser than a sum, so
/// `#A Nat -> Nat` is a function *from* the sum rather than a case carrying
/// an arrow. The printer puts the parentheses back where the grammar needs
/// them.
#[test]
fn a_case_carries_one_atom() {
    assert_eq!(
        parse_one("type F = #Some (Pair A B)"),
        "type F = #Some (Pair A B)"
    );
    assert_eq!(
        parse_one("type F = #A Nat -> Nat"),
        "type F = #A Nat -> Nat"
    );
    assert_eq!(
        parse_one("type F = #A (Nat -> Nat)"),
        "type F = #A (Nat -> Nat)"
    );
    // A sum in a position an argument could follow needs parentheses of its
    // own, which is the whole of why it sits above the arrow.
    assert_eq!(
        parse_one("type F = Pair (#A Nat | #B) Nat"),
        "type F = Pair (#A Nat | #B) Nat"
    );
    // To the right of an arrow nothing can follow it, so none are written.
    assert_eq!(
        parse_one("type F = Nat -> #A Nat | #B"),
        "type F = Nat -> #A Nat | #B"
    );
}

/// A tag takes one operand, and takes it greedily: `f #A 1` is `f` applied
/// to the case rather than the case applied to `1`. A tag carries one thing, so
/// there is nothing the other reading could mean.
#[test]
fn a_tag_binds_tighter_than_application() {
    assert_eq!(parse_one("let v = #Some 1"), "let v = #Some 1");
    assert_eq!(parse_one("let v = #None"), "let v = #None");
    assert_eq!(parse_one("let v = f #A 1"), "let v = f (#A 1)");
    // Which is why a bare tag written as an argument comes back in
    // parentheses: nothing follows it here, but the printer decides one node
    // at a time, and a second argument after it would be read as the payload
    // it has not got.
    assert_eq!(parse_one("let v = f #A"), "let v = f (#A)");
    // The same rule at the head of an application, where 'leaving them off
    // would turn the argument into a payload and print the tree above.
    assert_eq!(parse_one("let v = f (#A) 1"), "let v = f (#A) 1");
    assert_eq!(parse_one("let v = (#A) 1"), "let v = (#A) 1");
    // The payload is a projection, so the field is carried rather than the
    // record it was read off.
    assert_eq!(parse_one("let v = #Some p.x"), "let v = #Some p.x");
    // And an application as a payload takes parentheses, which the printer
    // supplies because the tag groups as an application itself.
    assert_eq!(parse_one("let v = #Some (f x)"), "let v = #Some (f x)");
}

/// The two shapes nest inside each other, and each closes itself: a struct's
/// braces end a sum written in a field, and a case's payload is one atom, so a
/// struct written as one needs no parentheses either.
#[test]
fn a_sum_and_a_struct_nest_without_help() {
    assert_eq!(
        parse_one("let x : { f: #A Nat | #B, g: Nat } = y"),
        "let x : { f: #A Nat | #B, g: Nat } = y"
    );
    assert_eq!(
        parse_one("type List a = #Nil | #Cons { head: a, tail: List a }"),
        "type List a = #Nil | #Cons { head: a, tail: List a }"
    );
    // A case with no payload followed by another field ends where the comma
    // does, since a comma begins no atom.
    assert_eq!(
        parse_one("let x : { f: #A, g: Nat } = y"),
        "let x : { f: #A, g: Nat } = y"
    );
}

/// Every production that can give up, made to. A production that returned
/// `None` quietly would drop the statement it was parsing and leave the run
/// looking successful, so what this pins is that each one reports before it
/// does: one complaint per source, and the statement gone.
///
/// The list is by position rather than by kind — a name, a separator, a
/// delimiter, or the thing after it — because that is what the parser is made
/// of, and a position with no case here is one whose failure nothing watches.
#[test]
fn every_position_that_can_fail_reports_before_it_does() {
    for src in [
        // A definition's own name, and the two halves of an ascription.
        "let = 1",
        "let x : = 1",
        "let x : Nat",
        // An application's argument: the token begins an atom, and the atom
        // still does not parse.
        "let v = f {",
        // Inside a struct term: the label, the colon, the value, the brace.
        "let v = { 1: 2 }",
        "let v = { x 2 }",
        "let v = { x: }",
        "let v = { x: 1",
        // A case's payload, which is an atom taken greedily.
        "let v = #A {",
        // Parentheses, around an expression that is not one and around one
        // that is never closed.
        "let v = (let)",
        "let v = (1",
        // A function's arrow and its body.
        "let f = fn x y",
        "let f = fn x =>",
        // The type language keeps the same positions: a case's payload, an
        // argument, a field's label, a field's type, and the parentheses.
        "type X = #A {",
        "type X = Box {",
        "type X = { 1: Nat }",
        "type X = { a: }",
        "type X = (let)",
        "type X = (Nat",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(
            out.stmts.is_empty(),
            "{src:?} kept a statement: {:#?}",
            out.stmts
        );
    }
}

/// `let <name> [: <type>] = <value> in <body>` is an expression, and it prints
/// back as what it was written as. The ascription is the same grammar the
/// statement has, so a definition and a binding inside one read alike.
#[test]
fn a_let_is_an_expression() {
    assert_eq!(
        parse_one("let a = let x = 1 in x"),
        "let a = let x = 1 in x"
    );
    assert_eq!(
        parse_one("let a = let x : Nat = 1 in x"),
        "let a = let x : Nat = 1 in x"
    );
    assert_eq!(
        parse_one("let a = let f : { x: Nat } -> Nat = fn p => p.x in f"),
        "let a = let f : { x: Nat } -> Nat = fn p => p.x in f"
    );
}

/// The body extends as far right as it can, ML-style, exactly as a `fn` body
/// does — so the application is inside the `let` rather than the `let` inside
/// the application.
#[test]
fn a_lets_body_runs_as_far_right_as_it_can() {
    assert_eq!(
        parse_one("let a = let f = g in f x y"),
        "let a = let f = g in f x y"
    );
    // And the value stops at the `in`, which is what makes that unambiguous:
    // `in` begins no atom, so the application gathering `g`'s arguments ends
    // in front of it.
    assert_eq!(
        parse_one("let a = let f = g x in f"),
        "let a = let f = g x in f"
    );
}

/// A `let` may start an expression anywhere 'an atom may.
#[test]
fn a_let_starts_an_expression_wherever_an_atom_does() {
    for (src, printed) in [
        // A `fn` body.
        (
            "let a = fn p => let x = p in x",
            "let a = fn p => let x = p in x",
        ),
        // A struct field's value.
        (
            "let a = { v: let n = 1 in n }",
            "let a = { v: let n = 1 in n }",
        ),
        // A parenthesized expression.
        ("let a = (let n = 1 in n)", "let a = let n = 1 in n"),
        // Another let's value, and another let's body.
        (
            "let a = let x = let y = 1 in y in x",
            "let a = let x = let y = 1 in y in x",
        ),
        (
            "let a = let x = 1 in let y = x in y",
            "let a = let x = 1 in let y = x in y",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src}");
    }
}

/// A `let` is not an application argument: the loop that gathers arguments
/// stops in front of one. Which is what keeps two definitions written one after
/// the other from reading as an application, and what makes the parentheses in
/// `f (let x = 1 in x)` the way to pass one.
#[test]
fn a_let_is_not_an_application_argument() {
    let out = parse(lex("let a = 1 let b = 2", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);

    assert_eq!(
        parse_one("let a = f (let x = 1 in x)"),
        "let a = f (let x = 1 in x)"
    );

    // Without them the application ends at `f`, the `let` is read as a new
    // definition, and its `in` is the token nothing can use.
    let src = "let a = f let x = 1 in x";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);
    assert_eq!(out.errors.len(), 1, "errors: {:#?}", out.errors);
    assert_eq!(
        out.errors[0].span.start,
        src.find(" in ").expect("the `in`") + 1
    );
}

/// Every position a nested `let` can fail at, reported where it is — and the
/// statement dropped, so recovery resumes at the next `let` or `type` the way
/// it does for any other malformed statement.
#[test]
fn a_let_expression_reports_where_it_fails() {
    // Each case is the source and the tail of it the complaint should land on,
    // which is the token the position had no reading for.
    for (src, from) in [
        // The name, the ascribed type, and the `=`.
        ("let a = let = 1 in x", "= 1 in x"),
        ("let a = let x : = 1 in x", "= 1 in x"),
        ("let a = let x 1 in x", "1 in x"),
        // The value, the `in` the body follows, and the body itself.
        ("let a = let x = in x", "in x"),
        ("let a = let x = 1 } x", "} x"),
        ("let a = let x = 1 in }", "}"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?} kept: {:#?}", out.stmts);
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}");
    }

    // A `let` with no `in` at all runs out of input, and is reported there.
    let src = "let a = let x = 1";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);

    // And recovery resumes at the next statement rather than swallowing it.
    let out = parse(lex("let a = let x = 1  let b = 2", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.stmts.len(), 1, "stmts: {:#?}", out.stmts);
}

/// The match expression, printed back as written: leading `|` on every arm —
/// the grammar makes the first one optional, so both spellings read back to
/// one printed form — zero arms, a sole arm, and projection off the `end`.
#[test]
fn parses_match_expressions() {
    assert_eq!(
        parse_one("let a = match x with #Some y => y | #None => 0 end"),
        "let a = match x with | #Some y => y | #None => 0 end"
    );
    // The leading bar is optional and not recorded: both spellings are one
    // tree.
    assert_eq!(
        parse_one("let a = match x with | #Some y => y end"),
        "let a = match x with | #Some y => y end"
    );
    assert_eq!(
        parse_one("let a = match x with end"),
        "let a = match x with end"
    );
    assert_eq!(
        parse_one("let a = match x with w => w end"),
        "let a = match x with | w => w end"
    );
    // The scrutinee is a full expression, ending at the `with` of its own
    // accord.
    assert_eq!(
        parse_one("let a = match f x with | y => y end"),
        "let a = match f x with | y => y end"
    );
    // Projection off the `end` works the way it does off a parenthesized
    // expression; printing brackets the match, which reads back as the same
    // tree.
    assert_eq!(
        parse_one("let a = match x with | w => w end.field"),
        "let a = (match x with | w => w end).field"
    );
}

/// A match may head an application — it is reachable from atom position — but
/// is not an application argument: `f match ... end` ends the application at
/// `f` and leaves the `match` as the token nothing can use.
#[test]
fn a_match_is_not_an_application_argument() {
    assert_eq!(
        parse_one("let a = f (match x with | w => w end)"),
        "let a = f (match x with | w => w end)"
    );
    assert_eq!(
        parse_one("let a = match x with | w => w end 1"),
        "let a = match x with | w => w end 1"
    );
    let src = "let a = f match x with | w => w end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    assert_eq!(
        out.errors[0].span.start,
        src.find("match").expect("the match")
    );
}

/// Every kind of pattern, everywhere a pattern goes: puns, renames, nesting,
/// naturals, `()`, and the tag payload taken greedily — `#A #B x` is
/// `#A` carrying `(#B x)`, which the printer brackets and the grammar
/// reads back the same.
#[test]
fn parses_every_kind_of_pattern() {
    for (src, printed) in [
        (
            "let a = match x with { p, q: r } => r end",
            "let a = match x with | { p, q: r } => r end",
        ),
        (
            "let a = match x with {} => 1 end",
            "let a = match x with | {} => 1 end",
        ),
        // A trailing comma among the fields is allowed, as in a struct
        // expression.
        (
            "let a = match x with { p, } => p end",
            "let a = match x with | { p } => p end",
        ),
        (
            "let a = match x with { pos: { x, y } } => x end",
            "let a = match x with | { pos: { x, y } } => x end",
        ),
        (
            "let a = match x with #A #B y => y end",
            "let a = match x with | #A (#B y) => y end",
        ),
        // The parenthesized spelling is the same tree.
        (
            "let a = match x with #A (#B y) => y end",
            "let a = match x with | #A (#B y) => y end",
        ),
        (
            "let a = match x with 0 => 1 | k => k end",
            "let a = match x with | 0 => 1 | k => k end",
        ),
        (
            "let a = match x with () => 1 end",
            "let a = match x with | () => 1 end",
        ),
        // Grouping parentheses around a pattern are discarded, as around an
        // expression.
        (
            "let a = match x with (w) => w end",
            "let a = match x with | w => w end",
        ),
        (
            "let a = match x with #Cons { head: #Some y, tail: t } => y end",
            "let a = match x with | #Cons { head: #Some y, tail: t } => y end",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src}");
    }
}

/// A `let` — statement and expression — takes a pattern where it took a name.
#[test]
fn parses_pattern_lets() {
    assert_eq!(parse_one("let {x, y} = p"), "let { x, y } = p");
    assert_eq!(
        parse_one("let {x: a} : { x: Nat } = p"),
        "let { x: a } : { x: Nat } = p"
    );
    assert_eq!(parse_one("let () = p"), "let () = p");
    assert_eq!(
        parse_one("let f = fn p => let {pos: {x, y}} = p in x"),
        "let f = fn p => let { pos: { x, y } } = p in x"
    );
    // The parser records what was written and judges nothing: a refutable
    // pattern on a `let` parses, and refusing it is lowering's rule.
    assert_eq!(parse_one("let #Some x = opt"), "let #Some x = opt");
    assert_eq!(parse_one("let 0 = n"), "let 0 = n");
}

/// Every way a match can fail to parse, reported at the token that had no
/// reading: the missing `end`, the missing pattern, the arm without a body,
/// the trailing `|` that promised another arm, and `match` where a name goes.
#[test]
fn a_match_reports_where_it_fails() {
    for (src, from) in [
        // An arm needs a pattern, and `=>` does not begin one.
        ("let a = match x with => 1 end", "=> 1 end"),
        // A `|` between arms promises another one; `end` is where the promise
        // breaks.
        ("let a = match x with #A y => 1 | end", "end"),
        // An arm without a body: `|` begins no expression.
        (
            "let a = match x with #A y => | #B z => 2 end",
            "| #B z => 2 end",
        ),
        // `match` is a keyword now, so it no longer names anything.
        ("let match = 1", "match = 1"),
        // Patterns on function arguments do not exist.
        ("let f = fn {x} => x", "{x} => x"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}");
    }

    // A match with no `end` runs out of input, and is reported there.
    let src = "let a = match x with #A y => 1";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);
}

/// A pattern position at the end of input is reported there — `let` alone,
/// and a match cut off at its arm.
#[test]
fn a_pattern_at_the_end_of_input_is_reported_there() {
    for src in ["let", "let a = match x with |"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert_eq!(out.errors[0].span.start, src.len(), "{src:?}");
        assert_eq!(out.errors[0].span.width, 0, "{src:?}");
    }
}

/// A bare tag as another tag's payload is bracketed when 'printed — it would
/// otherwise swallow whatever follows as the payload it has not got — and the
/// bracketed form reads back to the same tree.
#[test]
fn a_bare_tag_payload_is_bracketed() {
    assert_eq!(
        parse_one("let a = match x with #A #B => 1 end"),
        "let a = match x with | #A (#B) => 1 end"
    );
}

/// `_` parses wherever a pattern does — a whole arm, a struct sub-pattern, a
/// tag's payload, inside grouping parentheses, and the pattern of a `let` in
/// both forms — and prints back as the `_` it was written as.
#[test]
fn a_wildcard_parses_in_every_pattern_position() {
    for (src, printed) in [
        // A whole arm, and grouping parentheses discarded around one.
        (
            "let a = match x with _ => 1 end",
            "let a = match x with | _ => 1 end",
        ),
        (
            "let a = match x with (_) => 1 end",
            "let a = match x with | _ => 1 end",
        ),
        // A struct pattern's sub-pattern, beside a pun and a named binder.
        (
            "let a = match x with { p: _, q } => q end",
            "let a = match x with | { p: _, q } => q end",
        ),
        // A tag's payload, taken greedily like any other.
        (
            "let a = match x with #Some _ => 1 | #None => 0 end",
            "let a = match x with | #Some _ => 1 | #None => 0 end",
        ),
        // The pattern of a `let`, statement and expression.
        ("let _ = f 1", "let _ = f 1"),
        ("let _ : Nat = g 3", "let _ : Nat = g 3"),
        ("let a = let _ = f 1 in 2", "let a = let _ = f 1 in 2"),
        // Nested, and repeated: two `_` in one pattern parse — whether that
        // binds anything twice is not a question, since it binds nothing.
        (
            "let a = match x with #Pair { a: _, b: _ } => 1 | _ => 2 end",
            "let a = match x with | #Pair { a: _, b: _ } => 1 | _ => 2 end",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src}");
    }
}

/// `fn _ => e` is legal — the argument is taken and thrown away — and so is
/// any mix of `_` with names. The printed form re-parses to the same tree.
#[test]
fn a_fn_argument_may_be_a_wildcard() {
    assert_eq!(parse_one("let f = fn _ => 1"), "let f = fn _ => 1");
    assert_eq!(parse_one("let f = fn _ _ => 1"), "let f = fn _ _ => 1");
    assert_eq!(parse_one("let f = fn _ x _ => x"), "let f = fn _ x _ => x");
    assert_eq!(
        parse_one("let const = fn x _ => x"),
        "let const = fn x _ => x"
    );
}

/// `_` anywhere it is not legal gets the dedicated complaint — `_` stands for
/// a value being thrown away — worded for the position, at the `_` itself.
/// The statement is dropped like any other malformed one.
#[test]
fn a_misplaced_wildcard_gets_its_own_complaint() {
    for (src, place) in [
        // Expression position: a definition's value, and an application's
        // argument.
        ("let x = _", Place::Value),
        ("let y = f _", Place::Value),
        // A field's name, in a struct expression and a struct pattern.
        ("let v = {_: 1}", Place::Field),
        ("let a = match x with { _: 1 } => 0 end", Place::Field),
        // The pun: a field bound to its own name, which `_` is not.
        ("let a = match x with {_} => 0 end", Place::Pun),
        // A projection, and the operation that most nearly is one: `!Log._`
        // names nothing to perform the way `x._` names no field.
        ("let p = x._", Place::Projection),
        ("let p = !Log._", Place::Projection),
        // A `type` declaration's name and its parameters, which are names
        // rather than types — a `_` in a type is the hole, and reads.
        ("type _ = Nat", Place::Type),
        ("type T _ = Nat", Place::Type),
        // And a name in a `where` clause's formula, which is written about a
        // presence a `when` gave a name to.
        ("let f : { x when 'a: Nat } where _ = { x: 1 }", Place::Type),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?} kept: {:#?}", out.stmts);
        let error = &out.errors[0];
        assert_eq!(
            error.kind,
            ErrorKind::Wildcard { place },
            "{src:?}: {:#?}",
            out.errors
        );
        // At the `_` itself: every one of these sources writes its stray `_`
        // last-but-something, so find the last one.
        assert_eq!(
            error.span.start,
            src.rfind('_').expect("the `_`"),
            "{src:?}"
        );
        assert_eq!(error.span.width, 1, "{src:?}");
    }
}

/// R1: a struct pattern may end with `..` — with fields before it, bare, and
/// with a trailing comma before it — and the printed form re-parses, `..`
/// last as written. The marker lands on the tree with the `..`'s own span.
#[test]
fn a_struct_pattern_may_end_with_a_rest() {
    assert_eq!(
        parse_one("let f = fn v => match v with | { a, .. } => a end"),
        "let f = fn v => match v with | { a, .. } => a end"
    );
    assert_eq!(
        parse_one("let f = fn v => match v with | { .. } => 1 end"),
        "let f = fn v => match v with | { .. } => 1 end"
    );
    // On a `let`, in both forms, and beside a renamed field.
    assert_eq!(parse_one("let {x, ..} = p"), "let { x, .. } = p");
    assert_eq!(
        parse_one("let a = let {x: y, ..} = p in y"),
        "let a = let { x: y, .. } = p in y"
    );

    let src = "let { a, .. } = p";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Let { pattern, .. } = &out.stmts[0].tracked else {
        panic!("a let");
    };
    let ruddy::parse::PatternKind::Struct { fields, rest } = &pattern.tracked else {
        panic!("a struct pattern");
    };
    assert_eq!(fields.len(), 1);
    let rest = rest.expect("the rest marker survives");
    assert_eq!(rest.start, src.find("..").expect("the dots"));
    assert_eq!(rest.width, 2);
}

/// R1's refusals: `..` is only legal as the final element of a struct
/// pattern. Anything after it — a comma, a second field, the name a future
/// named-rest might want — is the parser's usual unexpected-token complaint,
/// at the token after the `..`; input running out is reported at the end.
#[test]
fn a_rest_must_end_the_struct_pattern() {
    for (src, from) in [
        ("let {.., a} = p", ", a} = p"),
        ("let {a, .., b} = p", ", b} = p"),
        ("let {a, ..r} = p", "r} = p"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?} kept: {:#?}", out.stmts);
        assert_eq!(out.errors[0].kind, ErrorKind::Unexpected, "{src:?}");
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}: {:#?}", out.errors);
    }

    // `{..` at the end of input: the complaint points at the zero-width
    // position where the brace would have gone.
    let src = "let {..";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    assert_eq!(out.errors[0].kind, ErrorKind::Unexpected);
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);
}

/// Both forms of an `effect` declaration, printed back as they were written.
/// Which of the two a declaration is, is not the parser's to say — it records
/// the cases and lowering reads the `:`s — so a mixed one parses too.
#[test]
fn effect_declarations() {
    assert_eq!(
        parse_one("effect Log = write : Nat -> ()"),
        "effect Log = | write : Nat -> ()"
    );
    assert_eq!(
        parse_one("effect Log = write : Nat -> () | flush : () -> ()"),
        "effect Log = | write : Nat -> () | flush : () -> ()"
    );
    // An alias: effects, no signatures, unioned with the `+` a row writes —
    // and no leading mark, since a union has nothing to lead with.
    assert_eq!(
        parse_one("effect Console = !Log + !IO"),
        "effect Console = !Log + !IO"
    );
    // A bar between aliases is the union too, and prints back as the `+` it
    // means: the leading one is still optional, as it is on a sum.
    assert_eq!(
        parse_one("effect Console = | !Log | !IO"),
        "effect Console = !Log + !IO"
    );
    assert_eq!(
        parse_one("effect Console = | !Log"),
        "effect Console = !Log"
    );
    // The empty effect declares nothing and is written with the bar alone.
    assert_eq!(parse_one("effect Nil = |"), "effect Nil = |");
    // A mix of the two forms parses; refusing it is lowering's. Each case
    // prints with the mark its own form writes, which is what re-parses.
    assert_eq!(
        parse_one("effect Bad = !Log | w : Nat -> ()"),
        "effect Bad = !Log | w : Nat -> ()"
    );
}

/// A mark between cases promises another one, so nothing after it is reported
/// at the mark — the rule a sum type keeps. The leading bar promises nothing.
#[test]
fn an_effect_case_list_ends_at_a_bar_that_promised_one() {
    let out = parse(lex("effect E = !a + ", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::Unexpected);
}

/// The `+` clause binds to the arrow parsed at its own level, which the
/// right-associative recursion makes the innermost one. There is no spelling of
/// `(A -> B) + E`, so the printer's parentheses are the only thing that says
/// which arrow a row is on — and the round-trip in [`parse_one`] is what holds
/// it to that.
#[test]
fn an_effect_row_binds_to_the_innermost_arrow() {
    // Bare: the row sits on the one arrow there is.
    assert_eq!(
        parse_one("let f : A -> B + !Log = g"),
        "let f : A -> B + !Log = g"
    );
    // Three types, one arrow written twice: the row is on `B -> C`.
    let inner = parse_stmt("let f : A -> B -> C + !Log = g");
    let TypeKind::Arrow { to, effects, .. } = arrow_of(&inner) else {
        panic!("expected an arrow");
    };
    assert!(effects.is_none(), "the outer arrow carries no row");
    assert!(
        matches!(
            &to.tracked,
            TypeKind::Arrow {
                effects: Some(_),
                ..
            }
        ),
        "the inner arrow carries it: {to:#?}"
    );
    assert_eq!(
        parse_one("let f : A -> B -> C + !Log = g"),
        "let f : A -> B -> C + !Log = g"
    );

    // Parenthesized, the result comes back through an atom and never builds an
    // arrow at this level, so the row lands on the outer one.
    let outer = parse_stmt("let f : A -> (B -> C) + !Log = g");
    let TypeKind::Arrow { to, effects, .. } = arrow_of(&outer) else {
        panic!("expected an arrow");
    };
    assert!(effects.is_some(), "the outer arrow carries the row");
    assert!(
        matches!(&to.tracked, TypeKind::Arrow { effects: None, .. }),
        "the inner arrow carries none: {to:#?}"
    );
    assert_eq!(
        parse_one("let f : A -> (B -> C) + !Log = g"),
        "let f : A -> (B -> C) + !Log = g"
    );

    // And a higher-order argument that may perform something.
    assert_eq!(
        parse_one("let f : (A -> B + !Log) -> C = g"),
        "let f : (A -> B + !Log) -> C = g"
    );
}

/// The row is written in the syntax a sum's cases are: a `|` between effects,
/// an optional `..` tail, a `when` clause in parentheses, and the `\` of one
/// written absent. `+ |` is the empty row — the same thing a bare arrow means,
/// written out.
#[test]
fn an_effect_row_is_written_as_a_sum_row_is() {
    for source in [
        "let f : A -> B + !Log + !IO = g",
        "let f : A -> B + !Log + ..'e = g",
        "let f : A -> B + ..'e = g",
        "let f : A -> B + !Log (when 'a) + !IO (when 'b) = g",
        "let f : A -> B + \\!IO + ..'e = g",
        "let f : A -> B + | = g",
        "let f : A -> B + !Log where 'a = g",
    ] {
        assert_eq!(parse_one(source), source);
    }
}

/// A row of effects is a type at one position — an argument — and is written
/// there as an arrow's row is, minus the `+` that hangs one off an arrow. The
/// sigil on the first label is the whole of what tells it from a sum.
#[test]
fn a_row_of_effects_is_written_as_an_argument() {
    for source in [
        "let f : Runner (!Log) -> Nat = g",
        "let f : Runner (!Log + !IO) -> Nat = g",
        "let f : Runner (!Log (when 'a) + ..'r) -> Nat = g",
        "let f : Runner (\\!Log + ..) -> Nat = g",
        // Nothing but a rest, which is the row a declaration is handed when
        // the caller is passing on whatever it was given.
        "let f : Runner (..'r) -> Nat = g",
        "let f : Runner (..) -> Nat = g",
        // Not only inside parentheses: a row is a type wherever the grammar
        // reads one, and refusing the rest is lowering's.
        "let f : !Log = g",
    ] {
        assert_eq!(parse_one(source), source);
    }
    // A `\` heads a sum's absent case as well, so what follows it is what
    // says which of the two rows is being written.
    assert_eq!(
        parse_one("let f : Fallible (\\#Ok) -> Nat = g"),
        "let f : Fallible (\\#Ok) -> Nat = g"
    );
    // A `\` in front of neither is the unexpected token it looks like.
    let out = parse(lex("let f : Runner (\\1) -> Nat = g", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{:#?}", out.errors);
}

/// A `+` with no arrow to attach to is reported where it was written rather
/// than left for whatever the type was being read for to complain about a
/// token further along. The mark is what the row is reported at, which is the
/// whole of what it is still there for.
#[test]
fn a_row_with_no_arrow_is_a_parse_error() {
    for source in ["let x : Nat + !Log = 1", "type T = Nat + !Log"] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{source}: {:#?}", out.errors);
        assert_eq!(out.errors[0].kind, ErrorKind::Unexpected, "{source}");
        let plus = source.find('+').expect("the source writes one");
        assert_eq!(out.errors[0].span.start, plus, "{source}");
    }
}

/// `handle` keeps a `match`'s shape: the leading `|` is optional, zero arms
/// parse, and each body ends at the next `|` or the `end` of its own accord.
/// The `return` arm may sit anywhere 'among them.
#[test]
fn handle_expressions() {
    for source in [
        "let a = handle e with end",
        "let a = handle e with | !Log.write s => () end",
        "let a = handle e with | !Log.write s => () | !Log.flush _ => () end",
        "let a = handle e with | return x => x end",
        "let a = handle e with | return x => x | !Log.write s => () end",
        "let a = handle e with | !Log.write s => () | return x => x end",
        // The handled expression ends at the `with` however far right it runs.
        "let a = handle f x y with | !Log.write s => () end",
        // And a handler is self-delimiting — the `end` closes it — so it may
        // head an application with no parentheses, exactly as a match does.
        "let a = handle e with end 1",
    ] {
        assert_eq!(parse_one(source), source);
    }
    // The leading bar is optional and printed back on.
    assert_eq!(
        parse_one("let a = handle e with !Log.write s => () end"),
        "let a = handle e with | !Log.write s => () end"
    );
}

/// `raise` takes a whole expression, which extends as far right as it can — a
/// `fn` body's rule — so nothing after it is applied to what it gives back.
#[test]
fn raise_expressions() {
    assert_eq!(parse_one("let a = raise 1"), "let a = raise 1");
    assert_eq!(parse_one("let a = raise f x"), "let a = raise f x");
    assert_eq!(
        parse_one("let a = handle e with | !Log.write s => raise 0 end"),
        "let a = handle e with | !Log.write s => raise 0 end"
    );
}

/// An effect heads an operation reference and nothing else heads one, so the
/// `.` after it reads a name that is never a field: the two productions cannot
/// swallow each other, and neither needs lookahead to be told from the other.
#[test]
fn an_operation_is_told_from_a_projection_by_the_sigil() {
    assert_eq!(parse_one("let a = !Log.write"), "let a = !Log.write");
    assert_eq!(parse_one("let a = base.field"), "let a = base.field");
    // An operation is a value like any other, so it applies and is applied.
    assert_eq!(parse_one("let a = !Log.write 1"), "let a = !Log.write 1");
    assert_eq!(parse_one("let a = f !Log.write"), "let a = f !Log.write");
    // A projection off a case is still a projection: the sigil that heads it
    // says which row the label belongs to, so neither reading is in doubt. The
    // printed form brackets the case, which is how a tag carrying a payload is
    // told from one being read off.
    assert_eq!(parse_one("let a = #Ok.x"), "let a = (#Ok).x");
    // An effect is a value of nothing, so one written alone is refused where
    // it stands rather than lowered into a complaint about a term.
    let out = parse(lex("let a = !Log", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::Unexpected);
}

/// One statement's parsed tree, for the tests that assert about the shape
/// rather than about what it prints back as.
fn parse_stmt(src: &str) -> StmtKind {
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    let [stmt] = &out.stmts[..] else {
        panic!("{src}: {:#?}", out.stmts);
    };
    stmt.tracked.clone()
}

/// The annotation of a `let`, which every arrow-binding test writes one of.
fn arrow_of(stmt: &StmtKind) -> &TypeKind {
    let StmtKind::Let { ty: Some(ty), .. } = stmt else {
        panic!("expected an annotated let");
    };
    &ty.ty.tracked
}

/// Every position the effect grammar can be given a token it has no reading
/// for, each reported where it was written.
#[test]
fn the_effect_grammar_reports_what_it_cannot_read() {
    for (source, at) in [
        // A declaration's name is a name; a `_` is not one, and a type is
        // nothing a value could be thrown away from.
        ("effect _ = |", "_"),
        // An arm head is `!Eff.op`, so a bare name is not one: an arm heads
        // an operation, and only the sigil says which effect's.
        ("let a = handle e with | Log.write s => () end", "Log"),
        // A binder is a name or `_`, as a `fn` header's argument is.
        ("let a = handle e with | !Log.write 1 => () end", "1"),
        // The operation an arm answers is a name, and so is the one a
        // reference reads: the `.` promises one either way.
        ("let a = !Log.1", "1"),
        // An operation declares a signature, so the `:` is not optional: a
        // case with no `:` is the alias it does not look like.
        ("effect E = op | w : Nat -> ()", "|"),
        // And the signature is a type, whatever else was written there.
        ("effect E = op : ,", ","),
        // A `\` promises an effect, so anything but one after it is
        // reported — the rule a sum's absent case keeps.
        ("let f : A -> B + \\x = g", "x"),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{source}: {:#?}", out.errors);
        let start = source.find(at).expect("the source writes it");
        assert_eq!(out.errors[0].span.start, start, "{source}");
    }

    // A `|` between effects promises another one, so nothing after it is
    // reported at the bar — the leading one promises nothing.
    // A `+` with no row after it at all promises nothing: the row is empty,
    // and what follows is somebody else's token to refuse.
    let out = parse(lex("let f : A -> B + = g", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{:#?}", out.errors);

    let out = parse(lex("let f : A -> B + !Log + = g", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let bar = "let f : A -> B + !Log ".len();
    assert_eq!(out.errors[0].span.start, bar);
}

/// Running out of input where 'an arm's binder goes is reported at the end of
/// what there was, the way every other production that needs more reports it.
#[test]
fn a_handler_arm_that_runs_out_reports_where_the_input_ended() {
    let source = "let a = handle e with | !Log.write";
    let out = parse(lex(source, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::Unexpected);
    assert_eq!(out.errors[0].span.start, source.len());
}
