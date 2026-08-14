//! Tests for [`ruddy_debug::stage`].

use std::rc::Rc;

use regex::Regex;
use ruddy::types::{Core, Rest, Row, Ty};
use ruddy_debug::{
    stage::{Build, REGISTRY, Spec, panicked, skipped},
    wire::Stage,
};

/// The page finds a stage's variables by the pattern the stage declares, and by
/// nothing else — it never learns what a match means. It both colours them and
/// cross-highlights them, so a pattern that stopped describing what the type
/// printer writes would take the distinction between a variable and a concrete
/// type with it, and the only symptom would be a tab gone monochrome.
///
/// So the three spellings are checked against the printer here rather than by
/// eye in the browser. A regex engine is more than this needs: each form is a
/// sigil and then a tail, which is what the pattern says too.
#[test]
fn the_type_tabs_declare_how_a_variable_is_spelled() {
    // Every tab that renders the type language agrees on the notation, since
    // it is one language and one printer.
    let declared: Vec<Option<&str>> = ["constraints", "solve", "types", "patterns"]
        .iter()
        .map(|id| {
            REGISTRY
                .iter()
                .find(|spec| spec.id == *id)
                .unwrap_or_else(|| panic!("{id} is registered"))
                .highlight
        })
        .collect();
    let pattern = declared[0].expect("the constraints tab declares a pattern");
    assert!(
        declared.iter().all(|one| *one == Some(pattern)),
        "{declared:?}"
    );
    assert_eq!(pattern, r"\B\?\d*|'[a-z]\d*");

    // `\?\d*`: a `?`, then digits — of which a bare `?` is the empty case.
    for (ty, tail) in [(Core::Var(4), "4"), (Core::Undecided, "")] {
        let printed = ty.to_string();
        assert_eq!(printed, format!("?{tail}"));
        assert!(tail.chars().all(|c| c.is_ascii_digit()), "{printed}");
    }

    // `'[a-z]\d*`: a quote, a letter, then the digits that appear once the
    // alphabet runs out.
    for index in [0, 25, 26] {
        let printed = Core::Bound(index).to_string();
        let mut chars = printed.chars();
        assert_eq!(chars.next(), Some('\''), "{printed}");
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_lowercase()),
            "{printed}"
        );
        assert!(chars.all(|c| c.is_ascii_digit()), "{printed}");
    }
}

/// The spelling test above pins what the pattern says; this one pins what it
/// matches. The `?` a presence writes on its label — the one in `a?: Nat` — is
/// spelled like a variable's sigil, so the pattern has to decline it by
/// position: a match may not begin right against a label's last character.
/// The check runs on this side's engine while the page matches in the
/// browser's, which is tolerable because the pattern stays inside the syntax
/// the two share.
#[test]
fn a_presence_mark_is_not_lit_as_a_variable() {
    let pattern = REGISTRY
        .iter()
        .find(|spec| spec.id == "types")
        .expect("the types tab is registered")
        .highlight
        .expect("the types tab declares a pattern");
    let pattern = Regex::new(pattern).expect("the pattern compiles");

    // The optional fields' `?`s sit against their labels and stay dark; the
    // quantified type beside them still lights.
    let matches: Vec<&str> = pattern
        .find_iter("{ a when a: Nat, b when b: 'b }")
        .map(|found| found.as_str())
        .collect();
    assert_eq!(matches, vec!["'b"]);

    // Each variable spelling begins after space or punctuation, and still
    // matches whole: a solver's `?4`, an undecided `?`, a quantified `'a2`.
    let matches: Vec<&str> = pattern
        .find_iter("{ x: ?4 } -> ? -> 'a2")
        .map(|found| found.as_str())
        .collect();
    assert_eq!(matches, vec!["?4", "?", "'a2"]);
}

