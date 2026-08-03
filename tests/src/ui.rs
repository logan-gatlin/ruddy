//! Tests for [`ruddy::ui`].
//!
//! The module exists so that everything the compiler says to a person can be
//! audited in one place. These check the properties that audit relies on: that
//! every complaint a phase can raise reaches a reader worded and coded, that no
//! two of them are coded the same, and that the wording is shaped so a reporter
//! can drop it into a line of its own choosing.

use std::{collections::HashSet, rc::Rc};

use ruddy::{
    inference::{self, Constraint, ConstraintKind, Effect, ErrorKind as TypeError, Rule},
    ir::{self, ErrorKind as IrError},
    parse,
    symbol::{Bundle, Mint, Namespace, Version},
    token::{self, ErrorKind as LexError},
    tracking::{FileID, Span},
    types::{RowField, Ty},
    ui,
};

/// Every error kind in the compiler, with the phase that raises it. Listed by
/// hand because nothing can force it: a new variant added without a line here
/// is the exact thing this module exists to catch, so it is worth the reminder
/// that adding one means coming back.
fn diagnostics() -> Vec<(&'static str, &'static str, String)> {
    let span = Span::generated(0, 1);
    let nat = Rc::new(Ty::Nat);

    let mut all: Vec<(&str, &str, String)> = Vec::new();
    for kind in [
        LexError::Unrecognized,
        LexError::MalformedNatural,
        LexError::NaturalTooLarge,
    ] {
        all.push(("lex", kind.code(), kind.to_string()));
    }

    let unexpected = parse::Error { span };
    all.push(("parse", unexpected.code(), unexpected.to_string()));

    // Both namespaces of both name errors: the namespace is part of the code,
    // so an undefined type and an undefined term are two diagnostics here.
    for namespace in [Namespace::Terms, Namespace::Types] {
        for kind in [
            IrError::Undefined { namespace },
            IrError::Duplicate {
                namespace,
                previous: span,
            },
        ] {
            all.push(("ir", kind.code(), kind.to_string()));
        }
    }
    for kind in [
        IrError::DuplicateField,
        IrError::Circular,
        IrError::OpenDeclaredType,
        IrError::Arity {
            expected: 2,
            found: 1,
        },
        IrError::NotAConstructor,
        IrError::ParameterApplied,
        IrError::DuplicateParameter { previous: span },
        IrError::NonUniformRecursion,
    ] {
        all.push(("ir", kind.code(), kind.to_string()));
    }

    for kind in [
        TypeError::Mismatch {
            expected: nat.clone(),
            actual: Rc::new(Ty::Undecided),
        },
        TypeError::Recursive,
        TypeError::MissingField {
            base: nat.clone(),
            field: "x".to_string(),
        },
        TypeError::ExtraField {
            base: nat.clone(),
            field: "x".to_string(),
        },
        TypeError::NotAStruct { base: nat.clone() },
        TypeError::AnnotationTooOpen,
        TypeError::RepeatedField {
            field: "x".to_string(),
        },
    ] {
        all.push(("types", kind.code(), kind.to_string()));
    }
    all
}

/// Every rule the solver can apply. Same reasoning as [`diagnostics`].
const RULES: &[Rule] = &[
    Rule::Absorb,
    Rule::Same,
    Rule::Congruent,
    Rule::Bind,
    Rule::Occurs,
    Rule::Overlap,
    Rule::Prim,
    Rule::Arrow,
    Rule::Struct,
    Rule::Presence,
    Rule::Unfold,
    Rule::Assume,
    Rule::Mismatch,
    Rule::Recover,
];

/// A reporter embeds a message in a line it lays out itself — the CLI puts a
/// position before it and a quoted snippet after, the debugger's strip puts a
/// snippet after and sometimes a second line under. That only works while every
/// message is a phrase rather than a sentence: no leading capital to look wrong
/// mid-line, no trailing period to sit beside the snippet that follows it.
#[test]
fn every_complaint_is_a_phrase_a_reporter_can_place() {
    for (phase, code, message) in diagnostics() {
        let first = message
            .chars()
            .next()
            .unwrap_or_else(|| panic!("{phase}/{code} says something"));
        assert!(!first.is_ascii_uppercase(), "{phase}/{code}: {message}");
        assert!(!message.ends_with('.'), "{phase}/{code}: {message}");
    }
}

/// Codes are what a reporter keys on — the strip tags a diagnostic with one,
/// and a test greps for one — so two kinds sharing a code would silently
/// conflate them. Checked across the whole compiler rather than per phase: the
/// strip mixes every phase's diagnostics into one list.
#[test]
fn no_two_kinds_of_error_are_coded_the_same() {
    let all = diagnostics();
    let codes: HashSet<_> = all.iter().map(|(_, code, _)| *code).collect();
    assert_eq!(codes.len(), all.len(), "{all:#?}");
}

