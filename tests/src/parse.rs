//! Tests for [`ruddy::parse`].

use std::fmt::{self, Write};

use ruddy::{
    parse::{
        DataKind, EffectBody, EffectLabel, ErrorKind, Expected, ExprKind, ExternTypeKind, Found,
        Path, PatternKind, Place, Related, RelatedKind, StmtKind, SumCase, Type, TypeField,
        TypeKind, parse,
    },
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
    assert!(matches!(out.stmts[0].kind, StmtKind::Let { .. }));
    assert!(matches!(out.stmts[1].kind, StmtKind::Type { .. }));
    assert!(matches!(out.stmts[2].kind, StmtKind::Let { .. }));
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
    print::ast::stmt(&out.stmts[0]).to_string()
}

#[test]
fn parses_expression_pattern_and_type_tuples() {
    let out = parse(
        lex(
            "let (a, b,) : (A, B,) = (f x, y,)\nlet grouped = (x)\nlet one = (x,)",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    assert_eq!(out.stmts.len(), 3);

    let StmtKind::Let {
        pattern,
        ty: Some(ty),
        body,
    } = &out.stmts[0].kind
    else {
        panic!("expected ascribed tuple let: {:#?}", out.stmts[0]);
    };
    assert!(matches!(&pattern.tracked, PatternKind::Tuple(items) if items.len() == 2));
    assert!(matches!(&ty.ty.tracked, TypeKind::Tuple(items) if items.len() == 2));
    assert!(matches!(&body.tracked.tracked, ExprKind::Tuple(items) if items.len() == 2));

    let StmtKind::Let { body, .. } = &out.stmts[1].kind else {
        unreachable!()
    };
    assert!(matches!(&body.tracked.tracked, ExprKind::Ident { .. }));
    let StmtKind::Let { body, .. } = &out.stmts[2].kind else {
        unreachable!()
    };
    assert!(matches!(&body.tracked.tracked, ExprKind::Tuple(items) if items.len() == 1));
}

#[test]
fn parses_homogeneous_array_literals_and_types() {
    let out = parse(
        lex(
            "let empty : [Real] = []\nlet one = [1]\nlet many = [1, 2, 3,]",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    assert_eq!(out.stmts.len(), 3);

    let StmtKind::Let {
        ty: Some(ty), body, ..
    } = &out.stmts[0].kind
    else {
        panic!("expected an ascribed array: {:#?}", out.stmts[0]);
    };
    assert!(
        matches!(&ty.ty.tracked, TypeKind::Array(element) if matches!(element.tracked, TypeKind::Ident { .. }))
    );
    assert!(matches!(&body.tracked.tracked, ExprKind::Array(items) if items.is_empty()));

    let StmtKind::Let { body, .. } = &out.stmts[1].kind else {
        unreachable!()
    };
    assert!(matches!(&body.tracked.tracked, ExprKind::Array(items) if items.len() == 1));

    let StmtKind::Let { body, .. } = &out.stmts[2].kind else {
        unreachable!()
    };
    assert!(matches!(&body.tracked.tracked, ExprKind::Array(items) if items.len() == 3));

    assert_eq!(
        parse_one("let values : [Real] = [1, 2, 3,]"),
        "let values : [Real] = [1, 2, 3]"
    );
}

#[test]
fn parses_array_patterns_with_one_rest_anywhere() {
    let out = parse(
        lex(
            "let f = fn v => match v with\n\
             | [] => 0\n\
             | [x] => 1\n\
             | [x, ..] => 2\n\
             | [.., last] => 3\n\
             | [first, ..middle, last,] => 4\n\
             | [#Some x, [y, ..], ..] => 5\n\
             end",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    let StmtKind::Let { body, .. } = &out.stmts[0].kind else {
        panic!("expected a let: {:#?}", out.stmts[0]);
    };
    let ExprKind::Function { body, .. } = &body.tracked.tracked else {
        panic!("expected a function: {:#?}", body);
    };
    let ExprKind::Match { arms, .. } = &body.tracked else {
        panic!("expected a match: {:#?}", body);
    };
    let shapes: Vec<(usize, Option<Option<&str>>, usize)> = arms
        .iter()
        .map(|arm| match &arm.pattern.tracked {
            PatternKind::Array {
                before,
                rest,
                after,
            } => (
                before.len(),
                rest.as_ref()
                    .map(|rest| rest.name.as_ref().map(|name| name.tracked.as_str())),
                after.len(),
            ),
            other => panic!("expected an array pattern: {other:?}"),
        })
        .collect();
    assert_eq!(
        shapes,
        [
            (0, None, 0),
            (1, None, 0),
            (1, Some(None), 0),
            (0, Some(None), 1),
            (1, Some(Some("middle")), 1),
            (2, Some(None), 0),
        ]
    );
    let PatternKind::Array { before, .. } = &arms[5].pattern.tracked else {
        unreachable!()
    };
    assert!(matches!(before[0].tracked, PatternKind::Tag { .. }));
    assert!(matches!(
        &before[1].tracked,
        PatternKind::Array { before, rest: Some(rest), after }
            if before.len() == 1 && rest.name.is_none() && after.is_empty()
    ));

    assert_eq!(
        parse_one("let [first, ..middle, last,] = values"),
        "let [first, ..middle, last] = values"
    );
    assert_eq!(parse_one("let [x, ..] = values"), "let [x, ..] = values");
    assert_eq!(parse_one("let [.., x] = values"), "let [.., x] = values");
    assert_eq!(parse_one("let [..] = values"), "let [..] = values");
    assert_eq!(parse_one("let [] = values"), "let [] = values");
    assert_eq!(
        parse_one("let [#Some (a, b), [..inner]] = values"),
        "let [#Some (a, b), [..inner]] = values"
    );
}

#[test]
fn an_array_pattern_allows_only_one_rest() {
    let out = parse(lex("let [..a, ..b] = values", FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert_eq!(error.span.start, 10);
    assert!(matches!(
        error.kind,
        ErrorKind::SecondArrayRest { previous } if previous.start == 5
    ));
}

#[test]
fn an_unclosed_array_pattern_is_reported_at_its_opener() {
    for src in ["let [..", "let [.. ", "let [..rest"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{src}: expected one error: {:#?}", out.errors);
        };
        assert!(
            matches!(
                error.kind,
                ErrorKind::Expected {
                    expected: Expected::Punctuation("]"),
                    ..
                }
            ),
            "{src}: {error:#?}"
        );
    }
}

#[test]
fn an_array_rest_is_discarded_by_writing_it_bare() {
    let out = parse(lex("let [x, .._] = values", FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert_eq!(error.kind, ErrorKind::DiscardedArrayRest);
    assert_eq!((error.span.start, error.span.end()), (8, 11));
}

#[test]
fn parses_array_spreads_anywhere_in_a_literal() {
    let out = parse(
        lex(
            "let joined = [..a, 1, ..b, ..f x,]\nlet copy = [..a]",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    let StmtKind::Let { body, .. } = &out.stmts[0].kind else {
        unreachable!()
    };
    let ExprKind::Array(items) = &body.tracked.tracked else {
        panic!("expected an array: {:#?}", body);
    };
    let spreads: Vec<bool> = items.iter().map(|item| item.spread.is_some()).collect();
    assert_eq!(spreads, [true, false, true, true]);
    assert!(matches!(items[3].value.tracked, ExprKind::Apply { .. }));

    assert_eq!(
        parse_one("let joined = [..a, 1, ..b, ..f x,]"),
        "let joined = [..a, 1, ..b, ..f x]"
    );
    assert_eq!(parse_one("let copy = [..a]"), "let copy = [..a]");
}

/// A struct literal spreads one value after the fields it names: on its own,
/// after any number of fields — quoted and numeric labels included — as a
/// whole expression, nested, and with or without the trailing comma the
/// field list has always allowed. It prints back as the `..` it was written
/// as, with braces whatever the fields are named: the tuple and unit
/// spellings would say the fields written are all there are.
#[test]
fn parses_a_struct_spread_after_its_fields() {
    let out = parse(
        lex(
            "let ext = { a: 1, \"b c\": 2, 0: 3, ..f x, }\nlet copy = { ..c }",
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    let StmtKind::Let { body, .. } = &out.stmts[0].kind else {
        unreachable!()
    };
    let ExprKind::Struct {
        fields,
        spread: Some(spread),
    } = &body.tracked.tracked
    else {
        panic!("expected a spread struct: {:#?}", body);
    };
    let names: Vec<&str> = fields.keys().map(|name| name.tracked.as_str()).collect();
    assert_eq!(names, ["a", "b c", "0"]);
    assert!(matches!(spread.value.tracked, ExprKind::Apply { .. }));
    assert_eq!((spread.span.start, spread.span.end()), (34, 36));
    let StmtKind::Let { body, .. } = &out.stmts[1].kind else {
        unreachable!()
    };
    let ExprKind::Struct {
        fields,
        spread: Some(spread),
    } = &body.tracked.tracked
    else {
        panic!("expected a spread struct: {:#?}", body);
    };
    assert!(fields.is_empty());
    assert!(matches!(spread.value.tracked, ExprKind::Ident { .. }));

    assert_eq!(
        parse_one("let ext = { a: 1, \"b c\": 2, 0: 3, ..f x, }"),
        "let ext = { a: 1, \"b c\": 2, 0: 3, ..f x }"
    );
    assert_eq!(parse_one("let copy = { ..c, }"), "let copy = { ..c }");
    assert_eq!(
        parse_one("let nested = { inner: { x: 1, ..c }, ..{ y: 2, ..d } }"),
        "let nested = { inner: { x: 1, ..c }, ..{ y: 2, ..d } }"
    );
    assert_eq!(
        parse_one("let pair = { 0: 1, 1: 2, ..c }"),
        "let pair = { 0: 1, 1: 2, ..c }"
    );
    assert_eq!(parse_one("let unit = { ..() }"), "let unit = { ..() }");
    assert_eq!(
        parse_one("let chosen = { x: 1, ..match c with | v => v end }"),
        "let chosen = { x: 1, ..match c with | v => v end }"
    );
}

/// A struct literal gets one spread, and it comes last: a second `..` and a
/// field after the `..` are each refused where they begin, pointing back at
/// the `..` they conflict with, and told apart so the reader knows which of
/// the two rules they broke.
#[test]
fn a_struct_literal_spreads_one_value_and_spreads_it_last() {
    let out = parse(lex("let bad = { ..a, ..b }", FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert_eq!((error.span.start, error.span.end()), (17, 19));
    assert!(matches!(
        error.kind,
        ErrorKind::SecondStructSpread { previous } if previous.start == 12
    ));

    let out = parse(lex("let bad = { x: 1, ..a, y: 2 }", FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert_eq!((error.span.start, error.span.end()), (23, 24));
    assert!(matches!(
        error.kind,
        ErrorKind::FieldAfterSpread { spread } if spread.start == 18
    ));

    // A quoted or numeric label after the `..` is the same complaint, at the
    // label; so is a `_`, which is refused as a field after the spread
    // before it can be refused as a field named nothing.
    for src in [
        "let bad = { ..a, \"q\": 1 }",
        "let bad = { ..a, 0: 1 }",
        "let bad = { ..a, _: 1 }",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{src}: expected one error: {:#?}", out.errors);
        };
        assert_eq!(error.span.start, 17, "{src}");
        assert!(
            matches!(error.kind, ErrorKind::FieldAfterSpread { spread } if spread.start == 12),
            "{src}: {:?}",
            error.kind
        );
    }
}

#[test]
fn parses_numeric_projection_canonically() {
    let out = parse(lex("let value = pair.001", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    let StmtKind::Let { body, .. } = &out.stmts[0].kind else {
        unreachable!()
    };
    assert!(matches!(
        &body.tracked.tracked,
        ExprKind::Project { field, .. } if field.tracked == "1"
    ));
}

#[test]
fn malformed_tuple_elements_are_reported() {
    for src in ["let x = (,)", "let x = (a,, b)", "let x = (a,"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "expected an error for {src:?}");
    }
}

#[test]
fn pipeline_is_left_associative_and_looser_than_arithmetic() {
    assert_eq!(
        parse_one("let value = 1 + 2 |> f |> g"),
        "let value = 1 + 2 |> f |> g"
    );
    assert_eq!(
        parse_one("let value = 1 |> fn x => x"),
        "let value = 1 |> (fn x => x)"
    );
}

#[test]
fn real_number_operators_have_the_usual_precedence() {
    assert_eq!(
        parse_one("let value = -1 + 2 * 3 - 4 / 5"),
        "let value = -1 + 2 * 3 - 4 / 5"
    );
    assert_eq!(parse_one("let value = -(1 + 2)"), "let value = -(1 + 2)");
}

#[test]
fn boolean_operators_have_the_usual_precedence() {
    assert_eq!(
        parse_one("let value = not true and false xor true or false"),
        "let value = not true and false xor true or false"
    );
    assert_eq!(
        parse_one("let value = not (true or false)"),
        "let value = not (true or false)"
    );
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

/// `when` and `where` are contextual, but Boolean operators are reserved
/// wherever an expression may appear.
#[test]
fn only_non_operator_clause_keywords_are_contextual() {
    for src in [
        "let when = 1n",
        "let where = 1n",
        "let x = fn when => when",
        "let x = { when: 1n, where: 2n }",
        "let x : { when: A, where: B } = y",
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
        ("let x : { a when 1n: A } = y", "1n: A } = y"),
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

/// A `;` promises another condition, so a trailing one asks specifically for
/// the condition that is missing.
#[test]
fn a_trailing_separator_requires_another_condition() {
    for src in [
        "type T = { x when 'a: A } where 'a;",
        "type T = { x when 'a: A } where ",
        "let f : { x when 'a: A } where 'a; = z",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{src:?}: expected one error: {:#?}", out.errors);
        };
        assert!(
            matches!(
                error.kind,
                ErrorKind::Expected {
                    expected: Expected::Clause,
                    ..
                }
            ),
            "{src}: {error:#?}"
        );
    }
}

/// `_` in a type is the hole — a position left for inference — so it parses
/// wherever a type does, as often as one likes, and prints back as itself.
#[test]
fn a_type_hole_parses_as_a_type() {
    for src in [
        "let k : _ -> Nat = fn x => 0n",
        "let k : _ -> _ = fn x => x",
        "let k : { x: _, y: _ } -> Nat = fn p => 0n",
        "let k : #Some _ | #None = nothing",
        "let k : List _ = nil",
    ] {
        assert_eq!(parse_one(src), src);
    }

    // And it is the node it says it is, rather than a name spelled `_`.
    let out = parse(lex("let k : _ = 1n", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Let { ty, .. } = &out.stmts[0].kind else {
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
        match &out.stmts[0].kind {
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
    // Every numeric literal can be passed as an argument, not only a natural.
    assert_eq!(
        parse_one("let values = f 1n 2i 3"),
        "let values = f 1n 2i 3"
    );
}

#[test]
fn match_function_shorthand_parses_and_prints() {
    assert_eq!(
        parse_one(
            "let unwrap = fn\n\
             | #Some x => x\n\
             | #None => 0n"
        ),
        "let unwrap = fn | #Some x => x | #None => 0n"
    );

    let src = "let identity = fn | value => value";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "errors: {:#?}", out.errors);
    let StmtKind::Let { body, .. } = &out.stmts[0].kind else {
        panic!("expected a let");
    };
    let ExprKind::MatchFunction { fn_span, arms } = &body.tracked.tracked else {
        panic!("expected a match function");
    };
    assert_eq!(fn_span.start, src.find("fn").expect("the keyword"));
    assert_eq!(fn_span.width, 2);
    assert_eq!(arms.len(), 1);
}

#[test]
fn match_function_shorthand_has_function_precedence_and_greedy_arms() {
    assert_eq!(
        parse_one("let mapped = map fn | #Some x => x | #None => 0n"),
        "let mapped = map (fn | #Some x => x | #None => 0n)"
    );
    assert_eq!(
        parse_one("let identity = (fn | x => x) 1n"),
        "let identity = (fn | x => x) 1n"
    );
    assert_eq!(
        parse_one(
            "let nested = fn\n\
             | #Outer x => (fn | #InnerA => 1n | #InnerB => 2n)\n\
             | #Other => 3n"
        ),
        "let nested = fn | #Outer x => (fn | #InnerA => 1n | #InnerB => 2n) | #Other => 3n"
    );
    assert_eq!(
        parse_one(
            "let nested = fn\n\
             | #Outer => (fn x => fn | #Inner => 1n)\n\
             | #Other => 2n"
        ),
        "let nested = fn | #Outer => (fn x => fn | #Inner => 1n) | #Other => 2n"
    );
}

#[test]
fn malformed_match_function_shorthand_uses_existing_parse_errors() {
    for (src, expected) in [
        ("let f = fn |", Expected::Pattern),
        ("let f = fn | value", Expected::Punctuation("=>")),
        ("let f = fn | value =>", Expected::Value),
        ("let f = fn value | _ => 0n", Expected::Punctuation("=>")),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{src}: expected one error: {:#?}", out.errors);
        };
        let ErrorKind::Expected {
            expected: actual, ..
        } = error.kind
        else {
            panic!("{src}: wrong error: {error:#?}");
        };
        assert_eq!(actual, expected, "{src}");
    }
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
    assert_eq!(parse_one("let n = 0n"), "let n = 0n");
    // An atom, so it takes part in application on either side and never
    // needs parentheses of its own.
    assert_eq!(parse_one("let a = f 1n 2n"), "let a = f 1n 2n");
    assert_eq!(parse_one("let b = 1n f"), "let b = 1n f");
    assert_eq!(parse_one("let c = f (g 3n)"), "let c = f (g 3n)");
    assert_eq!(parse_one("let d = fn x => 7n"), "let d = fn x => 7n");
    assert_eq!(
        parse_one("let e = { width: 3n, height: 4n }"),
        "let e = { width: 3n, height: 4n }"
    );
}

#[test]
fn naturals_are_terms_only() {
    // There is no type-level natural, so a literal in type position is the
    // unexpected token it looks like rather than a silently accepted one.
    let out = parse(lex("type T = 42n", FileID::GENERATED).tokens);
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

/// A closing delimiter belongs to the innermost delimiter still open. The
/// complaint names that opener's matching mark regardless of whether the
/// parser was reading a value, pattern, type, field, or foreign parameter.
#[test]
fn a_mismatched_closer_points_to_the_innermost_opener() {
    for (source, expected, opener, found) in [
        ("let value = (}", ")", '(', '}'),
        ("let value : (} = source", ")", '(', '}'),
        ("let {) = value", "}", '{', ')'),
        ("type T = {)", "}", '{', ')'),
        ("effect E = {)", "}", '{', ')'),
        ("extern f : fn(} -> Nat = \"host.f\"", ")", '(', '}'),
        // Each shape nested in the other: the innermost unmatched opener wins.
        ("let value = { field: ( }", ")", '(', '}'),
        ("let value = ({ field: item )", "}", '{', ')'),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{source:?}: expected one error: {:#?}", out.errors);
        };
        let ErrorKind::Expected {
            expected: Expected::Punctuation(actual),
            found: Found::Token,
            related: Some(related),
            ..
        } = error.kind
        else {
            panic!("{source:?}: wrong error: {error:#?}");
        };
        assert_eq!(actual, expected, "{source:?}");
        assert_eq!(related.kind, RelatedKind::Opener, "{source:?}");
        assert_eq!(
            related.span.start,
            source.rfind(opener).unwrap(),
            "{source:?}"
        );
        assert_eq!(error.span.start, source.rfind(found).unwrap(), "{source:?}");
    }
}

/// A closer that matched nothing spends the opener it was diagnosed against.
/// The spent opener must not hijack the diagnosis of a stray closer in a
/// later statement, which has no opener of its own.
#[test]
fn a_diagnosed_opener_does_not_hijack_a_later_stray_closer() {
    let source = "let a = (1}\nlet c = 3}";
    let out = parse(lex(source, FileID::GENERATED).tokens);
    let opener = source.find('(').unwrap();
    // The first complaint is the line-1 mismatch, pointing at its own opener.
    let first = out.errors.first().expect("the mismatch is diagnosed");
    assert!(
        matches!(
            first.kind,
            ErrorKind::Expected {
                expected: Expected::Punctuation(")"),
                related: Some(related),
                ..
            } if related.span.start == opener
        ),
        "first error: {first:#?}"
    );
    assert!(
        out.errors.len() > 1,
        "the stray closer on the second line is diagnosed too: {:#?}",
        out.errors
    );
    // Every later complaint is about its own statement, not the spent opener.
    for error in &out.errors[1..] {
        let hijacked = matches!(
            error.kind,
            ErrorKind::Expected {
                related: Some(related),
                ..
            } if related.span.start == opener
        );
        assert!(!hijacked, "spent opener reused: {error:#?}");
    }
}

/// Empty `()` and `{}` are complete values, patterns, and types. If the token
/// after an opener belongs to the surrounding construct, the missing part is
/// therefore the closer—not imaginary content before that token.
#[test]
fn a_delimiter_boundary_asks_for_the_matching_closer() {
    for (source, expected, opener, boundary) in [
        ("let value : ( = source", ")", '(', "="),
        ("let value : { = source", "}", '{', "="),
        ("let ( = value", ")", '(', "="),
        ("let { = value", "}", '{', "="),
        (
            "let value = do let inner = ( return inner end",
            ")",
            '(',
            "return inner end",
        ),
        (
            "let value = do let inner = { return inner end",
            "}",
            '{',
            "return inner end",
        ),
        ("extern f : fn( -> Nat = \"host.f\"", ")", '(', "->"),
        // A trailing comma is legal; at the boundary it is the closer, not
        // another tuple element or field, which is absent.
        ("let value : (Nat, = source", ")", '(', "="),
        ("let value : { field: Nat, = source", "}", '{', "="),
        ("let (item, = value", ")", '(', "="),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        let [error] = out.errors.as_slice() else {
            panic!("{source:?}: expected one error: {:#?}", out.errors);
        };
        let ErrorKind::Expected {
            expected: Expected::Punctuation(actual),
            found: Found::Token,
            related: Some(related),
            ..
        } = error.kind
        else {
            panic!("{source:?}: wrong error: {error:#?}");
        };
        assert_eq!(actual, expected, "{source:?}");
        assert_eq!(related.kind, RelatedKind::Opener, "{source:?}");
        assert_eq!(
            related.span.start,
            source.rfind(opener).unwrap(),
            "{source:?}"
        );
        assert_eq!(
            error.span.start,
            source.rfind(boundary).unwrap(),
            "{source:?}"
        );
    }
}

/// A matching closer does not hide genuinely missing content, and an invalid
/// token inside a delimiter is not mistaken for syntax outside it.
#[test]
fn delimiter_diagnostics_preserve_missing_content() {
    for (source, expected) in [
        ("let value = (,)", Expected::Value),
        ("let (,) = value", Expected::Pattern),
        ("let value : (42n) = source", Expected::Type),
        ("let value = { field: }", Expected::Value),
        ("let value : { field: = source }", Expected::Type),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        let Some(error) = out.errors.first() else {
            panic!("{source:?}: expected an error");
        };
        assert!(
            matches!(
                error.kind,
                ErrorKind::Expected {
                    expected: actual,
                    ..
                } if actual == expected
            ),
            "{source:?}: {:#?}",
            out.errors
        );
    }
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

/// String literals are labels in every field position. Printing keeps only
/// labels that are valid ordinary field names bare, and the printed tree is
/// reparsed by `parse_one`, so these also pin the canonical fixed point.
#[test]
fn quoted_field_labels_parse_everywhere_and_print_canonically() {
    for (src, printed) in [
        (
            r###"let v = {"field name": x, "plain": y, "let": z, "if": a, "then": b, "else": c}"###,
            r###"let v = { "field name": x, plain: y, "let": z, "if": a, "then": b, "else": c }"###,
        ),
        (
            r###"let v = record."field name"."plain""###,
            r###"let v = record."field name".plain"###,
        ),
        (
            r###"let x : { "field name" when 'p: Nat, "plain": Nat, \"gone field", .. } = y"###,
            r###"let x : { "field name" when 'p: Nat, plain: Nat, \"gone field", .. } = y"###,
        ),
        (
            r###"let v = match x with | { "field name": y, "plain": z } => y end"###,
            r###"let v = match x with | { "field name": y, plain: z } => y end"###,
        ),
        // Decoding happens before label identity and rendering. Unicode is a
        // valid identifier start, while punctuation and the empty label are not.
        (
            r###"let v = {"λ": x, "line\nname": y, "": z}"###,
            "let v = { λ: x, \"line\\nname\": y, \"\": z }",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
}

#[test]
fn bare_numeric_field_labels_parse_canonically_everywhere() {
    for (src, printed) in [
        ("let v = { 001: x, 3: 4 }", "let v = { 1: x, 3: 4 }"),
        (
            "let v = match x with | { 001: y, 2: z } => y end",
            "let v = match x with | { 1: y, 2: z } => y end",
        ),
        (
            "let x : { 001 when 'p: Nat, \\02, 3: Nat } = y",
            "let x : { 1 when 'p: Nat, \\2, 3: Nat } = y",
        ),
        // A quoted canonical numeric label now has a shorter bare spelling,
        // but a leading-zero string cannot be printed bare without changing
        // its identity through numeric canonicalization.
        (
            "let v = { \"1\": x, \"001\": y }",
            "let v = { 1: x, \"001\": y }",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
}

#[test]
fn a_numeric_pattern_field_requires_a_colon() {
    for src in [
        "let v = match x with | { 0 } => x end",
        "let v = match x with | { 0, rest: y } => y end",
        "let v = match x with | { 0: } => x end",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }
}

/// Unlike an identifier field, a quoted pattern label cannot pun: no source
/// binding can have the arbitrary decoded spelling, so `:` and a subpattern
/// are required. Identifier puns remain unchanged.
#[test]
fn a_quoted_pattern_field_requires_a_colon() {
    assert_eq!(
        parse_one("let v = match x with | { plain } => plain end"),
        "let v = match x with | { plain } => plain end"
    );

    for src in [
        r###"let v = match x with | { "plain" } => x end"###,
        r###"let v = match x with | { "field name", rest: y } => y end"###,
        r###"let v = match x with | { "field name": } => x end"###,
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }
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
        "let a = 1n ..'x",
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
fn an_operator_points_to_the_value_it_requires() {
    let src = "let x = 1n +";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        error.kind,
        ErrorKind::Expected {
            expected: Expected::Value,
            found: Found::End,
            related: Some(related),
        ..
        } if related.kind == RelatedKind::Operator
    ));
    assert_eq!(error.span.start, src.len());
}

/// An invalid token has already been explained by the lexer. Its placeholder
/// lets parsing abandon the construct without issuing a redundant expectation.
#[test]
fn an_invalid_token_does_not_cascade_into_a_parse_error() {
    let lexed = lex("let x = @", FileID::GENERATED);
    assert_eq!(lexed.errors.len(), 1, "{:#?}", lexed.errors);
    let out = parse(lexed.tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(out.stmts.is_empty(), "{:#?}", out.stmts);
}

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
        parse_one("type Pair 'A 'B = { a: 'A, b: 'B }"),
        "type Pair 'A 'B = { a: 'A, b: 'B }"
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
        parse_one("type Option 'T = #Some 'T | #None"),
        "type Option 'T = #Some 'T | #None"
    );
    // The leading `|` is accepted and dropped.
    assert_eq!(
        parse_one("type Option 'T = | #Some 'T | #None"),
        "type Option 'T = #Some 'T | #None"
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
    assert_eq!(parse_one("type Only 'r = | ..'r"), "type Only 'r = | ..'r");
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
    for (src, found) in [
        ("let x : #A Nat | = 1n", Found::Token),
        ("let x : #A Nat | | #B = 1n", Found::Token),
        ("type T = #A | ", Found::End),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        let first = out.errors.first().unwrap_or_else(|| panic!("{src}"));
        let separator = src.find('|').expect("the source writes the separator");
        assert!(
            matches!(
                first.kind,
                ErrorKind::Expected {
                    expected: Expected::Case,
                    found: actual,
                    related: Some(related),
                ..
                } if actual == found
                    && related.kind == RelatedKind::Separator
                    && related.span.start == separator
            ),
            "{src}: {:#?}",
            out.errors
        );
        let expected_start = separator + if found == Found::Token { 2 } else { 1 };
        assert_eq!(first.span.start, expected_start, "{src}: {:#?}", out.errors);
        assert_eq!(first.span.width, usize::from(found == Found::Token));
    }

    // The leading bar promises nothing, and neither does a case with no bar
    // after it.
    for src in ["type Void = |", "type Only 'r = | ..'r", "type T = #A | #B"] {
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
        parse_one("type Tagged 'r = #Err Nat | ..'r"),
        "type Tagged 'r = #Err Nat | ..'r"
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
        "type NoErr 'r = #Ok Nat | \\#Err | ..'r",
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
        ("let x : { \\y: Nat, .. } = 1n", ": Nat, .. } = 1n"),
        ("let x : { \\y when a, .. } = 1n", "when a, .. } = 1n"),
        ("let x : \\#B (when 'a) | .. = 1n", "(when 'a) | .. = 1n"),
        ("let x : #A | \\#B Nat | .. = 1n", "Nat | .. = 1n"),
        ("let x : { \\, .. } = 1n", ", .. } = 1n"),
        // In a type, what follows a `\` must be a tag or a field name.
        ("let x : \\= 1n", "= 1n"),
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
/// Quoted tags occupy every place a bare tag does. They decode to the same
/// case names, and printing chooses a bare spelling exactly when the decoded
/// label satisfies the tag-name lexical rule.
#[test]
fn quoted_tags_parse_everywhere_and_print_canonically() {
    for (src, printed) in [
        (
            r###"let v = #"Some case" 1n"###,
            r###"let v = #"Some case" 1n"###,
        ),
        (r###"let v = #"Some""###, "let v = #Some"),
        (
            r###"let v = match x with | #"Some case" y => y | #"None" => 0n end"###,
            r###"let v = match x with | #"Some case" y => y | #None => 0n end"###,
        ),
        (
            r###"let x : #"Some case" Nat | #"None" | \#"gone case" | .. = y"###,
            r###"let x : #"Some case" Nat | #None | \#"gone case" | .. = y"###,
        ),
        (r###"let v = #"""###, r###"let v = #"""###),
        (r###"let v = #"λ""###, "let v = #λ"),
        (r###"let v = #"line\ncase""###, "let v = #\"line\\ncase\""),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
}

#[test]
fn a_tag_binds_tighter_than_application() {
    assert_eq!(parse_one("let v = #Some 1n"), "let v = #Some 1n");
    assert_eq!(parse_one("let v = #None"), "let v = #None");
    assert_eq!(parse_one("let v = f #A 1n"), "let v = f (#A 1n)");
    // Which is why a bare tag written as an argument comes back in
    // parentheses: nothing follows it here, but the printer decides one node
    // at a time, and a second argument after it would be read as the payload
    // it has not got.
    assert_eq!(parse_one("let v = f #A"), "let v = f (#A)");
    // The same rule at the head of an application, where 'leaving them off
    // would turn the argument into a payload and print the tree above.
    assert_eq!(parse_one("let v = f (#A) 1n"), "let v = f (#A) 1n");
    assert_eq!(parse_one("let v = (#A) 1n"), "let v = (#A) 1n");
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
        parse_one("type List 'a = #Nil | #Cons { head: 'a, tail: List 'a }"),
        "type List 'a = #Nil | #Cons { head: 'a, tail: List 'a }"
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
    // An effect's `=` commits to a case list, but keeps the empty declaration
    // for lowering after reporting a token that cannot begin a case.
    let out = parse(lex("effect E = {}", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(out.stmts.len(), 1, "{:#?}", out.stmts);

    for src in [
        // Declaration and path heads, including each dotted/path-qualified
        // production after it has committed to reading one.
        "module",
        "effect",
        "extern",
        "extern x :",
        "extern x : Nat =",
        "let x = A::",
        "type T = A::",
        // A definition's own name, and the two halves of an ascription.
        "let = 1n",
        "let x : = 1n",
        "let x : Nat",
        // Each expression operator after it has consumed the operator.
        "let x = y |>",
        "let x = 1n +",
        "let x = -",
        // An application's argument: the token begins an atom, and the atom
        // still does not parse.
        "let v = f {",
        // Inside a struct term: the colon, the value, and the brace.
        "let v = { x 2n }",
        "let v = { x: }",
        "let v = { x: 1n",
        // A case's payload, which is an atom taken greedily.
        "let v = #A {",
        // Pattern atoms, nested pattern payloads, and delimiters.
        "let #A ( = value",
        "let { 1 } = value",
        "let { field: } = value",
        "let ( = value",
        "let (name = value",
        // Parentheses, around an expression that is not one and around one
        // that is never closed.
        "let v = (let)",
        "let v = (1n",
        // A function's arrow and its body.
        "let f = fn x y",
        "let f = fn x =>",
        // The type language keeps the same positions: a case's payload, an
        // argument, a field's label, a field's type, and the parentheses.
        "type X = #A {",
        "type X = Box {",
        "type X = { a: }",
        "type X = (let)",
        "type X = (Nat",
        "type X = #A (when)",
        "let x : !E (when) = value",
        // Boolean-clause operators and grouping after each has committed.
        "let x : Nat where ; = value",
        "type X = Nat where 'a or",
        "type X = Nat where 'a and",
        "type X = Nat where not",
        "type X = Nat where ('a",
        // Match, handler, arm, and raise positions.
        "let x = match with end",
        "let x = match value end",
        "let x = handle with end",
        "let x = handle value end",
        "let x = handle value with",
        "let x = handle value with !E end",
        "let x = handle value with !E. end",
        "let x = handle value with return arg end",
        "let x = handle value with return arg =>",
        "let x = handle value with return arg => result",
        "let x = raise",
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

/// Path display must propagate failures from the caller's writer.
#[test]
fn paths_propagate_formatter_failures() {
    struct FailsAfter(usize);

    impl Write for FailsAfter {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            if self.0 == 0 {
                return Err(fmt::Error);
            }
            self.0 -= 1;
            Ok(())
        }
    }

    let segment = |name: &str| {
        FileID::GENERATED
            .span(0, name.len())
            .track(name.to_string())
    };
    let qualified = Path {
        modules: vec![segment("Module")],
        name: segment("value"),
    };
    assert!(write!(&mut FailsAfter(0), "{qualified}").is_err());
}

/// `do <let>* [return <expr>] end` is an expression, and it prints back as
/// what it was written as. Each `let` is the statement grammar, so a
/// definition and a binding inside a block read alike.
#[test]
fn a_do_block_is_an_expression() {
    for src in [
        "let a = do let x = 1n return x end",
        "let a = do let x : Nat = 1n return x end",
        "let a = do let f : { x: Nat } -> Nat = fn p => p.x return f end",
        "let a = do let x = 1n let y = { v: x } return y end",
        "let a = do let x = 1n end",
        "let a = do end",
        "let a = do return 1n end",
        "let a = do let (x, y) = p return x end",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// A `return`'s value extends as far right as it can, ML-style, exactly as
/// a `fn` body does — so the application is inside the `return` and the block
/// ends at the `end`. A `let`'s value stops in front of the next `let` or
/// `return`, since neither begins an atom.
#[test]
fn a_blocks_values_run_as_far_right_as_they_can() {
    assert_eq!(
        parse_one("let a = do let f = g return f x y end"),
        "let a = do let f = g return f x y end"
    );
    assert_eq!(
        parse_one("let a = do let f = g x let h = f y return h end"),
        "let a = do let f = g x let h = f y return h end"
    );
    assert_eq!(
        parse_one("let a = do let f = g x return f end"),
        "let a = do let f = g x return f end"
    );
}

/// A block may start an expression anywhere an atom may, and, like a `match`,
/// it may head an application or be projected from.
#[test]
fn a_do_block_starts_an_expression_wherever_an_atom_does() {
    for (src, printed) in [
        // A `fn` body.
        (
            "let a = fn p => do let x = p return x end",
            "let a = fn p => do let x = p return x end",
        ),
        // A struct field's value.
        (
            "let a = { v: do let n = 1n return n end }",
            "let a = { v: do let n = 1n return n end }",
        ),
        // A parenthesized expression.
        (
            "let a = (do let n = 1n return n end)",
            "let a = do let n = 1n return n end",
        ),
        // Another block's value, and another block's return.
        (
            "let a = do let x = do let y = 1n return y end return x end",
            "let a = do let x = do let y = 1n return y end return x end",
        ),
        (
            "let a = do let x = 1n return do let y = x return y end end",
            "let a = do let x = 1n return do let y = x return y end end",
        ),
        // Heading an application, and projected from.
        (
            "let a = do let f = g return f end 1n",
            "let a = do let f = g return f end 1n",
        ),
        (
            "let a = do let p = q return p end.x",
            "let a = (do let p = q return p end).x",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src}");
    }
}

/// A block is not an application argument: the loop that gathers arguments
/// stops in front of one, so the parentheses in `f (do ... end)` are the way
/// to pass one — and the printer writes them back.
#[test]
fn a_do_block_is_not_an_application_argument() {
    assert_eq!(
        parse_one("let a = f (do let x = 1n return x end)"),
        "let a = f (do let x = 1n return x end)"
    );

    // Without them the application ends at `f`, and the `do` is the token
    // nothing can use.
    let src = "let a = f do let x = 1n return x end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    assert_eq!(
        out.errors[0].span.start,
        src.find(" do ").expect("the `do`") + 1
    );
}

/// `in` is a name like any other now that no form reads it.
#[test]
fn in_is_an_ordinary_name() {
    assert_eq!(parse_one("let in = 1n"), "let in = 1n");
    assert_eq!(parse_one("let a = f in"), "let a = f in");
}

/// `do` and `return` are reserved: neither is a name anywhere.
#[test]
fn do_and_return_are_not_names() {
    for src in ["let do = 1n", "let return = 1n", "let a = fn do => 1n"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    }
}

/// Every position a block can fail at, reported where it is. A broken
/// statement inside the block is skipped to the next `let` or the `end`, the
/// way a broken definition is at file level, so the block is still read
/// through to its `end` and the definitions after it are untouched.
#[test]
fn a_do_block_reports_where_it_fails() {
    // Each case is the source and the tail of it the complaint should land on,
    // which is the token the position had no reading for.
    for (src, from) in [
        // A `let`'s name, ascribed type, and `=`.
        ("let a = do let = 1n return x end", "= 1n return x end"),
        ("let a = do let x : = 1n return x end", "= 1n return x end"),
        ("let a = do let x 1n return x end", "1n return x end"),
        // A `let`'s value, and the returned value.
        ("let a = do let x = return x end", "return x end"),
        ("let a = do let x = 1n return } end", "} end"),
        // A block that closes with something other than `end`.
        ("let a = do let x = 1n return x }", "}"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}");
    }

    // A broken statement is skipped and the block read through: one complaint,
    // and the definitions on either side of it survive.
    let src = "let a = do let = 1n let x = 2n return x end\nlet b = 3n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);

    // A block with no `end` at all runs out of input, and is reported there.
    let src = "let a = do let x = 1n return x";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);
}

/// A `return` is the last thing in its block: a statement written after one
/// is refused where it begins, pointing back at the `return`, and the block
/// is read through to its `end` so the rest of the file is untouched.
#[test]
fn a_statement_after_return_is_refused() {
    let src = "let a = do return 1n let x = 2n end\nlet b = 3n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let error = &out.errors[0];
    assert_eq!(
        error.span.start,
        src.find("let x").expect("the stray `let`")
    );
    assert!(
        matches!(error.kind, ErrorKind::StatementAfterReturn { .. }),
        "{error:?}"
    );
    let ErrorKind::StatementAfterReturn { returned } = error.kind else {
        unreachable!()
    };
    assert_eq!(returned.start, src.find("return").expect("the `return`"));
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);

    // A second `return` is a statement after the first, and the same
    // complaint.
    let src = "let a = do return 1n return 2n end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::StatementAfterReturn { .. }
    ));
    assert_eq!(
        out.errors[0].span.start,
        src.rfind("return").expect("the second `return`")
    );

    // What follows is read through and dropped, complaints included: a
    // declaration after the `return` is one mistake, not two, and a nested
    // `end` inside what is skipped is not taken for the block's.
    for src in [
        "let a = do return 1n type T = Nat end\nlet b = 3n",
        "let a = do return 1n let x = match y with | _ => 1n end end\nlet b = 3n",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(out.errors[0].kind, ErrorKind::StatementAfterReturn { .. }),
            "{src:?}"
        );
        assert_eq!(out.stmts.len(), 2, "{src:?}: {:#?}", out.stmts);
    }

    // Something after the value that begins no statement is the block's
    // missing `end`, and is reported as that rather than as a stray statement.
    let src = "let a = do return x }";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(
        matches!(
            out.errors[0].kind,
            ErrorKind::Expected {
                expected: Expected::Keyword("end"),
                ..
            }
        ),
        "{:#?}",
        out.errors
    );
    assert_eq!(out.errors[0].span.start, src.find('}').expect("the brace"));

    // A `return` whose value is broken is one complaint at the value, and the
    // block is still read through to its `end`, so the definition survives.
    let src = "let a = do let x = 1n return } let y = 2n end\nlet b = 3n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.find('}').expect("the brace"));
    assert_eq!(out.stmts.len(), 2, "stmts: {:#?}", out.stmts);
}

/// A `return` carries a value: one written with nothing after it is refused
/// at the `return`, since leaving it out is how a block evaluates to `()`.
#[test]
fn a_bare_return_is_refused() {
    let src = "let a = do let x = 1n return end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(out.errors[0].kind, ErrorKind::BareReturn));
    assert_eq!(
        out.errors[0].span.start,
        src.find("return").expect("the `return`")
    );
}

/// A `return` outside any block is refused where it is, and what it carries
/// is still read so the rest of the definition is checked.
#[test]
fn a_return_outside_a_block_is_refused() {
    for src in [
        "let a = fn x => return x",
        "let a = return 1n",
        "let a = do let x = return 1n end",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert!(
            matches!(out.errors[0].kind, ErrorKind::ReturnOutsideBlock),
            "{src:?}"
        );
        assert_eq!(
            out.errors[0].span.start,
            src.find("return").expect("the `return`"),
            "{src:?}"
        );
        assert_eq!(out.stmts.len(), 1, "{src:?}: {:#?}", out.stmts);
    }
}

/// Only a `let` may be written in a block. Any other definition is refused
/// at its keyword and dropped, and the block is still read through.
#[test]
fn a_declaration_in_a_block_is_refused() {
    for (src, keyword) in [
        ("let a = do type T = Nat let x = 1n return x end", "type"),
        ("let a = do effect E = () -> () return 1n end", "effect"),
        ("let a = do module M = end return 1n end", "module"),
        ("let a = do extern f : Nat = \"f\" return 1n end", "extern"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(
            out.errors[0].kind,
            ErrorKind::DeclarationInBlock { keyword },
            "{src:?}"
        );
        assert_eq!(
            out.errors[0].span.start,
            src.find(keyword).expect("the keyword"),
            "{src:?}"
        );
        assert_eq!(out.stmts.len(), 1, "{src:?}: {:#?}", out.stmts);
    }
}

/// A conditional is a self-delimiting expression whose three positions each
/// accept a full expression. Its printer preserves the surface construct
/// rather than exposing the match used by lowering.
#[test]
fn parses_if_expressions_and_full_branches() {
    for src in [
        "let a = if true then 1n else 2n end",
        "let a = if not false and true then f 1n else do let x = 2n return x end end",
        "let a = if p then fn x => x else fn y => y end",
        "let a = { value: if p then a else b end }",
    ] {
        assert_eq!(parse_one(src), src);
    }
}

/// An ungrouped `else if` is one chain and spends one final `end`. The nested
/// node is the outer alternative, so a longer chain associates to the right.
/// A grouped conditional with its own `end` remains accepted as the explicit
/// nested spelling and canonicalizes to the chain.
#[test]
fn else_if_chains_share_the_final_end() {
    assert_eq!(
        parse_one("let a = if p then x else if q then y else z end"),
        "let a = if p then x else if q then y else z end"
    );
    assert_eq!(
        parse_one("let a = if p then w else if q then x else if r then y else z end"),
        "let a = if p then w else if q then x else if r then y else z end"
    );
    assert_eq!(
        parse_one("let a = if p then x else (if q then y else z end) end"),
        "let a = if p then x else if q then y else z end"
    );
    // A conditional in the consequent is independent and therefore closes
    // before the outer `else`.
    assert_eq!(
        parse_one("let a = if p then if q then x else y end else z end"),
        "let a = if p then if q then x else y end else z end"
    );
}

/// Like `match`, an if-expression can head application and projection, but an
/// application must parenthesize one used as its argument.
#[test]
fn an_if_has_match_expression_precedence() {
    assert_eq!(
        parse_one("let a = if p then f else g end x"),
        "let a = if p then f else g end x"
    );
    assert_eq!(
        parse_one("let a = if p then x else y end.field"),
        "let a = (if p then x else y end).field"
    );
    assert_eq!(
        parse_one("let a = f (if p then x else y end)"),
        "let a = f (if p then x else y end)"
    );

    let src = "let a = f if p then x else y end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    assert_eq!(out.errors[0].span.start, src.find("if").unwrap());
}

/// Every promised part of a conditional is mandatory. In particular there is
/// no implicit unit branch and no optional `else`.
#[test]
fn an_if_reports_each_missing_part() {
    for src in [
        "let a = if then x else y end",
        "let a = if p x else y end",
        "let a = if p then else y end",
        "let a = if p then x y end",
        "let a = if p then x else end",
        "let a = if p then x else y",
        // Failure from a clause entered through the else-if recursion is
        // propagated to the outer conditional too.
        "let a = if p then x else if",
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        assert!(out.stmts.is_empty(), "{src:?}: {:#?}", out.stmts);
    }

    // All three words are globally reserved rather than contextual binders.
    for src in ["let if = 1n", "let then = 1n", "let else = 1n"] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    }
}

/// The match expression: every arm begins with `|`, including the first.
/// A match with no arms needs no bar; sole and multiple arms parse, as does a
/// projection off the closing `end`.
#[test]
fn parses_match_expressions() {
    assert_eq!(
        parse_one("let a = match x with | #Some y => y | #None => 0n end"),
        "let a = match x with | #Some y => y | #None => 0n end"
    );
    // The first arm keeps the same marker as every arm after it.
    assert_eq!(
        parse_one("let a = match x with | #Some y => y end"),
        "let a = match x with | #Some y => y end"
    );
    assert_eq!(
        parse_one("let a = match x with end"),
        "let a = match x with end"
    );
    assert_eq!(
        parse_one("let a = match x with | w => w end"),
        "let a = match x with | w => w end"
    );

    let missing = "let a = match x with w => w end";
    let out = parse(lex(missing, FileID::GENERATED).tokens);
    let [error] = out.errors.as_slice() else {
        panic!("expected one error: {:#?}", out.errors);
    };
    assert!(matches!(
        error.kind,
        ErrorKind::Expected {
            expected: Expected::Punctuation("|"),
            found: Found::Token,
            context: Some(Related {
                kind: RelatedKind::Construct("match"),
                ..
            }),
            ..
        }
    ));
    assert_eq!(
        out.errors[0].span.start,
        missing.find("with ").unwrap() + "with ".len()
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
        parse_one("let a = match x with | w => w end 1n"),
        "let a = match x with | w => w end 1n"
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
            "let a = match x with | { p, q: r } => r end",
            "let a = match x with | { p, q: r } => r end",
        ),
        (
            "let a = match x with | {} => 1n end",
            "let a = match x with | () => 1n end",
        ),
        // A trailing comma among the fields is allowed, as in a struct
        // expression.
        (
            "let a = match x with | { p, } => p end",
            "let a = match x with | { p } => p end",
        ),
        (
            "let a = match x with | { pos: { x, y } } => x end",
            "let a = match x with | { pos: { x, y } } => x end",
        ),
        (
            "let a = match x with | #A #B y => y end",
            "let a = match x with | #A (#B y) => y end",
        ),
        // The parenthesized spelling is the same tree.
        (
            "let a = match x with | #A (#B y) => y end",
            "let a = match x with | #A (#B y) => y end",
        ),
        (
            "let a = match x with | 0n => 1n | k => k end",
            "let a = match x with | 0n => 1n | k => k end",
        ),
        (
            "let a = match x with | () => 1n end",
            "let a = match x with | () => 1n end",
        ),
        // Grouping parentheses around a pattern are discarded, as around an
        // expression.
        (
            "let a = match x with | (w) => w end",
            "let a = match x with | w => w end",
        ),
        (
            "let a = match x with | #Cons { head: #Some y, tail: t } => y end",
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
        parse_one("let f = fn p => do let {pos: {x, y}} = p return x end"),
        "let f = fn p => do let { pos: { x, y } } = p return x end"
    );
    // The parser records what was written and judges nothing: a refutable
    // pattern on a `let` parses, and refusing it is lowering's rule.
    assert_eq!(parse_one("let #Some x = opt"), "let #Some x = opt");
    assert_eq!(parse_one("let 0n = n"), "let 0n = n");
}

/// Every way a match can fail to parse, reported at the token that had no
/// reading: the missing `end`, the missing pattern, the arm without a body,
/// the trailing `|` that promised another arm, and `match` where a name goes.
#[test]
fn a_match_reports_where_it_fails() {
    for (src, from) in [
        // An arm needs a pattern, and `=>` does not begin one.
        ("let a = match x with | => 1n end", "=> 1n end"),
        // A `|` between arms promises another one; `end` is where the promise
        // breaks.
        ("let a = match x with | #A y => 1n | end", "end"),
        // An arm without a body: `|` begins no expression.
        (
            "let a = match x with | #A y => | #B z => 2n end",
            "| #B z => 2n end",
        ),
        // `match` is a keyword now, so it no longer names anything.
        ("let match = 1n", "match = 1n"),
        // Patterns on function arguments do not exist.
        ("let f = fn {x} => x", "{x} => x"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}");
    }

    // A match with no `end` runs out of input, and is reported there.
    let src = "let a = match x with | #A y => 1n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);

    // When another definition follows, it is where `end` was noticed missing,
    // but the related pointer belongs at the completed arm body—not at the
    // `match` keyword, which is not where the closing word is written.
    let src = "let _ = match 1 with | f => ()\n\nlet a = 1";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let ErrorKind::Expected {
        expected: Expected::Keyword("end"),
        found: Found::Token,
        related: Some(related),
        ..
    } = out.errors[0].kind
    else {
        panic!("wrong error: {:#?}", out.errors[0]);
    };
    assert_eq!(out.errors[0].span.start, src.rfind("let").unwrap());
    assert_eq!(related.kind, RelatedKind::Anchor);
    assert_eq!(related.span.start, src.find("()").unwrap());
    assert_eq!(related.span.width, 2);
    assert_eq!(
        out.stmts.len(),
        1,
        "the following definition should recover"
    );
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
        parse_one("let a = match x with | #A #B => 1n end"),
        "let a = match x with | #A (#B) => 1n end"
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
            "let a = match x with | _ => 1n end",
            "let a = match x with | _ => 1n end",
        ),
        (
            "let a = match x with | (_) => 1n end",
            "let a = match x with | _ => 1n end",
        ),
        // A struct pattern's sub-pattern, beside a pun and a named binder.
        (
            "let a = match x with | { p: _, q } => q end",
            "let a = match x with | { p: _, q } => q end",
        ),
        // A tag's payload, taken greedily like any other.
        (
            "let a = match x with | #Some _ => 1n | #None => 0n end",
            "let a = match x with | #Some _ => 1n | #None => 0n end",
        ),
        // The pattern of a `let`, statement and expression.
        ("let _ = f 1n", "let _ = f 1n"),
        ("let _ : Nat = g 3n", "let _ : Nat = g 3n"),
        (
            "let a = do let _ = f 1n return 2n end",
            "let a = do let _ = f 1n return 2n end",
        ),
        // Nested, and repeated: two `_` in one pattern parse — whether that
        // binds anything twice is not a question, since it binds nothing.
        (
            "let a = match x with | #Pair { a: _, b: _ } => 1n | _ => 2n end",
            "let a = match x with | #Pair { a: _, b: _ } => 1n | _ => 2n end",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src}");
    }
}

/// `fn _ => e` is legal — the argument is taken and thrown away — and so is
/// any mix of `_` with names. The printed form re-parses to the same tree.
#[test]
fn a_fn_argument_may_be_a_wildcard() {
    assert_eq!(parse_one("let f = fn _ => 1n"), "let f = fn _ => 1n");
    assert_eq!(parse_one("let f = fn _ _ => 1n"), "let f = fn _ _ => 1n");
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
        ("let v = {_: 1n}", Place::Field),
        ("let a = match x with | { _: 1n } => 0n end", Place::Field),
        // The pun: a field bound to its own name, which `_` is not.
        ("let a = match x with | {_} => 0n end", Place::Pun),
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
        (
            "let f : { x when 'a: Nat } where _ = { x: 1n }",
            Place::Type,
        ),
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
        parse_one("let f = fn v => match v with | { .. } => 1n end"),
        "let f = fn v => match v with | { .. } => 1n end"
    );
    // On a `let`, in both forms, and beside a renamed field.
    assert_eq!(parse_one("let {x, ..} = p"), "let { x, .. } = p");
    assert_eq!(
        parse_one("let a = do let {x: y, ..} = p return y end"),
        "let a = do let { x: y, .. } = p return y end"
    );

    let src = "let { a, .. } = p";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Let { pattern, .. } = &out.stmts[0].kind else {
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
/// named-rest might want — leaves the opening brace waiting for its close,
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
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::Expected {
                    expected: Expected::Punctuation("}"),
                    found: Found::Token,
                    related: Some(related),
                ..
                } if related.kind == RelatedKind::Opener
            ),
            "{src:?}: {:#?}",
            out.errors
        );
        let at = src.len() - from.len();
        assert_eq!(out.errors[0].span.start, at, "{src:?}: {:#?}", out.errors);
    }

    // `{..` at the end of input: the complaint points at the zero-width
    // position where the brace would have gone.
    let src = "let {..";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{src:?} parsed without complaint");
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Punctuation("}"),
            found: Found::End,
            related: Some(related),
        ..
        } if related.kind == RelatedKind::Opener
    ));
    assert_eq!(out.errors[0].span.start, src.len());
    assert_eq!(out.errors[0].span.width, 0);
}

/// Every accepted effect body is disjoint and round-trips canonically.
#[test]
fn effect_declarations() {
    assert_eq!(
        parse_one("effect Log = String -> ()"),
        "effect Log = String -> ()"
    );
    assert_eq!(
        parse_one("effect Log = { write: Nat -> () }"),
        "effect Log = { write: Nat -> () }"
    );
    assert_eq!(
        parse_one("effect Log = { write: Nat -> (), flush: () -> (), }"),
        "effect Log = { write: Nat -> (), flush: () -> () }"
    );
    assert_eq!(
        parse_one("effect StructInput = { value: Nat } -> ()"),
        "effect StructInput = { value: Nat } -> ()"
    );
    assert_eq!(
        parse_one("effect Console = !Log + !IO"),
        "effect Console = !Log + !IO"
    );
    assert_eq!(parse_one("effect Nil"), "effect Nil");
    for source in [
        "effect Nil = |",
        "effect Nil =",
        "effect Old = write : Nat -> () | flush : () -> ()",
        "effect Empty = {}",
        "effect NonFunction = { op: Nat }",
        "effect MissingColon = { op Nat -> () }",
        "effect Unclosed = {",
        "effect Open = { op: Nat -> (), .. }",
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{source}: {:#?}", out.errors);
    }
}

/// An effect declaration binds parameters the way a type declaration does,
/// and an alias body may apply effects and end in one spliced tail.
#[test]
fn effect_declarations_bind_parameters() {
    for source in [
        "effect Ask 'a = { get: () -> 'a }",
        "effect Log 'a = 'a -> ()",
        "effect Nil 'a 'b",
        "effect State 's = { get: () -> 's, put: 's -> () }",
        "effect Both 'a 'e = !Ask 'a + !Log + ..'e",
        "effect Forward 'e = ..'e",
        "effect Fields 'r = !State { x: Nat, ..'r }",
        "effect Deep 'a = !Ask (List 'a) + Sys::!Log Nat",
    ] {
        assert_eq!(parse_one(source), source);
    }
    let StmtKind::Effect { params, body, .. } = parse_stmt("effect Both 'a 'e = !Ask 'a + ..'e")
    else {
        panic!("expected an effect declaration");
    };
    assert_eq!(
        params
            .iter()
            .map(|param| param.tracked.as_str())
            .collect::<Vec<_>>(),
        ["a", "e"]
    );
    let EffectBody::Alias(row) = body else {
        panic!("expected an alias body");
    };
    let (_, label) = row.effects.first().expect("the alias names an effect");
    let EffectLabel::Written { args, .. } = label else {
        panic!("expected a written label");
    };
    assert_eq!(args.len(), 1);
    assert!(row.tail.is_some());
    // An alias body is a row of applications and one tail: the marks a written
    // row may wear are not its to wear.
    for source in [
        "effect Quiet 'e = \\!Log + ..'e",
        "effect Maybe 'e = !Log (when 'p) + ..'e",
        "effect Twice 'e = ..'e + !Log",
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{source}: {:#?}", out.errors);
    }
}

/// An effect used in a row is applied with the grammar a type application
/// has: atoms follow the label, and anything larger is parenthesized. A
/// `(when` after the label is its presence clause, never an argument.
#[test]
fn effect_applications_use_type_argument_syntax() {
    for source in [
        "let f : () -> Nat + !Ask Nat = g",
        "let f : () -> Nat + !Ask (List Nat) = g",
        "let f : () -> Nat + !Ask Nat + !Log = g",
        "let f : () -> Nat + !State { x: Nat, ..'r } = g",
        "let f : () -> Nat + !Run (!Log + !IO) = g",
        "let f : () -> Nat + !Ask Nat (when 'p) + ..'e = g",
        "let f : () -> Nat + \\!Ask Nat + ..'e = g",
        "let f : () -> Nat + Sys::!Ask Nat = g",
        "let f : Runner (!Ask Nat + !Log) -> Nat = g",
        "let f : () -> Nat + !Ask 'a = g",
        "let f : () -> Nat + !Pair Nat (Nat -> Nat) = g",
        "let f : () -> Nat + !Ask _ = g",
    ] {
        assert_eq!(parse_one(source), source);
    }
    let stmt = parse_stmt("let f : () -> Nat + !Pair Nat (Nat -> Nat) (when 'p) = g");
    let TypeKind::Arrow {
        effects: Some(effects),
        ..
    } = arrow_of(&stmt)
    else {
        panic!("expected an arrow with a row");
    };
    let (_, label) = effects.effects.first().expect("one label");
    let EffectLabel::Written { args, when } = label else {
        panic!("expected a written label");
    };
    assert_eq!(args.len(), 2);
    assert!(when.is_some());
}

/// A mark between cases promises another one, so nothing after it is reported
/// at the mark — the rule a sum type keeps. A leading bar needs a case too.
#[test]
fn an_effect_case_list_ends_at_a_bar_that_promised_one() {
    let out = parse(lex("effect E = !a + ", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Effect,
            found: Found::End,
            related: Some(related),
        ..
        } if related.kind == RelatedKind::Separator
    ));
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
    let out = parse(lex("let f : Runner (\\1n) -> Nat = g", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{:#?}", out.errors);
}

/// A `+` with no arrow to attach to is reported where it was written rather
/// than left for whatever the type was being read for to complain about a
/// token further along. The mark is what the row is reported at, which is the
/// whole of what it is still there for.
#[test]
fn a_row_with_no_arrow_is_a_parse_error() {
    for source in ["let x : Nat + !Log = 1n", "type T = Nat + !Log"] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{source}: {:#?}", out.errors);
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::Expected {
                    expected: Expected::FunctionType,
                    found: Found::Token,
                    related: None,
                    ..
                }
            ),
            "{source}: {:#?}",
            out.errors
        );
        let plus = source.find('+').expect("the source writes one");
        assert_eq!(out.errors[0].span.start, plus, "{source}");
    }
}

/// `handle` uses the same `with`/`|`/`end` shape as `match`, but retains its
/// optional leading `|`. Zero arms parse, and each body ends at the next `|`
/// or `end` of its own accord. The `return` arm may sit anywhere among them.
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
        "let a = handle e with end 1n",
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
    assert_eq!(parse_one("let a = raise 1n"), "let a = raise 1n");
    assert_eq!(parse_one("let a = raise f x"), "let a = raise f x");
    assert_eq!(
        parse_one("let a = handle e with | !Log.write s => raise 0n end"),
        "let a = handle e with | !Log.write s => raise 0n end"
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
    assert_eq!(parse_one("let a = !Log.write 1n"), "let a = !Log.write 1n");
    assert_eq!(parse_one("let a = f !Log.write"), "let a = f !Log.write");
    // A projection off a case is still a projection: the sigil that heads it
    // says which row the label belongs to, so neither reading is in doubt. The
    // printed form brackets the case, which is how a tag carrying a payload is
    // told from one being read off.
    assert_eq!(parse_one("let a = #Ok.x"), "let a = (#Ok).x");
    // A bare effect label is the unnamed selector and is first-class.
    assert_eq!(parse_one("let a = !Log"), "let a = !Log");
}

/// One statement's parsed tree, for the tests that assert about the shape
/// rather than about what it prints back as.
fn parse_stmt(src: &str) -> StmtKind {
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
    let [stmt] = &out.stmts[..] else {
        panic!("{src}: {:#?}", out.stmts);
    };
    stmt.kind.clone()
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
        ("let a = handle e with | !Log.write 1n => () end", "1"),
        // The operation an arm answers is a name, and so is the one a
        // reference reads: the `.` promises one either way.
        ("let a = !Log.1", "1"),
        // An operation declares a signature, so the `:` is not optional: a
        // case with no `:` is the alias it does not look like.
        ("effect E = op | w : Nat -> ()", "op |"),
        // And the signature is a type, whatever else was written there.
        ("effect E = { op: , }", ","),
        // A `\` promises an effect, so anything but one after it is
        // reported — the rule a sum's absent case keeps.
        ("let f : A -> B + \\x = g", "x"),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert!(!out.errors.is_empty(), "{source}: {:#?}", out.errors);
        let start = source.find(at).expect("the source writes it");
        assert_eq!(out.errors[0].span.start, start, "{source}");
    }

    // A `+` between effects promises another one, so the unusable token after
    // it is primary and the separator is retained as related context.
    // A `+` with no row after it at all promises nothing: the row is empty,
    // and what follows is somebody else's token to refuse.
    let out = parse(lex("let f : A -> B + = g", FileID::GENERATED).tokens);
    assert!(!out.errors.is_empty(), "{:#?}", out.errors);

    let out = parse(lex("let f : A -> B + !Log + = g", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    let separator = "let f : A -> B + !Log ".len();
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Effect,
            found: Found::Token,
            related: Some(related),
        ..
        } if related.kind == RelatedKind::Separator && related.span.start == separator
    ));
    assert_eq!(out.errors[0].span.start, separator + 2);
}

/// Running out of input where 'an arm's binder goes is reported at the end of
/// what there was, the way every other production that needs more reports it.
#[test]
fn a_handler_arm_that_runs_out_reports_where_the_input_ended() {
    let source = "let a = handle e with | !Log.write";
    let out = parse(lex(source, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Argument,
            found: Found::End,
            related: None,
            ..
        }
    ));
    assert_eq!(out.errors[0].span.start, source.len());
}

/// Both module forms, and the nesting that makes a bundle a tree. A body of
/// `None` is the file module, which the loader fills in; an empty body is a
/// module written inline with nothing in it, which is legal and is not the
/// same thing.
#[test]
fn both_module_forms_parse() {
    let out = parse(lex("module A", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Module { name, body } = &out.stmts[0].kind else {
        panic!("expected a module: {:#?}", out.stmts);
    };
    assert_eq!(name.tracked, "A");
    assert!(body.is_none());

    let out = parse(lex("module A = end", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Module {
        body: Some(body), ..
    } = &out.stmts[0].kind
    else {
        panic!("expected an inline module: {:#?}", out.stmts);
    };
    assert!(body.is_empty());
}

/// A module's body is a statement list, in any order and any number, and a
/// module is one of the things that may be in it.
#[test]
fn a_module_body_holds_every_kind_of_statement() {
    let source = "module A = type T = Nat effect E let x = 1 module B = let y = 2n end end";
    let out = parse(lex(source, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let StmtKind::Module {
        body: Some(body), ..
    } = &out.stmts[0].kind
    else {
        panic!("expected an inline module: {:#?}", out.stmts);
    };
    assert_eq!(body.len(), 4);
    assert!(matches!(body[0].kind, StmtKind::Type { .. }));
    assert!(matches!(body[1].kind, StmtKind::Effect { .. }));
    assert!(matches!(body[2].kind, StmtKind::Let { .. }));
    let StmtKind::Module {
        body: Some(inner), ..
    } = &body[3].kind
    else {
        panic!("expected a nested module: {:#?}", body);
    };
    assert_eq!(inner.len(), 1);
}

/// Paths of one, two and three segments, in the two positions R6 allows, with
/// a bare name still being a path with no segments at all.
#[test]
fn paths_parse_in_expression_and_type_positions() {
    assert_eq!(parse_one("let a = x"), "let a = x");
    assert_eq!(parse_one("let a = Math::double"), "let a = Math::double");
    assert_eq!(
        parse_one("let a = Math::Vec::zero"),
        "let a = Math::Vec::zero"
    );
    assert_eq!(
        parse_one("let a : Math::Pair = z"),
        "let a : Math::Pair = z"
    );
    assert_eq!(
        parse_one("let a : Math::Vec::Pair Nat Nat = z"),
        "let a : Math::Vec::Pair Nat Nat = z"
    );
    assert_eq!(
        parse_one("type T = Math::Pair Nat Nat"),
        "type T = Math::Pair Nat Nat"
    );
}

/// `::` binds tighter than application and than projection, which is what makes
/// `Math::mk 1 2` an application of `Math::mk` rather than of `Math`.
#[test]
fn a_path_binds_tighter_than_application_and_projection() {
    assert_eq!(
        parse_one("let made = M::mk 1n 2n"),
        "let made = M::mk 1n 2n"
    );
    assert_eq!(parse_one("let got = M::p.x"), "let got = M::p.x");
    assert_eq!(parse_one("let got = M::p.x.y"), "let got = M::p.x.y");
}

/// The path qualifies the whole sigilled label, in every position R7 names: a
/// row, an alias case, an operation expression and a handler arm's head.
#[test]
fn an_effect_path_qualifies_the_whole_label() {
    assert_eq!(
        parse_one("let f : () -> Nat + Sys::!Log = g"),
        "let f : () -> Nat + Sys::!Log = g"
    );
    assert_eq!(
        parse_one("let f : () -> Nat + Sys::!Log + !Local = g"),
        "let f : () -> Nat + Sys::!Log + !Local = g"
    );
    assert_eq!(
        parse_one("effect Console = Sys::!Log + !IO"),
        "effect Console = Sys::!Log + !IO"
    );
    assert_eq!(
        parse_one("let a = Sys::!Log.write 1n"),
        "let a = Sys::!Log.write 1n"
    );
    assert_eq!(
        parse_one("let a = handle e with | Sys::!Log.write s => s end"),
        "let a = handle e with | Sys::!Log.write s => s end"
    );
    // The `\` of a label written absent takes the path too.
    assert_eq!(
        parse_one("let f : () -> Nat + \\Sys::!Log + ..'r = g"),
        "let f : () -> Nat + \\Sys::!Log + ..'r = g"
    );
}

/// A run of names leading somewhere other than a label is not a label: the row
/// reader puts the cursor back and the type reader takes it as the application
/// it is.
#[test]
fn a_path_that_is_not_an_effect_is_read_as_a_type() {
    assert_eq!(
        parse_one("let f : Math::Pair Nat Nat -> Nat = g"),
        "let f : Math::Pair Nat Nat -> Nat = g"
    );
}

/// R8: a path is not legal where a declaration binds a name or where a pattern
/// takes one apart. Each is the unexpected `::` it looks like.
#[test]
fn a_path_is_refused_where_a_name_is_bound() {
    for (source, expected) in [
        ("let A::x = 1n", Expected::Punctuation("=")),
        ("module A::B = end", Expected::Statement),
        ("type A::T = Nat", Expected::Punctuation("=")),
        ("effect A::E", Expected::Statement),
        ("let f = fn A::x => x", Expected::Punctuation("=>")),
        (
            "let a = match e with | A::x => 1n end",
            Expected::Punctuation("=>"),
        ),
    ] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert!(
            !out.errors.is_empty(),
            "{source:?} parsed: {:#?}",
            out.stmts
        );
        assert!(
            matches!(
                out.errors[0].kind,
                ErrorKind::Expected {
                    expected: found,
                    found: Found::Token,
                    ..
                } if found == expected
            ),
            "{source:?}: {:#?}",
            out.errors
        );
    }
    // At the `::` itself for the two the error table names.
    for source in ["let A::x = 1n", "module A::B = end"] {
        let out = parse(lex(source, FileID::GENERATED).tokens);
        assert_eq!(
            out.errors[0].span.start,
            source.find("::").expect("the separator"),
            "{source:?}"
        );
    }
}

/// An inline module that runs out of input is reported where the `end` should
/// have gone, and a malformed statement inside one recovers to the `end` rather
/// than eating the rest of the file.
#[test]
fn a_module_body_recovers_at_its_end() {
    let out = parse(lex("module A = let x = 1n", FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Keyword("end"),
            found: Found::End,
            related: Some(related),
        ..
        } if related.kind == RelatedKind::Anchor
    ));

    let source = "module A = with end let after = 1n";
    let out = parse(lex(source, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.stmts.len(), 2, "{:#?}", out.stmts);
    assert!(matches!(out.stmts[1].kind, StmtKind::Let { .. }));
}

#[test]
fn an_extern_declares_a_string_foreign_target_of_any_type() {
    let source = "extern answer : Nat = \"host.answer\"";
    assert_eq!(parse_one(source), source);
    let output = parse(lex(source, FileID::GENERATED).tokens);
    let StmtKind::Extern {
        name, ty, target, ..
    } = &output.stmts[0].kind
    else {
        panic!("extern did not parse: {:#?}", output.stmts);
    };
    assert_eq!(name.tracked, "answer");
    assert!(matches!(ty.ty.tracked, TypeKind::Ident { .. }));
    assert_eq!(target.tracked, "host.answer");
    assert_eq!(target.span, FileID::GENERATED.span(22, 13));

    for source in [
        "extern answer = \"host.answer\"",
        "extern answer : Nat",
        "extern answer : Nat = host",
    ] {
        let output = parse(lex(source, FileID::GENERATED).tokens);
        assert!(
            !output.errors.is_empty(),
            "{source:?} parsed without an error"
        );
    }
}

#[test]
fn extern_fn_abi_desugars_to_curried_arrows_and_retains_arity() {
    let source = "extern add : fn(Nat, Nat,) -> Nat + !IO = \"host.add\"";
    let output = parse(lex(source, FileID::GENERATED).tokens);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let StmtKind::Extern { ty, abi, .. } = &output.stmts[0].kind else {
        panic!("extern did not parse: {:#?}", output.stmts);
    };
    let ExternTypeKind::Function {
        parameters,
        result,
        effects,
    } = &abi.tracked
    else {
        panic!("marked ABI was not retained: {abi:#?}");
    };
    assert_eq!(parameters.len(), 2);
    assert!(matches!(result.tracked, ExternTypeKind::Ordinary(_)));
    assert!(effects.is_some());
    let TypeKind::Arrow {
        to: second,
        effects: outer_effects,
        ..
    } = &ty.ty.tracked
    else {
        panic!("marked ABI did not desugar to an arrow: {:#?}", ty.ty);
    };
    assert!(outer_effects.is_none(), "partial application must be pure");
    assert!(matches!(
        second.tracked,
        TypeKind::Arrow {
            effects: Some(_),
            ..
        }
    ));
}

#[test]
fn extern_fn_abi_supports_nullary_nested_grouped_and_where_forms() {
    let source =
        "extern build : fn() -> (fn(Nat, String) -> Nat) + !Outer where 'a = 'a = \"host.build\"";
    let output = parse(lex(source, FileID::GENERATED).tokens);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let StmtKind::Extern { ty, abi, .. } = &output.stmts[0].kind else {
        panic!("extern did not parse: {:#?}", output.stmts);
    };
    assert!(ty.clause.is_some());
    let TypeKind::Arrow { from, .. } = &ty.ty.tracked else {
        panic!("nullary ABI did not desugar to a unit arrow");
    };
    assert!(matches!(from.tracked, TypeKind::Unit));
    let ExternTypeKind::Function {
        parameters, result, ..
    } = &abi.tracked
    else {
        panic!("marked ABI was not retained");
    };
    assert!(parameters.is_empty());
    assert!(matches!(result.tracked, ExternTypeKind::Group(_)));
}

#[test]
fn extern_boundary_metadata_retains_nesting_values_and_round_trips() {
    let source = "extern use : @outer { version: 1n } fn(@async fn(Nat) -> Nat) -> (@async fn(Nat) -> Nat) = \"host.use\"";
    let output = parse(lex(source, FileID::GENERATED).tokens);
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    let StmtKind::Extern { abi, .. } = &output.stmts[0].kind else {
        panic!("extern")
    };
    let ExternTypeKind::Annotated { attributes, inner } = &abi.tracked else {
        panic!("metadata")
    };
    assert_eq!(attributes[0].key.tracked, "outer");
    assert!(matches!(
        attributes[0].value.as_ref().unwrap().tracked,
        DataKind::Struct(_)
    ));
    let ExternTypeKind::Function {
        parameters, result, ..
    } = &inner.tracked
    else {
        panic!("function")
    };
    assert!(matches!(
        parameters[0].tracked,
        ExternTypeKind::Annotated { .. }
    ));
    assert!(matches!(result.tracked, ExternTypeKind::Group(_)));
    let printed = print::ast::stmt(&output.stmts[0]).to_string();
    let reparsed = parse(lex(&printed, FileID::GENERATED).tokens);
    assert!(
        reparsed.errors.is_empty(),
        "{printed}: {:#?}",
        reparsed.errors
    );
    assert_eq!(printed, print::ast::stmt(&reparsed.stmts[0]).to_string());

    // Metadata does not impose a separate nesting limit on boundary functions.
    let mut signature = "Nat".to_owned();
    for _ in 0..32 {
        signature = format!("@async fn() -> {signature}");
    }
    let output = parse(
        lex(
            &format!("extern deep : {signature} = \"host.deep\""),
            FileID::GENERATED,
        )
        .tokens,
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    assert_eq!(
        print::ast::stmt(&output.stmts[0])
            .to_string()
            .matches("@async")
            .count(),
        32
    );
}

#[test]
fn fn_abi_syntax_is_extern_only_and_directly_nested() {
    for source in [
        "let f : fn(Nat) -> Nat = value",
        "type F = fn(Nat) -> Nat",
        "extern bad : { callback: fn(Nat) -> Nat } = \"host.bad\"",
        "extern bad : fn(Nat -> fn(String) -> Nat) -> Nat = \"host.bad\"",
        "extern bad : { callback: @async fn(Nat) -> Nat } = \"host.bad\"",
        "extern bad : fn(Nat, @async) -> Nat = \"host.bad\"",
    ] {
        let output = parse(lex(source, FileID::GENERATED).tokens);
        assert!(
            !output.errors.is_empty(),
            "{source:?} parsed without an error"
        );
    }

    let source = "extern bad : fn(Nat,, String) -> Nat = \"host.bad\"\nextern good : fn(Nat) -> Nat = \"host.good\"";
    let output = parse(lex(source, FileID::GENERATED).tokens);
    assert!(!output.errors.is_empty());
    assert_eq!(output.stmts.len(), 1, "recovery skipped the next extern");
    assert!(matches!(output.stmts[0].kind, StmtKind::Extern { .. }));
}

/// Metadata is written in front of a definition as attributes, `@key` or
/// `@key <literal>`, and every kind of top-level definition — inside an
/// inline module too — may carry them. Printing writes them back in front of
/// the definition, one per attribute, so the printed statement reparses to
/// the tree it came from.
#[test]
fn attributes_are_read_in_front_of_every_definition_kind() {
    for (src, printed) in [
        ("@test let x = 1n", "@test let x = 1n"),
        (
            "@since 2n extern f : Nat = \"f\"",
            "@since 2n extern f : Nat = \"f\"",
        ),
        ("@doc \"text\" type T = Nat", "@doc \"text\" type T = Nat"),
        (
            "@stability #Experimental effect E = () -> ()",
            "@stability #Experimental effect E = () -> ()",
        ),
        ("@owner \"core\" module M", "@owner \"core\" module M"),
        (
            "@outer module M = @inner let y = 1n end",
            "@outer module M = @inner let y = 1n end",
        ),
        (
            "@a @b 1n\n@c \"three\"\nlet x = 1n",
            "@a @b 1n @c \"three\" let x = 1n",
        ),
        ("@k let (a, b) = (1n, 2n)", "@k let (a, b) = (1n, 2n)"),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
}

/// An attribute's key is read from the sigilled token, not re-parsed as an
/// identifier, so a keyword spelling like `if` names a tag the same way
/// `test` or `doc` would.
#[test]
fn a_keyword_spelling_names_an_attribute_like_any_other() {
    for (src, printed) in [
        ("@if let x = 1n", "@if let x = 1n"),
        ("@match \"m\" let x = 1n", "@match \"m\" let x = 1n"),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
    let out = parse(lex("@if let x = 1n", FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(out.stmts[0].attributes[0].key.tracked, "if");
}

/// A value is literal data and nothing else: scalars, a tag with an optional
/// literal payload, and tuples, arrays, and structs of those. A written `()`
/// is the same as no value, so it prints as the bare attribute; grouping
/// parentheses and trailing commas are dropped the way expressions drop them.
#[test]
fn attribute_values_are_literal_data() {
    for (src, printed) in [
        ("@k \"s\" let x = 1n", "@k \"s\" let x = 1n"),
        ("@k 1n let x = 1n", "@k 1n let x = 1n"),
        ("@k 1i let x = 1n", "@k 1i let x = 1n"),
        ("@k 1.5 let x = 1n", "@k 1.5 let x = 1n"),
        ("@k true let x = 1n", "@k true let x = 1n"),
        ("@k () let x = 1n", "@k let x = 1n"),
        ("@k (1n, \"a\") let x = 1n", "@k (1n, \"a\") let x = 1n"),
        ("@k (1n, 2n,) let x = 1n", "@k (1n, 2n) let x = 1n"),
        ("@k (1n,) let x = 1n", "@k (1n,) let x = 1n"),
        ("@k ((1n)) let x = 1n", "@k 1n let x = 1n"),
        ("@k [1n, 2n,] let x = 1n", "@k [1n, 2n] let x = 1n"),
        ("@k [] let x = 1n", "@k [] let x = 1n"),
        (
            "@k { a: 1n, \"b c\": 2n, 0: 3n, } let x = 1n",
            "@k { a: 1n, \"b c\": 2n, 0: 3n } let x = 1n",
        ),
        ("@k {} let x = 1n", "@k let x = 1n"),
        ("@k #Tag let x = 1n", "@k #Tag let x = 1n"),
        ("@k #Tag 1n let x = 1n", "@k #Tag 1n let x = 1n"),
        (
            "@k #Tag #Inner 2n let x = 1n",
            "@k #Tag (#Inner 2n) let x = 1n",
        ),
        (
            "@k #\"two words\" let x = 1n",
            "@k #\"two words\" let x = 1n",
        ),
        (
            "@k { a: [#T { b: () }, (1n, 2n)] } let x = 1n",
            "@k { a: [#T { b: () }, (1n, 2n)] } let x = 1n",
        ),
        (
            "@k \\\\ one\n   \\\\ two\nlet x = 1n",
            "@k \" one\\n two\" let x = 1n",
        ),
    ] {
        assert_eq!(parse_one(src), printed, "{src:?}");
    }
}

/// The tree records what was written: the key without its sigil, the value
/// or its absence, and the spans of each. The definition's own span starts at
/// its keyword, so nothing that pointed at a definition moves.
#[test]
fn attributes_keep_their_spans_and_leave_the_definition_where_it_was() {
    let src = "@a @b (1n, 2n) let x = 1n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    let [stmt] = &out.stmts[..] else {
        panic!("one statement: {:#?}", out.stmts);
    };
    assert_eq!(stmt.span.start, src.find("let").expect("the `let`"));
    let [a, b] = &stmt.attributes[..] else {
        panic!("two attributes: {:#?}", stmt.attributes);
    };
    assert_eq!(a.key.tracked, "a");
    assert_eq!((a.key.span.start, a.key.span.width), (0, 2));
    assert_eq!((a.span.start, a.span.width), (0, 2));
    assert!(a.value.is_none());
    assert_eq!(b.key.tracked, "b");
    let value = b.value.as_ref().expect("`@b` carries a value");
    assert!(matches!(&value.tracked, DataKind::Tuple(elements) if elements.len() == 2));
    assert_eq!((value.span.start, value.span.width), (6, 8));
    assert_eq!((b.span.start, b.span.width), (3, 11));

    let out = parse(lex("@k () let x = 1n", FileID::GENERATED).tokens);
    let value = out.stmts[0].attributes[0].value.as_ref().expect("written");
    assert!(matches!(value.tracked, DataKind::Unit));
    let out = parse(lex("@k #Some let x = 1n", FileID::GENERATED).tokens);
    let value = out.stmts[0].attributes[0].value.as_ref().expect("written");
    assert!(matches!(
        &value.tracked,
        DataKind::Tag { payload: None, .. }
    ));
}

/// Attributes with no definition after them describe nothing, and are
/// reported at the attributes rather than at whatever stopped them. The
/// statement list around them goes on: a module whose last thing is a stray
/// attribute still closes.
#[test]
fn attributes_without_a_definition_are_refused_at_the_attributes() {
    for (src, start, width, stmts) in [
        ("@k", 0, 2, 0),
        ("let x = 1n\n@a @b 1n", 11, 8, 1),
        ("module M = @k end\nlet x = 1n", 11, 2, 2),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(
            out.errors[0].kind,
            ErrorKind::AttributeWithoutDefinition,
            "{src:?}"
        );
        assert_eq!(out.errors[0].span.start, start, "{src:?}");
        assert_eq!(out.errors[0].span.width, width, "{src:?}");
        assert_eq!(out.stmts.len(), stmts, "{src:?}: {:#?}", out.stmts);
    }
}

/// A value that computes — a name, an application, an operator, anything
/// that begins an expression but not a literal — is refused where it begins,
/// at the top of a value and inside a tuple, array, struct field, or tag
/// payload alike. The definition after it is still read.
#[test]
fn a_metadata_value_that_is_not_a_literal_is_refused_where_it_begins() {
    for (src, at) in [
        ("@since add 1n 2n\nlet x = 1n", "add"),
        ("@k -1i\nlet x = 1n", "-"),
        ("@k not true\nlet x = 1n", "not"),
        ("@k fn a => a\nlet x = 1n", "fn"),
        ("@k if true then 1n else 2n end\nlet x = 1n", "if"),
        ("@k !Log\nlet x = 1n", "!Log"),
        ("@k _\nlet x = 1n", "_"),
        ("@k { a: add }\nlet x = 1n", "add"),
        ("@k { ..base }\nlet x = 1n", ".."),
        ("@k [..base]\nlet x = 1n", ".."),
        ("@k (1n, y)\nlet x = 1n", "y"),
        ("@k #Removed since\nlet x = 1n", "since"),
        ("@k Math::pi\nlet x = 1n", "Math"),
    ] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(out.errors[0].kind, ErrorKind::MetadataNotLiteral, "{src:?}");
        assert_eq!(
            out.errors[0].span.start,
            src.find(at).expect("the offending token"),
            "{src:?}"
        );
        assert_eq!(out.stmts.len(), 1, "{src:?}: {:#?}", out.stmts);
        assert!(
            out.stmts[0].attributes.is_empty(),
            "{src:?}: {:#?}",
            out.stmts[0].attributes
        );
    }
}

/// Metadata belongs to a top-level definition. On a block's `let` it is
/// refused at the attribute and the `let` is kept without it; where an
/// expression is expected it is refused as belonging in front of a
/// definition, and the value after it is read so the definition is checked.
#[test]
fn attributes_are_refused_in_blocks_and_in_expressions() {
    let src = "let a = do @k @j 1n let x = 1n return x end";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(out.errors[0].kind, ErrorKind::AttributeInBlock);
    assert_eq!(
        out.errors[0].span.start,
        src.find("@k").expect("the attribute")
    );
    assert_eq!(out.errors[0].span.width, "@k @j 1n".len());
    let [stmt] = &out.stmts[..] else {
        panic!("one statement: {:#?}", out.stmts);
    };
    let StmtKind::Let { body, .. } = &stmt.kind else {
        panic!("a let: {stmt:#?}");
    };
    let ExprKind::Do { stmts, result } = &body.tracked.tracked else {
        panic!("a block: {body:#?}");
    };
    assert_eq!(stmts.len(), 1);
    assert!(stmts[0].attributes.is_empty());
    assert!(result.is_some());

    for (src, at) in [("let x = @k 1n", "@k"), ("let x = (1n, @k 2n)", "@k")] {
        let out = parse(lex(src, FileID::GENERATED).tokens);
        assert_eq!(out.errors.len(), 1, "{src:?}: {:#?}", out.errors);
        assert_eq!(
            out.errors[0].kind,
            ErrorKind::AttributeInExpression,
            "{src:?}"
        );
        assert_eq!(
            out.errors[0].span.start,
            src.find(at).expect("the attribute"),
            "{src:?}"
        );
        assert_eq!(out.stmts.len(), 1, "{src:?}: {:#?}", out.stmts);
    }
}

/// A metadata value that breaks in some way other than computing gets the
/// complaint the same shape gets in an expression: a value expected, a closer
/// expected, a discard where a field name goes. The definition after it is
/// still read.
#[test]
fn malformed_metadata_values_get_the_ordinary_complaints() {
    let src = "@k { a: = 1n } let x = 1n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Value,
            found: Found::Token,
            ..
        }
    ));
    assert_eq!(out.errors[0].span.start, src.find('=').expect("the `=`"));
    assert_eq!(out.stmts.len(), 1);

    let src = "@k (] let x = 1n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(matches!(
        out.errors[0].kind,
        ErrorKind::Expected {
            expected: Expected::Punctuation(")"),
            ..
        }
    ));
    assert_eq!(out.stmts.len(), 1);

    let src = "@k { _: 1n } let x = 1n";
    let out = parse(lex(src, FileID::GENERATED).tokens);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert_eq!(
        out.errors[0].kind,
        ErrorKind::Wildcard {
            place: Place::Field
        }
    );
    assert_eq!(out.errors[0].span.start, src.find('_').expect("the `_`"));
    assert_eq!(out.stmts.len(), 1);
}
