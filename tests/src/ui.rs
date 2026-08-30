//! Tests for [`ruddy::ui`].
//!
//! The module exists so that everything the compiler says to a person can be
//! audited in one place. These check the properties that audit relies on: that
//! every complaint a phase can raise reaches a reader worded and coded, that no
//! two of them are coded the same, and that the wording is shaped so a reporter
//! can drop it into a line of its own choosing.

use std::{
    collections::HashSet,
    fmt::{self, Write as _},
    rc::Rc,
};

use indexmap::IndexMap;
use ruddy::{
    bundle::ErrorKind as BundleError,
    inference::{self, Constraint, ConstraintKind, Effect, ErrorKind as TypeError, Goal, Rule},
    ir::{self, ErrorKind as IrError},
    parse,
    patterns::ErrorKind as PatternError,
    symbol::{Bundle, Mint, Namespace, Version},
    token::{self, ErrorKind as LexError, Kind as TokenKind},
    tracking::{FileID, Span},
    types::{Assigned, EffectId, Formula, Presence, Prim, Rest, Row, RowField, Sense, Shape, Ty},
    ui::{self, Entry, Mark},
};
use ruddy_debug::print;

/// One value of every inference error variant. Shared by the inventory and the
/// structured-diagnostic audit so those two hand-maintained checks cannot
/// silently drift apart.
fn inference_error_kinds(span: Span) -> Vec<TypeError> {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    vec![
        TypeError::NotAStruct { base: nat.clone() },
        TypeError::Mismatch {
            expected: nat.clone(),
            actual: Rc::new(Ty::default()),
        },
        TypeError::Recursive,
        TypeError::MissingField {
            shape: Shape::Struct,
            base: nat.clone(),
            field: "x".to_string(),
        },
        TypeError::ExtraField {
            shape: Shape::Struct,
            base: nat.clone(),
            field: "x".to_string(),
        },
        TypeError::RigidBroken {
            found: nat.clone(),
            name: "a".into(),
            sense: Sense::Type,
            declared: span,
        },
        TypeError::RigidField {
            shape: Shape::Struct,
            field: "x".to_string(),
            name: "a".into(),
            declared: span,
        },
        TypeError::RigidEscapes { name: "a".into() },
        TypeError::RepeatedField {
            shape: Shape::Struct,
            field: "x".to_string(),
        },
        TypeError::PresenceRequired {
            formula: "x != y".to_string(),
            shape: Some(Shape::Struct),
        },
        TypeError::PresenceImpossible {
            formula: "x and y".to_string(),
        },
        TypeError::ClauseImpossible {
            formula: "a and not a".to_string(),
        },
        TypeError::AnnotationAllows {
            allowed: "a or b".to_string(),
            required: "a".to_string(),
        },
        TypeError::Unhandled {
            effect: "Log".to_string(),
        },
        TypeError::NotAllowed {
            effect: "Log".to_string(),
        },
        TypeError::CallbackEffectsNotCovered,
        TypeError::PolymorphicExternBoundary,
    ]
}

/// Every error kind in the compiler, with the phase that raises it. Listed by
/// hand because nothing can force it: a new variant added without a line here
/// is the exact thing this module exists to catch, so it is worth the reminder
/// that adding one means coming back.
fn diagnostics() -> Vec<(&'static str, &'static str, String)> {
    let span = Span::generated(0, 1);

    let mut all: Vec<(&str, &str, String)> = Vec::new();
    for kind in [
        LexError::InvalidCharacter { character: '@' },
        LexError::MalformedTag,
        LexError::MalformedEffectLabel,
        LexError::MalformedVariable,
        LexError::NumberFollowedByName,
        LexError::DecimalWithWholeSuffix { suffix: 'n' },
        LexError::MalformedNumericField,
        LexError::NaturalTooLarge,
        LexError::IntegerTooLarge,
        LexError::RealTooLarge,
        LexError::NumericFieldTooLarge,
        LexError::UnknownStringEscape { escape: 'q' },
        LexError::MissingClosingQuote,
    ] {
        all.push(("lex", kind.code(), kind.to_string()));
    }

    // Every kind. The expected category supplies the specific code and prose;
    // focused tests below cover its context-sensitive forms.
    for kind in [
        parse::ErrorKind::Expected {
            expected: parse::Expected::Value,
            found: parse::Found::Token,
            related: None,
            context: None,
        },
        parse::ErrorKind::Wildcard {
            place: parse::Place::Value,
        },
    ] {
        let error = parse::Error { span, kind };
        all.push(("parse", error.code(), error.to_string()));
    }

    // Loading can refuse either spelling of a module file.
    for kind in [
        BundleError::ModuleFileMissing {
            beside: "Math.hc".to_string(),
            inside: "Math/module.hc".to_string(),
        },
        BundleError::ModuleFileAmbiguous {
            beside: "Math.hc".to_string(),
            inside: "Math/module.hc".to_string(),
        },
    ] {
        all.push(("bundle", kind.code(), kind.to_string()));
    }

    // Every namespace of the two name errors: the namespace is part of the
    // code, so an undefined type and an undefined term are two diagnostics
    // here, and so are the two halves of a loop of bare names.
    for namespace in [
        Namespace::Terms,
        Namespace::Types,
        Namespace::Effects,
        Namespace::Modules,
    ] {
        for kind in [
            IrError::Undefined {
                name: "x".to_string(),
                namespace,
            },
            IrError::Duplicate {
                name: "x".to_string(),
                namespace,
                previous: span,
            },
        ] {
            all.push(("ir", kind.code(), kind.to_string()));
        }
    }
    // A loop of bare names is a term's mistake or a type's; an effect stands
    // for a set of effects however many aliases it reaches through, so nothing
    // about one can lead nowhere.
    for namespace in [Namespace::Terms, Namespace::Types] {
        let kind = IrError::Circular { namespace };
        all.push(("ir", kind.code(), kind.to_string()));
    }
    for kind in [
        IrError::InvalidDependencyAlias {
            alias: "bad-alias".to_string(),
        },
        IrError::DuplicateDependencyAlias {
            alias: "math".to_string(),
        },
        IrError::DuplicateDependency {
            name: "math".to_string(),
            version: "1.2.3".to_string(),
        },
        IrError::DuplicateField {
            name: "x".to_string(),
            previous: span,
        },
        IrError::AbsentInClosed {
            shape: Shape::Struct,
            label: "x".to_string(),
        },
        IrError::OpenDeclaredType {
            shape: Shape::Struct,
        },
        IrError::Arity {
            name: "Pair".to_string(),
            expected: 2,
            found: 1,
        },
        IrError::NotAConstructor,
        IrError::ParameterApplied {
            name: "f".to_string(),
        },
        IrError::DuplicateParameter {
            name: "a".to_string(),
            previous: span,
        },
        IrError::GrowingRecursion,
        IrError::DuplicateCase {
            shape: Shape::Sum,
            name: "A".to_string(),
            previous: span,
        },
        IrError::MixedTail {
            first: Sense::Type,
            second: Sense::Cases,
            previous: span,
        },
        IrError::MixedParameter {
            first: Sense::Type,
            second: Sense::Cases,
        },
        IrError::NotARow {
            sense: Sense::Cases,
        },
        IrError::RepeatedRowField {
            shape: Shape::Struct,
            field: "x".to_string(),
        },
        IrError::EndlessFields,
        IrError::RefutableBinding {
            found: ir::Refuter::Case("Some".to_string()),
        },
        IrError::DuplicateBinding {
            name: "x".to_string(),
            previous: span,
        },
        IrError::ClauseInDeclaration,
        IrError::VariableInDeclaration {
            name: "a".to_string(),
        },
        IrError::HoleInDeclaration,
        IrError::HoleInOperation,
        IrError::UnboundPresence {
            name: "c".to_string(),
        },
        IrError::IncompatiblePresenceOwnership {
            name: "c".to_string(),
            previous: span,
        },
        IrError::DuplicateOperation {
            name: "write".to_string(),
            previous: span,
        },
        IrError::NotAnOperation {
            name: "here".to_string(),
        },
        IrError::ImpureOperation {
            found: ir::OperationTypeProblem::OpenPart,
        },
        IrError::EffectsOutsideRow,
        IrError::BareOperationUnavailable {
            effect: "Log".to_string(),
            suggestion: Some("write".to_string()),
        },
        IrError::NamedOperationOnUnnamed {
            effect: "Log".to_string(),
            op: "write".to_string(),
        },
        IrError::OperationOnAlias {
            effect: "Console".to_string(),
        },
        IrError::UnknownOperation {
            effect: "Log".to_string(),
            op: "writ".to_string(),
        },
        IrError::PartialHandler {
            effect: "Log".to_string(),
            missing: vec!["flush".to_string()],
        },
        IrError::DuplicateArm {
            effect: "Log".to_string(),
            selector: ir::OperationSelector::Named("write".to_string()),
            previous: span,
        },
        IrError::DuplicateReturn { previous: span },
        IrError::RaiseOutsideArm,
        IrError::RaiseInFunction { function: span },
    ] {
        all.push(("ir", kind.code(), kind.to_string()));
    }

    // The pattern checks, which own the match complaints since they moved
    // out of lowering and behind inference.
    for kind in [
        PatternError::MisplacedCatchAll,
        PatternError::UnreachableArm,
        PatternError::UnhandledValues {
            witness: ir::Witness::Tag {
                name: "A".to_string(),
                payload: None,
            },
        },
        PatternError::UnhandledNumbers,
    ] {
        all.push(("patterns", kind.code(), kind.to_string()));
    }

    for kind in inference_error_kinds(span) {
        all.push(("types", kind.code(), kind.to_string()));
    }
    all
}

/// Every rule the solver can apply. Same reasoning as [`diagnostics`].
///
/// One shape apiece for the two rules that carry one: what they say about the
/// other is the same sentence in the other noun, which
/// [`a_shaped_rule_is_read_in_the_nouns_of_its_shape`] pins on its own. Listed
/// twice here, they would read as two rules sharing a code, which is exactly
/// what [`every_rule_is_named_and_explained_distinctly`] exists to refuse.
const RULES: &[Rule] = &[
    Rule::Absorb,
    Rule::Same,
    Rule::Congruent,
    Rule::Bind,
    Rule::Occurs,
    Rule::Overlap {
        shape: Shape::Struct,
    },
    Rule::Prim,
    Rule::Arrow,
    Rule::Performs,
    Rule::Struct,
    Rule::Refine,
    Rule::Sum,
    Rule::Presence {
        shape: Shape::Struct,
    },
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
        assert!(
            !first.is_ascii_uppercase() || message.starts_with("Ruddy "),
            "{phase}/{code}: {message}"
        );
        assert!(!message.ends_with('.'), "{phase}/{code}: {message}");
    }
}

/// Codes are what a reporter keys on — the strip tags a diagnostic with one,
/// and a test greps for one — so two kinds sharing a code would silently
/// conflate them. Checked across the whole compiler rather than per phase: the
/// strip mixes every phase's diagnostics into one list.
#[test]
fn numeric_lexer_complaints_name_the_number_category_written() {
    for (kind, code, message) in [
        (
            LexError::NumberFollowedByName,
            "number-joined-to-name",
            "a number cannot run directly into a name",
        ),
        (
            LexError::DecimalWithWholeSuffix { suffix: 'i' },
            "decimal-marked-whole",
            "`i` is only used with whole numbers",
        ),
        (
            LexError::MalformedNumericField,
            "invalid-field-number",
            "a numbered field can contain only digits",
        ),
        (
            LexError::NaturalTooLarge,
            "whole-number-too-large",
            "this whole number is too large",
        ),
        (
            LexError::IntegerTooLarge,
            "integer-too-large",
            "this integer is too large",
        ),
        (
            LexError::RealTooLarge,
            "number-too-large",
            "this number is too large",
        ),
        (
            LexError::NumericFieldTooLarge,
            "field-number-too-large",
            "this field number is too large",
        ),
    ] {
        assert_eq!(kind.code(), code);
        assert_eq!(kind.to_string(), message);
    }
}

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

/// The two rules that decide one label at a time say which kind of label it
/// was, because a reader stepping through a solve is reading about their own
/// program: "whether the field is there" over a goal about `#Some` and
/// `#None` describes something they never wrote. The same reason
/// `Rule::Struct` and `Rule::Sum` are two rules rather than one worded about
/// rows — and the reason neither says "label", a word the language does not
/// have anywhere a reader can see it.
///
/// The code is shared by the two shapes on purpose: it names which act of the
/// solver ran, and the goal beside it already shows which shape it ran on.
#[test]
fn a_shaped_rule_is_read_in_the_nouns_of_its_shape() {
    for shape in [Shape::Struct, Shape::Sum, Shape::Effect] {
        let (theirs, others) = match shape {
            Shape::Struct => ("field", "case"),
            Shape::Sum => ("case", "field"),
            Shape::Effect => ("effect", "field"),
        };
        for rule in [Rule::Presence { shape }, Rule::Overlap { shape }] {
            let message = rule.to_string();
            assert!(message.contains(theirs), "{rule:?}: {message}");
            assert!(!message.contains(others), "{rule:?}: {message}");
            // The two words that name the representation the shapes share
            // rather than anything a reader wrote.
            assert!(!message.contains("label"), "{rule:?}: {message}");
            assert!(!message.contains("row"), "{rule:?}: {message}");
        }
    }

    for (struct_shaped, sum_shaped) in [
        (
            Rule::Presence {
                shape: Shape::Struct,
            },
            Rule::Presence { shape: Shape::Sum },
        ),
        (
            Rule::Overlap {
                shape: Shape::Struct,
            },
            Rule::Overlap { shape: Shape::Sum },
        ),
    ] {
        assert_eq!(struct_shaped.code(), sum_shaped.code());
        assert_ne!(struct_shaped.to_string(), sum_shaped.to_string());
    }
}