/// The notation is shared, but how far one spelling reaches is not. A `?4` is
/// one entry of the solver's one table, so it is the same variable in every
/// row of the Constraints and Solve tabs. A scheme's `'a` is not: generalization
/// numbers each definition's quantifiers from `'a` again, so `id : 'a -> 'a`
/// and `k : 'a -> 'b -> 'a` share a spelling and nothing else — and the page
/// matches on spelling alone, which is why the Types tab has to say so.
#[test]
fn only_the_types_tab_scopes_a_name_to_its_row() {
    let scoped = |id: &str| {
        REGISTRY
            .iter()
            .find(|spec| spec.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
            .scoped
    };
    assert!(scoped("types"));
    assert!(!scoped("constraints"));
    assert!(!scoped("solve"));

    // A stage that highlights nothing has nothing to scope, so saying it does
    // would be a claim with no reading.
    for spec in REGISTRY {
        assert!(
            !spec.scoped || spec.highlight.is_some(),
            "{} scopes a pattern it does not declare",
            spec.id
        );
    }
}

/// A stage that skipped or panicked is still the same stage. `annotates` is
/// what the page reads to decide whether a stage owns a tab at all, so losing
/// it on those paths turns an annotator into a second, empty panel — exactly
/// when a phase has panicked and the tool is supposed to be degrading well.
#[test]
fn a_stage_that_did_not_run_still_describes_itself() {
    for spec in REGISTRY {
        for stage in [skipped(spec, "did not run"), panicked(spec)] {
            assert_eq!(stage.id, spec.id);
            assert_eq!(stage.title, spec.title, "{}", spec.id);
            assert_eq!(stage.annotates, spec.annotates, "{}", spec.id);
            assert_eq!(stage.highlight, spec.highlight, "{}", spec.id);
            assert_eq!(stage.scoped, spec.scoped, "{}", spec.id);
        }
    }
    // Otherwise the loop above proves nothing about annotators.
    assert!(
        REGISTRY.iter().any(|spec| spec.annotates.is_some()),
        "no stage annotates another"
    );
}

/// The page builds its tab strip from the stages that annotate nothing, and
/// selects a tab by position in that strip. Two tabs sharing a title is what a
/// stage that forgot what it annotates looks like from the front end, and the
/// extra one opens blank.
#[test]
fn the_tab_strip_never_repeats_a_title() {
    let outcomes: [Vec<Stage>; 2] = [
        REGISTRY.iter().map(|spec| skipped(spec, "why")).collect(),
        REGISTRY.iter().map(panicked).collect(),
    ];
    for stages in outcomes {
        let mut titles: Vec<&str> = stages
            .iter()
            .filter(|stage| stage.annotates.is_none())
            .map(|stage| stage.title)
            .collect();
        let tabs = titles.len();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), tabs, "{titles:?}");
    }
}

/// An annotator reads the trace its target published rather than rebuilding
/// it, which only works if the target is registered — and registered first.
#[test]
fn every_annotator_reads_a_trace_published_before_it() {
    for (i, spec) in REGISTRY.iter().enumerate() {
        // The two halves of the contract have to agree: a stage names a target
        // exactly when it is built as an annotator.
        assert_eq!(
            matches!(spec.build, Build::Annotator(_)),
            spec.annotates.is_some(),
            "{}",
            spec.id
        );
        let Some(target) = spec.annotates else {
            continue;
        };
        let at = position(target)
            .unwrap_or_else(|| panic!("{} annotates {target}, which is not registered", spec.id));
        assert!(at < i, "{} is built before {target}", spec.id);
        assert!(
            matches!(REGISTRY[at].build, Build::Traced(_)),
            "{target} publishes no trace for {} to read",
            spec.id
        );
    }
}

fn position(id: &str) -> Option<usize> {
    REGISTRY.iter().position(|spec: &Spec| spec.id == id)
}