/// A code is meant to be grepped for and typed into a filter, so it stays a
/// lowercase kebab-case word rather than anything needing quoting.
#[test]
fn a_code_is_one_greppable_word() {
    for (phase, code, _) in diagnostics() {
        assert!(
            !code.is_empty()
                && code
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{phase}: {code}"
        );
    }
}

/// The debugger's Solve tab labels a row with the code and explains it with the
/// message, so a rule that shared either with another would make two different
/// acts of the solver read as one.
#[test]
fn every_rule_is_named_and_explained_distinctly() {
    let codes: HashSet<_> = RULES.iter().map(|rule| rule.code()).collect();
    let messages: HashSet<_> = RULES.iter().map(|rule| rule.to_string()).collect();
    assert_eq!(codes.len(), RULES.len());
    assert_eq!(messages.len(), RULES.len());
    for rule in RULES {
        assert!(!rule.to_string().is_empty(), "{rule:?}");
    }
}

/// The namespaces appear in a message as the noun the complaint is about —
/// "undefined type", "duplicate term" — so two of them spelled the same would
/// make one error read as another.
#[test]
fn the_namespaces_are_spelled_apart() {
    let spellings: HashSet<_> = [Namespace::Terms, Namespace::Types, Namespace::Modules]
        .iter()
        .map(|namespace| namespace.to_string())
        .collect();
    assert_eq!(spellings.len(), 3, "{spellings:?}");
}

/// A constraint prints as what it demands, in the notation the Constraints tab
/// shows it in. `~` is "must unify with".
#[test]
fn a_constraint_reads_as_what_it_demands() {
    let nat = Rc::new(Ty::Nat);
    let span = Span::generated(0, 1);

    let equal = Constraint {
        span,
        base_span: None,
        kind: ConstraintKind::Equal {
            expected: nat.clone(),
            actual: Rc::new(Ty::Var(0)),
        },
    };
    assert_eq!(equal.to_string(), "Nat ~ ?0");
    // The constraint prints as its kind, so the two cannot drift.
    assert_eq!(equal.to_string(), equal.kind.to_string());
}

/// An effect is one line beside the rule that produced it. A failure says the
/// error and nothing else, so the row reads as the complaint rather than as a
/// wrapper around one.
#[test]
fn an_effect_reads_as_the_one_thing_that_changed() {
    assert_eq!(Effect::None.to_string(), "no change");
    assert_eq!(
        Effect::Bound {
            var: 3,
            ty: Rc::new(Ty::Nat)
        }
        .to_string(),
        "?3 := Nat"
    );
    let failure = TypeError::Recursive;
    assert_eq!(
        Effect::Failed(failure.clone()).to_string(),
        failure.to_string()
    );
}