/// The Solve tab lays the rule out as a column, so that the goal beside it
/// starts at the same place on every row and two rows can be compared by eye.
/// The column's width is a number in the stylesheet and the codes are strings
/// in `ruddy::ui`, and nothing but this connects them: a rule spelled longer
/// than the column pushes the goal on its own rows and nothing lines up, which
/// is the whole failure the fixed width exists to prevent.
#[test]
fn the_solve_tab_is_wide_enough_for_every_rule() {
    let css = include_str!("../../debug/web/style.css");
    let rule = css
        .split(".step-row .label {")
        .nth(1)
        .expect("the rule column is styled");
    let width: usize = rule
        .split("min-width:")
        .nth(1)
        .and_then(|rest| rest.split("ch").next())
        .expect("the column has a min-width in characters")
        .trim()
        .parse()
        .expect("the min-width is a whole number of characters");

    let longest = RULES
        .iter()
        .map(|rule| rule.code().chars().count())
        .max()
        .expect("there are rules");
    assert_eq!(
        width, longest,
        "the column is {width}ch and the longest rule code is {longest} characters"
    );
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

/// One rule about both namespaces, worded twice. A reader who wrote `let` is
/// being told about a value they never gave, and one who wrote `type` about a
/// type that stands for nothing; a single sentence covering both would describe
/// neither.
#[test]
fn a_loop_of_bare_names_is_worded_for_its_namespace() {
    let term = IrError::Circular {
        namespace: Namespace::Terms,
    };
    assert_eq!(term.code(), "circular-term");
    assert_eq!(
        term.to_string(),
        "this definition is never given a value of its own"
    );

    let ty = IrError::Circular {
        namespace: Namespace::Types,
    };
    assert_eq!(ty.code(), "circular-type");
    assert_eq!(ty.to_string(), "type defined only as another name");
}

/// A constraint prints as what it demands, in the notation the Constraints tab
/// shows it in. `~` is "must unify with".
#[test]
fn a_constraint_reads_as_what_it_demands() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let span = Span::generated(0, 1);

    let equal = Constraint {
        id: inference::ConstraintId::synthetic(0),
        reason: inference::ReasonId::synthetic(0),
        span,
        origin: inference::ConstraintOrigin::ContextualCheck,
        subjects: inference::ConstraintSubjects::pair(
            inference::Subject::Context,
            inference::Subject::Term,
        ),
        kind: ConstraintKind::Equal {
            expected: nat.clone(),
            actual: Rc::new(Ty::plain(Ty::Var(0))),
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
            value: Assigned::Ty(Rc::new(Ty::plain(Ty::Nat))),
            by: inference::ReasonId::synthetic(0),
            because: None,
        }
        .to_string(),
        "?3 := Nat"
    );
    let failure = TypeError::Recursive;
    assert_eq!(
        Effect::Failed(failure.clone()).to_string(),
        failure.to_string()
    );
    assert_eq!(
        Effect::Guarded {
            premise: Formula::var(1),
            obligation: Formula::var(2).not(),
        }
        .to_string(),
        "requires not ?2 when ?1"
    );
}

#[test]
fn every_inference_error_exposes_a_complete_structured_diagnostic() {
    let use_span = Span::generated(4, 5);
    let declared = Span::generated(1, 2);
    let kinds = inference_error_kinds(declared);
    assert_eq!(kinds.len(), 17);

    for kind in kinds {
        let diagnostic = inference::Error::new(use_span, kind).diagnostic();
        assert_eq!(diagnostic.primary.span, use_span, "{}", diagnostic.code);
        assert!(!diagnostic.code.is_empty());
        assert!(!diagnostic.title.is_empty(), "{}", diagnostic.code);
        assert!(
            !diagnostic.primary.message.is_empty(),
            "{} has no primary label",
            diagnostic.code
        );
        assert!(
            !diagnostic.help.is_empty() && diagnostic.help.iter().all(|line| !line.is_empty()),
            "{} has no repair help",
            diagnostic.code
        );
    }
}

