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

use ruddy::{
    inference::{self, Constraint, ConstraintKind, Effect, ErrorKind as TypeError, Goal, Rule},
    ir::{self, ErrorKind as IrError},
    parse,
    symbol::{Bundle, Mint, Namespace, Version},
    token::{self, ErrorKind as LexError, Kind as TokenKind},
    tracking::{FileID, Span},
    types::{Assigned, Core, Presence, Prim, Rest, Row, RowField, Sense, Shape, Ty},
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

    let unexpected = parse::Error { span };
    all.push(("parse", unexpected.code(), unexpected.to_string()));

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
            first: Shape::Struct,
            second: Shape::Sum,
            previous: span,
        },
        IrError::MixedParameter {
            first: Sense::Type,
            second: Sense::Row(Shape::Struct),
        },
        IrError::NotARow {
            shape: Shape::Struct,
        },
        IrError::RepeatedRowField {
            shape: Shape::Struct,
            field: "x".to_string(),
        },
    ] {
        all.push(("ir", kind.code(), kind.to_string()));
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
                fields: Row {
                    labels: Default::default(),
                    rest: Rest::Closed,
                },
            }),
            "{}",
        ),
        (
            "",
            Rc::new(Ty {
                core: Core::Unit,
                fields: Row {
                    labels: [("x".to_string(), RowField::present(endo.clone()))]
                        .into_iter()
                        .collect(),
                    rest: Rest::Closed,
                },
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
            core: Core::Unit,
            fields: Row {
                labels: [("x".to_string(), RowField::present(nat.clone()))]
                    .into_iter()
                    .collect(),
                rest: Rest::Bound(0),
            },
        }
        .to_string(),
        "{ x: Nat, ..'a }"
    );
    assert_eq!(
        Ty {
            core: Core::Unit,
            fields: Row {
                labels: [("x".to_string(), RowField::present(nat.clone()))]
                    .into_iter()
                    .collect(),
                rest: Rest::Undecided,
            },
        }
        .to_string(),
        "{ x: Nat, .. }"
    );
    assert_eq!(
        Ty {
            core: Core::Unit,
            fields: Row {
                labels: Default::default(),
                rest: Rest::Bound(0),
            },
        }
        .to_string(),
        "{ ..'a }"
    );

    // A field's presence prints as its surface spelling too: certainly there
    // is unmarked, undecided either way is `?`, and certainly absent is not
    // part of what the type says at all.
    assert_eq!(
        Ty {
            core: Core::Unit,
            fields: Row {
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
                rest: Rest::Closed,
            },
        }
        .to_string(),
        "{ x?: Nat }"
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
    let not_a_row = IrError::NotARow { shape: Shape::Sum };
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

/// A parameter read more than one way names the two readings it was found to
/// have, so the reader is told what to choose between rather than that there
/// was a choice.
#[test]
fn a_mixed_parameter_names_both_readings() {
    let mixed = IrError::MixedParameter {
        first: Sense::Row(Shape::Struct),
        second: Sense::Row(Shape::Sum),
    };
    assert_eq!(
        mixed.to_string(),
        "this stands for the rest of a struct's fields in one place \
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
        TokenKind::Fn,
        TokenKind::Equal,
        TokenKind::FatArrow,
        TokenKind::Arrow,
        TokenKind::Colon,
        TokenKind::Comma,
        TokenKind::Dot,
        TokenKind::DotDot,
        TokenKind::Question,
        TokenKind::Pipe,
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

    // A case carrying a row that is not unit keeps its payload, open tail and
    // all: only the empty closed struct is written as no payload at all.
    let open = Rc::new(Ty::plain(Core::Sum(Row {
        labels: [(
            "A".to_string(),
            RowField::present(Rc::new(Ty {
                core: Core::Unit,
                fields: Row {
                    labels: Default::default(),
                    rest: Rest::Bound(0),
                },
            })),
        )]
        .into_iter()
        .collect(),
        rest: Rest::Closed,
    })));
    assert_eq!(open.to_string(), "`A { ..\'a }");

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
                core: Core::Unit,
                fields: Row {
                    labels: [
                        ("x".to_string(), RowField::present(nat.clone())),
                        ("y".to_string(), optional.clone()),
                    ]
                    .into_iter()
                    .collect(),
                    rest: Rest::Bound(0),
                },
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
        (
            "a case carrying unit",
            Rc::new(Ty::plain(Core::Sum(Row {
                labels: [(
                    "None".to_string(),
                    RowField::present(Rc::new(Ty {
                        core: Core::Unit,
                        fields: Row {
                            labels: Default::default(),
                            rest: Rest::Closed,
                        },
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
    let source = "type Pair a b = { first: a, second: b }\n\
                  let f = fn g => fn p => g p.first\n\
                  let v : { first: Nat, second: Nat } = { first: 1, second: 2 }";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    let mut program_mint = Mint::new(Bundle::new("test", Version::new(0, 1, 0)).expect("valid"));
    let built = ruddy::ir::build(&mut program_mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{:#?}", built.errors);

    every_failure_is_reported(
        "a lowered program",
        &print::ir::program(&built.program, &program_mint),
    );
}

/// A type carrying fields that its core is not unit, which no source syntax
/// writes and inference builds every time a projection is left unannotated.
fn with_fields(core: Core, labels: Vec<(&str, RowField)>, rest: Rest) -> Rc<Ty> {
    Rc::new(Ty {
        core,
        fields: Row {
            labels: labels
                .into_iter()
                .map(|(name, field)| (name.to_string(), field))
                .collect(),
            rest,
        },
    })
}

/// Every form a type can print as, constructed directly, because most of them
/// are forms no program can be written to produce. A type carrying no fields
/// prints as its core; one whose core is unit prints as braces; anything else
/// carrying fields wears the `with` that says so.
///
/// Pinned as printing and nothing more: `with` has no surface syntax, so there
/// is nothing to read these back from. See
/// [`an_open_row_prints_in_surface_notation_it_cannot_be_read_back_from`] for
/// why that is the right trade for a form the reader is shown rather than
/// handed.
#[test]
fn a_type_carrying_fields_prints_its_core_and_then_the_fields() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let x_nat = || vec![("x", RowField::present(nat.clone()))];

    // No fields at all: the core alone, which is every type the language could
    // write before fields were a property of all of them.
    assert_eq!(nat.to_string(), "Nat");
    // Unit carrying fields is a struct, and prints as one.
    assert_eq!(
        with_fields(Core::Unit, x_nat(), Rest::Closed).to_string(),
        "{ x: Nat }"
    );
    // Anything else carrying fields wears the `with`.
    assert_eq!(
        with_fields(Core::Nat, x_nat(), Rest::Closed).to_string(),
        "Nat with { x: Nat }"
    );
    // An arrow core and a sum core are both bracketed: each extends rightward,
    // so the fields would otherwise read as part of them.
    assert_eq!(
        with_fields(Core::Arrow(nat.clone(), nat.clone()), x_nat(), Rest::Closed).to_string(),
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
            x_nat(),
            Rest::Closed
        )
        .to_string(),
        "(`A) with { x: Nat }"
    );
    // The fields print exactly as a struct's do: the same `?` on an undecided
    // presence, and the same `..` on an open tail.
    assert_eq!(
        with_fields(
            Core::Nat,
            vec![(
                "x",
                RowField {
                    presence: Presence::Undecided,
                    ty: nat.clone(),
                }
            )],
            Rest::Undecided
        )
        .to_string(),
        "Nat with { x?: Nat, .. }"
    );
}

/// Where a `with` type needs parentheses and where it does not. It sits above
/// the arrow and the sum, so neither side of an arrow brackets it; it sits
/// below an atom, so every position that takes one — a type constructor's
/// argument, a tag's payload — does.
#[test]
fn a_with_type_is_bracketed_wherever_something_could_follow_its_fields() {
    let nat = Rc::new(Ty::plain(Core::Nat));
    let quantified = with_fields(
        Core::Bound(0),
        vec![("x", RowField::present(Rc::new(Ty::plain(Core::Bound(1)))))],
        Rest::Bound(2),
    );
    assert_eq!(quantified.to_string(), "'a with { x: 'b, ..'c }");

    // Either side of an arrow, bare: nothing in an arrow can swallow the
    // fields, and bracketing here would be noise on the commonest form there
    // is — the type of an unannotated accessor.
    assert_eq!(
        Ty::plain(Core::Arrow(quantified.clone(), nat.clone())).to_string(),
        "'a with { x: 'b, ..'c } -> Nat"
    );
    assert_eq!(
        Ty::plain(Core::Arrow(nat.clone(), quantified.clone())).to_string(),
        "Nat -> 'a with { x: 'b, ..'c }"
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
        "Pair ('a with { x: 'b, ..'c }) Nat"
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
        "`Some ('a with { x: 'b, ..'c })"
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
        (Presence::Bound(1), "'b"),
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
