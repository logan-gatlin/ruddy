//! Tests for [`ruddy::ui`].
//!
//! The module exists so that everything the compiler says to a person can be
//! audited in one place. These check the properties that audit relies on: that
//! every complaint a phase can raise reaches a reader worded and coded, that no
//! two of them are coded the same, and that the wording is shaped so a reporter
//! can drop it into a line of its own choosing.

use std::{collections::HashSet, rc::Rc};

use ruddy::{
    inference::{Constraint, ConstraintKind, Effect, ErrorKind as TypeError, Rule},
    ir::ErrorKind as IrError,
    parse,
    symbol::{Bundle, Mint, Namespace, Version},
    token::ErrorKind as LexError,
    tracking::Span,
    types::Ty,
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
    for kind in [IrError::DuplicateField, IrError::Circular] {
        all.push(("ir", kind.code(), kind.to_string()));
    }

    for kind in [
        TypeError::Mismatch {
            expected: nat.clone(),
            actual: Rc::new(Ty::Undecided),
        },
        TypeError::Recursive,
        TypeError::NotAStruct { base: nat.clone() },
        TypeError::UnknownBase,
        TypeError::MissingField {
            base: nat.clone(),
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
    Rule::Bind,
    Rule::Occurs,
    Rule::Prim,
    Rule::Arrow,
    Rule::Struct,
    Rule::Unfold,
    Rule::Assume,
    Rule::Mismatch,
    Rule::Project,
    Rule::Defer,
    Rule::Stuck,
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
/// shows it in. `~` is "must unify with"; a projection keeps its dot.
#[test]
fn a_constraint_reads_as_what_it_demands() {
    let nat = Rc::new(Ty::Nat);
    let span = Span::generated(0, 1);

    let equal = Constraint {
        span,
        kind: ConstraintKind::Equal {
            expected: nat.clone(),
            actual: Rc::new(Ty::Var(0)),
        },
    };
    assert_eq!(equal.to_string(), "Nat ~ ?0");
    // The constraint prints as its kind, so the two cannot drift.
    assert_eq!(equal.to_string(), equal.kind.to_string());

    let field = ConstraintKind::Field {
        base: Rc::new(Ty::Var(1)),
        base_span: span,
        name: "x".to_string(),
        result: nat,
    };
    assert_eq!(field.to_string(), "?1.x ~ Nat");
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

/// The writers that put the parentheses in, exercised through the one compiler
/// type that prints as surface syntax directly. The debugger's tree printers go
/// through the same four functions, so what holds here holds of a printed tree.
#[test]
fn a_printed_type_re_parses_as_the_type_it_was_printed_from() {
    let nat = Rc::new(Ty::Nat);
    let endo = Rc::new(Ty::Arrow(nat.clone(), nat.clone()));

    // Right-associative: only the left side can ever need grouping, and the
    // right side must not acquire any.
    assert_eq!(
        Ty::Arrow(endo.clone(), endo.clone()).to_string(),
        "(Nat -> Nat) -> Nat -> Nat"
    );

    // The empty struct is unit, and prints as the one spelling this language
    // has for it. See `Ty::Struct` for why it is `{}` and not `()`.
    assert_eq!(Ty::Struct(Default::default()).to_string(), "{}");
    assert_eq!(
        Ty::Struct([("x".to_string(), endo)].into_iter().collect()).to_string(),
        "{ x: Nat -> Nat }"
    );

    // A declared type prints as its name and is an atom whatever it stands
    // for, so the arrow behind this one leaks no parentheses through it.
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Endo")
        .expect("a fresh name");
    let named = Rc::new(Ty::Named {
        symbol,
        name: "Endo".into(),
    });
    assert_eq!(named.to_string(), "Endo");
    assert_eq!(Ty::Arrow(named.clone(), named).to_string(), "Endo -> Endo");
}

/// The note is held in [`ui`] rather than in either reporter, because both
/// print it: the CLI as a second indented line, the strip as a second highlight
/// on the same diagnostic.
#[test]
fn the_duplicate_note_is_worded_once() {
    assert!(!ui::FIRST_DEFINITION.is_empty());
    assert!(!ui::FIRST_DEFINITION.ends_with('.'));
}