/// A printed type, put back through the compiler: lexed, parsed, lowered and
/// inferred as part of a definition's annotation, and printed again from the
/// scheme that came out. `prelude` declares any type the printed one names.
///
/// The type is planted in a field of the parameter, where the surface grammar
/// can never want parentheses around it. So what comes back is the printed
/// type verbatim inside a wrapper of a shape this function already knows,
/// rather than a string the test would have to re-derive the grouping rules to
/// predict — which would be the rules under test standing in as their own
/// expectation.
fn round_trip(prelude: &str, printed: &str) -> String {
    let source = format!("{prelude}let f : {{ v: {printed} }} -> Nat = fn r => 1\n");
    let lexed = token::lex(&source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{source}: {:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);

    let bundle = Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{source}: {:#?}", built.errors);
    let inferred = inference::infer(&mint, &mut built.program);
    assert!(
        inferred.errors.is_empty(),
        "{source}: {:#?}",
        inferred.errors
    );

    let (symbol, _) = built.program.terms.last().expect("the definition");
    let scheme = inferred.schemes[symbol].to_string();
    scheme
        .strip_prefix("{ v: ")
        .and_then(|rest| rest.strip_suffix(" } -> Nat"))
        .unwrap_or_else(|| panic!("{source}: unexpected scheme `{scheme}`"))
        .to_string()
}

/// The writers that put the parentheses in, exercised through the one compiler
/// type that prints as surface syntax directly. The debugger's tree printers go
/// through the same four functions, so what holds here holds of a printed tree.
///
/// Each closed type here is printed *and* read back: the string goes through
/// the lexer, the parser, lowering and inference, and has to come out spelling
/// itself. String equality alone would only pin that the printer does not
/// change; what is worth pinning is that what it prints is source, which is the
/// whole claim a diagnostic quoting a type makes.
#[test]
fn a_printed_closed_type_reads_back_as_the_type_it_was_printed_from() {
    let nat = Rc::new(Ty::Nat);
    let endo = Rc::new(Ty::Arrow(nat.clone(), nat.clone()));

    // A declared type prints as its name and is an atom whatever it stands
    // for, so the arrow behind this one leaks no parentheses through it. It
    // needs a declaration to be read back, which the prelude supplies.
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Endo")
        .expect("a fresh name");
    let named = Rc::new(Ty::Named {
        symbol,
        name: "Endo".into(),
        args: Rc::from([]),
    });

    for (prelude, ty, printed) in [
        ("", nat.clone(), "Nat"),
        // Right-associative: only the left side can ever need grouping, and
        // the right side must not acquire any.
        (
            "",
            Rc::new(Ty::Arrow(endo.clone(), endo.clone())),
            "(Nat -> Nat) -> Nat -> Nat",
        ),
        // The empty struct is unit, and prints as the one spelling this
        // language has for it. See `Ty::Struct` for why it is `{}` and not
        // `()`.
        (
            "",
            Rc::new(Ty::Struct {
                fields: Default::default(),
                rest: Rc::new(Ty::Empty),
            }),
            "{}",
        ),
        (
            "",
            Rc::new(Ty::Struct {
                fields: [("x".to_string(), RowField::present(endo.clone()))]
                    .into_iter()
                    .collect(),
                rest: Rc::new(Ty::Empty),
            }),
            "{ x: Nat -> Nat }",
        ),
        (
            "type Endo = Nat -> Nat\n",
            Rc::new(Ty::Arrow(named.clone(), named.clone())),
            "Endo -> Endo",
        ),
    ] {
        assert_eq!(ty.to_string(), printed);
        assert_eq!(round_trip(prelude, printed), printed);
    }
}

/// An open row prints in the surface notation too, and cannot be read back
/// from it — so these are pinned as printing and nothing more.
///
/// Not an oversight in the printer. What a row's tail and a field's `?` stand
/// for is an identity: `{ x: Nat, ..'a } -> { x: Nat, ..'a }` says the two
/// tails are the *same* rest, and `..'a` is how that is spelled. The surface
/// syntax has `..r`, which says the same thing by a name the writer chose —
/// but a scheme has no names to offer, only numbers the quantifier handed out,
/// so printing one back as `..r` would be inventing a name that was never
/// written. `..` alone loses the identity, and `?` on a field is likewise a
/// variable the syntax gives no way to name.
///
/// So the printed form is a faithful picture and not a re-readable one. Every
/// reader of it is being shown a conclusion rather than handed something to
/// compile, which is what makes that the right trade.
#[test]
fn an_open_row_prints_in_surface_notation_it_cannot_be_read_back_from() {
    let nat = Rc::new(Ty::Nat);

    // A row's tail prints in the surface spelling, after the fields; a
    // quantified tail wears its letter and an undecided one has nothing to
    // report. An open row with no fields still shows it is open.
    assert_eq!(
        Ty::Struct {
            fields: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
            rest: Rc::new(Ty::Bound(0)),
        }
        .to_string(),
        "{ x: Nat, ..'a }"
    );
    assert_eq!(
        Ty::Struct {
            fields: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
            rest: Rc::new(Ty::Undecided),
        }
        .to_string(),
        "{ x: Nat, .. }"
    );
    assert_eq!(
        Ty::Struct {
            fields: Default::default(),
            rest: Rc::new(Ty::Bound(0)),
        }
        .to_string(),
        "{ ..'a }"
    );

    // A field's presence prints as its surface spelling too: certainly there
    // is unmarked, undecided either way is `?`, and certainly absent is not
    // part of what the type says at all.
    assert_eq!(
        Ty::Struct {
            fields: [
                (
                    "x".to_string(),
                    RowField {
                        presence: Rc::new(Ty::Bound(0)),
                        ty: nat.clone(),
                    }
                ),
                (
                    "y".to_string(),
                    RowField {
                        presence: Rc::new(Ty::Absent),
                        ty: nat.clone(),
                    }
                ),
            ]
            .into_iter()
            .collect(),
            rest: Rc::new(Ty::Empty),
        }
        .to_string(),
        "{ x?: Nat }"
    );

    // `{ x: Nat, .. }` does parse, and what it parses to is a row with a fresh
    // tail of its own — not the one it was printed from. The string survives;
    // the identity it was standing for does not.
    assert_eq!(round_trip("", "{ x: Nat, .. }"), "{ x: Nat, ..'a }");
}

/// The note is held in [`ui`] rather than in either reporter, because both
/// print it: the CLI as a second indented line, the strip as a second highlight
/// on the same diagnostic.
#[test]
fn the_duplicate_note_is_worded_once() {
    assert!(!ui::FIRST_DEFINITION.is_empty());
    assert!(!ui::FIRST_DEFINITION.ends_with('.'));
}