/// The Patterns tab: one section per match, the solved scrutinee type on the
/// match's row, one row per arm wearing its verdict, and the coverage line —
/// exhaustive, or the witness — with the skipped honesty when the typing
/// failed and the checks stood aside.
#[test]
fn the_patterns_tab_renders_verdicts_and_coverage() {
    use ruddy_debug::{
        snapshot::compile,
        wire::{BundleSpec, CompileRequest, Node},
    };

    let tab = |source: &str| -> Vec<Node> {
        let snapshot = compile(
            &CompileRequest {
                source: source.to_string(),
                revision: 0,
                bundle: BundleSpec::default(),
            },
            0,
        );
        let stage = snapshot
            .stages
            .into_iter()
            .find(|stage| stage.id == "patterns")
            .expect("the patterns stage is registered");
        stage.nodes
    };

    // A misplaced catch-all: the arm rows carry the verdicts, the scrutinee
    // type is the match row's text, and the match stays exhaustive.
    let nodes = tab("let f = fn n => match n with | x => 1 | 2 => 3 | 4 => 5 end");
    assert_eq!(nodes.len(), 1, "{nodes:#?}");
    let rows: Vec<(&str, &str)> = nodes[0]
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(nodes[0].label, "match");
    assert_eq!(nodes[0].text, "Nat");
    assert_eq!(
        rows,
        [
            ("scrutinee", "Nat"),
            ("reachable", "x"),
            ("starved", "2"),
            ("starved", "4"),
            ("coverage", "exhaustive"),
        ],
        "{nodes:#?}"
    );

    // An unhandled match: the coverage row carries the witness, marked as the
    // error it reports.
    let nodes = tab("let bad = match {x: 1, y: 2} with {x} => {} | {y} => {} end");
    let coverage = nodes[0]
        .children
        .iter()
        .find(|child| child.label == "coverage")
        .expect("a coverage row");
    assert_eq!(coverage.text, "unhandled: { x, y }");
    assert!(coverage.error);

    // An unreachable arm is marked as the error its row reports.
    let nodes = tab("let f = fn e => match e with | `A x => 1 | `A y => 2 end");
    let unreachable = nodes[0]
        .children
        .iter()
        .find(|child| child.label == "unreachable")
        .expect("an unreachable row");
    assert!(unreachable.error);
    assert_eq!(unreachable.text, "`A y");

    // A mixed match: the checks stood aside, and the tab says so per arm and
    // for the coverage.
    let nodes = tab("let f = fn e => match e with | 1 => 2 | `A => 3 end");
    let verdicts: Vec<&str> = nodes[0]
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(
        verdicts,
        ["scrutinee", "skipped", "skipped", "coverage"],
        "{nodes:#?}"
    );
    let coverage = nodes[0].children.last().expect("a coverage row");
    assert_eq!(coverage.text, "skipped");
}

/// The Presence tab: one section per batch of the store, in program order,
/// each with what it came from, the formula it contributed and the running
/// verdict — and then what every definition's scheme ended up requiring, beside
/// what the patterns phase walks that definition under.
#[test]
fn the_presence_tab_renders_the_store_and_the_clauses() {
    use ruddy_debug::{
        snapshot::compile,
        wire::{BundleSpec, CompileRequest, Node},
    };

    let tab = |source: &str| -> Vec<Node> {
        let snapshot = compile(
            &CompileRequest {
                source: source.to_string(),
                revision: 0,
                bundle: BundleSpec::default(),
            },
            0,
        );
        snapshot
            .stages
            .into_iter()
            .find(|stage| stage.id == "presence")
            .expect("the presence stage is registered")
            .nodes
    };

    // A match whose column converted: one coverage batch, satisfiable, with a
    // disjunct per arm and the labels its presences decide — and the clause
    // the definition ends up publishing.
    let nodes = tab("let p = fn a => match a with | {x} => {} | {y} => {} end");
    let labels: Vec<&str> = nodes.iter().map(|node| node.label.as_str()).collect();
    assert_eq!(labels, ["match-coverage", "let p"], "{nodes:#?}");
    assert_eq!(nodes[1].text, "where a != b");
    assert_eq!(nodes[1].children[0].label, "patterns assume");
    let rows: Vec<(&str, &str)> = nodes[0]
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(rows[0].0, "origin");
    assert_eq!(rows[1].0, "verdict");
    assert!(rows[1].1.starts_with("satisfiable"), "{rows:#?}");
    assert_eq!(rows[2].0, "arm 0");
    assert_eq!(rows[3].0, "arm 1");
    assert_eq!(rows[4].0, "field x");
    assert_eq!(rows[5].0, "field y");
    // Every row points at the match it came from, so a click lights it.
    assert!(nodes[0].span.is_some(), "{nodes:#?}");

    // A use site that cannot be satisfied: the batch that flipped the store is
    // marked as the error it owns, and it is the only one that is.
    let nodes = tab("let p = fn a => match a with | {x} => {} | {y} => {} end\nlet bad = p {}");
    let flipped: Vec<&str> = nodes
        .iter()
        .filter(|node| node.error)
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(flipped, ["use-site"], "{nodes:#?}");
    let use_site = nodes
        .iter()
        .find(|node| node.label == "use-site")
        .expect("a use-site batch");
    let verdict = use_site
        .children
        .iter()
        .find(|child| child.label == "verdict")
        .expect("a verdict row");
    assert!(verdict.text.starts_with("unsatisfiable"), "{nodes:#?}");
    assert!(verdict.error);
    // And it says which label each of its presences decides, which is how its
    // complaint is worded.
    assert!(
        use_site
            .children
            .iter()
            .any(|child| child.label == "label x"),
        "{nodes:#?}"
    );

    // The store never recovers, so every batch past the flip renders an
    // unsatisfiable verdict too — and each of them names the batch that did the
    // flipping rather than itself, which would blame each in turn for the one
    // thing only the first of them did.
    let nodes = tab("let p = fn a => match a with | {x} => {} | {y} => {} end\n\
         let bad = p {}\n\
         let q = fn a => match a with | {x} => {} | {y} => {} end");
    let unsatisfiable: Vec<&str> = nodes
        .iter()
        .filter_map(|node| node.children.iter().find(|child| child.label == "verdict"))
        .map(|verdict| verdict.text.as_str())
        .filter(|text| text.starts_with("unsatisfiable"))
        .collect();
    assert!(unsatisfiable.len() > 1, "{nodes:#?}");
    assert!(
        unsatisfiable
            .iter()
            .all(|text| *text == "unsatisfiable — flipped by batch 1"),
        "{unsatisfiable:#?}"
    );

    // An annotation's own clause is a batch of its own.
    let nodes = tab("let f : { x when a: Nat } where a = { x: 1 }");
    assert!(
        nodes.iter().any(|node| node.label == "annotation"),
        "{nodes:#?}"
    );

    // What the patterns phase assumes is the other half of the definition's
    // row, and it is not the scheme's clause: a definition whose presences all
    // live in a nested binding publishes no clause at all and is still walked
    // under everything the store says about them.
    let nodes = tab("let outer = fn z =>\n  \
           let g = fn v =>\n    \
             let w = match v with | {x} => 0 | {y} => 0 end in\n    \
             match v with | {x: 1} => 1 | {x: n} => 2 | {y} => 3 end in\n  \
           0");
    let outer = nodes
        .iter()
        .find(|node| node.label == "let outer")
        .expect("the definition's row");
    assert_eq!(outer.text, "unconstrained");
    let assumed = outer
        .children
        .iter()
        .find(|child| child.label == "patterns assume")
        .expect("the promise row");
    assert_eq!(assumed.text, "a and not b or not a and b", "{nodes:#?}");

    // A program that constrains nothing still says so, per definition: the
    // ordinary case is a tab full of "unconstrained" rather than an empty one,
    // promise row included.
    let nodes = tab("let id = fn x => x");
    assert_eq!(nodes.len(), 1, "{nodes:#?}");
    assert_eq!(nodes[0].label, "let id");
    assert_eq!(nodes[0].text, "unconstrained");
    let rows: Vec<(&str, &str)> = nodes[0]
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(rows, [("patterns assume", "unconstrained")], "{nodes:#?}");
}