/// Phase 1 owns user-facing inference prose even though it does not yet own the
/// provenance needed to replace every expected/found or unknown-type fallback.
/// Keep solver implementation terms out of the structured fields it did migrate.
#[test]
fn inference_diagnostic_prose_avoids_solver_jargon() {
    let span = Span::generated(0, 1);
    for kind in inference_error_kinds(span) {
        let diagnostic = inference::Error::new(span, kind).diagnostic();
        let prose = std::iter::once(diagnostic.title.as_str())
            .chain(std::iter::once(diagnostic.primary.message.as_str()))
            .chain(diagnostic.related.iter().map(|note| note.message.as_str()))
            .chain(diagnostic.help.iter().map(String::as_str))
            .chain(diagnostic.notes.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let words: HashSet<_> = prose
            .split(|character: char| !character.is_ascii_alphabetic())
            .filter(|word| !word.is_empty())
            .collect();
        for forbidden in [
            "unify",
            "unification",
            "rigid",
            "row",
            "presence",
            "sat",
            "cnf",
            "covered",
        ] {
            assert!(
                !words.contains(forbidden),
                "{} exposes `{forbidden}`: {prose}",
                diagnostic.code
            );
        }
        for forbidden in ["occurs check", "runtime representation", "boundary leaf"] {
            assert!(
                !prose.contains(forbidden),
                "{} exposes `{forbidden}`: {prose}",
                diagnostic.code
            );
        }
        assert!(
            !prose.contains('~'),
            "{} exposes `~`: {prose}",
            diagnostic.code
        );
        assert!(
            !prose
                .as_bytes()
                .windows(2)
                .any(|pair| pair[0] == b'?' && pair[1].is_ascii_digit()),
            "{} exposes a solver variable: {prose}",
            diagnostic.code
        );
    }
}

#[test]
fn rigid_field_diagnostics_name_the_caller_chosen_set_by_shape() {
    let use_span = Span::generated(4, 5);
    let declared = Span::generated(1, 2);
    for (shape, field, action, choices) in [
        (Shape::Struct, "item", "reads field", "struct fields"),
        (Shape::Sum, "Some", "matches case", "cases"),
        (Shape::Effect, "Log", "requires effect", "effects"),
    ] {
        let diagnostic = inference::Error::new(
            use_span,
            TypeError::RigidField {
                shape,
                field: field.to_string(),
                name: "r".into(),
                declared,
            },
        )
        .diagnostic();

        assert_eq!(diagnostic.code, "rigid-field");
        assert_eq!(diagnostic.related[0].span, declared);
        assert_eq!(
            diagnostic.related[0].message,
            format!("the caller's choice of {choices} starts here")
        );
        assert!(diagnostic.title.starts_with(&format!("this {action} `")));
        assert!(diagnostic.primary.message.contains(choices));
        assert!(diagnostic.help[0].contains(choices));
        assert!(!diagnostic.title.contains("whatever type"));
    }
}

#[test]
fn required_presence_diagnostics_name_labels_by_shape() {
    for (shape, labels) in [
        (Shape::Struct, "fields"),
        (Shape::Sum, "cases"),
        (Shape::Effect, "effects"),
    ] {
        let kind = TypeError::PresenceRequired {
            formula: "x and y".to_string(),
            shape: Some(shape),
        };
        assert_eq!(
            kind.to_string(),
            format!("this value needs `x and y` among its {labels}, and it does not have that")
        );
    }
}

#[test]
fn guarded_origins_keep_their_source_and_premise_readable() {
    let source = inference::Origin::Refinement(inference::Named {
        labels: vec![("x".to_string(), Presence::Var(1))],
        shape: Some(Shape::Struct),
    });
    assert_eq!(source.code(), "branch-refinement");
    assert!(source.to_string().contains("structural presence relation"));

    let guarded = inference::Origin::Guarded(inference::GuardedOrigin {
        premise: Formula::var(0),
        obligation: Formula::var(1).not(),
        origin: Box::new(source),
    });
    assert_eq!(guarded.code(), "guarded");
    assert_eq!(
        guarded.to_string(),
        "a structural presence relation produced in this arm when ?0"
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
fn inference_fixture_errors(source: &str) -> Vec<inference::Error> {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(
        parsed.errors.is_empty(),
        "parse errors: {:#?}",
        parsed.errors
    );
    let bundle = Bundle::new("diagnostics", Version::new(0, 1, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "IR errors: {:#?}", built.errors);
    inference::infer(&mint, &mut built.program).errors
}

fn inference_fixture_diagnostics(source: &str) -> Vec<ui::Diagnostic> {
    inference_fixture_errors(source)
        .into_iter()
        .map(|error| error.diagnostic())
        .collect()
}

#[test]
fn ordinary_mismatch_explanations_keep_full_and_abridged_causal_evidence() {
    let source = include_str!("../diagnostics/inference/repeated-calls.hc");
    let first = inference_fixture_errors(source);
    let second = inference_fixture_errors(source);
    let [first] = first.as_slice() else {
        panic!("the repeated calls fixture must have one error: {first:#?}");
    };
    let [second] = second.as_slice() else {
        panic!("the repeated calls fixture must be deterministic: {second:#?}");
    };
    let explanation = first.explanation.as_ref().expect("mismatch explanation");
    let repeated = second.explanation.as_ref().expect("second explanation");
    assert_eq!(explanation, repeated);
    assert!((2..=4).contains(&explanation.abridged.len()));
    assert!(explanation.full_facts.len() >= explanation.abridged.len());
    assert!(!explanation.cause.reasons.is_empty());
    assert!(!explanation.cause.constraints.is_empty());
    assert_eq!(
        explanation.contradiction.repairs,
        [
            inference::RepairDirection::ChangeFirstUse,
            inference::RepairDirection::ChangeSecondUse,
        ]
    );
    let rendered = first.diagnostic();
    for forbidden in ["expected", "found", "?", "~", "->"] {
        assert!(
            !rendered.title.contains(forbidden)
                && !rendered.primary.message.contains(forbidden)
                && rendered
                    .related
                    .iter()
                    .all(|note| !note.message.contains(forbidden)),
            "migrated mismatch leaked `{forbidden}`: {rendered:#?}"
        );
    }
}

/// Source-owned examples pin the diagnostic a reader is meant to see, rather
/// than only constructing semantic payloads. Spans are deliberately omitted
/// from this abridged form: the corpus owns source shapes, while later
/// provenance phases will improve and then pin the causal locations.
#[test]
fn inference_source_corpus_matches_abridged_structured_goldens() {
    let fixtures = [
        (
            "not-a-struct",
            include_str!("../diagnostics/inference/not-a-struct.hc"),
        ),
        (
            "type-mismatch",
            include_str!("../diagnostics/inference/type-mismatch.hc"),
        ),
        (
            "recursive-type",
            include_str!("../diagnostics/inference/recursive-type.hc"),
        ),
        (
            "missing-field",
            include_str!("../diagnostics/inference/missing-field.hc"),
        ),
        (
            "extra-field-struct",
            include_str!("../diagnostics/inference/extra-field-struct.hc"),
        ),
        (
            "extra-field-sum",
            include_str!("../diagnostics/inference/extra-field-sum.hc"),
        ),
        (
            "rigid-broken",
            include_str!("../diagnostics/inference/rigid-broken.hc"),
        ),
        (
            "rigid-field-struct",
            include_str!("../diagnostics/inference/rigid-field-struct.hc"),
        ),
        (
            "rigid-field-sum",
            include_str!("../diagnostics/inference/rigid-field-sum.hc"),
        ),
        (
            "rigid-field-effect",
            include_str!("../diagnostics/inference/rigid-field-effect.hc"),
        ),
        (
            "rigid-escapes",
            include_str!("../diagnostics/inference/rigid-escapes.hc"),
        ),
        (
            "repeated-field-struct",
            include_str!("../diagnostics/inference/repeated-field-struct.hc"),
        ),
        (
            "repeated-field-sum",
            include_str!("../diagnostics/inference/repeated-field-sum.hc"),
        ),
        (
            "repeated-field-effect",
            include_str!("../diagnostics/inference/repeated-field-effect.hc"),
        ),
        (
            "presence-required",
            include_str!("../diagnostics/inference/presence-required.hc"),
        ),
        (
            "presence-impossible",
            include_str!("../diagnostics/inference/presence-impossible.hc"),
        ),
        (
            "clause-impossible",
            include_str!("../diagnostics/inference/clause-impossible.hc"),
        ),
        (
            "annotation-allows-more",
            include_str!("../diagnostics/inference/annotation-allows-more.hc"),
        ),
        (
            "unhandled-effect",
            include_str!("../diagnostics/inference/unhandled-effect.hc"),
        ),
        (
            "effect-not-allowed",
            include_str!("../diagnostics/inference/effect-not-allowed.hc"),
        ),
        (
            "callback-effects-not-covered",
            include_str!("../diagnostics/inference/callback-effects-not-covered.hc"),
        ),
        (
            "polymorphic-extern-boundary",
            include_str!("../diagnostics/inference/polymorphic-extern-boundary.hc"),
        ),
        (
            "repeated-calls",
            include_str!("../diagnostics/inference/repeated-calls.hc"),
        ),
        (
            "branches",
            include_str!("../diagnostics/inference/branches.hc"),
        ),
        (
            "non-function",
            include_str!("../diagnostics/inference/non-function.hc"),
        ),
        (
            "cross-definition",
            include_str!("../diagnostics/inference/cross-definition.hc"),
        ),
        (
            "long-alias",
            include_str!("../diagnostics/inference/long-alias.hc"),
        ),
    ];

    let mut found = String::new();
    let mut seen = HashSet::new();
    for (name, source) in fixtures {
        writeln!(found, "## {name}").unwrap();
        let diagnostics = inference_fixture_diagnostics(source);
        assert!(!diagnostics.is_empty(), "{name} produced no diagnostic");
        if name == "repeated-calls" {
            let [diagnostic] = diagnostics.as_slice() else {
                panic!(
                    "repeated monomorphic calls should produce one diagnostic: {diagnostics:#?}"
                );
            };
            assert_eq!(diagnostic.code, "type-mismatch");
            assert_eq!(
                diagnostic.title,
                "a natural number and a boolean cannot be the same type"
            );
            assert_eq!(
                diagnostic.primary.message,
                "this call fixes what type the argument must have"
            );
        }
        for diagnostic in diagnostics {
            seen.insert(diagnostic.code);
            writeln!(found, "[{}] {}", diagnostic.code, diagnostic.title).unwrap();
            writeln!(found, "primary: {}", diagnostic.primary.message).unwrap();
            for related in diagnostic.related {
                writeln!(found, "related: {}", related.message).unwrap();
            }
            for help in diagnostic.help {
                writeln!(found, "help: {help}").unwrap();
            }
            for note in diagnostic.notes {
                writeln!(found, "note: {note}").unwrap();
            }
        }
        writeln!(found).unwrap();
    }
    found.pop();
    let expected: HashSet<_> = inference_error_kinds(Span::generated(0, 1))
        .into_iter()
        .map(|kind| kind.code())
        .collect();
    assert_eq!(
        seen, expected,
        "the source corpus must reach every ErrorKind"
    );
    assert_eq!(
        found,
        include_str!("../diagnostics/inference/abridged.golden")
    );
}

fn round_trip(prelude: &str, printed: &str) -> String {
    let source = format!("{prelude}let f : {{ v: {printed} }} -> Nat = fn r => 1n\n");
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
    // The `where 'let` a scheme now declares its letters with is not part of the
    // row being read: what this reads back is the one field's type.
    let body = scheme.split(" where 'let ").next().unwrap_or(&scheme);
    body.strip_prefix("{ v: ")
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
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let endo = Rc::new(Ty::plain(Ty::pure(nat.clone(), nat.clone())));

    // A declared type prints as its name and is an atom whatever it stands
    // for, so the arrow behind this one leaks no parentheses through it. It
    // needs a declaration to be read back, which the prelude supplies.
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Endo")
        .expect("a fresh name");
    let named = Rc::new(Ty::plain(Ty::Named {
        symbol,
        name: "Endo".into(),
        args: Rc::from([]),
    }));

    for (prelude, ty, printed) in [
        ("", nat.clone(), "Nat"),
        // Right-associative: only the left side can ever need grouping, and
        // the right side must not acquire any.
        (
            "",
            Rc::new(Ty::plain(Ty::pure(endo.clone(), endo.clone()))),
            "(Nat -> Nat) -> Nat -> Nat",
        ),
        // The empty struct is unit, and prints as the one spelling this
        // language has for it. See `Ty::Unit` for why it is `{}` and not
        // `()`.
        ("", Rc::new(Ty::unit()), "()"),
        (
            "",
            Rc::new(Ty::Struct(Row {
                labels: [("x".to_string(), RowField::present(endo.clone()))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            })),
            "{ x: Nat -> Nat }",
        ),
        (
            "type Endo = Nat -> Nat\n",
            Rc::new(Ty::plain(Ty::pure(named.clone(), named.clone()))),
            "Endo -> Endo",
        ),
    ] {
        assert_eq!(ty.to_string(), printed);
        assert_eq!(round_trip(prelude, printed), printed);
    }
}

/// Effect rows temporarily use sum-shaped arguments internally. Their opaque
/// identity suffixes are hidden only at that boundary; ordinary labels carrying
/// the same control character remain user data.
#[test]
fn applied_effect_rows_hide_only_their_generated_identity_suffixes() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let unit = Rc::new(Ty::unit());
    let row = Row {
        labels: [
            (
                EffectId::structural("Log".into(), "write:Nat".into()).row_key(),
                RowField::present(unit.clone()),
            ),
            (
                EffectId::structural("Gone".into(), "gone:Nat".into()).row_key(),
                RowField {
                    presence: Presence::Absent,
                    ty: unit.clone(),
                },
            ),
            ("plain".to_string(), RowField::present(unit)),
        ]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    };
    let effect_argument = Rc::new(Ty::plain(Ty::Sum(row.clone())));
    let ordinary_sum_argument = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: [(
            "user\u{1f}payload".to_string(),
            RowField::present(nat.clone()),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    let fielded_argument = Rc::new(Ty::Struct(Row {
        labels: [("x".to_string(), RowField::present(nat))]
            .into_iter()
            .collect(),
        rest: Rest::Closed,
    }));

    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Runner")
        .expect("a fresh name");
    let applied = Ty::plain(Ty::Named {
        symbol,
        name: "Runner".into(),
        args: Rc::from([effect_argument, ordinary_sum_argument, fielded_argument]),
    })
    .to_string();

    assert!(
        applied.starts_with("Runner (#Log | #plain) (#\"user\u{1f}payload\" Nat) "),
        "{applied:?}"
    );
    assert!(applied.ends_with("{ x: Nat }"), "{applied:?}");
}

/// An open row prints in the surface notation too, and cannot be read back
/// from it — so these are pinned as printing and nothing more.
///
/// Not an oversight in the printer. What a row's tail and a field's `?` stand
/// for is an identity: `{ x: Nat, ..'a } -> { x: Nat, ..'a }` says the two
/// tails are the *same* rest, and `..'a` is how that is spelled. The surface
/// syntax has `..'r`, which says the same thing by a name the writer chose —
/// but a scheme has no names to offer, only numbers the quantifier handed out,
/// so printing one back as `..'r` would be inventing a name that was never
/// written. `..` alone loses the identity, and `?` on a field is likewise a
/// variable the syntax gives no way to name.
///
/// So the printed form is a faithful picture and not a re-readable one. Every
/// reader of it is being shown a conclusion rather than handed something to
/// compile, which is what makes that the right trade.
#[test]
fn an_open_row_prints_in_surface_notation_it_cannot_be_read_back_from() {
    let nat = Rc::new(Ty::plain(Ty::Nat));

    // A row's tail prints in the surface spelling, after the fields; a
    // quantified tail wears its letter and an undecided one has nothing to
    // report. An open row with no fields still shows it is open.
    assert_eq!(
        Ty::Struct(Row {
            labels: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
            rest: Rest::Bound(0)
        })
        .to_string(),
        "{ x: Nat, ..'a }"
    );
    assert_eq!(
        Ty::Struct(Row {
            labels: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
            rest: Rest::Undecided
        })
        .to_string(),
        "{ x: Nat, .. }"
    );
    // A type carrying no labels prints as its core alone, whatever the core is:
    // there is no `..` to write when there is nothing for it to follow, and a
    // quantified core is exactly a quantified type.
    assert_eq!(Ty::Bound(0).to_string(), "'a");

    // A field's presence prints as its surface spelling too: certainly there
    // is unmarked, undecided either way is `?`, and certainly absent is not
    // part of what the type says at all.
    assert_eq!(
        Ty::Struct(Row {
            labels: [
                (
                    "x".to_string(),
                    RowField {
                        presence: Presence::Bound(0),
                        ty: nat.clone(),
                    }
                ),
                (
                    "y".to_string(),
                    RowField {
                        presence: Presence::Absent,
                        ty: nat.clone(),
                    }
                ),
            ]
            .into_iter()
            .collect(),
            rest: Rest::Closed
        })
        .to_string(),
        "{ x when 'a: Nat }"
    );

    // `{ x: Nat, .. }` does parse, and what it parses to is a row with a fresh
    // tail of its own — not the one it was printed from. The string survives;
    // the identity it was standing for does not.
    assert_eq!(round_trip("", "{ x: Nat, .. }"), "{ x: Nat, ..'a }");
}

/// The notes are held in [`ui`] rather than in either reporter, because both
/// print them: the CLI as a second indented line, the strip as a second
/// highlight on the same diagnostic. One for the complaints about a name
/// defined twice, one for the name declared twice in one `where` clause, one
/// for the name used two ways, and one for the `where 'let` a body broke its
/// promise about — the second place is a definition in the first, a declaration
/// in the second and the fourth, and a use in the third.
#[test]
fn the_notes_pointing_elsewhere_are_worded_once() {
    let notes = [
        ui::FIRST_DEFINITION,
        ui::FIRST_DECLARATION,
        ui::FIRST_USE,
        ui::DECLARED_HERE,
    ];
    for note in notes {
        assert!(!note.is_empty());
        assert!(!note.ends_with('.'));
        assert!(
            !note.starts_with(|c: char| c.is_ascii_uppercase()),
            "{note}"
        );
    }
    let distinct: HashSet<&str> = notes.into_iter().collect();
    assert_eq!(distinct.len(), notes.len());

    // And each says which of the three it is. Nothing in a `where` clause is
    // defined, so a repeat there may not borrow the definition's wording.
    assert_eq!(ui::FIRST_DEFINITION, "first defined here");
    assert_eq!(ui::FIRST_DECLARATION, "first declared here");
    assert_eq!(ui::FIRST_USE, "first used here");
    assert_eq!(ui::DECLARED_HERE, "declared here");
}

/// Which kind of row a complaint is about decides the noun and the spelling,
/// and nothing else: the shape is not part of the code, because a missing case
/// and a missing field are one thing gone wrong and a reporter that wants to
/// tell them apart is reading the type rather than the complaint.
///
/// The two that carry a base read the shape off it. That is the one way the
/// word and the type printed beside it cannot come out disagreeing, and it is
/// what this pins.
#[test]
fn a_complaint_about_a_sum_says_case_and_writes_the_sigil() {
    let sum = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField {
                presence: Presence::Present,
                ty: Rc::new(Ty::plain(Ty::Nat)),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    let nat = Rc::new(Ty::plain(Ty::Nat));

    let missing = TypeError::MissingField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(missing.to_string(), "no case `#B` on `#A Nat`");
    let extra = TypeError::ExtraField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert!(extra.to_string().contains("extra case `#B`"), "{extra}");
    // The same two about a struct keep the wording they always had.
    let field = TypeError::MissingField {
        shape: Shape::Struct,
        base: nat.clone(),
        field: "x".to_string(),
    };
    assert_eq!(field.to_string(), "no field `x` on `Nat`");

    // And the codes do not split, because the complaints do not.
    assert_eq!(missing.code(), field.code());

    // The ones handed a shape rather than a type say the same thing.
    let repeated = TypeError::RepeatedField {
        shape: Shape::Sum,
        field: "A".to_string(),
    };
    assert!(repeated.to_string().contains("cases"), "{repeated}");
    assert!(repeated.to_string().contains("`#A`"), "{repeated}");
    let not_a_row = IrError::NotARow {
        sense: Sense::Cases,
    };
    assert!(not_a_row.to_string().contains("sum's cases"), "{not_a_row}");
    let effects = IrError::NotARow {
        sense: Sense::Effects,
    };
    assert!(effects.to_string().contains("arrow's effects"), "{effects}");
    assert_eq!(not_a_row.code(), effects.code());

    // Including the one a declaration raises about being left open: a `?` on a
    // case and a `?` on a field are the same mistake, and lowering reaches them
    // through the same check, so the shape is the only thing telling the reader
    // which of the two they made.
    let open = IrError::OpenDeclaredType { shape: Shape::Sum };
    assert!(open.to_string().contains("its cases"), "{open}");
    let fields = IrError::OpenDeclaredType {
        shape: Shape::Struct,
    };
    assert!(fields.to_string().contains("its fields"), "{fields}");
    assert_eq!(open.code(), fields.code());
}

/// One wording for both shapes of an absence with no `..` to speak about,
/// quoting the label the way it was written: a field bare, a case wearing the
/// `#` that makes it one.
#[test]
fn an_absence_in_a_closed_type_quotes_the_label_as_written() {
    let field = IrError::AbsentInClosed {
        shape: Shape::Struct,
        label: "y".to_string(),
    };
    assert_eq!(field.code(), "absent-in-closed");
    assert_eq!(
        field.to_string(),
        "a type with no `..` already says `y` is not there"
    );

    let case = IrError::AbsentInClosed {
        shape: Shape::Sum,
        label: "B".to_string(),
    };
    assert_eq!(case.code(), field.code());
    assert_eq!(
        case.to_string(),
        "a type with no `..` already says `#B` is not there"
    );
}

/// The printer keeps dropping absent labels: a type ascribed with `\y` still
/// prints without it, so no existing printed output changes, and what a
/// scheme shows is what a value can have rather than what it cannot.
#[test]
fn a_written_absence_is_not_printed() {
    let source = "let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x\n\
                  let s : #Ok Nat | \\#Err | .. = #Ok 1n\n";
    let lexed = token::lex(source, FileID::GENERATED);
    assert!(lexed.errors.is_empty(), "{:#?}", lexed.errors);
    let parsed = parse::parse(lexed.tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let bundle = Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle");
    let mut mint = Mint::new(bundle);
    let mut built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{:#?}", built.errors);
    let inferred = inference::infer(&mint, &mut built.program);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);

    let printed: Vec<String> = built
        .program
        .terms
        .keys()
        .map(|symbol| inferred.schemes[symbol].to_string())
        .collect();
    assert_eq!(printed, ["{ x: Nat, ..'a } -> Nat", "#Ok Nat | ..'a"]);
}

/// A parameter read more than one way names the two readings it was found to
/// have, so the reader is told what to choose between rather than that there
/// was a choice.
#[test]
fn a_mixed_parameter_names_both_readings() {
    let mixed = IrError::MixedParameter {
        first: Sense::Type,
        second: Sense::Cases,
    };
    assert_eq!(
        mixed.to_string(),
        "this parameter is used as a whole type and as the rest of a sum's cases"
    );
}

/// Every token with a fixed spelling, printed and lexed again. `Display for
/// Kind` is what the debugger's Tokens tab and the parse-tree printers write
/// with, so a kind that printed as something else — or as what another kind
/// prints as — would show a reader a stream that is not the one the compiler
/// holds. The keywords the parser has no use for yet are here for that reason:
/// they lex, so they print, and nothing else would notice if they printed
/// wrong.
#[test]
fn every_fixed_token_prints_as_the_spelling_it_lexes_from() {
    let fixed = [
        TokenKind::Let,
        TokenKind::Extern,
        TokenKind::In,
        TokenKind::If,
        TokenKind::Then,
        TokenKind::Else,
        TokenKind::Type,
        TokenKind::End,
        TokenKind::With,
        TokenKind::Match,
        TokenKind::Fn,
        TokenKind::Effect,
        TokenKind::Handle,
        TokenKind::Raise,
        TokenKind::And,
        TokenKind::Or,
        TokenKind::Xor,
        TokenKind::Not,
        TokenKind::Module,
        TokenKind::Equal,
        TokenKind::FatArrow,
        TokenKind::Arrow,
        TokenKind::Colon,
        TokenKind::ColonColon,
        TokenKind::Comma,
        TokenKind::Semicolon,
        TokenKind::Dot,
        TokenKind::DotDot,
        TokenKind::NotEqual,
        TokenKind::Plus,
        TokenKind::Minus,
        TokenKind::Star,
        TokenKind::Slash,
        TokenKind::Backslash,
        TokenKind::Pipe,
        TokenKind::PipeForward,
        TokenKind::Underscore,
        TokenKind::LeftBrace,
        TokenKind::RightBrace,
        TokenKind::LeftParen,
        TokenKind::RightParen,
    ];

    let spellings: HashSet<_> = fixed.iter().map(|kind| kind.to_string()).collect();
    assert_eq!(spellings.len(), fixed.len(), "{spellings:?}");

    for kind in fixed {
        let printed = kind.to_string();
        let out = token::lex(&printed, FileID::GENERATED);
        assert!(out.errors.is_empty(), "{printed}: {:#?}", out.errors);
        assert_eq!(out.tokens.len(), 1, "{printed}");
        assert_eq!(out.tokens[0].tracked.to_string(), printed);
    }

    // The kinds that carry something print it, and re-lex to themselves as
    // well: each sigil stays on and each literal keeps its value.
    for kind in [
        TokenKind::Tag("Some".to_string()),
        TokenKind::Tag("case name".to_string()),
        TokenKind::Tag("1leading".to_string()),
        // Keywords and the lone underscore are valid bare names after `#`.
        TokenKind::Tag("let".to_string()),
        TokenKind::Tag("true".to_string()),
        TokenKind::Tag("_".to_string()),
        TokenKind::Tag("with_underscore".to_string()),
        // The effect-identity separator has no special meaning in a sum label.
        TokenKind::Tag("left\u{1f}right".to_string()),
        TokenKind::EffectLabel("Log".to_string()),
        TokenKind::Variable("a".to_string()),
        TokenKind::Identifier("x".to_string()),
        TokenKind::Natural(4096),
        TokenKind::Integer(42),
        TokenKind::Real(1.25),
        TokenKind::String("quote\" slash\\ newline\n carriage\r tab\t".to_string()),
        TokenKind::Boolean(true),
    ] {
        let printed = kind.to_string();
        let out = token::lex(&printed, FileID::GENERATED);
        assert!(out.errors.is_empty(), "{printed}: {:#?}", out.errors);
        assert_eq!(out.tokens.len(), 1, "{printed}");
        assert_eq!(out.tokens[0].tracked.to_string(), printed);
    }
}

/// Literal patterns use the same spellings as literal expressions, so the
/// debugger's parsed-tree view shows source rather than Rust's representation.
#[test]
fn literal_patterns_print_as_written() {
    for (pattern, printed) in [
        (parse::PatternKind::Integer(-2), "-2i"),
        (parse::PatternKind::Real(1.5), "1.5"),
        (
            parse::PatternKind::String("a\nstring".to_string()),
            "\"a\\nstring\"",
        ),
        (parse::PatternKind::Boolean(false), "false"),
    ] {
        assert_eq!(pattern.to_string(), printed);
    }
}

/// An arity complaint counts in words, and both halves of it — what the type
/// takes and what was written — count the same way. Nine is the last word;
/// past that it is a numeral, because a type taking ten arguments has a
/// problem the sentence is not going to help with.
#[test]
fn an_arity_complaint_counts_in_words_as_far_as_words_go() {
    let said = |expected, found| {
        IrError::Arity {
            name: "T".to_string(),
            expected,
            found,
        }
        .to_string()
    };

    assert_eq!(said(0, 1), "`T` expects no arguments, but one was written");
    assert_eq!(said(1, 0), "`T` expects one argument, but none was written");
    assert_eq!(
        said(2, 9),
        "`T` expects two arguments, but nine were written"
    );
    // Beyond the words, the numeral — on both sides.
    assert_eq!(
        said(10, 13),
        "`T` expects 10 arguments, but 13 were written"
    );
}

/// A primitive prints as the one name it is written with, so a message quoting
/// a type and the parser reading one back agree about the word.
#[test]
fn a_primitive_prints_as_the_name_it_is_written_with() {
    assert_eq!(Prim::Nat.to_string(), Prim::Nat.name());
    assert_eq!(Prim::Nat.to_string(), "Nat");
}

/// A case the solver settled absent is not part of what the sum says, so it is
/// not printed — the same rule a struct's absent field keeps. Without it a
/// complaint would quote a type carrying a case the reader was just told it
/// does not have.
#[test]
fn a_case_settled_absent_is_not_part_of_the_sum() {
    let sum = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: [
            (
                "A".to_string(),
                RowField::present(Rc::new(Ty::plain(Ty::Nat))),
            ),
            (
                "B".to_string(),
                RowField {
                    presence: Presence::Absent,
                    ty: Rc::new(Ty::plain(Ty::Nat)),
                },
            ),
        ]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    assert_eq!(sum.to_string(), "#A Nat");

    // A case carrying anything that is not unit keeps its payload: only the
    // type with nothing of its own and no fields is written as no payload at
    // all.
    let open = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField::present(Rc::new(Ty::Struct(Row {
                labels: [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
                    .into_iter()
                    .collect(),
                rest: Rest::Bound(0),
            }))),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    assert_eq!(open.to_string(), "#A { x: (), ..'a }");

    // The two forms that write no case at all keep the leading bar, which is
    // the only thing that makes either read back as a sum.
    let empty = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: Default::default(),
        rest: Rest::Closed,
    })));
    assert_eq!(empty.to_string(), "|");
    let only_tail = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: Default::default(),
        rest: Rest::Bound(0),
    })));
    assert_eq!(only_tail.to_string(), "| ..'a");
}

/// A sink that accepts `left` writes and then fails, so a printer can be run
/// against a failure at every point it has one.
struct Failing {
    left: usize,
}

/// A direct display wrapper for the effect-row writer. Reaching it only through
/// a whole type puts another formatter between the refusing sink and these
/// writes, which cannot exercise each of this writer's own propagation points.
struct ShownEffects<'a> {
    effects: &'a [ui::Entry<&'a str, ()>],
    tail: Option<&'a dyn fmt::Display>,
}

