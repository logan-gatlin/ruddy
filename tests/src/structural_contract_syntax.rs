//! Writable structural contracts keep the same branch relationships through
//! parsing, source printing, and comment-preserving formatting.

use ruddy::{
    format::format,
    parse::{self, StmtKind, TypeKind},
    token::lex,
    tracking::FileID,
};
use ruddy_debug::print;

fn parsed(source: &str) -> parse::Output {
    let tokens = lex(source, FileID::GENERATED);
    assert!(tokens.errors.is_empty(), "{source}: {:#?}", tokens.errors);
    let parsed = parse::parse(tokens.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    parsed
}

fn printed(source: &str) -> String {
    parsed(source)
        .stmts
        .iter()
        .map(|stmt| print::ast::stmt(stmt).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn inferred_signatures(source: &str) -> Vec<(String, String)> {
    use ruddy::{
        inference, ir,
        symbol::{Bundle, Mint, Version},
    };
    let parsed = parsed(source);
    let mut mint = Mint::new(Bundle::new("syntax", Version::new(0, 0, 0)).unwrap());
    let lowered = ir::build(&mut mint, parsed.stmts);
    assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
    let inferred = inference::infer(&mint, &lowered.program, inference::Trace::Off);
    assert!(
        inferred.errors().is_empty(),
        "{source}\n{:#?}",
        inferred.errors()
    );
    inferred
        .semantics()
        .schemes()
        .iter()
        .map(|(symbol, scheme)| (mint.name(*symbol).to_string(), scheme.to_string()))
        .collect()
}

#[test]
fn match_type_arms_keep_tag_and_tuple_relationships() {
    let source = "let get: match | #Red => { r: 'a, ..'rest } -> 'a | #Green => { g: 'b, ..'rest } -> 'b end = get";
    let output = parsed(source);
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("an annotated binding");
    };
    let TypeKind::Match(arms) = &annotation.ty.tracked else {
        panic!("a match contract");
    };
    assert_eq!(arms.len(), 2);
    assert!(arms.iter().all(|(from, to)| {
        matches!(from.tracked, TypeKind::Sum { .. })
            && matches!(to.tracked, TypeKind::Arrow { .. })
            && from.span.end() < to.span.start
    }));

    let tuples = parsed(
        "type Pad = match | () => (#None, #None) | ('a,) => ('a, #None) | ('a, 'b) => ('a, 'b) end",
    );
    let StmtKind::Type { .. } = tuples.stmts[0].kind else {
        panic!("a type declaration");
    };
}

#[test]
fn match_type_pipes_distinguish_sum_results_from_following_arms() {
    for source in [
        "let f: match | #A => #B | #C => #D end = f",
        "let f: match | #A => #B | #C | #D => #E end = f",
        "let f: match | (#A | #B) => #C | (#D | #E) => #F end = f",
        "let f: match | #A => { x: #B | #C } | #D => { x: #E } end = f",
        "let f: match | #A => (#B | #C, #D) | (#E | #F) => (#G, #H) end = f",
        "let f: match | #A => match | #B => #C | #D => #E end | #F => #G end = f",
        "let f: (match | #A => #B | #C => #D end) -> Nat = f",
        "let f: match | () => () -> Nat + !IO | #A => Nat end = f",
    ] {
        let output = printed(source);
        assert_eq!(printed(&output), output, "{source}");
        let parsed = parsed(source);
        let StmtKind::Let {
            ty: Some(annotation),
            ..
        } = &parsed.stmts[0].kind
        else {
            panic!("an annotated binding");
        };
        if let TypeKind::Match(arms) = &annotation.ty.tracked {
            assert_eq!(arms.len(), 2, "{source}");
        }
    }
}

#[test]
fn match_type_formatting_preserves_comments_and_structure() {
    for source in [
        "let f:match|#A=>#B|#C=>#D end=f",
        "let f: match\n(* before first *)\n| #A => (* result *) #B\n| #C => #D\n(* after last *)\nend = f",
        "let f: match | (#A | #B) => #C | (#D | #E) => #F end = f",
        "let f: match\n| #A => match\n| #B => #C\n| #D => #E\nend\n| #F => #G\nend = f",
    ] {
        let output = format(source, FileID::GENERATED);
        assert!(!output.has_errors(), "{:#?}", output.parse_errors);
        assert_eq!(printed(&output.text), printed(source), "{}", output.text);
        assert_eq!(format(&output.text, FileID::GENERATED).text, output.text);
        if source.contains("(* before first *)") {
            for comment in ["(* before first *)", "(* result *)", "(* after last *)"] {
                assert_eq!(output.text.matches(comment).count(), 1, "{}", output.text);
            }
        }
    }
}

#[test]
fn malformed_match_type_reports_parse_errors() {
    for source in [
        "let f: match | #A #B end = f",
        "let f: match | #A => end = f",
        "let f: match | #A => #B | end = f",
        "let f: match | #A => #B = f",
    ] {
        let output = parse::parse(lex(source, FileID::GENERATED).tokens);
        assert!(!output.errors.is_empty(), "accepted {source}");
    }
}

#[test]
fn untagged_union_annotations_are_rejected() {
    for source in [
        "let f: Nat or Int = f",
        "let f: Nat | Int = f",
        "let f: Nat or String -> Bool = f",
        "let f: (Nat -> Nat) or (String -> String) = f",
        "let f: { r: Nat } or { g: String } = f",
        "let f: (#A | #B) or (#C | #D) = f",
        "let f: match | #A => Nat or String | #B => Bool end = f",
        "let f: fn | #A => capture (Nat or String) = f",
        "type Choice 'a 'b = 'a or 'b",
    ] {
        let output = parse::parse(lex(source, FileID::GENERATED).tokens);
        assert!(!output.errors.is_empty(), "accepted {source}");
    }
    // `or` remains a Boolean operator in expressions and presence formulas.
    for source in [
        "let either = fn a b => a or b",
        "let f: { r when 'a: Nat, g when 'b: Nat } -> Nat where 'a or 'b = f",
    ] {
        let output = printed(source);
        assert_eq!(printed(&output), output, "{source}");
    }
}

#[test]
fn structural_type_graphs_print_as_writable_source() {
    for source in [
        "let f: fn 'image => { r: ('image).g, ..'image } = f",
        "let f: fn 'image => match 'image with | { running: 'world } => { paused: 'world } | { paused: 'world } => { running: 'world } end = f",
        "let f: fn 'tuple => match 'tuple with | () => (#None, #None) | ('a,) => ('a, #None) | ('a, 'b) => ('a, 'b) end = f",
        "let f: fn 'value => match 'value with | #Some 'payload => 'payload | #None => capture (Nat) end = f",
        "let f: fn 'function 'value => ('function) ('value) = f",
        "let f: fn 'value => do _ = (capture (Nat -> Nat)) ('value); return 'value end = f",
        "let f: fn 'value => do _ = ('value).a; _ = ('value).b; return 'value end = f",
        "let f: (fn 'value => #Some ('value)) -> Nat = f",
        "let f: fn 'value => { \"two words\": ('value).\"source field\", ..'value } = f",
        "let f: fn => capture (Nat) = f",
        "let f: fn => capture (Nat) + | = f",
        "let f: fn 'x => 'x + !IO = f",
        "let f: fn 'x => do let 'twice = { left: 'x, right: 'x }; return { first: 'twice, second: 'twice } end = f",
    ] {
        let output = printed(source);
        assert_eq!(printed(&output), output, "{source}\n{output}");
        let formatted = format(source, FileID::GENERATED);
        assert!(
            !formatted.has_errors(),
            "{source}: {:#?}",
            formatted.parse_errors
        );
        assert_eq!(printed(&formatted.text), output, "{}", formatted.text);
        assert_eq!(
            format(&formatted.text, FileID::GENERATED).text,
            formatted.text
        );
    }
}

#[test]
fn match_function_contracts_lower_to_the_same_graph_as_explicit_matches() {
    for arms in [
        "| 'value => 'value",
        "| () => (#None, #None) | ('a,) => ('a, #None) | ('a, 'b) => ('a, 'b)",
        "| #Some 'payload => 'payload | #None => ()",
        "| { item: 'value, .. } => { first: 'value, second: 'value }",
        "| ('outer, 'nested) => (match 'nested with | ('outer,) => 'outer end, 'outer) | #Some 'outer => 'outer",
    ] {
        let (parameters, shorthand) = structural_pattern_body(&format!("fn {arms}"));
        let (explicit_parameters, explicit) =
            structural_pattern_body(&format!("fn 'input => match 'input with {arms} end"));
        assert_eq!(parameters, 1);
        assert_eq!(parameters, explicit_parameters);
        assert!(
            ruddy::contracts::Expr::same_structure(&shorthand, &explicit),
            "{arms}"
        );
        let signature = structural_pattern_roundtrip(parameters, &shorthand);
        assert!(signature.starts_with("fn |"), "{signature}");
    }
}

#[test]
fn match_function_contracts_roundtrip_with_effects_and_nested_types() {
    for source in [
        "let f: fn | #A => () | #B => () + !IO = f",
        "let f: fn | 'value => 'value + | = f",
        "let f: (fn | ('value,) => 'value) -> Nat = f",
        "let f: fn 'value => (capture (fn | #Some 'payload => 'payload | #None => ())) ('value) = f",
        "let f: fn | #Some 'value => (capture (fn | ('item,) => 'item)) ('value) | #None => () = f",
        "let f: match | #A => (fn | ('value,) => 'value) | #B => () end = f",
        "let f: match | #A => (Nat -> (fn | ('value,) => 'value)) | #B => () end = f",
        "let f: match | #A => (hide 'hidden => fn | ('value,) => 'value) | #B => () end = f",
        "type Unwrap = fn | #Some 'payload => 'payload | #None => ()\nlet after = 1n",
    ] {
        let canonical = printed(source);
        assert_eq!(printed(&canonical), canonical, "{source}");
        let formatted = format(source, FileID::GENERATED);
        assert!(
            !formatted.has_errors(),
            "{source}: {:#?}",
            formatted.parse_errors
        );
        assert_eq!(printed(&formatted.text), canonical);
        assert_eq!(
            format(&formatted.text, FileID::GENERATED).text,
            formatted.text
        );
    }

    for effects in ["+ !IO", "+ |"] {
        let source = format!("let f: fn | 'value => 'value {effects} = f");
        let output = parsed(&source);
        let StmtKind::Let {
            ty: Some(annotation),
            ..
        } = &output.stmts[0].kind
        else {
            panic!("an annotated binding");
        };
        assert!(matches!(
            &annotation.ty.tracked,
            TypeKind::Structural {
                parameters: 1,
                effect_sink: Some(_),
                ..
            }
        ));
        assert!(printed(&source).contains(effects));
    }
}

#[test]
fn match_function_contract_formatting_preserves_arm_comments() {
    let source = "let f: fn\n(* before first *)\n| #Some 'value => (* payload *) 'value\n| #None => ()\n(* after last *)\n= f";
    let formatted = format(source, FileID::GENERATED);
    assert!(!formatted.has_errors(), "{:#?}", formatted.parse_errors);
    for comment in ["(* before first *)", "(* payload *)", "(* after last *)"] {
        assert_eq!(
            formatted.text.matches(comment).count(),
            1,
            "{}",
            formatted.text
        );
    }
    assert_eq!(printed(&formatted.text), printed(source));
    assert_eq!(
        format(&formatted.text, FileID::GENERATED).text,
        formatted.text
    );
}

#[test]
fn malformed_match_function_contracts_report_parse_errors() {
    for source in [
        "let f: fn | = f",
        "let f: fn | #A = f",
        "let f: fn | #A => = f",
        "let f: fn | #A => () | = f",
        "let f: fn 'input | #A => () = f",
    ] {
        let output = parse::parse(lex(source, FileID::GENERATED).tokens);
        assert!(!output.errors.is_empty(), "accepted {source}");
    }
}

#[test]
fn explicit_and_shorthand_contracts_keep_the_same_branch_result_types() {
    let arms = "| () => (#None, #None) | ('a,) => ('a, #None) | ('a, 'b) => ('a, 'b)";
    let declarations = "= fn | () => (#None, #None) | (a,) => (a, #None) | (a, b) => (a, b)\n\
        let empty: (#None, #None) = extend2 ()\n\
        let one: (#Red, #None) = extend2 (#Red,)\n\
        let two: (#Red, #Green) = extend2 (#Red, #Green)";
    let explicit = inferred_signatures(&format!(
        "let extend2: fn 'tuple => match 'tuple with {arms} end {declarations}"
    ));
    let shorthand = inferred_signatures(&format!("let extend2: fn {arms} {declarations}"));
    assert_eq!(shorthand, explicit);
}

#[test]
fn match_function_display_keeps_a_parameter_when_its_name_is_still_needed() {
    for source in [
        "fn 'input => match 'input with | { item: 'value, .. } => 'input end",
        "fn 'input => match 'input with | { item: 'value, .. } => ('input).other end",
        "fn 'input => match 'input with | #Some 'value => 'input end",
        "fn 'input 'other => match 'input with | 'value => ('value, 'other) end",
        "fn 'input => match ('input).item with | 'value => 'value end",
    ] {
        let (parameters, body) = structural_pattern_body(source);
        let signature = structural_pattern_roundtrip(parameters, &body);
        assert!(signature.starts_with("fn 'input0"), "{signature}");
    }
}

#[test]
fn saturated_semantic_contract_keeps_effects_and_is_writable() {
    use ruddy::{
        contracts::{Contract, Expr},
        types::{Row, Ty},
    };
    use std::sync::Arc;
    let ty = Ty::Contract {
        fallback: Arc::new(Ty::Nat),
        contract: Arc::new(Contract {
            parameters: 1,
            body: Arc::new(Expr::Input(0)),
            captures: Arc::new([]),
            arguments: vec![Arc::new(Ty::Nat)].into(),
            effect_sink: Some(Arc::new(Ty::Arrow(
                Arc::new(Ty::unit()),
                Arc::new(Ty::unit()),
                Row::default(),
            ))),
        }),
    };
    let signature = ty.to_string();
    assert_eq!(signature, "fn => capture (Nat) + |");
    let source = format!("let f: {signature} = f");
    assert_eq!(printed(&source), format!("let f : {signature} = f"));
    let output = parsed(&source);
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("annotation")
    };
    assert!(matches!(
        &annotation.ty.tracked,
        TypeKind::Structural {
            parameters: 0,
            effect_sink: Some(_),
            ..
        }
    ));
}

#[test]
fn partially_applied_semantic_contract_prints_bound_arguments_as_captures() {
    use ruddy::{
        contracts::{Contract, Expr},
        types::Ty,
    };
    use std::sync::Arc;
    let ty = Ty::Contract {
        fallback: Arc::new(Ty::Undecided),
        contract: Arc::new(Contract {
            parameters: 2,
            body: Arc::new(Expr::Record {
                fields: vec![
                    ("first".into(), Arc::new(Expr::Input(0))),
                    ("second".into(), Arc::new(Expr::Input(1))),
                ]
                .into(),
                spread: None,
            }),
            captures: Arc::new([]),
            arguments: vec![Arc::new(Ty::Nat)].into(),
            effect_sink: None,
        }),
    };
    let signature = ty.to_string();
    assert!(signature.starts_with("fn 'input1 =>"), "{signature}");
    assert!(signature.contains("capture (Nat)"), "{signature}");
    let source = format!("let f: {signature} = f");
    let canonical = printed(&source);
    assert_eq!(printed(&canonical), canonical);
}

#[test]
fn partially_applied_match_contract_uses_its_one_remaining_input() {
    use ruddy::{
        contracts::{Arm, Contract, Expr, Pattern},
        types::Ty,
    };
    use std::sync::Arc;
    let ty = Ty::Contract {
        fallback: Arc::new(Ty::Undecided),
        contract: Arc::new(Contract {
            parameters: 2,
            body: Arc::new(Expr::Match {
                scrutinee: Arc::new(Expr::Input(1)),
                arms: vec![Arm {
                    pattern: Pattern::Record {
                        fields: vec![("0".into(), Pattern::Any)].into(),
                        open: false,
                    },
                    body: Arc::new(Expr::Record {
                        fields: vec![
                            ("bound".into(), Arc::new(Expr::Input(0))),
                            (
                                "item".into(),
                                Arc::new(Expr::Field {
                                    base: Arc::new(Expr::Input(1)),
                                    label: "0".into(),
                                }),
                            ),
                        ]
                        .into(),
                        spread: None,
                    }),
                }]
                .into(),
            }),
            captures: Arc::new([]),
            arguments: vec![Arc::new(Ty::Nat)].into(),
            effect_sink: None,
        }),
    };
    let signature = ty.to_string();
    assert_eq!(
        signature,
        "fn | ('a,) => { bound: capture (Nat), item: 'a }"
    );
    let source = format!(
        "let f: {signature} = fn | (item,) => {{ bound: 1n, item: item }}\n\
        let result: {{ bound: Nat, item: String }} = f (\"value\",)"
    );
    let canonical = printed(&source);
    assert_eq!(printed(&canonical), canonical);
    inferred_signatures(&source);
}

#[test]
fn returned_match_function_contracts_keep_enclosing_arms_and_effects() {
    use ruddy::{
        contracts::{Arm, Contract, Expr, Pattern},
        types::{EffectId, Rest, Row, RowField, Ty},
    };
    use std::sync::Arc;
    let (_, body) = structural_pattern_body("fn | ('value,) => 'value");
    let result = Arc::new(Ty::Contract {
        fallback: Arc::new(Ty::Undecided),
        contract: Arc::new(Contract {
            parameters: 1,
            body,
            captures: Arc::new([]),
            arguments: Arc::new([]),
            effect_sink: None,
        }),
    });
    let unit = Arc::new(Ty::unit());
    let cases = ["A", "B"];
    let matched = Ty::Contract {
        fallback: Arc::new(Ty::Undecided),
        contract: Arc::new(Contract {
            parameters: 1,
            body: Arc::new(Expr::Match {
                scrutinee: Arc::new(Expr::Input(0)),
                arms: cases
                    .iter()
                    .enumerate()
                    .map(|(index, label)| Arm {
                        pattern: Pattern::Tag {
                            label: (*label).into(),
                            payload: Box::new(Pattern::Record {
                                fields: Arc::new([]),
                                open: false,
                            }),
                        },
                        body: Arc::new(Expr::Apply {
                            function: Arc::new(Expr::Capture(index)),
                            argument: Arc::new(Expr::Input(0)),
                        }),
                    })
                    .collect::<Vec<_>>()
                    .into(),
            }),
            captures: cases
                .iter()
                .map(|label| {
                    Arc::new(Ty::Arrow(
                        Arc::new(Ty::Sum(Row {
                            labels: [((*label).into(), RowField::present(unit.clone()))]
                                .into_iter()
                                .collect(),
                            rest: Rest::Closed,
                        })),
                        result.clone(),
                        Row::closed(),
                    ))
                })
                .collect::<Vec<_>>()
                .into(),
            arguments: Arc::new([]),
            effect_sink: None,
        }),
    };
    let signature = matched.to_string();
    assert_eq!(
        signature,
        "match | #A => (fn | ('a,) => 'a) | #B => (fn | ('a,) => 'a) end"
    );
    let source = format!("let f: {signature} = f");
    let canonical = printed(&source);
    assert_eq!(printed(&canonical), canonical);
    let output = parsed(&source);
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("an annotated binding");
    };
    let TypeKind::Match(arms) = &annotation.ty.tracked else {
        panic!("an explicit match type");
    };
    assert_eq!(arms.len(), 2);
    assert!(
        arms.iter()
            .all(|(_, to)| matches!(to.tracked, TypeKind::Structural { parameters: 1, .. }))
    );

    let arrow = Ty::Arrow(
        Arc::new(Ty::Nat),
        result,
        Row {
            labels: [(
                EffectId::structural("Clock".into(), "() -> ()".into()).row_key(),
                RowField::present(unit),
            )]
            .into_iter()
            .collect(),
            rest: Rest::Closed,
        },
    );
    let signature = arrow.to_string();
    assert_eq!(signature, "Nat -> (fn | ('a,) => 'a) + !Clock");
    let source = format!("let f: {signature} = f");
    let canonical = printed(&source);
    assert_eq!(printed(&canonical), canonical);
    let output = parsed(&source);
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("an annotated binding");
    };
    let TypeKind::Arrow { to, effects, .. } = &annotation.ty.tracked else {
        panic!("the enclosing arrow");
    };
    assert!(effects.is_some());
    assert!(matches!(
        to.tracked,
        TypeKind::Structural {
            effect_sink: None,
            ..
        }
    ));
}