/// A program that never reaches inference leaves the tab with nothing to
/// render, and the summary says so rather than the tab pretending the store was
/// empty on purpose.
#[test]
fn the_presence_tab_counts_what_it_rendered() {
    use ruddy_debug::{
        snapshot::compile,
        wire::{BundleSpec, CompileRequest},
    };

    let snapshot = compile(
        &CompileRequest {
            source: "let p = fn a => match a with | {x} => {} | {y} => {} end".to_string(),
            revision: 0,
            bundle: BundleSpec::default(),
        },
        0,
    );
    let stage = snapshot
        .stages
        .into_iter()
        .find(|stage| stage.id == "presence")
        .expect("the presence stage is registered");
    assert_eq!(stage.summary, "1 constraint");
    assert!(stage.micros.is_some());
}

/// An effect tail prints `..'a` the way a struct's and a sum's do, so the
/// pattern the tabs declare has to light one up wherever it lands — including
/// after the `!`, which is a position that did not exist when the pattern was
/// written.
#[test]
fn the_variables_pattern_reaches_an_effect_tail() {
    let pattern = REGISTRY
        .iter()
        .find(|spec| spec.id == "types")
        .expect("the types tab is registered")
        .highlight
        .expect("it declares a pattern");
    let printed = Ty::plain(Core::Arrow(
        Rc::new(Ty::plain(Core::Bound(0))),
        Rc::new(Ty::plain(Core::Bound(0))),
        Row::of(Rest::Bound(1)),
    ))
    .to_string();
    assert_eq!(printed, "'a -> 'a ! ..'b");
    let found: Vec<&str> = Regex::new(pattern)
        .expect("the pattern compiles")
        .find_iter(&printed)
        .map(|at| at.as_str())
        .collect();
    assert_eq!(found, ["'a", "'a", "'b"]);
}