impl fmt::Display for ShownEffects<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        ui::write_effects(f, self.effects, self.tail)
    }
}

impl fmt::Write for Failing {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        match self.left.checked_sub(1) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None => Err(fmt::Error),
        }
    }
}

/// How many writes rendering `shown` takes.
fn write_count(shown: &dyn fmt::Display) -> usize {
    struct Counting {
        writes: usize,
    }
    impl fmt::Write for Counting {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            self.writes += 1;
            Ok(())
        }
    }

    let mut counting = Counting { writes: 0 };
    write!(counting, "{shown}").expect("counting cannot fail");
    counting.writes
}

/// Fail the sink at each write in turn and check the printer says so. A `?`
/// dropped anywhere 'in a printer would show up here as a render that claimed to
/// succeed after the sink had refused it — and, for the printers that write in
/// pieces, as a half-written type reaching whoever asked for it.
fn every_failure_is_reported(what: &str, shown: &dyn fmt::Display) {
    let total = write_count(shown);
    assert!(total > 0, "{what} wrote nothing");
    for left in 0..total {
        let mut failing = Failing { left };
        assert!(
            write!(failing, "{shown}").is_err(),
            "{what}: the failure at write {left} of {total} was swallowed"
        );
    }
    // And the same render against a sink that never refuses.
    let mut enough = Failing { left: total };
    assert!(write!(enough, "{shown}").is_ok(), "{what}");
}

/// Everything the compiler prints goes to a `fmt::Write` it does not own, and
/// one that fails is a real thing — a socket, a full disk, the debugger's own
/// buffers. A printer that swallowed the failure would hand back a type or a
/// program that was never written, which is worse than the error it hid.
#[test]
fn generic_rows_render_undecided_marks() {
    struct Marked;
    impl fmt::Display for Marked {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            ui::write_row(
                f,
                [Entry::Written {
                    name: "x",
                    mark: Some(Mark::Undecided),
                    holds: "Nat",
                }],
                None,
            )?;
            f.write_str(" / ")?;
            ui::write_sum(
                f,
                [Entry::Written {
                    name: "A",
                    mark: Some(Mark::Undecided),
                    holds: Some(&Ty::Nat),
                }],
                None,
            )?;
            f.write_str(" / ")?;
            ui::write_effects(
                f,
                &[Entry::Written {
                    name: "Log",
                    mark: Some(Mark::Undecided),
                    holds: (),
                }],
                None,
            )
        }
    }
    assert_eq!(Marked.to_string(), "{ x?: Nat } / #A? Nat / !Log?");
}