#[test]
fn structural_body_comments_are_preserved_by_formatting() {
    let source = "let f: fn 'x =>\n  (* branch shape *) match 'x with\n  | #Some 'y => (* payload *) 'y\n  | #None => #None\n  end = f";
    let formatted = format(source, FileID::GENERATED);
    assert!(!formatted.has_errors());
    for comment in ["(* branch shape *)", "(* payload *)"] {
        assert_eq!(
            formatted.text.matches(comment).count(),
            1,
            "{}",
            formatted.text
        );
    }
    assert_eq!(
        format(&formatted.text, FileID::GENERATED).text,
        formatted.text
    );
}

#[test]
fn shared_structural_subexpressions_print_once_and_keep_their_graph() {
    use ruddy::contracts::Expr;
    use std::sync::Arc;
    let pair = Arc::new(Expr::Record {
        fields: vec![
            ("left".into(), Arc::new(Expr::Input(0))),
            ("right".into(), Arc::new(Expr::Input(0))),
        ]
        .into(),
        spread: None,
    });
    let body = Arc::new(Expr::Record {
        fields: vec![("first".into(), pair.clone()), ("second".into(), pair)].into(),
        spread: None,
    });
    let signature = ruddy::ui::structural_type_text(1, &body, &[]);
    assert_eq!(signature.matches("left:").count(), 1, "{signature}");
    assert!(signature.contains("let 'node0"), "{signature}");
    let output = parsed(&format!("let f: {signature} = f"));
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("annotation");
    };
    let TypeKind::Structural {
        body: roundtrip, ..
    } = &annotation.ty.tracked
    else {
        panic!("structural contract");
    };
    assert!(Expr::same_structure(&body, roundtrip));
    assert_eq!(
        printed(&format!("let f: {signature} = f")),
        printed(&printed(&format!("let f: {signature} = f")))
    );
}

