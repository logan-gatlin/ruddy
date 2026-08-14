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
    inference::{self, Constraint, ConstraintKind, Effect, ErrorKind as TypeError, Goal, Rule},
    ir::{self, ErrorKind as IrError},
    parse,
    patterns::ErrorKind as PatternError,
    symbol::{Bundle, Mint, Namespace, Version},
    token::{self, ErrorKind as LexError, Kind as TokenKind},
    tracking::{FileID, Span},
    types::{Assigned, Core, Formula, Presence, Prim, Rest, Row, RowField, Sense, Shape, Ty},
    ui,
};
use ruddy_debug::print;

/// Every error kind in the compiler, with the phase that raises it. Listed by
/// hand because nothing can force it: a new variant added without a line here
/// is the exact thing this module exists to catch, so it is worth the reminder
/// that adding one means coming back.
fn diagnostics() -> Vec<(&'static str, &'static str, String)> {
    let span = Span::generated(0, 1);
    let nat = Rc::new(Ty::plain(Core::Nat));

    let mut all: Vec<(&str, &str, String)> = Vec::new();
    for kind in [
        LexError::Unrecognized,
        LexError::MalformedNatural,
        LexError::NaturalTooLarge,
    ] {
        all.push(("lex", kind.code(), kind.to_string()));
    }

    // Every kind and, for the wildcard, every position it can be worded for:
    // one meaning, five phrasings, and each has to hold to the phrase rules.
    for kind in [
        parse::ErrorKind::Unexpected,
        parse::ErrorKind::Wildcard {
            place: parse::Place::Value,
        },
    ] {
        let error = parse::Error { span, kind };
        all.push(("parse", error.code(), error.to_string()));
    }

    // Both namespaces of all three name errors: the namespace is part of the
    // code, so an undefined type and an undefined term are two diagnostics
    // here, and so are the two halves of a loop of bare names.
    for namespace in [Namespace::Terms, Namespace::Types] {
        for kind in [
            IrError::Undefined { namespace },
            IrError::Duplicate {
                namespace,
                previous: span,
            },
            IrError::Circular { namespace },
        ] {
            all.push(("ir", kind.code(), kind.to_string()));
        }
    }
    for kind in [
        IrError::DuplicateField,
        IrError::AbsentInClosed {
            shape: Shape::Struct,
            label: "x".to_string(),
        },
        IrError::OpenDeclaredType {
            shape: Shape::Struct,
        },
        IrError::Arity {
            expected: 2,
            found: 1,
        },
        IrError::NotAConstructor,
        IrError::ParameterApplied,
        IrError::DuplicateParameter { previous: span },
        IrError::GrowingRecursion,
        IrError::DuplicateCase,
        IrError::MixedTail {
            first: Sense::Type,
            second: Sense::Cases,
            previous: span,
        },
        IrError::MixedParameter {
            first: Sense::Type,
            second: Sense::Cases,
        },
        IrError::NotARow,
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
        },
        IrError::ClauseInDeclaration,
        IrError::UnboundPresence {
            name: "c".to_string(),
        },
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

    for kind in [
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
        TypeError::AnnotationTooOpen,
        TypeError::RepeatedField {
            shape: Shape::Struct,
            field: "x".to_string(),
        },
        TypeError::PresenceRequired {
            formula: "x != y".to_string(),
        },
        TypeError::PresenceImpossible {
            formula: "x and y".to_string(),
        },
        TypeError::AnnotationAllows {
            allowed: "a or b".to_string(),
            required: "a".to_string(),
        },
    ] {
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
    Rule::Struct,
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

/// The two rules that decide one label at a time say which kind of label it
/// was, because a reader stepping through a solve is reading about their own
/// program: "whether the field is there" over a goal about `` `Some `` and
/// `` `None `` describes something they never wrote. The same reason
/// `Rule::Struct` and `Rule::Sum` are two rules rather than one worded about
/// rows — and the reason neither says "label", a word the language does not
/// have anywhere a reader can see it.
///
/// The code is shared by the two shapes on purpose: it names which act of the
/// solver ran, and the goal beside it already shows which shape it ran on.
#[test]
fn a_shaped_rule_is_read_in_the_nouns_of_its_shape() {
    for shape in [Shape::Struct, Shape::Sum] {
        let (theirs, others) = match shape {
            Shape::Struct => ("field", "case"),
            Shape::Sum => ("case", "field"),
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
    let nat = Rc::new(Ty::plain(Core::Nat));
    let span = Span::generated(0, 1);

    let equal = Constraint {
        span,
        kind: ConstraintKind::Equal {
            expected: nat.clone(),
            actual: Rc::new(Ty::plain(Core::Var(0))),
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
            value: Assigned::Ty(Rc::new(Ty::plain(Core::Nat)))
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
    let nat = Rc::new(Ty::plain(Core::Nat));
    let endo = Rc::new(Ty::plain(Core::Arrow(nat.clone(), nat.clone())));

    // A declared type prints as its name and is an atom whatever it stands
    // for, so the arrow behind this one leaks no parentheses through it. It
    // needs a declaration to be read back, which the prelude supplies.
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Endo")
        .expect("a fresh name");
    let named = Rc::new(Ty::plain(Core::Named {
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
            Rc::new(Ty::plain(Core::Arrow(endo.clone(), endo.clone()))),
            "(Nat -> Nat) -> Nat -> Nat",
        ),
        // The empty struct is unit, and prints as the one spelling this
        // language has for it. See `Core::Unit` for why it is `{}` and not
        // `()`.
        (
            "",
            Rc::new(Ty {
                core: Core::Unit,
                fields: Default::default(),
            }),
            "{}",
        ),
        (
            "",
            Rc::new(Ty {
                core: Core::Unit,
                fields: [("x".to_string(), RowField::present(endo.clone()))]
                    .into_iter()
                    .collect(),
            }),
            "{ x: Nat -> Nat }",
        ),
        (
            "type Endo = Nat -> Nat\n",
            Rc::new(Ty::plain(Core::Arrow(named.clone(), named.clone()))),
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
    let nat = Rc::new(Ty::plain(Core::Nat));

    // A row's tail prints in the surface spelling, after the fields; a
    // quantified tail wears its letter and an undecided one has nothing to
    // report. An open row with no fields still shows it is open.
    assert_eq!(
        Ty {
            core: Core::Bound(0),
            fields: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
        }
        .to_string(),
        "{ x: Nat, ..'a }"
    );
    assert_eq!(
        Ty {
            core: Core::Undecided,
            fields: [("x".to_string(), RowField::present(nat.clone()))]
                .into_iter()
                .collect(),
        }
        .to_string(),
        "{ x: Nat, .. }"
    );
    // A type carrying no labels prints as its core alone, whatever the core is:
    // there is no `..` to write when there is nothing for it to follow, and a
    // quantified core is exactly a quantified type.
    assert_eq!(
        Ty {
            core: Core::Bound(0),
            fields: Default::default(),
        }
        .to_string(),
        "'a"
    );

    // A field's presence prints as its surface spelling too: certainly there
    // is unmarked, undecided either way is `?`, and certainly absent is not
    // part of what the type says at all.
    assert_eq!(
        Ty {
            core: Core::Unit,
            fields: [
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
        }
        .to_string(),
        "{ x when a: Nat }"
    );

    // `{ x: Nat, .. }` does parse, and what it parses to is a row with a fresh
    // tail of its own — not the one it was printed from. The string survives;
    // the identity it was standing for does not.
    assert_eq!(round_trip("", "{ x: Nat, .. }"), "{ x: Nat, ..'a }");
}

/// The notes are held in [`ui`] rather than in either reporter, because both
/// print them: the CLI as a second indented line, the strip as a second
/// highlight on the same diagnostic. One for the complaints about a name
/// defined twice and one for the name used two ways, since the second place is
/// a definition in the first and a use in the other.
#[test]
fn the_notes_pointing_elsewhere_are_worded_once() {
    for note in [ui::FIRST_DEFINITION, ui::FIRST_USE] {
        assert!(!note.is_empty());
        assert!(!note.ends_with('.'));
        assert!(
            !note.starts_with(|c: char| c.is_ascii_uppercase()),
            "{note}"
        );
    }
    assert_ne!(ui::FIRST_DEFINITION, ui::FIRST_USE);
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
fn a_complaint_about_a_sum_says_case_and_writes_the_backtick() {
    let sum = Rc::new(Ty::plain(Core::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField {
                presence: Presence::Present,
                ty: Rc::new(Ty::plain(Core::Nat)),
            },
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    let nat = Rc::new(Ty::plain(Core::Nat));

    let missing = TypeError::MissingField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(missing.to_string(), "no case ``B` on ``A Nat`");
    let extra = TypeError::ExtraField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert!(extra.to_string().contains("extra case ``B`"), "{extra}");
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
    assert!(repeated.to_string().contains("``A`"), "{repeated}");
    let not_a_row = IrError::NotARow;
    assert!(not_a_row.to_string().contains("sum's cases"), "{not_a_row}");

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
/// backtick that makes it one.
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
        "a type with no `..` already says ``B` is not there"
    );
}

/// The printer keeps dropping absent labels: a type ascribed with `\y` still
/// prints without it, so no existing printed output changes, and what a
/// scheme shows is what a value can have rather than what it cannot.
#[test]
fn a_written_absence_is_not_printed() {
    let source = "let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x\n\
                  let s : `Ok Nat | \\`Err | .. = `Ok 1\n";
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
    assert_eq!(printed, ["{ x: Nat, ..'a } -> Nat", "`Ok Nat | ..'a"]);
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
        "this stands for a whole type in one place \
         and for the rest of a sum's cases in another"
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
        TokenKind::In,
        TokenKind::Type,
        TokenKind::End,
        TokenKind::With,
        TokenKind::Match,
        TokenKind::Fn,
        TokenKind::Equal,
        TokenKind::FatArrow,
        TokenKind::Arrow,
        TokenKind::Colon,
        TokenKind::Comma,
        TokenKind::Dot,
        TokenKind::DotDot,
        TokenKind::NotEqual,
        TokenKind::Pipe,
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

    // The three that carry something print it, and re-lex to themselves as
    // well: the backtick stays on, and a number keeps its value.
    for kind in [
        TokenKind::Tag("Some".to_string()),
        TokenKind::Identifier("x".to_string()),
        TokenKind::Natural(4096),
    ] {
        let printed = kind.to_string();
        let out = token::lex(&printed, FileID::GENERATED);
        assert!(out.errors.is_empty(), "{printed}: {:#?}", out.errors);
        assert_eq!(out.tokens.len(), 1, "{printed}");
        assert_eq!(out.tokens[0].tracked.to_string(), printed);
    }
}

/// An arity complaint counts in words, and both halves of it — what the type
/// takes and what was written — count the same way. Nine is the last word;
/// past that it is a numeral, because a type taking ten arguments has a
/// problem the sentence is not going to help with.
#[test]
fn an_arity_complaint_counts_in_words_as_far_as_words_go() {
    let said = |expected, found| IrError::Arity { expected, found }.to_string();

    assert_eq!(
        said(0, 1),
        "this type takes no arguments, and one was written"
    );
    assert_eq!(
        said(1, 0),
        "this type takes one argument, and none was written"
    );
    assert_eq!(
        said(2, 9),
        "this type takes two arguments, and nine were written"
    );
    // Beyond the words, the numeral — on both sides.
    assert_eq!(
        said(10, 13),
        "this type takes 10 arguments, and 13 were written"
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
    let sum = Rc::new(Ty::plain(Core::Sum(Row {
        labels: [
            (
                "A".to_string(),
                RowField::present(Rc::new(Ty::plain(Core::Nat))),
            ),
            (
                "B".to_string(),
                RowField {
                    presence: Presence::Absent,
                    ty: Rc::new(Ty::plain(Core::Nat)),
                },
            ),
        ]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    assert_eq!(sum.to_string(), "`A Nat");

    // A case carrying anything that is not unit keeps its payload: only the
    // type with nothing of its own and no fields is written as no payload at
    // all.
    let open = Rc::new(Ty::plain(Core::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField::present(Rc::new(Ty {
                core: Core::Bound(0),
                fields: [("x".to_string(), RowField::present(Rc::new(Ty::unit())))]
                    .into_iter()
                    .collect(),
            })),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    assert_eq!(open.to_string(), "`A { x: {}, ..\'a }");

    // The two forms that write no case at all keep the leading bar, which is
    // the only thing that makes either read back as a sum.
    let empty = Rc::new(Ty::plain(Core::Sum(Row {
        labels: Default::default(),
        rest: Rest::Closed,
    })));
    assert_eq!(empty.to_string(), "|");
    let only_tail = Rc::new(Ty::plain(Core::Sum(Row {
        labels: Default::default(),
        rest: Rest::Bound(0),
    })));
    assert_eq!(only_tail.to_string(), "| ..\'a");
}

/// A sink that accepts `left` writes and then fails, so a printer can be run
/// against a failure at every point it has one.
struct Failing {
    left: usize,
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
/// dropped anywhere in a printer would show up here as a render that claimed to
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
fn a_printer_reports_a_writer_that_refuses_it() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let module = mint.module(None, "util").expect("a fresh name");
    let local = mint.local(Some(module), Namespace::Terms, "x");
    let named = Rc::new(Ty::plain(Core::Named {
        symbol: mint
            .global(None, Namespace::Types, "Pair")
            .expect("a fresh name"),
        name: "Pair".into(),
        args: Rc::from([nat.clone(), nat.clone()]),
    }));

    // A path, which writes the bundle, a module, and the anonymous segment a
    // local is shown under.
    every_failure_is_reported("a path", &mint.path(local));

    let optional = RowField {
        presence: Presence::Undecided,
        ty: nat.clone(),
    };
    for (what, ty) in [
        (
            "an arrow",
            Rc::new(Ty::plain(Core::Arrow(nat.clone(), nat.clone()))),
        ),
        ("an application", named.clone()),
        (
            "an open struct",
            Rc::new(Ty {
                core: Core::Bound(0),
                fields: [
                    ("x".to_string(), RowField::present(nat.clone())),
                    ("y".to_string(), optional.clone()),
                ]
                .into_iter()
                .collect(),
            }),
        ),
        (
            "an open sum",
            Rc::new(Ty::plain(Core::Sum(Row {
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
            Rc::new(Ty::plain(Core::Sum(Row {
                labels: Default::default(),
                rest: Rest::Closed,
            }))),
        ),
        (
            "the sum that is only its tail",
            Rc::new(Ty::plain(Core::Sum(Row {
                labels: Default::default(),
                rest: Rest::Bound(0),
            }))),
        ),
        // The `with` form, whose core and whose braces are two writes with a
        // word between them: a refusal on any of the three has to come back.
        (
            "a type carrying fields",
            Rc::new(Ty {
                core: Core::Nat,
                fields: [("x".to_string(), RowField::present(nat.clone()))]
                    .into_iter()
                    .collect(),
            }),
        ),
        (
            "a case carrying unit",
            Rc::new(Ty::plain(Core::Sum(Row {
                labels: [(
                    "None".to_string(),
                    RowField::present(Rc::new(Ty {
                        core: Core::Unit,
                        fields: Default::default(),
                    })),
                )]
                .into_iter()
                .collect(),
                rest: Rest::Closed,
            }))),
        ),
    ] {
        every_failure_is_reported(what, &ty);
    }

    // And the term printers, which have forms of their own: an application and
    // a projection, neither of which a type can be.
    // And a nested `let`, which writes its name, its ascription and its two
    // halves in pieces the way a path does.
    // An explicitly absent label in each shape, so a refusal inside the `\`
    // the printers write for one comes back too.
    let source = "type Pair a b = { first: a, second: b }\n\
                  let f = fn g => fn p => g p.first\n\
                  let h = let n : Nat = 1 in n\n\
                  let v : { first: Nat, second: Nat } = { first: 1, second: 2 }\n\
                  type Gap r = { first: Nat, \\hole, ..r }\n\
                  let w : `Ok Nat | \\`Err | .. = `Ok 1";
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
    let source = "let x : { first: Nat, opt when b: Nat, \\hole, .. } \
                  -> (|) -> (| ..r) -> (`Ok Nat | `Err (when a) | \\`B | ..) -> (`A | `B) = 1";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    every_failure_is_reported(
        "a parsed statement",
        &print::ast::stmt(&parsed.stmts[0].tracked),
    );
}

/// A type carrying fields, whatever its core: the shape no source syntax writes
/// and inference builds every time a projection is left unannotated. What the
/// core is decides everything about how it prints — braces with a `..`, or a
/// `with` — so it is the one thing the caller varies.
fn with_fields(core: Core, labels: Vec<(&str, RowField)>) -> Rc<Ty> {
    Rc::new(Ty {
        core,
        fields: labels
            .into_iter()
            .map(|(name, field)| (name.to_string(), field))
            .collect(),
    })
}

/// Where a `with` type needs parentheses and where it does not. It sits above
/// the arrow and the sum, so neither side of an arrow brackets it; it sits
/// below an atom, so every position that takes one — a type constructor's
/// argument, a tag's payload — does.
#[test]
fn a_with_type_is_bracketed_wherever_something_could_follow_its_fields() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let quantified = with_fields(
        Core::Nat,
        vec![("x", RowField::present(Rc::new(Ty::plain(Core::Bound(0)))))],
    );
    assert_eq!(quantified.to_string(), "Nat with { x: 'a }");

    // Either side of an arrow, bare: nothing in an arrow can swallow the
    // fields, and bracketing here would be noise on the commonest form there
    // is — the type of an unannotated accessor.
    assert_eq!(
        Ty::plain(Core::Arrow(quantified.clone(), nat.clone())).to_string(),
        "Nat with { x: 'a } -> Nat"
    );
    assert_eq!(
        Ty::plain(Core::Arrow(nat.clone(), quantified.clone())).to_string(),
        "Nat -> Nat with { x: 'a }"
    );

    // An argument of a declared type, bracketed: the fields would otherwise
    // read as the next argument along.
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "Pair")
        .expect("a fresh name");
    assert_eq!(
        Ty::plain(Core::Named {
            symbol,
            name: "Pair".into(),
            args: Rc::from([quantified.clone(), nat.clone()]),
        })
        .to_string(),
        "Pair (Nat with { x: 'a }) Nat"
    );

    // And a tag's payload, for the same reason.
    assert_eq!(
        Ty::plain(Core::Sum(Row {
            labels: [("Some".to_string(), RowField::present(quantified.clone()))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }))
        .to_string(),
        "`Some (Nat with { x: 'a })"
    );
}

/// A sum's tail decided to be more cases prints as those cases, in the notation
/// of the row it ends — and a splice that came to nothing prints as no tail at
/// all.
///
/// Both are what a sum's row parameter leaves behind, so both are what the
/// debugger's IR tab shows for a term checked against a declared type. `..r`
/// handed a sum's cases is those cases, and spelling them with colons would show
/// a reader a case list as if it were a set of fields. `..r` handed nothing
/// allows nothing more, which is what a closed row already says — and the
/// solver's own mark for that is never part of a printed type.
///
/// A struct has none of this any more. Its `..` is the core beside its fields,
/// and a core standing for a type with fields is spliced by `Table::resolve`
/// before anything prints it, so no chain ever reaches the page.
#[test]
fn a_spliced_tail_prints_in_the_notation_of_the_row_it_ends() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let more = |labels: Vec<(&str, RowField)>, rest: Rest| {
        Rest::More(Rc::new(Row {
            labels: labels
                .into_iter()
                .map(|(name, field)| (name.to_string(), field))
                .collect(),
            rest,
        }))
    };

    // `` type G r = `Err Nat | ..r `` applied to `` `Ok Nat ``, and to a row
    // naming nothing, which leaves the sum closed at the case it wrote.
    let err_nat = |rest: Rest| {
        Rc::new(Ty::plain(Core::Sum(Row {
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
        "`Err Nat | ..`Ok Nat"
    );
    assert_eq!(err_nat(more(vec![], Rest::Closed)).to_string(), "`Err Nat");

    // A chain of splices is read to its end, and a case the splice settled
    // absent counts for nothing on the way: what is written is whatever the
    // chain still leaves open.
    assert_eq!(
        err_nat(more(vec![], more(vec![], Rest::Closed))).to_string(),
        "`Err Nat"
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
        "`Err Nat | ..?3"
    );

    // A case whose payload is unit is still a case carrying unit, so it prints
    // with no payload at all.
    assert_eq!(
        Ty::plain(Core::Sum(Row {
            labels: [("A".to_string(), RowField::present(Rc::new(Ty::unit())))]
                .into_iter()
                .collect(),
            rest: Rest::Closed,
        }))
        .to_string(),
        "`A"
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
                RowField::present(Rc::new(Ty::plain(Core::Nat)))
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
        // A presence has its own alphabet, and no quote: the letter is the
        // name a `when` clause gave it.
        (Presence::Bound(1), "b"),
        (Presence::Undecided, "?"),
    ] {
        assert_eq!(presence.to_string(), printed);
    }

    // A binding prints as the value, whichever sort it is, so the Solve tab's
    // one column serves all three.
    for (value, printed) in [
        (Assigned::Ty(Rc::new(Ty::plain(Core::Nat))), "?2 := Nat"),
        (Assigned::Row(Rc::new(Row::closed())), "?2 := ∅"),
        (Assigned::Presence(Presence::Absent), "?2 := absent"),
    ] {
        assert_eq!(
            Effect::Bound { var: 2, value }.to_string(),
            printed.to_string()
        );
    }
}

/// A goal is a constraint the solver may have taken apart, so it prints as one
/// in whichever sort it ended up about. Generation only ever equates types; the
/// other two are what taking a type apart reaches.
#[test]
fn a_goal_prints_as_the_constraint_it_is() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    assert_eq!(
        Goal::Type {
            expected: nat.clone(),
            actual: Rc::new(Ty::plain(Core::Var(0))),
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
    let sum = Rc::new(Ty::plain(Core::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField::present(Rc::new(Ty::plain(Core::Nat))),
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
    assert_eq!(missing_field.to_string(), "no field `x` on ``A Nat`");
    let missing_case = TypeError::MissingField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(missing_case.to_string(), "no case ``B` on ``A Nat`");
    assert_eq!(missing_field.code(), missing_case.code());

    // And the same for the extra one, whose sentence names the noun twice.
    let extra_field = TypeError::ExtraField {
        shape: Shape::Struct,
        base: sum.clone(),
        field: "x".to_string(),
    };
    assert_eq!(
        extra_field.to_string(),
        "extra field `x`: the type ``A Nat` lists every field it allows"
    );
    let extra_case = TypeError::ExtraField {
        shape: Shape::Sum,
        base: sum.clone(),
        field: "B".to_string(),
    };
    assert_eq!(
        extra_case.to_string(),
        "extra case ``B`: the type ``A Nat` lists every case it allows"
    );
}

/// The complaint about a base with no fields is gone, and with it its code. A
/// projection now demands a fresh core rather than a struct, so it fits any
/// type carrying the field and the only way it can fail is the field itself.
#[test]
fn nothing_is_coded_as_not_a_struct_any_more() {
    assert!(
        diagnostics()
            .iter()
            .all(|(_, code, _)| *code != "not-a-struct"),
        "{:#?}",
        diagnostics()
    );
    // And the codes the change leaves alone are still there.
    let codes: HashSet<&str> = diagnostics().iter().map(|(_, code, _)| *code).collect();
    for code in [
        "missing-field",
        "extra-field",
        "type-mismatch",
        "recursive-type",
        "annotation-too-open",
        "repeated-field",
    ] {
        assert!(codes.contains(code), "{code}");
    }
}

/// Every form a type prints as, by its core and whether it carries labels.
///
/// The table in one test, because it is one rule: a type carrying nothing
/// prints as its core alone; one carrying labels prints in braces whenever its
/// core is something a `..` has a spelling for; and anything else wears the
/// `with` that no source syntax writes.
///
/// The braced forms are the point. `{ x: 'a, ..'b }` is what a reader would
/// have written, and `'b with { x: 'a }` is not something the parser could read
/// back — so the `..` spelling is what keeps a printed type re-lowerable to the
/// type it was printed from.
#[test]
fn a_type_prints_by_its_core_and_whether_it_carries_labels() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let x = || vec![("x", RowField::present(nat.clone()))];
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint
        .global(None, Namespace::Types, "WithX")
        .expect("a fresh name");
    let with_x = || Core::Named {
        symbol,
        name: "WithX".into(),
        args: Rc::from([nat.clone()]),
    };

    // No labels: the core alone, whatever the core is.
    for (core, printed) in [
        (Core::Nat, "Nat"),
        (Core::Bound(0), "'a"),
        (Core::Var(3), "?3"),
        (Core::Undecided, "?"),
        (Core::Unit, "{}"),
        (with_x(), "WithX Nat"),
    ] {
        assert_eq!(Ty::plain(core).to_string(), printed);
    }

    // Labels, and a core a `..` can be written with: braces, and the `..`
    // spelled by what the core is.
    assert_eq!(with_fields(Core::Unit, x()).to_string(), "{ x: Nat }");
    assert_eq!(
        with_fields(Core::Var(3), x()).to_string(),
        "{ x: Nat, ..?3 }"
    );
    assert_eq!(
        with_fields(
            Core::Bound(1),
            vec![("x", RowField::present(Rc::new(Ty::plain(Core::Bound(0)))))],
        )
        .to_string(),
        "{ x: 'a, ..'b }"
    );
    assert_eq!(
        with_fields(Core::Undecided, x()).to_string(),
        "{ x: Nat, .. }"
    );
    assert_eq!(
        with_fields(
            Core::Undecided,
            vec![(
                "x",
                RowField {
                    presence: Presence::Undecided,
                    ty: nat.clone(),
                }
            )],
        )
        .to_string(),
        // The one presence still written `?`: nothing parses it, and what it
        // reports is that there is nothing to write.
        "{ x?: Nat, .. }"
    );
    // Every other undecided presence wears the `when` clause that names it.
    assert_eq!(
        with_fields(
            Core::Undecided,
            vec![(
                "x",
                RowField {
                    presence: Presence::Bound(0),
                    ty: nat.clone(),
                }
            )],
        )
        .to_string(),
        "{ x when a: Nat, .. }"
    );

    // A field the solver settled absent is not part of what the type says, so
    // it is not written at all — and a type whose every field settled absent is
    // its `..` alone, with no comma in front of it separating it from the
    // nothing that precedes it.
    let gone = || {
        vec![(
            "x",
            RowField {
                presence: Presence::Absent,
                ty: nat.clone(),
            },
        )]
    };
    assert_eq!(with_fields(Core::Var(3), gone()).to_string(), "{ ..?3 }");
    assert_eq!(with_fields(Core::Undecided, gone()).to_string(), "{ .. }");
    // And one whose core names no others either is unit, which it prints as.
    assert_eq!(with_fields(Core::Unit, gone()).to_string(), "{}");

    // Labels, and a core a `..` has no spelling for: the `with` form, with the
    // core bracketed wherever it extends rightward.
    assert_eq!(
        with_fields(Core::Nat, x()).to_string(),
        "Nat with { x: Nat }"
    );
    assert_eq!(
        with_fields(Core::Arrow(nat.clone(), nat.clone()), x()).to_string(),
        "(Nat -> Nat) with { x: Nat }"
    );
    assert_eq!(
        with_fields(
            Core::Sum(Row {
                labels: [("A".to_string(), RowField::present(Rc::new(Ty::unit())))]
                    .into_iter()
                    .collect(),
                rest: Rest::Closed,
            }),
            x(),
        )
        .to_string(),
        "(`A) with { x: Nat }"
    );
    assert_eq!(
        with_fields(with_x(), vec![("y", RowField::present(nat))]).to_string(),
        "WithX Nat with { y: Nat }"
    );
}

/// The new complaint about a declaration whose fields never run out, and the
/// reworded one about an argument that names a label twice.
#[test]
fn the_complaints_about_a_declarations_own_labels_are_worded_once() {
    assert_eq!(IrError::EndlessFields.code(), "endless-fields");
    assert_eq!(
        IrError::EndlessFields.to_string(),
        "this type adds fields to itself, so it never has all of them"
    );

    // The repeat is worded as what the argument does rather than as what goes
    // where it was written: a struct's `..` takes any type at all, so there is
    // no reading to open with. The noun and the label's own notation follow the
    // shape it was found in.
    let field = IrError::RepeatedRowField {
        shape: Shape::Struct,
        field: "x".to_string(),
    };
    assert_eq!(
        field.to_string(),
        "this names `x`, which the struct it goes into already has"
    );
    let case = IrError::RepeatedRowField {
        shape: Shape::Sum,
        field: "A".to_string(),
    };
    assert_eq!(
        case.to_string(),
        "this names ``A`, which the sum it goes into already has"
    );
    assert_eq!(field.code(), case.code());
}

/// A name given two rests is a name given two *senses*: a struct's `..` is the
/// whole type its fields sit on, and a sum's is the cases it does not write out.
#[test]
fn a_mixed_tail_names_the_two_senses_it_was_given() {
    let span = Span::generated(0, 1);
    assert_eq!(
        IrError::MixedTail {
            first: Sense::Type,
            second: Sense::Cases,
            previous: span,
        }
        .to_string(),
        "this stands for a whole type in one place \
         and for the rest of a sum's cases in another"
    );
    assert_eq!(
        IrError::MixedTail {
            first: Sense::Cases,
            second: Sense::Type,
            previous: span,
        }
        .to_string(),
        "this stands for the rest of a sum's cases in one place \
         and for a whole type in another"
    );
}

/// A row lifted out of the type it belongs to has no shape to be read in, so it
/// falls back to braces — including a tail already spliced to more labels, which
/// prints as those labels in the same notation. The solver's own record is where
/// one surfaces.
#[test]
fn a_row_with_no_shape_to_hand_down_prints_in_braces() {
    let nat = Rc::new(Ty::plain(Core::Nat));
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
    assert_eq!(row.to_string(), "{ x: Nat, ..{ y: Nat } }");
}

/// A constraint kind is coded the way an error kind is, and for the same
/// reason: the Constraints tab labels a row with it, and two kinds sharing a
/// code would make two different demands read as one.
#[test]
fn no_two_kinds_of_constraint_are_coded_the_same() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint.local(None, Namespace::Terms, "x");

    let kinds = [
        ConstraintKind::Equal {
            expected: nat.clone(),
            actual: nat.clone(),
        },
        ConstraintKind::Let {
            symbol,
            bound: nat.clone(),
            level: 1,
            promised: Formula::True,
            value: Vec::new(),
            body: Vec::new(),
        },
        ConstraintKind::Instance {
            symbol,
            ty: nat.clone(),
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
    let nat = Rc::new(Ty::plain(Core::Nat));
    let mut mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid bundle"));
    let symbol = mint.local(None, Namespace::Terms, "x");

    let bound = Constraint {
        span: Span::generated(0, 1),
        kind: ConstraintKind::Let {
            symbol,
            bound: nat.clone(),
            level: 2,
            promised: Formula::True,
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
        value: Vec::new(),
        body: Vec::new(),
    };
    assert_eq!(
        promised.to_string(),
        "Nat generalized at level 2 where ?0 != ?1"
    );

    let use_site = ConstraintKind::Instance {
        symbol,
        ty: Rc::new(Ty::plain(Core::Var(4))),
    };
    assert_eq!(use_site.code(), "instance");
    assert_eq!(
        use_site.to_string(),
        "?4 ~ a fresh copy of what this name was bound to"
    );
}

/// The complaints pattern matching added, worded and coded. The wording quotes
/// what the reader wrote — the tag with its backtick, the number as itself,
/// the name that was bound twice — and the codes are the stable names the
/// error tests in `ir.rs` key on.
#[test]
fn the_pattern_complaints_say_what_was_written() {
    let case = IrError::RefutableBinding {
        found: ir::Refuter::Case("Some".to_string()),
    };
    assert_eq!(case.code(), "binding-can-fail");
    assert_eq!(
        case.to_string(),
        "this binding has to accept every value, but a value here might not be ``Some`"
    );
    // A number is the same complaint made about the other kind of test, and
    // the same code: what went wrong is the binding either way.
    let number = IrError::RefutableBinding {
        found: ir::Refuter::Number(0),
    };
    assert_eq!(number.code(), "binding-can-fail");
    assert_eq!(
        number.to_string(),
        "this binding has to accept every value, but the number `0` makes it able to fail"
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
    // the hole's own example value, backticks and braces as a reader would
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
        "some values are not handled — for example `{ a: `A, b: `Y }`; add an arm for them or a final arm naming the rest"
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
    };
    assert_eq!(bound.code(), "duplicate-binding");
    assert_eq!(bound.to_string(), "this binds `x` twice");
}

/// Every shape a witness takes, rendered: numbers as themselves, tags bare
/// and carrying — a carried prose form bracketed, so the example still reads
/// as one value — an open position as "anything other than …", and the
/// position nothing constrains as "anything".
#[test]
fn a_witness_renders_in_source_syntax() {
    assert_eq!(ir::Witness::Natural(2).to_string(), "2");
    assert_eq!(ir::Witness::Any.to_string(), "anything");
    assert_eq!(
        ir::Witness::Tag {
            name: "A".to_string(),
            payload: Some(Box::new(ir::Witness::Natural(1))),
        }
        .to_string(),
        "`A 1"
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
        "`A (`X 1)"
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
        "`A (`X)"
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
        "`B (anything other than `X or `Y)"
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
        "{ a: `A, b: anything other than `X }"
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
            [
                ("a".to_string(), ir::Witness::Any),
                ("b".to_string(), ir::Witness::Natural(1)),
            ]
            .into_iter()
            .collect(),
        )
        .to_string(),
        "{ a, b: 1 }"
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

/// The misplaced wildcard is one meaning worded five ways: every position's
/// phrasing opens with what `_` stands for, every wording is distinct, all
/// share one code, and each holds to the phrase rules like any diagnostic.
/// The wording per position is pinned so a reworded meaning cannot slip by.
#[test]
fn a_misplaced_wildcard_is_worded_for_its_position() {
    let span = Span::generated(0, 1);
    let said = |place: parse::Place| {
        let error = parse::Error {
            span,
            kind: parse::ErrorKind::Wildcard { place },
        };
        assert_eq!(error.code(), "misplaced-wildcard", "{place:?}");
        error.to_string()
    };

    assert_eq!(
        said(parse::Place::Value),
        "`_` stands for a value being thrown away, so it can't be used as a value here"
    );
    assert_eq!(
        said(parse::Place::Field),
        "`_` stands for a value being thrown away, so it can't be a field's name"
    );
    assert_eq!(
        said(parse::Place::Pun),
        "`_` stands for a value being thrown away, and a field written bare binds to its own name, so there is no name here to bind"
    );
    assert_eq!(
        said(parse::Place::Projection),
        "`_` stands for a value being thrown away, so it can't name a field to read"
    );
    assert_eq!(
        said(parse::Place::Type),
        "`_` stands for a value being thrown away, so it can't be used as a type"
    );

    // One meaning, five phrasings: each opens with what `_` stands for, no
    // two are the same sentence, and each is a placeable phrase — no leading
    // capital, no trailing period — that names no machinery.
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
        assert!(
            wording.starts_with("`_` stands for a value being thrown away"),
            "{wording}"
        );
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
        (a.clone(), "a"),
        (a.clone().not(), "not a"),
        (a.clone().not().not(), "a"),
        (a.clone().and(b.clone()), "a and b"),
        (a.clone().or(b.clone()), "a or b"),
        (a.clone().iff(b.clone()), "a = b"),
        (a.clone().xor(b.clone()), "a != b"),
        // `and` binds tighter than `or`, which binds tighter than a
        // comparison, so none of these needs a bracket.
        (a.clone().or(b.clone().and(c.clone())), "a or b and c"),
        (a.clone().and(b.clone()).or(c.clone()), "a and b or c"),
        (a.clone().iff(b.clone().or(c.clone())), "a = b or c"),
        // And these do, because the grammar reads them the other way round
        // without them.
        (a.clone().or(b.clone()).and(c.clone()), "(a or b) and c"),
        (a.clone().and(b.clone()).not(), "not (a and b)"),
        (a.clone().and(b.clone().or(c.clone())), "a and (b or c)"),
        (a.clone().iff(b.clone()).or(c.clone()), "(a = b) or c"),
        (a.clone().xor(b.clone().iff(c.clone())), "a != (b = c)"),
        // Left-associative, so a right-nested one of the same level brackets.
        (a.clone().or(b.clone().or(c.clone())), "a or (b or c)"),
        (a.clone().and(b.clone().and(c.clone())), "a and (b and c)"),
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
        "a",
        "not a",
        "a and b",
        "a or b",
        "a = b",
        "a != b",
        "a or b and c",
        "a and b or c",
        "a = b or c",
        "(a or b) and c",
        "not (a and b)",
        "a and (b or c)",
        "(a = b) or c",
        "a != (b = c)",
        "a or (b or c)",
        "a and (b and c)",
    ] {
        let src =
            format!("type T = {{ a when a: Nat, b when b: Nat, c when c: Nat }} where {clause}");
        let out = parse::parse(token::lex(&src, FileID::GENERATED).tokens);
        assert!(out.errors.is_empty(), "{src}: {:#?}", out.errors);
        let parse::StmtKind::Type { body, .. } = &out.stmts[0].tracked else {
            panic!("expected a declaration");
        };
        let written = body.clause.as_ref().expect("the clause");
        assert_eq!(
            print::ast::clause(&written.tracked).to_string(),
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
    let nat = Rc::new(Ty::plain(Core::Nat));
    let fields: IndexMap<String, RowField> = [("x".to_string(), undecided(nat.clone()))]
        .into_iter()
        .collect();
    assert_eq!(
        Ty {
            core: Core::Unit,
            fields
        }
        .to_string(),
        "{ x?: Nat }"
    );

    let cases: IndexMap<String, RowField> =
        [("A".to_string(), undecided(nat))].into_iter().collect();
    assert_eq!(
        Ty::plain(Core::Sum(Row {
            labels: cases,
            rest: Rest::Closed,
        }))
        .to_string(),
        "`A? Nat"
    );
}

/// A presence has an alphabet of its own — the bare `a` of a `when` clause —
/// and a type variable keeps its quote. The two pools are independent, so one
/// scheme's `'a` and `a` are two variables however alike they look.
#[test]
fn the_two_alphabets_are_told_apart_by_the_quote() {
    assert_eq!(Core::Bound(0).to_string(), "'a");
    assert_eq!(Presence::Bound(0).to_string(), "a");
    assert_eq!(Presence::Bound(26).to_string(), "a1");
    assert_eq!(Formula::bound(25).to_string(), "z");
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
    // A presence no label decides falls back to the presence itself.
    assert_eq!(
        ui::in_labels(&formula, &[("x".to_string(), Presence::Var(0))]),
        "x != ?1"
    );
}