#[test]
fn a_printer_reports_a_writer_that_refuses_it() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let module = mint.module(None, "util").expect("a fresh name");
    let local = mint.local(Some(module), Namespace::Terms, "x");
    let named = Rc::new(Ty::plain(Ty::Named {
        symbol: mint
            .global(None, Namespace::Types, "Pair")
            .expect("a fresh name"),
        name: "Pair".into(),
        args: Rc::from([nat.clone(), nat.clone()]),
    }));

    // A path, which writes the bundle, a module, and the anonymous segment a
    // local is shown under.
    every_failure_is_reported("a path", &mint.path(local));
    every_failure_is_reported(
        "an escaped string",
        &TokenKind::String("quote\" slash\\ newline\n carriage\r tab\t".to_string()),
    );

    let optional = RowField {
        presence: Presence::Undecided,
        ty: nat.clone(),
    };
    for (what, ty) in [
        (
            "an arrow",
            Rc::new(Ty::plain(Ty::pure(nat.clone(), nat.clone()))),
        ),
        ("an application", named.clone()),
        (
            "an open struct",
            Rc::new(Ty::Struct(Row {
                labels: [
                    ("x".to_string(), RowField::present(nat.clone())),
                    ("y".to_string(), optional.clone()),
                ]
                .into_iter()
                .collect(),
                rest: Rest::Bound(0),
            })),
        ),
        (
            "an open sum",
            Rc::new(Ty::plain(Ty::Sum(Row {
                labels: [
                    ("A".to_string(), RowField::present(nat.clone())),
                    ("B".to_string(), optional.clone()),
                ]
                .into_iter()
                .collect(),
                rest: Rest::Bound(0),
            }))),
        ),
        (
            "the empty sum",
            Rc::new(Ty::plain(Ty::Sum(Row {
                labels: Default::default(),
                rest: Rest::Closed,
            }))),
        ),
        (
            "the sum that is only its tail",
            Rc::new(Ty::plain(Ty::Sum(Row {
                labels: Default::default(),
                rest: Rest::Bound(0),
            }))),
        ),
        (
            "a case carrying unit",
            Rc::new(Ty::plain(Ty::Sum(Row {
                labels: [("None".to_string(), RowField::present(Rc::new(Ty::unit())))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }))),
        ),
    ] {
        every_failure_is_reported(what, &ty);
    }

    // Compound displays of their own, including every recursive formula form
    // and every piece of a witness, report a refusal at any depth.
    let witness = ir::Witness::Struct(
        [
            ("a".to_string(), ir::Witness::Any),
            (
                "b".to_string(),
                ir::Witness::Other(vec!["X".to_string(), "Y".to_string()]),
            ),
        ]
        .into_iter()
        .collect(),
    );
    every_failure_is_reported("a witness", &witness);

    let formula = Formula::Iff(
        Rc::new(Formula::And(
            Rc::new(Formula::True),
            Rc::new(Formula::var(0)),
        )),
        Rc::new(Formula::Xor(
            Rc::new(Formula::Not(Rc::new(Formula::False))),
            Rc::new(Formula::Or(
                Rc::new(Formula::var(1)),
                Rc::new(Formula::var(2)),
            )),
        )),
    );
    every_failure_is_reported("a presence formula", &formula);
    every_failure_is_reported(
        "a constrained scheme",
        &ruddy::types::Scheme::constrained(1, 1, nat.clone(), Formula::bound(0)),
    );
    every_failure_is_reported(
        "a let constraint",
        &ConstraintKind::Let {
            symbol: local,
            bound: nat.clone(),
            level: 1,
            promised: Formula::var(0),
            rigids: Vec::new(),
            value: Vec::new(),
            body: Vec::new(),
        },
    );

    let effects = [
        ui::Entry::Written {
            name: "Log",
            mark: None,
            holds: (),
        },
        ui::Entry::Written {
            name: "IO",
            mark: Some(ui::Mark::Undecided),
            holds: (),
        },
        ui::Entry::Written {
            name: "Net",
            mark: Some(ui::Mark::When("'a".to_string())),
            holds: (),
        },
        ui::Entry::Absent { name: "Gone" },
    ];
    let effect_tail = Rest::Var(3);
    every_failure_is_reported(
        "an effect row",
        &ShownEffects {
            effects: &effects,
            tail: Some(&effect_tail),
        },
    );
    every_failure_is_reported(
        "an effect-row tail",
        &ShownEffects {
            effects: &[],
            tail: Some(&effect_tail),
        },
    );
    every_failure_is_reported(
        "an empty effect row",
        &ShownEffects {
            effects: &[],
            tail: None,
        },
    );
    every_failure_is_reported(
        "a closed effect row",
        &ShownEffects {
            effects: &effects[..1],
            tail: None,
        },
    );

    // And the term printers, which have forms of their own: an application and
    // a projection, neither of which a type can be.
    // And a nested `let`, which writes its name, its ascription and its two
    // halves in pieces the way a path does.
    // An explicitly absent label in each shape, so a refusal inside the `\`
    // the printers write for one comes back too.
    let source = "type Pair 'a 'b = { first: 'a, second: 'b }\n\
                  let f = fn g => fn p => g p.first\n\
                  let h = let n : Nat = 1n in n\n\
                  let arithmetic = -1i + 2i\n\
                  let v : { first: Nat, second: Nat } = { first: 1n, second: 2n }\n\
                  type Gap 'r = { first: Nat, \\hole, ..'r }\n\
                  let w : #Ok Nat | \\#Err | .. = #Ok 1n";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut program_mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid"));
    let built = ruddy::ir::build(&mut program_mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{:#?}", built.errors);

    every_failure_is_reported(
        "a lowered program",
        &print::ir::program(&built.program, &program_mint),
    );

    // And the parse tree's printers, which render an absence as written from
    // the surface tree rather than the lowered one — through every form a
    // written row can take, so a refusal at any of its writes comes back
    // whichever form is being written.
    let source = "let x : { first: Nat, opt when 'b: Nat, \\hole, .. } \
                  -> (|) -> (| ..'r) -> (#Ok Nat | #Err (when 'a) | \\#B | ..) \
                  -> (Nat -> Nat + !Log (when 'e) + \\!Gone + ..) = 1n\n\
                  let y = match (-x + 2i * 3i) |> fn z => z with \
                  | { a: true, b, .. } => 1n | _ => 2n end";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    for stmt in &parsed.stmts {
        every_failure_is_reported("a parsed statement", &print::ast::stmt(&stmt.tracked));
    }
}

/// A sum's tail decided to be more cases prints as those cases, in the notation
/// of the row it ends — and a splice that came to nothing prints as no tail at
/// all.
///
/// Both are what a sum's row parameter leaves behind, so both are what the
/// debugger's IR tab shows for a term checked against a declared type. `..'r`
/// handed a sum's cases is those cases, and spelling them with colons would show
/// a reader a case list as if it were a set of fields. `..'r` handed nothing
/// allows nothing more, which is what a closed row already says — and the
/// solver's own mark for that is never part of a printed type.
///
/// Struct rows use the same explicit `Rest`, so a decided empty splice likewise
/// prints as no tail at all.
#[test]
fn a_spliced_tail_prints_in_the_notation_of_the_row_it_ends() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let more = |labels: Vec<(&str, RowField)>, rest: Rest| {
        Rest::More(Rc::new(Row {
            labels: labels
                .into_iter()
                .map(|(name, field)| (name.to_string(), field))
                .collect(),
            rest,
        }))
    };

    // `type G 'r = #Err Nat | ..'r` applied to `#Ok Nat`, and to a row
    // naming nothing, which leaves the sum closed at the case it wrote.
    let err_nat = |rest: Rest| {
        Rc::new(Ty::plain(Ty::Sum(Row {
            labels: [("Err".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
            rest,
        })))
    };
    assert_eq!(
        err_nat(more(
            vec![("Ok", RowField::present(nat.clone()))],
            Rest::Closed
        ))
        .to_string(),
        "#Err Nat | #Ok Nat"
    );
    assert_eq!(err_nat(more(vec![], Rest::Closed)).to_string(), "#Err Nat");

    // A chain of splices is read to its end, and a case the splice settled
    // absent counts for nothing on the way: what is written is whatever the
    // chain still leaves open.
    assert_eq!(
        err_nat(more(vec![], more(vec![], Rest::Closed))).to_string(),
        "#Err Nat"
    );
    assert_eq!(
        err_nat(more(
            vec![(
                "Gone",
                RowField {
                    presence: Presence::Absent,
                    ty: nat.clone(),
                }
            )],
            Rest::Var(3)
        ))
        .to_string(),
        "#Err Nat | ..?3"
    );

    // A case whose payload is unit is still a case carrying unit, so it prints
    // with no payload at all.
    assert_eq!(
        Ty::plain(Ty::Sum(Row {
            labels: [("A".to_string(), RowField::present(Rc::new(Ty::unit())))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }))
        .to_string(),
        "#A"
    );
    let nested_empty = Rc::new(Ty::Struct(Row {
        labels: Default::default(),
        rest: Rest::More(Rc::new(Row::closed())),
    }));
    assert_eq!(
        Ty::Sum(Row {
            labels: [("Nested".into(), RowField::present(nested_empty))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        })
        .to_string(),
        "#Nested"
    );
    let nested_open = Rc::new(Ty::Struct(Row {
        labels: Default::default(),
        rest: Rest::More(Rc::new(Row {
            labels: Default::default(),
            rest: Rest::Var(9),
        })),
    }));
    assert_eq!(
        Ty::Sum(Row {
            labels: [("Nested".into(), RowField::present(nested_open))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        })
        .to_string(),
        "#Nested { ..?9 }"
    );
}

/// The three sorts print on their own as well as inside a type, because the
/// solver's own record shows them there: a step binding a row variable or a
/// presence variable has nothing but the value to show.
#[test]
fn the_three_sorts_each_print_on_their_own() {
    // A row prints as what it allows. One that names nothing prints as its
    // rest alone, so closing a row reads as the nothing it closed to rather
    // than as an empty pair of braces standing for the same thing.
    assert_eq!(Row::closed().to_string(), "∅");
    assert_eq!(
        Row {
            labels: Default::default(),
            rest: Rest::Var(3),
        }
        .to_string(),
        "?3"
    );
    assert_eq!(
        Row {
            labels: [(
                "x".to_string(),
                RowField::present(Rc::new(Ty::plain(Ty::Nat)))
            )]
            .into_iter()
            .collect(),
            rest: Rest::Var(9),
        }
        .to_string(),
        "{ x: Nat, ..?9 }"
    );

    for (rest, printed) in [
        (Rest::Closed, "∅"),
        (Rest::Var(4), "?4"),
        (Rest::Bound(0), "'a"),
        (Rest::Undecided, "?"),
        (Rest::More(Rc::new(Row::closed())), "∅"),
    ] {
        assert_eq!(rest.to_string(), printed);
    }

    for (presence, printed) in [
        (Presence::Present, "present"),
        (Presence::Absent, "absent"),
        (Presence::Var(4), "?4"),
        // A presence is a variable like the other two, and wears the sigil a
        // `when` clause writes it with.
        (Presence::Bound(1), "'b"),
        (Presence::Undecided, "?"),
    ] {
        assert_eq!(presence.to_string(), printed);
    }

    // A binding prints as the value, whichever sort it is, so the Solve tab's
    // one column serves all three.
    for (value, printed) in [
        (Assigned::Ty(Rc::new(Ty::plain(Ty::Nat))), "?2 := Nat"),
        (Assigned::Row(Rc::new(Row::closed())), "?2 := ∅"),
        (Assigned::Presence(Presence::Absent), "?2 := absent"),
    ] {
        assert_eq!(
            Effect::Bound {
                var: 2,
                value,
                by: inference::ReasonId::synthetic(0),
                because: None,
            }
            .to_string(),
            printed.to_string()
        );
    }
}

/// A goal is a constraint the solver may have taken apart, so it prints as one
/// in whichever sort it ended up about. Generation only ever equates types; the
/// other two are what taking a type apart reaches.
#[test]
fn a_goal_prints_as_the_constraint_it_is() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    assert_eq!(
        Goal::Type {
            expected: nat.clone(),
            actual: Rc::new(Ty::plain(Ty::Var(0))),
        }
        .to_string(),
        "Nat ~ ?0"
    );
    assert_eq!(
        Goal::Row {
            expected: Rc::new(Row::closed()),
            actual: Rc::new(Row {
                labels: Default::default(),
                rest: Rest::Var(1),
            }),
        }
        .to_string(),
        "∅ ~ ?1"
    );
    assert_eq!(
        Goal::Presence {
            expected: Presence::Present,
            actual: Presence::Absent,
        }
        .to_string(),
        "present ~ absent"
    );
}

/// Which noun a complaint about one label uses comes off the shape the solver
/// carried, not off the type printed beside it. That is the one thing every
/// type having fields makes necessary: a sum-cored base can be missing a
/// *field*, and reading the word off the base would call it a case.
#[test]
fn a_label_complaint_reads_the_shape_it_was_handed() {
    let sum = Rc::new(Ty::plain(Ty::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField::present(Rc::new(Ty::plain(Ty::Nat))),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));

    // One base, two shapes, two nouns — and the type beside the word is the
    // same either way.
    let missing_field = TypeError::MissingField {
        shape: Shape::Struct,
        base: sum.clone(),
        field: "x".to_string(),
    };
    assert_eq!(missing_field.to_string(), "no field `x` on `#A Nat`");
    let missing_case = TypeError::MissingField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(missing_case.to_string(), "no case `#B` on `#A Nat`");
    assert_eq!(missing_field.code(), missing_case.code());

    // And the same for the extra one, whose sentence names the noun twice.
    let extra_field = TypeError::ExtraField {
        shape: Shape::Struct,
        base: sum.clone(),
        field: "x".to_string(),
    };
    assert_eq!(
        extra_field.to_string(),
        "extra field `x`: the type `#A Nat` lists every field it allows"
    );
    let extra_case = TypeError::ExtraField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(
        extra_case.to_string(),
        "extra case `#B`: the type `#A Nat` lists every case it allows"
    );
}

/// Projection from a known non-struct has its own stable diagnostic.
#[test]
fn not_a_struct_is_coded_and_worded() {
    let diagnostics = diagnostics();
    let (_, _, message) = diagnostics
        .iter()
        .find(|(_, code, _)| *code == "not-a-struct")
        .expect("the projection diagnostic");
    assert_eq!(
        message,
        "`Nat` is not a struct, so it has no fields to read"
    );
    let codes: HashSet<&str> = diagnostics.iter().map(|(_, code, _)| *code).collect();
    for code in [
        "not-a-struct",
        "missing-field",
        "extra-field",
        "type-mismatch",
        "recursive-type",
        "repeated-field",
    ] {
        assert!(codes.contains(code), "{code}");
    }
}

/// The complaint about an annotation the definition narrowed is gone, and with
/// it its code. Every `..`, `when _` and `_` a reader writes is a hole now —
/// there to be decided, which is what a hole is for — and what an annotation
/// still promises is what its `where 'let` declares, held to at the expression
/// that breaks it.
#[test]
fn nothing_is_coded_as_annotation_too_open_any_more() {
    let codes: HashSet<&str> = diagnostics().iter().map(|(_, code, _)| *code).collect();
    assert!(!codes.contains("annotation-too-open"), "{codes:#?}");

    // What replaces it is nothing at all in inference, and three complaints
    // about the promise a `where 'let` makes.
    for code in ["rigid-broken", "rigid-field", "rigid-escapes"] {
        assert!(codes.contains(code), "{code}");
    }
    // And lowering gained four, about the clause itself.
    for code in ["variable-in-declaration", "hole-in-declaration"] {
        assert!(codes.contains(code), "{code}");
    }
}

/// Every complaint a variable can raise, rendered. A wording is read once by
/// whoever reads the compiler's output and never again, so each is pinned here
/// where the whole surface can be read at once.
#[test]
fn the_variable_complaints_read_as_what_went_wrong() {
    let span = Span::generated(0, 1);
    let variable = ir::Error {
        span,
        kind: IrError::VariableInDeclaration {
            name: "a".to_string(),
        },
    }
    .diagnostic();
    assert_eq!(variable.title, "`'a` is not declared in this type's header");
    assert_eq!(
        variable.primary.message,
        "this name is not one of the type's parameters"
    );
    assert_eq!(
        variable.help,
        ["add `'a` to the header, or use an existing parameter"]
    );

    assert_eq!(
        IrError::HoleInDeclaration.to_string(),
        "a declared type cannot contain `_`"
    );
    // What a formula needs of a name and how to repair it remain structured:
    // the headline identifies the unused presence, while the label and help
    // connect it to the `when` spelling the reader can change.
    let unbound = ir::Error {
        span,
        kind: IrError::UnboundPresence {
            name: "b".to_string(),
        },
    }
    .diagnostic();
    assert_eq!(unbound.title, "`'b` does not control any field or case");
    assert_eq!(
        unbound.primary.message,
        "used here, but no label has `when 'b`"
    );
    assert_eq!(
        unbound.help,
        ["add `when 'b` to the intended label, or correct the name"]
    );

    // And the three inference raises, each naming what the expression turned
    // out to be beside what it had promised to be.
    assert_eq!(
        TypeError::RigidBroken {
            found: Rc::new(Ty::plain(Ty::Nat)),
            name: "a".into(),
            sense: Sense::Type,
            declared: span,
        }
        .to_string(),
        "this is `Nat`, but `'a` stands for whatever type the caller picks"
    );
    // The effect reading quotes no arrow: the rows differ only in their
    // tails, and the reader's fix is at the expression, not a type nobody
    // wrote.
    assert_eq!(
        TypeError::RigidBroken {
            found: Rc::new(Ty::plain(Ty::Nat)),
            name: "e".into(),
            sense: Sense::Effects,
            declared: span,
        }
        .to_string(),
        "this decides what it may perform, but `'e` stands for whatever effects the caller allows"
    );
    assert_eq!(
        TypeError::RigidField {
            shape: Shape::Struct,
            field: "x".to_string(),
            name: "r".into(),
            declared: span,
        }
        .to_string(),
        "this reads field `x`, but `'r` stands for whatever other struct fields the caller chooses, \
         so `x` cannot be assumed"
    );
    // In the reader's own nouns: someone who wrote `#`s is told about a
    // case, and the label is quoted with the `#` that makes it one.
    assert_eq!(
        TypeError::RigidField {
            shape: Shape::Sum,
            field: "B".to_string(),
            name: "r".into(),
            declared: span,
        }
        .to_string(),
        "this matches case `#B`, but `'r` stands for whatever other cases the caller chooses, \
         so `#B` cannot be assumed"
    );
    assert_eq!(
        TypeError::RigidEscapes { name: "a".into() }.to_string(),
        "`\'a` stands for whatever the caller picks, so it can't be part of a type \
         outside the annotation that declared it"
    );
}

/// The new complaint about a declaration whose fields never run out, and the
/// reworded one about an argument that names a label twice.
#[test]
fn the_complaints_about_a_declarations_own_labels_are_worded_once() {
    assert_eq!(IrError::EndlessFields.code(), "endless-fields");
    let endless = ir::Error {
        span: Span::generated(0, 1),
        kind: IrError::EndlessFields,
    }
    .diagnostic();
    assert_eq!(
        endless.title,
        "this recursive type adds more fields on every cycle"
    );
    assert_eq!(
        endless.primary.message,
        "the fields never reach a finite end"
    );
    assert_eq!(
        endless.help,
        ["place the recursion inside a field, or stop extending the `..` part"]
    );

    // The repeat is worded as what the argument does rather than as what goes
    // where it was written: a struct's `..` takes any type at all, so there is
    // no reading to open with. The noun and the label's own notation follow the
    // shape it was found in.
    let field = IrError::RepeatedRowField {
        shape: Shape::Struct,
        field: "x".to_string(),
    };
    assert_eq!(field.to_string(), "`x` would appear twice in this struct");
    let case = IrError::RepeatedRowField {
        shape: Shape::Sum,
        field: "A".to_string(),
    };
    assert_eq!(case.to_string(), "`#A` would appear twice in this sum");
    assert_eq!(field.code(), case.code());
}

/// A name used two ways is a name given two *senses*, and there are three of
/// them now: a struct's `..` is the whole type its fields sit on, a sum's is
/// the cases it does not write out, and a `when` names whether one label is
/// there. Every pairing reads, and each names the reading it was used at here
/// beside the one it was used at before.
#[test]
fn a_mixed_tail_names_the_two_senses_it_was_given() {
    let span = Span::generated(0, 1);
    let said = |first, second| {
        IrError::MixedTail {
            first,
            second,
            previous: span,
        }
        .to_string()
    };
    assert_eq!(
        said(Sense::Type, Sense::Cases),
        "one variable cannot stand for both a whole type and the rest of a sum's cases"
    );
    assert_eq!(
        said(Sense::Cases, Sense::Type),
        "one variable cannot stand for both the rest of a sum's cases and a whole type"
    );
    // The third sense, which only a `where 'let` variable can have: a formula is
    // written about presences, so a name in one is read as a presence.
    assert_eq!(
        said(Sense::Type, Sense::Presence),
        "one variable cannot stand for both a whole type and a presence"
    );
    assert_eq!(
        said(Sense::Presence, Sense::Cases),
        "one variable cannot stand for both a presence and the rest of a sum's cases"
    );
}

/// A row lifted out of the type it belongs to has no shape to be read in, so it
/// falls back to braces. A spliced tail is flattened into the same canonical,
/// source-representable row; the solver's own record is where one surfaces.
#[test]
fn a_row_with_no_shape_to_hand_down_prints_in_braces() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let row = Row {
        labels: [("x".to_string(), RowField::present(nat.clone()))]
            .into_iter()
            .collect(),
        rest: Rest::More(Rc::new(Row {
            labels: [("y".to_string(), RowField::present(nat))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        })),
    };
    assert_eq!(row.to_string(), "{ x: Nat, y: Nat }");
}

#[test]
fn flattened_rows_print_with_outer_wins_and_hide_interface_keys() {
    let nat = Rc::new(Ty::Nat);
    let inner = Row {
        labels: [
            ("masked".into(), RowField::present(nat.clone())),
            (
                "Log\u{1f}generated-interface".into(),
                RowField::present(Rc::new(Ty::unit())),
            ),
        ]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    };
    let outer = Row {
        labels: [(
            "masked".into(),
            RowField {
                presence: Presence::Absent,
                ty: Rc::new(Ty::String),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::More(Rc::new(inner)),
    };

    assert_eq!(
        Ty::Struct(outer.clone()).to_string(),
        "{ \"Log\u{1f}generated-interface\": () }"
    );
    assert_eq!(
        outer.to_string(),
        "{ \"Log\u{1f}generated-interface\": () }"
    );
    // In a normalized applied effect-interface position, every More layer
    // participates in identity stripping; a legacy separator alone remains
    // ordinary sum data, as the assertions above show.
    let effect_inner = Row {
        labels: [(
            EffectId::structural("Log".into(), "generated-interface".into()).row_key(),
            RowField::present(Rc::new(Ty::unit())),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    };
    let effect_outer = Row {
        labels: Default::default(),
        rest: Rest::More(Rc::new(effect_inner)),
    };
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).unwrap());
    let runner = mint.global(None, Namespace::Types, "Runner").unwrap();
    let applied = Ty::Named {
        symbol: runner,
        name: "Runner".into(),
        args: vec![Rc::new(Ty::Sum(effect_outer))].into(),
    };
    assert_eq!(applied.to_string(), "Runner (#Log)");

    let absent = Row {
        labels: [(
            "gone".into(),
            RowField {
                presence: Presence::Absent,
                ty: nat,
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    };
    assert_eq!(absent.to_string(), "∅");
    assert_eq!(Ty::Struct(absent.clone()).to_string(), "()");
    assert_eq!(Ty::Sum(absent).to_string(), "|");
}

/// A constraint kind is coded the way an error kind is, and for the same
/// reason: the Constraints tab labels a row with it, and two kinds sharing a
/// code would make two different demands read as one.
#[test]
fn no_two_kinds_of_constraint_are_coded_the_same() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint.local(None, Namespace::Terms, "x");

    let kinds = [
        ConstraintKind::Project {
            base: nat.clone(),
            field: "x".into(),
            result: nat.clone(),
            base_span: Span::generated(0, 1),
        },
        ConstraintKind::Equal {
            expected: nat.clone(),
            actual: nat.clone(),
        },
        ConstraintKind::Let {
            symbol,
            bound: nat.clone(),
            level: 1,
            promised: Formula::True,
            rigids: Vec::new(),
            value: Vec::new(),
            body: Vec::new(),
        },
        ConstraintKind::Instance {
            symbol,
            ty: nat.clone(),
            requirement: 0,
        },
        ConstraintKind::Match {
            scrutinee: nat.clone(),
            result: nat.clone(),
            arms: Vec::new(),
            store_end: 0,
        },
        ConstraintKind::Performs {
            performed: Row::closed(),
            ambient: Row::closed(),
            inside: true,
        },
    ];
    let codes: HashSet<_> = kinds.iter().map(|kind| kind.code()).collect();
    let messages: HashSet<_> = kinds.iter().map(|kind| kind.to_string()).collect();
    assert_eq!(codes.len(), kinds.len());
    assert_eq!(messages.len(), kinds.len());
    for kind in &kinds {
        assert!(!kind.to_string().is_empty(), "{}", kind.code());
    }
}

/// The two kinds a nested `let` adds, read as what they say. A `let` carries
/// two lists rather than a pair of types, so it prints as the header of the
/// tree its children make; a use of the name it bound cannot spell the name at
/// all, there being no mint here to spell one with, so it says what it is
/// instead. An annotation's clause follows the header, because that is what the
/// scheme the `let` publishes requires of its presences.
#[test]
fn the_scoping_constraints_read_as_what_they_do() {
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint.local(None, Namespace::Terms, "x");

    let bound = Constraint {
        id: inference::ConstraintId::synthetic(0),
        reason: inference::ReasonId::synthetic(0),
        span: Span::generated(0, 1),
        origin: inference::ConstraintOrigin::Binding,
        subjects: inference::ConstraintSubjects::one(inference::Subject::Binding),
        kind: ConstraintKind::Let {
            symbol,
            bound: nat.clone(),
            level: 2,
            promised: Formula::True,
            rigids: Vec::new(),
            value: Vec::new(),
            body: Vec::new(),
        },
    };
    assert_eq!(bound.kind.code(), "let");
    assert_eq!(bound.to_string(), "Nat generalized at level 2");
    assert_eq!(bound.to_string(), bound.kind.to_string());

    let promised = ConstraintKind::Let {
        symbol,
        bound: nat.clone(),
        level: 2,
        promised: Formula::var(0).xor(Formula::var(1)),
        rigids: Vec::new(),
        value: Vec::new(),
        body: Vec::new(),
    };
    assert_eq!(
        promised.to_string(),
        "Nat generalized at level 2 where ?0 != ?1"
    );

    let use_site = ConstraintKind::Instance {
        symbol,
        ty: Rc::new(Ty::plain(Ty::Var(4))),
        requirement: 0,
    };
    assert_eq!(use_site.code(), "instance");
    assert_eq!(
        use_site.to_string(),
        "?4 ~ a fresh copy of what this name was bound to"
    );
}

/// The complaints pattern matching added, worded and coded. The wording quotes
/// what the reader wrote — the tag with its `#`, the number as itself,
/// the name that was bound twice — and the codes are the stable names the
/// error tests in `ir.rs` key on.
#[test]
fn the_pattern_complaints_say_what_was_written() {
    let diagnose = |kind| {
        ir::Error {
            span: Span::generated(0, 1),
            kind,
        }
        .diagnostic()
    };
    let case = diagnose(IrError::RefutableBinding {
        found: ir::Refuter::Case("Some".to_string()),
    });
    assert_eq!(case.code, "binding-can-fail");
    assert_eq!(
        case.title,
        "this pattern can fail, but a binding must accept every value"
    );
    assert_eq!(case.primary.message, "a value here might not be `#Some`");
    assert_eq!(
        case.help,
        ["use `match` for this case or literal, or bind a name instead"]
    );

    // A number is the same complaint made about the other kind of test, and
    // the same code: what went wrong is the binding either way.
    let number = diagnose(IrError::RefutableBinding {
        found: ir::Refuter::Literal(ir::Literal::Natural(0)),
    });
    assert_eq!(number.code, "binding-can-fail");
    assert_eq!(number.primary.message, "a value here might not equal `0n`");
    let string = diagnose(IrError::RefutableBinding {
        found: ir::Refuter::Literal(ir::Literal::String("x".to_string())),
    });
    assert_eq!(string.code, number.code);
    assert_eq!(
        string.primary.message,
        "a value here might not equal `\"x\"`"
    );

    assert_eq!(PatternError::UnreachableArm.code(), "unreachable-arm");
    assert_eq!(
        PatternError::UnreachableArm.to_string(),
        "this case is already handled by the arms above it"
    );

    assert_eq!(
        PatternError::MisplacedCatchAll.code(),
        "misplaced-catch-all"
    );
    assert_eq!(
        PatternError::MisplacedCatchAll.to_string(),
        "this arm accepts everything, so the arms after it can never be reached"
    );

    assert_eq!(PatternError::UnhandledNumbers.code(), "unhandled-numbers");
    assert_eq!(
        PatternError::UnhandledNumbers.to_string(),
        "numbers not listed here are not handled; add a final arm that names the rest"
    );

    // The unhandled-values complaint renders its witness in source syntax:
    // the hole's own example value, `#`s and braces as a reader would
    // write them.
    let hole = PatternError::UnhandledValues {
        witness: ir::Witness::Struct(
            [
                (
                    "a".to_string(),
                    ir::Witness::Tag {
                        name: "A".to_string(),
                        payload: None,
                    },
                ),
                (
                    "b".to_string(),
                    ir::Witness::Tag {
                        name: "Y".to_string(),
                        payload: None,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
    };
    assert_eq!(hole.code(), "unhandled-values");
    assert_eq!(
        hole.to_string(),
        "some values are not handled — for example `{ a: #A, b: #Y }`; add an arm for them or a final arm naming the rest"
    );

    // The mixed match lost its dedicated complaint: the solver's ordinary
    // mismatch is the only thing left to say about one, so no kind in the
    // compiler answers to the old code any more.
    assert!(
        diagnostics()
            .iter()
            .all(|(_, code, _)| *code != "mixed-match"),
        "the mixed-match code should be gone"
    );

    let bound = IrError::DuplicateBinding {
        name: "x".to_string(),
        previous: Span::generated(0, 1),
    };
    assert_eq!(bound.code(), "duplicate-binding");
    assert_eq!(bound.to_string(), "this pattern binds `x` more than once");
}

/// Every shape a witness takes, rendered: numbers as themselves, tags bare
/// and carrying — a carried prose form bracketed, so the example still reads
/// as one value — an open position as "anything other than …", and the
/// position nothing constrains as "anything".
#[test]
fn a_witness_renders_in_source_syntax() {
    assert_eq!(ir::Witness::Natural(2).to_string(), "2n");
    for (literal, printed) in [
        (ir::Literal::Natural(3), "3n"),
        (ir::Literal::Integer(-4), "-4i"),
        (ir::Literal::Real(1.5), "1.5"),
        (ir::Literal::String("x".to_string()), "\"x\""),
        (ir::Literal::Boolean(true), "true"),
    ] {
        assert_eq!(ir::Witness::Literal(literal).to_string(), printed);
    }
    assert_eq!(ir::Witness::Any.to_string(), "anything");
    assert_eq!(
        ir::Witness::Tag {
            name: "A".to_string(),
            payload: Some(Box::new(ir::Witness::Natural(1))),
        }
        .to_string(),
        "#A 1n"
    );
    // A payload that is itself a tag — bare or carrying — or prose, is
    // bracketed: the example has to read back as the one value it is.
    assert_eq!(
        ir::Witness::Tag {
            name: "A".to_string(),
            payload: Some(Box::new(ir::Witness::Tag {
                name: "X".to_string(),
                payload: Some(Box::new(ir::Witness::Natural(1))),
            })),
        }
        .to_string(),
        "#A (#X 1n)"
    );
    assert_eq!(
        ir::Witness::Tag {
            name: "A".to_string(),
            payload: Some(Box::new(ir::Witness::Tag {
                name: "X".to_string(),
                payload: None,
            })),
        }
        .to_string(),
        "#A (#X)"
    );
    assert_eq!(
        ir::Witness::Tag {
            name: "B".to_string(),
            payload: Some(Box::new(ir::Witness::Other(vec![
                "X".to_string(),
                "Y".to_string()
            ]))),
        }
        .to_string(),
        "#B (anything other than #X or #Y)"
    );
    // A struct witness beside a prose field: the field list reads on.
    assert_eq!(
        ir::Witness::Struct(
            [
                (
                    "a".to_string(),
                    ir::Witness::Tag {
                        name: "A".to_string(),
                        payload: None,
                    },
                ),
                ("b".to_string(), ir::Witness::Other(vec!["X".to_string()]),),
            ]
            .into_iter()
            .collect(),
        )
        .to_string(),
        "{ a: #A, b: anything other than #X }"
    );

    // R13: a field held present with any value at all prints pun-style —
    // under exactness the presence is the information, so the field appears
    // rather than folding away into `{}`.
    assert_eq!(
        ir::Witness::Struct([("a".to_string(), ir::Witness::Any)].into_iter().collect())
            .to_string(),
        "{ a }"
    );
    assert_eq!(
        ir::Witness::Struct(
            [("field name".to_string(), ir::Witness::Any)]
                .into_iter()
                .collect()
        )
        .to_string(),
        "{ \"field name\": anything }"
    );
    assert_eq!(
        ir::Witness::Struct(
            [
                ("a".to_string(), ir::Witness::Any),
                ("b".to_string(), ir::Witness::Natural(1)),
            ]
            .into_iter()
            .collect(),
        )
        .to_string(),
        "{ a, b: 1n }"
    );
    // And the value with no fields at all is still the empty braces.
    assert_eq!(ir::Witness::Struct(Default::default()).to_string(), "{}");
}

/// The prose a person meets stays in their words: the compiler's own names
/// for the machinery — refutability, scrutinees, join points, pattern
/// matrices — appear in no diagnostic. Checked over every complaint the
/// compiler can raise, not just the new ones, so a reworded message cannot
/// quietly pick the jargon up.
#[test]
fn no_complaint_speaks_in_pattern_jargon() {
    for (phase, code, message) in diagnostics() {
        for jargon in ["refutable", "scrutinee", "join point", "pattern matrix"] {
            assert!(
                !message.to_lowercase().contains(jargon),
                "{phase}/{code}: {message}"
            );
        }
    }
}

#[test]
fn parse_expectations_are_worded_for_their_source_context() {
    let primary = Span::generated(8, 1);
    let related = Span::generated(4, 1);
    let error = |expected, kind| parse::Error {
        span: primary,
        kind: parse::ErrorKind::Expected {
            expected,
            found: parse::Found::Token,
            related: Some(parse::Related {
                span: related,
                kind,
            }),
            context: None,
        },
    };
    let said = |expected, kind| error(expected, kind).to_string();

    assert_eq!(
        said(
            parse::Expected::Punctuation("}"),
            parse::RelatedKind::Opener
        ),
        "add `}` to close this part"
    );
    assert_eq!(
        said(parse::Expected::Value, parse::RelatedKind::Operator),
        "write a value after this"
    );
    assert_eq!(
        said(parse::Expected::Effect, parse::RelatedKind::Separator),
        "write an effect name after this"
    );

    // A token after an unclosed delimiter belongs to the surrounding syntax;
    // do not blame it as unusable. Name the insertion before it and retain the
    // opener as the related source location.
    for (mark, code) in [
        (")", "expected-closing-parenthesis"),
        ("}", "expected-closing-brace"),
    ] {
        let diagnostic = error(
            parse::Expected::Punctuation(mark),
            parse::RelatedKind::Opener,
        )
        .diagnostic();
        assert_eq!(diagnostic.code, code);
        assert_eq!(diagnostic.title, format!("add `{mark}` to close this part"));
        assert_eq!(
            diagnostic.primary.message,
            format!("write `{mark}` before this")
        );
        assert_eq!(
            diagnostic.related,
            vec![ui::Annotation {
                span: related,
                message: "opened here".into(),
            }]
        );
    }
}

/// The misplaced discard is one meaning worded five ways. Every wording is
/// distinct, all share one code, and each holds to the phrase rules like any
/// diagnostic. The wording per position is pinned so a reworded meaning cannot
/// slip by.
#[test]
fn a_misplaced_wildcard_is_worded_for_its_position() {
    let span = Span::generated(0, 1);
    let said = |place: parse::Place| {
        let error = parse::Error {
            span,
            kind: parse::ErrorKind::Wildcard { place },
        };
        assert_eq!(error.code(), "misplaced-discard", "{place:?}");
        error.to_string()
    };

    assert_eq!(
        said(parse::Place::Value),
        "`_` throws a value away, so it cannot be read here"
    );
    assert_eq!(
        said(parse::Place::Field),
        "a field needs a name other than `_`"
    );
    assert_eq!(
        said(parse::Place::Pun),
        "write a field name, or give the field a value after `:`"
    );
    assert_eq!(
        said(parse::Place::Projection),
        "write the name of the field to read"
    );
    assert_eq!(
        said(parse::Place::Type),
        "this place needs a name rather than `_`"
    );

    // One meaning, five phrasings: no two are the same sentence, and each is a
    // placeable phrase — no leading capital, no trailing period — that names
    // no machinery.
    let places = [
        parse::Place::Value,
        parse::Place::Field,
        parse::Place::Pun,
        parse::Place::Projection,
        parse::Place::Type,
    ];
    let wordings: HashSet<String> = places.iter().map(|place| said(*place)).collect();
    assert_eq!(wordings.len(), places.len(), "{wordings:?}");
    for wording in &wordings {
        assert!(!wording.ends_with('.'), "{wording}");
        assert!(!wording.to_lowercase().contains("wildcard"), "{wording}");
    }
}

/// A formula prints in the surface `where` grammar, with exactly the
/// parentheses re-parsing needs and no others — and the two shapes a reader
/// wrote keep their own spellings.
#[test]
fn a_formula_prints_in_the_where_grammar() {
    let a = Formula::bound(0);
    let b = Formula::bound(1);
    let c = Formula::bound(2);
    for (formula, printed) in [
        (a.clone(), "'a"),
        (a.clone().not(), "not 'a"),
        (a.clone().not().not(), "'a"),
        (a.clone().and(b.clone()), "'a and 'b"),
        (a.clone().or(b.clone()), "'a or 'b"),
        (a.clone().iff(b.clone()), "'a = 'b"),
        (a.clone().xor(b.clone()), "'a != 'b"),
        // `and` binds tighter than `or`, which binds tighter than a
        // comparison, so none of these needs a bracket.
        (a.clone().or(b.clone().and(c.clone())), "'a or 'b and 'c"),
        (a.clone().and(b.clone()).or(c.clone()), "'a and 'b or 'c"),
        (a.clone().iff(b.clone().or(c.clone())), "'a = 'b or 'c"),
        // And these do, because the grammar reads them the other way round
        // without them.
        (a.clone().or(b.clone()).and(c.clone()), "('a or 'b) and 'c"),
        (a.clone().and(b.clone()).not(), "not ('a and 'b)"),
        (a.clone().and(b.clone().or(c.clone())), "'a and ('b or 'c)"),
        (a.clone().iff(b.clone()).or(c.clone()), "('a = 'b) or 'c"),
        (a.clone().xor(b.clone().iff(c.clone())), "'a != ('b = 'c)"),
        // Left-associative, so a right-nested one of the same level brackets.
        (a.clone().or(b.clone().or(c.clone())), "'a or ('b or 'c)"),
        (
            a.clone().and(b.clone().and(c.clone())),
            "'a and ('b and 'c)",
        ),
        // Neither constant has a spelling in the grammar, and neither has to.
        (Formula::True, "always"),
        (Formula::False, "never"),
    ] {
        assert_eq!(formula.to_string(), printed);
    }
}

/// What a printed formula says has to be readable back, so every parenthesised
/// form above re-parses to a clause spelling itself the same way.
#[test]
fn a_printed_formula_reads_back_as_itself() {
    for clause in [
        "'a",
        "not 'a",
        "'a and 'b",
        "'a or 'b",
        "'a = 'b",
        "'a != 'b",
        "'a or 'b and 'c",
        "'a and 'b or 'c",
        "'a = 'b or 'c",
        "('a or 'b) and 'c",
        "not ('a and 'b)",
        "'a and ('b or 'c)",
        "('a = 'b) or 'c",
        "'a != ('b = 'c)",
        "'a or ('b or 'c)",
        "'a and ('b and 'c)",
    ] {
        let src =
            format!("type T = {{ a when 'a: Nat, b when 'b: Nat, c when 'c: Nat }} where {clause}");
        let out = parse::parse(token::lex(&src, FileID::GENERATED).tokens);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
        let parse::StmtKind::Type { body, .. } = &out.stmts[0].tracked else {
            panic!("expected a declaration");
        };
        let written = body.clause.as_ref().expect("the clause");
        assert_eq!(
            print::ast::where_clause(written).to_string(),
            clause,
            "{src}"
        );
    }
}

/// The `?` survives as the one thing a printer still has to be able to say
/// about a presence a failure abandoned — on a field and on a case alike —
/// and nothing parses it back, which is exactly what it is reporting.
#[test]
fn an_abandoned_presence_still_prints_as_a_question_mark() {
    let undecided = |ty: Rc<Ty>| RowField {
        presence: Presence::Undecided,
        ty,
    };
    let nat = Rc::new(Ty::plain(Ty::Nat));
    let fields: IndexMap<String, RowField> = [("x".to_string(), undecided(nat.clone()))]
        .into_iter()
        .collect();
    assert_eq!(
        Ty::Struct(Row {
            labels: fields,
            rest: Rest::Closed
        })
        .to_string(),
        "{ x?: Nat }"
    );

    let cases: IndexMap<String, RowField> =
        [("A".to_string(), undecided(nat))].into_iter().collect();
    assert_eq!(
        Ty::plain(Ty::Sum(Row {
            labels: cases,
            rest: Rest::Closed,
        }))
        .to_string(),
        "#A? Nat"
    );
}

/// Every sort a scheme quantifies prints from one alphabet and wears one
/// sigil: a type, a rest and a presence all read `'a` at index nought, because
/// that is how each of them is written. A scheme's one index space is what
/// keeps two of them from ever colliding.
#[test]
fn the_two_alphabets_are_told_apart_by_the_quote() {
    assert_eq!(Ty::Bound(0).to_string(), "'a");
    assert_eq!(Presence::Bound(0).to_string(), "'a");
    assert_eq!(Presence::Bound(26).to_string(), "'a1");
    assert_eq!(Formula::bound(25).to_string(), "'z");
    assert_eq!(Formula::var(3).to_string(), "?3");
}

/// A complaint about a use site quotes its formula in the labels the presences
/// decide, because that is what the reader wrote — they never saw the variable.
#[test]
fn a_formula_can_be_quoted_in_its_labels() {
    let formula = Formula::var(0).xor(Formula::var(1));
    assert_eq!(ui::in_labels(&formula, &[]), "?0 != ?1");
    assert_eq!(
        ui::in_labels(
            &formula,
            &[
                ("x".to_string(), Presence::Var(0)),
                ("y".to_string(), Presence::Var(1)),
            ]
        ),
        "x != y"
    );
    assert_eq!(
        ui::in_labels(
            &formula,
            &[
                ("left\u{1f}right".to_string(), Presence::Var(0)),
                ("other".to_string(), Presence::Var(1)),
            ]
        ),
        "left\u{1f}right != other"
    );
    // A presence no label decides falls back to the presence itself.
    assert_eq!(
        ui::in_labels(&formula, &[("x".to_string(), Presence::Var(0))]),
        "x != ?1"
    );
}

/// Every complaint the effect feature added, in the words a reader meets. Kept
/// beside the codes rather than only among them, because what these say is the
/// whole of what a reader has to act on — and none of them may say "row",
/// which names the representation the three shapes share and nothing anybody
/// wrote.
#[test]
fn the_effect_complaints_are_read_in_effects() {
    for (kind, message) in [
        (
            IrError::NotAnOperation {
                name: "here".to_string(),
            },
            "`here` is not a function",
        ),
        (
            IrError::ImpureOperation {
                found: ir::OperationTypeProblem::OpenPart,
            },
            "an operation must have one fixed function type",
        ),
        (
            IrError::OperationOnAlias {
                effect: "Console".to_string(),
            },
            "effect alias `!Console` declares no operations",
        ),
        (
            IrError::UnknownOperation {
                effect: "Log".to_string(),
                op: "writ".to_string(),
            },
            "effect `!Log` has no operation `writ`",
        ),
        (
            IrError::PartialHandler {
                effect: "Log".to_string(),
                missing: vec!["flush".to_string()],
            },
            "this handler does not cover effect `!Log`",
        ),
        (
            IrError::DuplicateArm {
                effect: "Log".to_string(),
                selector: ir::OperationSelector::Named("write".to_string()),
                previous: Span::generated(0, 1),
            },
            "duplicate arm for `!Log.write`",
        ),
        (
            IrError::DuplicateArm {
                effect: "Log".to_string(),
                selector: ir::OperationSelector::Unnamed,
                previous: Span::generated(0, 1),
            },
            "duplicate arm for `!Log`",
        ),
        (
            IrError::DuplicateReturn {
                previous: Span::generated(0, 1),
            },
            "this handler has more than one `return` arm",
        ),
        (
            IrError::RaiseOutsideArm,
            "`raise` can be used only directly inside a handler arm",
        ),
        (
            IrError::EffectsOutsideRow,
            "effects cannot be used as a type by themselves",
        ),
        (
            IrError::RaiseInFunction {
                function: Span::generated(0, 1),
            },
            "`raise` cannot cross a function boundary",
        ),
        (
            IrError::OpenDeclaredType {
                shape: Shape::Effect,
            },
            "a declared type must list its effects exactly",
        ),
    ] {
        assert_eq!(kind.to_string(), message, "{}", kind.code());
    }

    // Each reason an operation signature is rejected keeps the actionable
    // detail in its label and repair rather than forcing reporters to split a
    // headline apart.
    let span = Span::generated(0, 1);
    for (found, title, label, help) in [
        (
            ir::OperationTypeProblem::Effects,
            "an operation signature cannot declare effects",
            "operation calls already perform the operation's own effect",
            "remove this `+` effect list",
        ),
        (
            ir::OperationTypeProblem::OpenPart,
            "an operation must have one fixed function type",
            "this leaves part of the operation's type undecided",
            "write this part explicitly; use `..` and `when` in annotations instead",
        ),
        (
            ir::OperationTypeProblem::Variable("a".to_string()),
            "`'a` is not declared by this operation",
            "operation signatures cannot introduce type variables",
            "replace it with a fixed type or a declared type application",
        ),
    ] {
        let diagnostic = ir::Error {
            span,
            kind: IrError::ImpureOperation { found },
        }
        .diagnostic();
        assert_eq!(diagnostic.title, title);
        assert_eq!(diagnostic.primary.message, label);
        assert_eq!(diagnostic.help, [help]);
    }

    let empty = ir::Error {
        span,
        kind: IrError::BareOperationUnavailable {
            effect: "Nil".to_string(),
            suggestion: None,
        },
    }
    .diagnostic();
    assert_eq!(empty.title, "effect `!Nil` declares no operations");
    assert_eq!(
        empty.primary.message,
        "there is nothing in this effect to perform"
    );
    assert_eq!(
        empty.help,
        ["remove this performance, or declare an operation on the effect"]
    );

    for (kind, message) in [
        (
            TypeError::Unhandled {
                effect: "Log".to_string(),
            },
            "nothing can handle `!Log` here: a definition's value is computed outside every handler",
        ),
        (
            TypeError::NotAllowed {
                effect: "Log".to_string(),
            },
            "this function performs `!Log`, which its type does not allow",
        ),
        (
            TypeError::ExtraField {
                shape: Shape::Effect,
                base: Rc::new(Ty::plain(Ty::pure(
                    Rc::new(Ty::plain(Ty::Nat)),
                    Rc::new(Ty::plain(Ty::Nat)),
                ))),
                field: "Log".to_string(),
            },
            "extra effect `!Log`: the type `Nat -> Nat` lists every effect it allows",
        ),
        (
            TypeError::RepeatedField {
                shape: Shape::Effect,
                field: "Log".to_string(),
            },
            "`..` covers only the effects a type does not already name, and here it would have to cover `!Log`",
        ),
    ] {
        assert_eq!(kind.to_string(), message, "{}", kind.code());
    }
}

/// A missing operation is named in a sentence, and every one of them: an arm
/// per operation is what the reader has to write, so listing all of them is
/// the instruction rather than a count of how far off they are.
#[test]
fn a_partial_handler_names_every_operation_with_no_arm() {
    let named = |missing: &[&str]| {
        ir::Error {
            span: Span::generated(0, 1),
            kind: IrError::PartialHandler {
                effect: "Log".to_string(),
                missing: missing.iter().map(|name| name.to_string()).collect(),
            },
        }
        .diagnostic()
    };
    for (missing, listed) in [
        (&["flush"][..], "`flush`"),
        (&["flush", "close"][..], "`flush` and `close`"),
        (
            &["flush", "close", "sync"][..],
            "`flush`, `close` and `sync`",
        ),
    ] {
        let diagnostic = named(missing);
        let arms = if missing.len() == 1 {
            format!("an arm for {listed}")
        } else {
            format!("{} arms: {listed}", missing.len())
        };
        assert_eq!(diagnostic.primary.message, format!("missing {arms}"));
        assert_eq!(diagnostic.help, [format!("add {arms}")]);
    }
}

/// The one rule that widens rather than equates reads as the widening it is,
/// and the constraint it comes from prints its two rows in the effect notation
/// — an ambient belongs to no type, so there is no arrow around it to hand one
/// down.
#[test]
fn the_performs_constraint_reads_as_a_widening() {
    let row = |labels: &[&str], rest: Rest| Row {
        labels: labels
            .iter()
            .map(|name| (name.to_string(), RowField::present(Rc::new(Ty::unit()))))
            .collect(),
        rest,
    };
    let kind = ConstraintKind::Performs {
        performed: row(&["Log"], Rest::Closed),
        ambient: row(&["Log", "IO"], Rest::Var(3)),
        inside: true,
    };
    assert_eq!(kind.code(), "performs");
    assert_eq!(
        kind.to_string(),
        "!Log performed where !Log + !IO + ..?3 is allowed"
    );
    // The empty row writes the `|` it is spelled with rather than nothing at
    // all, which would leave the line with a gap in it.
    let empty = ConstraintKind::Performs {
        performed: row(&[], Rest::Closed),
        ambient: row(&[], Rest::Closed),
        inside: false,
    };
    assert_eq!(empty.to_string(), "| performed where | is allowed");
}

/// The token spellings of the three reserved words, the `+` that introduces an
/// effect row and the effect itself, so a printed stream re-lexes to the tokens
/// it came from. An effect keeps its `!` for the reason a tag keeps its `#`: a
/// bare name would come back as an identifier.
#[test]
fn the_effect_tokens_print_as_they_were_written() {
    for (kind, spelled) in [
        (TokenKind::Effect, "effect"),
        (TokenKind::Handle, "handle"),
        (TokenKind::Raise, "raise"),
        (TokenKind::Plus, "+"),
        (TokenKind::NotEqual, "!="),
        (TokenKind::EffectLabel("Log".to_string()), "!Log"),
    ] {
        assert_eq!(kind.to_string(), spelled);
    }
}

/// Duplicate lowering complaints retain both actionable locations: the repeat
/// is primary and the occurrence that already stood is secondary. The variants
/// below cover each source-level duplicate payload, including the return arm
/// whose previous span predated the other redesigned variants.
#[test]
fn ir_duplicate_diagnostics_point_back_to_the_first_occurrence() {
    let primary = Span::generated(12, 2);
    let previous = Span::generated(3, 1);
    let examples = [
        (
            IrError::DuplicateField {
                name: "x".to_string(),
                previous,
            },
            "field `x` is written more than once",
            "written again here",
            ui::FIRST_WRITTEN,
        ),
        (
            IrError::DuplicateCase {
                shape: Shape::Sum,
                name: "Ready".to_string(),
                previous,
            },
            "case `#Ready` is included more than once",
            "included again here",
            ui::FIRST_WRITTEN,
        ),
        (
            IrError::DuplicateBinding {
                name: "value".to_string(),
                previous,
            },
            "this pattern binds `value` more than once",
            "bound again here",
            ui::FIRST_BINDING,
        ),
        (
            IrError::DuplicateOperation {
                name: "write".to_string(),
                previous,
            },
            "operation `write` is declared more than once",
            "declared again here",
            ui::FIRST_DECLARATION,
        ),
        (
            IrError::DuplicateArm {
                effect: "Log".to_string(),
                selector: ir::OperationSelector::Named("write".to_string()),
                previous,
            },
            "duplicate arm for `!Log.write`",
            "this operation is handled again",
            ui::FIRST_ARM,
        ),
        (
            IrError::DuplicateReturn { previous },
            "this handler has more than one `return` arm",
            "second return arm",
            ui::FIRST_ARM,
        ),
    ];

    for (kind, title, label, related_label) in examples {
        let diagnostic = ir::Error {
            span: primary,
            kind,
        }
        .diagnostic();
        assert_eq!(diagnostic.title, title);
        assert_eq!(diagnostic.primary.span, primary);
        assert_eq!(diagnostic.primary.message, label);
        assert_eq!(
            diagnostic.related,
            [ui::Annotation {
                span: previous,
                message: related_label.to_string(),
            }]
        );
    }
}

/// Headline, local explanation, and repair are separate parts of an IR
/// diagnostic. Pin representative declaration, row, and operation failures so
/// a reporter never has to split prose back apart to present them.
#[test]
fn redesigned_ir_diagnostics_expose_labels_and_help() {
    let span = Span::generated(9, 1);
    for (kind, title, label, help) in [
        (
            IrError::OpenDeclaredType { shape: Shape::Sum },
            "a declared type must list its cases exactly",
            "this leaves part of the declared type undecided",
            "list every label, use one of the declaration's parameters, or move this type to an annotation",
        ),
        (
            IrError::AbsentInClosed {
                shape: Shape::Sum,
                label: "Missing".to_string(),
            },
            "a type with no `..` already says `#Missing` is not there",
            "this mark repeats what the closed type already says",
            "remove this `\\` mark; a closed type already excludes labels it does not list",
        ),
        (
            IrError::EffectsOutsideRow,
            "effects cannot be used as a type by themselves",
            "there is no function arrow here to carry these effects",
            "write them after a function result, as in `Nat -> Nat + !Log`",
        ),
        (
            IrError::NotAnOperation {
                name: "write".to_string(),
            },
            "`write` is not a function",
            "an operation signature must be a function type",
            "write a signature such as `write : Nat -> ()`",
        ),
    ] {
        let diagnostic = ir::Error { span, kind }.diagnostic();
        assert_eq!(diagnostic.title, title);
        assert_eq!(diagnostic.primary.message, label);
        assert_eq!(diagnostic.help, [help]);
    }
}

/// Operation signatures are declarations of a fixed interface, but a hole in
/// one has its own identity and headline rather than borrowing the declared-
/// type complaint.
#[test]
fn a_hole_in_an_operation_has_its_own_code_and_title() {
    let diagnostic = ir::Error {
        span: Span::generated(5, 1),
        kind: IrError::HoleInOperation,
    }
    .diagnostic();
    assert_eq!(diagnostic.code, "hole-in-operation");
    assert_eq!(
        diagnostic.title,
        "an operation signature cannot contain `_`"
    );
}

/// The two codes `ui::code` used to fold into the term namespace's. A reporter
/// that wants to treat an undefined module differently from an undefined term
/// should not have to re-inspect the variant to tell them apart, which is the
/// whole reason the namespace is part of the code.
#[test]
fn the_module_namespace_has_codes_of_its_own() {
    let undefined = IrError::Undefined {
        name: "Math".to_string(),
        namespace: Namespace::Modules,
    };
    assert_eq!(undefined.code(), "undefined-module");
    assert_eq!(undefined.to_string(), "cannot find module `Math`");

    let duplicate = IrError::Duplicate {
        name: "Math".to_string(),
        namespace: Namespace::Modules,
        previous: Span::generated(0, 1),
    };
    assert_eq!(duplicate.code(), "duplicate-module");
    assert_eq!(duplicate.to_string(), "`Math` is defined more than once");
}

/// The two things reading a bundle's files can refuse, worded and coded like
/// every other phase's. They name the exact paths that were looked for, because
/// a reader told only "no file" still has to work out where one would have gone.
#[test]
fn the_bundle_phase_words_and_codes_its_refusals() {
    let span = Span::generated(7, 4);
    let missing = ruddy::bundle::Error {
        span,
        kind: BundleError::ModuleFileMissing {
            beside: "Math.hc".to_string(),
            inside: "Math/module.hc".to_string(),
        },
    };
    let diagnostic = missing.diagnostic();
    assert_eq!(diagnostic.code, "module-file-missing");
    assert_eq!(diagnostic.title, "this module needs a file");
    assert_eq!(diagnostic.primary.span, span);
    assert_eq!(
        diagnostic.primary.message,
        "no file was found for this module"
    );
    assert_eq!(diagnostic.help, ["create `Math.hc` or `Math/module.hc`"]);
    assert_eq!(diagnostic.notes.len(), 1);
    assert!(diagnostic.related.is_empty());

    let ambiguous = ruddy::bundle::Error {
        span,
        kind: BundleError::ModuleFileAmbiguous {
            beside: "Math.hc".to_string(),
            inside: "Math/module.hc".to_string(),
        },
    };
    let diagnostic = ambiguous.diagnostic();
    assert_eq!(diagnostic.code, "module-file-ambiguous");
    assert_eq!(diagnostic.title, "this module has two possible files");
    assert_eq!(
        diagnostic.primary.message,
        "Ruddy cannot choose which file defines this module"
    );
    assert_eq!(
        diagnostic.help,
        ["keep one of `Math.hc` or `Math/module.hc` and delete the other"]
    );
    assert_eq!(diagnostic.notes.len(), 1);
}

/// The tokens the module grammar added print as the lexemes they were written
/// with, so a printed stream re-lexes to the tokens it came from.
#[test]
fn the_module_tokens_print_as_they_were_written() {
    assert_eq!(TokenKind::Module.to_string(), "module");
    assert_eq!(TokenKind::ColonColon.to_string(), "::");
}

/// Effects are internally keyed by their full module path, but diagnostics and
/// printers must preserve the sigil's source position before the final name.
#[test]
fn a_qualified_effect_label_keeps_its_sigil_on_the_effect() {
    assert_eq!(ui::label(Shape::Effect, "Log"), "!Log");
    assert_eq!(ui::label(Shape::Effect, "Math::Log"), "Math::!Log");
}