#[test]
fn inferred_image_contracts_are_compact_writable_annotations() {
    const SOURCE: &str = r#"
let get = fn channel image => match channel with
| #Red => image.r | #Green => image.g | #Blue => image.b | #Alpha => image.a end
let set = fn channel value image => match channel with
| #Red => {r: value, ..image} | #Green => {g: value, ..image}
| #Blue => {b: value, ..image} | #Alpha => {a: value, ..image} end
let try_get_set = fn get_from set_to image => match (get_from, set_to) with
| (#None, _) => image | (_, #None) => image
| (get_from, set_to) => set set_to (get get_from image) image end
let extend4 = fn
| () => (#None, #None, #None, #None)
| (a,) => (a, #None, #None, #None)
| (a, b) => (a, b, #None, #None)
| (a, b, c) => (a, b, c, #None)
| (a, b, c, d) => (a, b, c, d)
let swizzle = fn in_channels out_channels image => do
  let (in_a, in_b, in_c, in_d) = extend4 in_channels
  let (out_a, out_b, out_c, out_d) = extend4 out_channels
  return image |> try_get_set in_a out_a |> try_get_set in_b out_b
    |> try_get_set in_c out_c |> try_get_set in_d out_d
end
"#;
    let signatures = inferred_signatures(SOURCE);
    for (name, limit) in [("get", 500), ("extend4", 1000), ("swizzle", 6000)] {
        let signature = &signatures
            .iter()
            .find(|(symbol, _)| symbol == name)
            .unwrap()
            .1;
        assert!(signature.starts_with("fn "), "{name}: {signature}");
        assert!(
            signature.len() < limit,
            "{name}: {} bytes\n{signature}",
            signature.len()
        );
        let annotation = format!("let roundtrip: {signature} = {name}");
        let canonical = printed(&annotation);
        assert_eq!(printed(&canonical), canonical);
        inferred_signatures(&format!("{SOURCE}\n{annotation}"));
    }
}

#[test]
fn grammar_contract_examples_typecheck_and_roundtrip() {
    let chapter = include_str!("../../docs/src/grammar.md")
        .split_once("### Structural contracts\n")
        .unwrap()
        .1
        .split_once("### Open types and constraints\n")
        .unwrap()
        .0;
    for block in chapter.split("```ruddy\n").skip(1) {
        let source = block.split_once("```").unwrap().0;
        inferred_signatures(source);
        let canonical = printed(source);
        assert_eq!(printed(&canonical), canonical);
        inferred_signatures(&canonical);
    }
}

#[test]
fn analysis_hover_uses_the_writable_structural_signature() {
    use ruddy::symbol::{Bundle, Version};
    let source = "let get = fn channel image => match channel with | #Red => image.r | #Green => image.g end\nlet copy = get";
    let signature = inferred_signatures(source)
        .into_iter()
        .find(|(name, _)| name == "get")
        .unwrap()
        .1;
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some(source.into()));
    let analysis = host.analyze(
        Bundle::new("hover", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:#?}",
        analysis.diagnostics
    );
    let hover = analysis
        .hover("main.rud", source.find("get").unwrap())
        .unwrap();
    assert_eq!(hover.ty, signature);
    inferred_signatures(&format!("{source}\nlet annotated: {} = get", hover.ty));
    let expression = analysis
        .hover("main.rud", source.rfind("get").unwrap())
        .unwrap();
    assert_eq!(expression.ty, signature);
    inferred_signatures(&format!(
        "{source}\nlet expression_copy: {} = get",
        expression.ty
    ));
}

#[test]
fn resolved_expression_hover_keeps_writable_shared_definitions_and_scope_names() {
    use ruddy::symbol::{Bundle, Version};
    let record = "{ first_long_field: Nat, second_long_field: Nat, third_long_field: Nat }";
    let source = format!(
        "type InferredRecord = Bool\nlet identity: {record} -> {record} = fn value => value\nlet copy = identity"
    );
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some(source.clone()));
    let analysis = host.analyze(
        Bundle::new("expression", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let hover = analysis
        .hover("main.rud", source.rfind("identity").unwrap())
        .unwrap();
    assert!(
        hover.ty.starts_with("type InferredRecord2 ="),
        "{}",
        hover.ty
    );
    let (definitions, annotation) = hover.ty.rsplit_once('\n').unwrap();
    inferred_signatures(&format!(
        "{source}\n{definitions}\nlet restored: {annotation} = identity"
    ));
}

#[test]
fn imported_compiled_contracts_display_as_writable_annotations() {
    use ruddy::{
        artifact::Artifact,
        compile, inference, ir,
        symbol::{Bundle, Mint, Version},
    };
    let source = "let get = fn channel image => match channel with | #Red => image.r | #Green => image.g end";
    let parsed = parsed(source);
    let artifact = compile::compile(
        Mint::new(Bundle::new("dep", Version::new(0, 0, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{partial:#?}"));
    let artifact = Artifact::try_parse(&artifact.artifact().print())
        .unwrap()
        .validate()
        .unwrap();
    let infer = |source: &str| {
        let parsed = self::parsed(source);
        let mut mint = Mint::new(Bundle::new("consumer", Version::new(0, 0, 0)).unwrap());
        let lowered =
            ir::build_with_dependencies(&mut mint, parsed.stmts, std::slice::from_ref(&artifact));
        assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
        let inferred = inference::infer(&mint, &lowered.program, inference::Trace::Off);
        assert!(
            inferred.errors().is_empty(),
            "{source}\n{:#?}",
            inferred.errors()
        );
        inferred
            .semantics()
            .schemes()
            .iter()
            .find(|(symbol, _)| mint.name(**symbol) == "copy")
            .unwrap()
            .1
            .to_string()
    };
    let signature = infer("let copy = dep::get");
    assert!(signature.starts_with("fn "), "{signature}");
    assert!(signature.len() < 250, "{signature}");
    let annotated = format!(
        "let copy: {signature} = dep::get\nlet red: Nat = copy #Red {{r: 1n}}\nlet green: String = copy #Green {{g: \"green\"}}"
    );
    assert_eq!(infer(&annotated), signature);
}

#[test]
fn unresolved_residual_annotations_cannot_erase_types_or_effects() {
    use ruddy::{
        inference, ir,
        symbol::{Bundle, Mint, Version},
    };
    for source in [
        "let forged: fn => match capture ('a) with | #A => capture (Nat) end = \"bad\"\nlet result: Nat = forged",
        "let forged: fn => match capture ('a) with | #A => capture ({r: Nat}) end = {r: \"bad\"}\nlet result: Nat = forged.r",
        "effect Log = () -> ()\nlet erased: fn => match capture ('a) with | #A => capture (() -> ()) end = fn _ => !Log ()\nlet pure: () -> () = erased",
    ] {
        let parsed = parsed(source);
        let mut mint = Mint::new(Bundle::new("residual", Version::new(0, 0, 0)).unwrap());
        let lowered = ir::build(&mut mint, parsed.stmts);
        assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
        let inferred = inference::infer(&mint, &lowered.program, inference::Trace::Off);
        assert!(
            !inferred.errors().is_empty(),
            "accepted an unresolved residual as an unchecked coercion:\n{source}"
        );
    }
}

#[test]
fn ordinary_functions_accept_equivalent_written_structural_contracts() {
    inferred_signatures(
        "let id: fn 'x => 'x = fn x => x\nlet n: Nat = id 1n\nlet s: String = id \"text\"",
    );
    inferred_signatures(
        "let get: fn 'x => 'x.r = fn x => x.r\nlet n: Nat = get {r: 1n}\nlet s: String = get {r: \"text\", other: true}",
    );
}

#[test]
fn a_captured_call_keeps_its_presence_requirement() {
    use ruddy::{
        inference, ir,
        symbol::{Bundle, Mint, Version},
    };
    let source = "let one: {x when 'p: Nat, y when 'q: Nat, ..} -> () where 'p != 'q = fn _ => ()\n\
        let route = fn v => match v with | {x, ..} => one v | _ => () end";
    let signature = inferred_signatures(source)
        .into_iter()
        .find(|(name, _)| name == "route")
        .unwrap()
        .1;
    let annotated = format!("{source}\nlet copy: {signature} = route");
    inferred_signatures(&format!(
        "{annotated}\nlet left: () = copy {{x: 1n}}\nlet right: () = copy {{y: 2n}}\nlet neither: () = copy ()"
    ));
    for source in [
        format!("{source}\nlet bad = route {{x: 1n, y: 2n}}"),
        format!("{annotated}\nlet bad = copy {{x: 1n, y: 2n}}"),
    ] {
        let parsed = parsed(&source);
        let mut mint = Mint::new(Bundle::new("captured", Version::new(0, 0, 0)).unwrap());
        let lowered = ir::build(&mut mint, parsed.stmts);
        assert!(lowered.errors.is_empty(), "{:#?}", lowered.errors);
        let inferred = inference::infer(&mint, &lowered.program, inference::Trace::Off);
        assert!(
            !inferred.errors().is_empty(),
            "capturing an arrow cannot erase its where clause: {source}"
        );
    }
}

#[test]
fn shared_contract_effect_identity_uses_the_complete_graph() {
    use ruddy::{
        ir,
        symbol::{Bundle, Mint, Version},
        types::EffectId,
    };

    let interface = |tail: &str| {
        let mut source = "type Result = fn 'x => do let 'n0 = ('x, 'x)\n".to_owned();
        for index in 1..20 {
            source.push_str(&format!(
                "let 'n{index} = ('n{}, 'n{})\n",
                index - 1,
                index - 1
            ));
        }
        source.push_str(&format!(
            "return ('n19, #{tail}) end\neffect Read = () -> Result"
        ));
        let parsed = parsed(&source);
        let mut mint = Mint::new(Bundle::new("identity", Version::new(0, 0, 0)).unwrap());
        let output = ir::build(&mut mint, parsed.stmts);
        assert!(output.errors.is_empty(), "{:#?}", output.errors);
        output
            .program
            .effect_ids
            .iter()
            .find_map(|(symbol, identity)| {
                (mint.name(*symbol) == "Read").then(|| match identity {
                    EffectId::Structural { interface, .. } => interface.clone(),
                    EffectId::Pending(_) => panic!("unfinished effect identity"),
                })
            })
            .unwrap()
    };

    // Both graphs have the same enormous unfolded prefix. Their final tag is
    // beyond the diagnostic printer's limit, but is part of effect identity.
    assert_ne!(interface("Left"), interface("Right"));
}

#[test]
fn contract_captures_cannot_hide_type_cycles() {
    use ruddy::{
        ir,
        symbol::{Bundle, Mint, Version},
    };
    for source in [
        "type T = fn 'x => (capture (T)) ('x)",
        "type T = fn => capture (T)",
    ] {
        let mut mint = Mint::new(Bundle::new("cycles", Version::new(0, 0, 0)).unwrap());
        let output = ir::build(&mut mint, parsed(source).stmts);
        assert!(
            output
                .errors
                .iter()
                .any(|error| matches!(error.kind, ir::ErrorKind::Circular { .. })),
            "unguarded cycle was accepted: {source}\n{:#?}",
            output.errors
        );
    }
    let source = "type T = fn 'x => capture ({next: T})";
    let mut mint = Mint::new(Bundle::new("productive", Version::new(0, 0, 0)).unwrap());
    let output = ir::build(&mut mint, parsed(source).stmts);
    assert!(
        output.errors.is_empty(),
        "productive recursion was rejected: {source}\n{:#?}",
        output.errors
    );
}

#[test]
fn imported_contract_aliases_keep_their_structural_effect_identity() {
    use ruddy::{
        compile, inference, ir,
        symbol::{Bundle, Mint, Version},
    };
    let dependency = compile::compile(
        Mint::new(Bundle::new("dep", Version::new(0, 0, 0)).unwrap()),
        parsed("type Callback = fn 'x => #A 'x\ntype Choice = match | #A => Nat | #B => Nat end")
            .stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|errors| panic!("{errors:#?}"));
    let source = "module Named = effect Read = () -> dep::Callback\neffect Select = () -> dep::Choice end\n\
        module Inline = effect Read = () -> (fn 'x => #A 'x)\neffect Select = () -> (match | #A => Nat | #B => Nat end) end\n\
        module Different = effect Read = () -> (fn 'x => #B 'x)\neffect Select = () -> (match | #A => Bool | #B => Bool end) end";
    let mut mint = Mint::new(Bundle::new("consumer", Version::new(0, 0, 0)).unwrap());
    let output = ir::build_with_dependencies(
        &mut mint,
        parsed(source).stmts,
        std::slice::from_ref(dependency.artifact()),
    );
    assert!(output.errors.is_empty(), "{:#?}", output.errors);
    for name in ["Read", "Select"] {
        let identities: Vec<_> = output
            .program
            .effect_ids
            .iter()
            .filter_map(|(symbol, identity)| (mint.name(*symbol) == name).then_some(identity))
            .collect();
        assert_eq!(identities.len(), 3);
        assert_eq!(identities[0], identities[1], "{name}");
        assert_ne!(identities[0], identities[2], "{name}");
    }
}

#[test]
fn unsupported_match_type_discriminators_report_the_input_type() {
    use ruddy::{
        ir,
        symbol::{Bundle, Mint, Version},
    };
    for source in [
        "type Choice = match | Nat => Nat | String => String end",
        "type Number = Nat\ntype Choice = match | Number => Nat | #Text => String end",
        "type Choice = match | (#A | #B) => Nat | #C => String end",
    ] {
        let mut mint = Mint::new(Bundle::new("discriminator", Version::new(0, 0, 0)).unwrap());
        let output = ir::build(&mut mint, parsed(source).stmts);
        assert_eq!(output.errors.len(), 1, "{source}\n{:#?}", output.errors);
        assert!(matches!(
            output.errors[0].kind,
            ir::ErrorKind::UnsupportedMatchDiscriminator
        ));
        assert_eq!(
            output.source.span(output.errors[0].at).start,
            source.find("match | ").unwrap() + "match | ".len()
        );
        assert!(
            output.errors[0]
                .kind
                .to_string()
                .contains("structural discriminator")
        );
    }
    inferred_signatures(
        "let identity: match | Nat => Nat end = fn x => x\nlet result: Nat = identity 1n",
    );
}

#[test]
fn nested_captured_contracts_share_writable_type_declarations() {
    use ruddy::{
        contracts::{Contract, Expr},
        symbol::{Bundle, Version},
        types::{Row, Scheme, Ty},
        ui::TypePresentation,
    };
    use std::sync::Arc;
    let contract = |body, captures: Vec<Arc<Ty>>, fallback| {
        Arc::new(Ty::Contract {
            fallback,
            contract: Arc::new(Contract {
                parameters: 1,
                body: Arc::new(body),
                captures: captures.into(),
                arguments: Arc::new([]),
                effect_sink: None,
            }),
        })
    };
    let apply = |capture| {
        Arc::new(Expr::Apply {
            function: Arc::new(Expr::Capture(capture)),
            argument: Arc::new(Expr::Input(0)),
        })
    };
    let parameter = Arc::new(Ty::Bound(0));
    let mut previous = contract(Expr::Capture(0), vec![parameter.clone()], parameter.clone());
    for _ in 0..20 {
        let fallback = Arc::new(Ty::Arrow(previous.clone(), previous.clone(), Row::closed()));
        let left = contract(
            Expr::Tag {
                label: "Left".into(),
                payload: apply(0),
            },
            vec![previous.clone()],
            fallback.clone(),
        );
        let right = contract(
            Expr::Tag {
                label: "Right".into(),
                payload: apply(0),
            },
            vec![previous],
            fallback.clone(),
        );
        previous = contract(
            Expr::Record {
                fields: vec![("left".into(), apply(0)), ("right".into(), apply(1))].into(),
                spread: None,
            },
            vec![left, right],
            fallback,
        );
    }
    let scheme = Scheme::new(1, previous);
    let raw = scheme.to_string();
    assert!(raw.contains("type display truncated"));
    assert!(raw.len() < 270_000, "{}", raw.len());
    let reserved = Scheme::new(0, Arc::new(Ty::Nat));
    let presentation = TypePresentation::new(&scheme, [("InferredContract", &reserved)]);
    assert_eq!(presentation.definitions.len(), 20);
    assert!(presentation.definitions[0].starts_with("type InferredContract2 'a ="));
    assert!(!presentation.to_string().contains("truncated"));
    assert!(
        presentation.to_string().len() < 15_000,
        "{}",
        presentation.to_string().len()
    );
    let source = format!(
        "type InferredContract = Nat\n{}",
        presentation.declaration("type Exported 'a = ")
    );
    inferred_signatures(&source);
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some(source.clone()));
    let analysis = host.analyze(
        Bundle::new("dag", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "{:#?}",
        analysis.diagnostics
    );
    let hover = analysis
        .hover("main.rud", source.find("Exported").unwrap())
        .unwrap();
    assert!(!hover.ty.contains("truncated"));
    assert!(hover.ty.len() < 2_000, "{}", hover.ty.len());
    inferred_signatures(&format!("{source}\ntype Restored 'a = {}", hover.ty));
}

#[test]
fn structural_extern_annotations_are_distinct_from_abi_function_spelling() {
    let source = r#"
extern structural: fn 'input => 'input = "host.structural"
extern grouped: (fn 'input => 'input) = "host.grouped"
extern shorthand: fn | 'input => 'input = "host.shorthand"
extern grouped_shorthand: (fn | 'input => 'input) = "host.grouped_shorthand"
extern foreign: fn(Nat) -> Nat = "host.foreign"
"#;
    let output = parsed(source);
    for statement in &output.stmts[..4] {
        let StmtKind::Extern { abi, .. } = &statement.kind else {
            panic!("the declaration is an extern");
        };
        assert!(matches!(abi.tracked, parse::ExternTypeKind::Ordinary(_)));
    }
    let StmtKind::Extern { abi, .. } = &output.stmts[4].kind else {
        panic!("the declaration is an extern");
    };
    assert!(matches!(
        abi.tracked,
        parse::ExternTypeKind::Function { .. }
    ));
    parsed(&printed(source));
}

fn structural_pattern_body(signature: &str) -> (usize, std::sync::Arc<ruddy::contracts::Expr>) {
    let output = parsed(&format!("let example: {signature} = example"));
    let StmtKind::Let {
        ty: Some(annotation),
        ..
    } = &output.stmts[0].kind
    else {
        panic!("an annotated binding");
    };
    let TypeKind::Structural {
        parameters,
        body,
        captures,
        effect_sink,
    } = &annotation.ty.tracked
    else {
        panic!("a structural annotation");
    };
    assert!(captures.is_empty());
    assert!(effect_sink.is_none());
    (*parameters, body.clone())
}

fn structural_pattern_roundtrip(
    parameters: usize,
    body: &std::sync::Arc<ruddy::contracts::Expr>,
) -> String {
    let signature = ruddy::ui::structural_type_text(parameters, body, &[]);
    let (restored_parameters, restored) = structural_pattern_body(&signature);
    assert_eq!(restored_parameters, parameters);
    assert!(
        ruddy::contracts::Expr::same_structure(body, &restored),
        "{signature}"
    );
    assert_eq!(
        ruddy::ui::structural_type_text(parameters, &restored, &[]),
        signature
    );
    signature
}

#[test]
fn inferred_tuple_padding_uses_pattern_names_in_its_annotation() {
    let source = r#"
let extend4 = fn
| () => (#None, #None, #None, #None)
| (a,) => (a, #None, #None, #None)
| (a, b) => (a, b, #None, #None)
| (a, b, c) => (a, b, c, #None)
| (a, b, c, d) => (a, b, c, d)
"#;
    let signatures = inferred_signatures(source);
    let signature = &signatures
        .iter()
        .find(|(name, _)| name == "extend4")
        .unwrap()
        .1;
    assert_eq!(
        signature,
        "fn | () => (#None, #None, #None, #None) | ('a,) => ('a, #None, #None, #None) | ('a, 'b) => ('a, 'b, #None, #None) | ('a, 'b, 'c) => ('a, 'b, 'c, #None) | ('a, 'b, 'c, 'd) => ('a, 'b, 'c, 'd)"
    );
    let (parameters, body) = structural_pattern_body(signature);
    structural_pattern_roundtrip(parameters, &body);
    inferred_signatures(&format!(
        "{source}\nlet annotated: {signature} = extend4\nlet result: (#Red, #Green, #None, #None) = annotated (#Red, #Green)"
    ));
}

#[test]
fn structural_pattern_names_keep_nested_scopes_and_unused_slots() {
    let (parameters, body) = structural_pattern_body(
        "fn 'outer 'inner => match 'outer with | ('outer_value,) => match 'inner with | ('inner_value, _) => ('outer_value, 'inner_value) | () => ('outer_value, #None) end | () => match 'inner with | ('only_value,) => 'only_value end end",
    );
    let signature = structural_pattern_roundtrip(parameters, &body);
    assert_eq!(
        signature,
        "fn 'input0 'input1 => match 'input0 with | ('a,) => match 'input1 with | ('b, _) => ('a, 'b) | () => ('a, #None) end | () => match 'input1 with | ('a,) => 'a end end"
    );
}

#[test]
fn structural_pattern_names_replace_nested_projection_prefixes() {
    let (parameters, body) = structural_pattern_body(
        "fn 'value => match 'value with | { payload: #Some ('first, 'second), .. } => { member: ('first).member, second: 'second, again: 'first } end",
    );
    let signature = structural_pattern_roundtrip(parameters, &body);
    assert!(
        signature.contains("{ payload: #Some (('a, 'b)), .. }"),
        "{signature}"
    );
    assert!(signature.contains("member: ('a).member"), "{signature}");
    assert!(signature.contains("second: 'b"), "{signature}");
    assert!(signature.contains("again: 'a"), "{signature}");
    assert!(!signature.contains("('input0)."), "{signature}");
}

#[test]
fn structural_pattern_names_do_not_merge_separate_computed_scrutinees() {
    use ruddy::contracts::{Arm, Expr, Pattern};
    use std::sync::Arc;

    let call = || {
        Arc::new(Expr::Apply {
            function: Arc::new(Expr::Input(0)),
            argument: Arc::new(Expr::Input(1)),
        })
    };
    let scrutinee = call();
    let body = Arc::new(Expr::Match {
        scrutinee: scrutinee.clone(),
        arms: vec![Arm {
            pattern: Pattern::Record {
                fields: vec![("0".into(), Pattern::Any)].into(),
                open: false,
            },
            body: Arc::new(Expr::Record {
                fields: vec![
                    (
                        "matched".into(),
                        Arc::new(Expr::Field {
                            base: scrutinee,
                            label: "0".into(),
                        }),
                    ),
                    (
                        "again".into(),
                        Arc::new(Expr::Field {
                            base: call(),
                            label: "0".into(),
                        }),
                    ),
                ]
                .into(),
                spread: None,
            }),
        }]
        .into(),
    });
    let signature = structural_pattern_roundtrip(2, &body);
    assert!(signature.contains("| ('a,) =>"), "{signature}");
    assert!(signature.contains("matched: 'a"), "{signature}");
    assert_eq!(
        signature.matches("('input0) ('input1)").count(),
        2,
        "{signature}"
    );
    let (_, restored) = structural_pattern_body(&signature);
    let Expr::Match { scrutinee, arms } = &*restored else {
        panic!("the match remains intact");
    };
    let Expr::Record { fields, .. } = &*arms[0].body else {
        panic!("the arm returns the two observations");
    };
    for (name, value) in fields.iter() {
        let Expr::Field { base, .. } = &**value else {
            panic!("each observation projects a result");
        };
        assert_eq!(Arc::ptr_eq(base, scrutinee), name == "matched");
    }
}

#[test]
fn structural_pattern_names_recognize_separately_allocated_input_paths() {
    use ruddy::contracts::{Arm, Expr, Pattern};
    use std::sync::Arc;

    let project = || {
        Arc::new(Expr::Field {
            base: Arc::new(Expr::Input(0)),
            label: "item".into(),
        })
    };
    let body = Arc::new(Expr::Match {
        scrutinee: Arc::new(Expr::Input(0)),
        arms: vec![Arm {
            pattern: Pattern::Record {
                fields: vec![("item".into(), Pattern::Any)].into(),
                open: true,
            },
            body: Arc::new(Expr::Record {
                fields: vec![("0".into(), project()), ("1".into(), project())].into(),
                spread: None,
            }),
        }]
        .into(),
    });
    assert_eq!(
        structural_pattern_roundtrip(1, &body),
        "fn | { item: 'a, .. } => ('a, 'a)"
    );

    // Tuple positions retain numeric order even when the semantic field list
    // arrived in lexical order. The nested patterns deliberately share their
    // leaf allocation: their binding names still belong to separate paths.
    let shared_fields: Arc<[(String, Pattern)]> = Arc::from([("item".into(), Pattern::Any)]);
    let mut fields: Vec<_> = (0..12)
        .map(|index| {
            (
                index.to_string(),
                Pattern::Record {
                    fields: shared_fields.clone(),
                    open: true,
                },
            )
        })
        .collect();
    fields.sort_by(|(left, _), (right, _)| left.cmp(right));
    let body = Arc::new(Expr::Match {
        scrutinee: Arc::new(Expr::Input(0)),
        arms: vec![Arm {
            pattern: Pattern::Record {
                fields: fields.into(),
                open: false,
            },
            body: Arc::new(Expr::Record {
                fields: (0..12)
                    .map(|index| {
                        (
                            index.to_string(),
                            Arc::new(Expr::Field {
                                base: Arc::new(Expr::Field {
                                    base: Arc::new(Expr::Input(0)),
                                    label: index.to_string(),
                                }),
                                label: "item".into(),
                            }),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
                spread: None,
            }),
        }]
        .into(),
    });
    let signature = ruddy::ui::structural_type_text(1, &body, &[]);
    let names: Vec<_> = ('a'..='l').map(|name| format!("'{name}")).collect();
    let patterns = names
        .iter()
        .map(|name| format!("{{ item: {name}, .. }}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        signature,
        format!("fn | ({patterns}) => ({})", names.join(", "))
    );
    let (parameters, restored) = structural_pattern_body(&signature);
    assert_eq!(
        structural_pattern_roundtrip(parameters, &restored),
        signature
    );
}

#[test]
fn structural_pattern_naming_leaves_deep_recovery_graphs_printable() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            use ruddy::contracts::{Arm, Expr, Pattern};
            use std::sync::Arc;

            const DEPTH: usize = 10_000;
            let mut chain = Arc::new(Expr::Field {
                base: Arc::new(Expr::Input(0)),
                label: "item".into(),
            });
            // Keep nodes alive independently so cleanup does not test Arc's
            // recursive destruction rather than the formatter's stack use.
            let mut retained = vec![chain.clone()];
            for _ in 0..DEPTH {
                chain = Arc::new(Expr::Field {
                    base: chain,
                    label: "next".into(),
                });
                retained.push(chain.clone());
            }
            let body = Arc::new(Expr::Match {
                scrutinee: Arc::new(Expr::Input(0)),
                arms: vec![Arm {
                    pattern: Pattern::Record {
                        fields: vec![("item".into(), Pattern::Any)].into(),
                        open: true,
                    },
                    body: chain,
                }]
                .into(),
            });
            let signature = ruddy::ui::structural_type_text(1, &body, &[]);
            assert!(signature.contains("| { item: _, .. } =>"));
            assert!(signature.contains("('input0).item"));
            assert_eq!(signature.matches(".next").count(), DEPTH);
            assert!(signature.ends_with(" end"));
            assert!(!signature.contains("truncated"));
            drop(body);
            while let Some(node) = retained.pop() {
                drop(node);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
