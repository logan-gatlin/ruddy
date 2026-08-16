//! Tests for [`ruddy_debug::stage`].

use regex::Regex;
use ruddy::types::Core;
use ruddy_debug::{
    snapshot::{ROOT, compile},
    stage::{Build, REGISTRY, Spec, panicked, skipped},
    wire::{CompileRequest, FileSpec, Loc, Node, Snapshot, Stage},
};

/// The header every snippet is compiled under: a bundle's root file must open
/// with one, so a snippet that did not write its own would be told so. Every
/// span in the snippet therefore sits this many bytes further in.
const HEADER: &str = "bundle demo 0.1.0\n";

/// A bundle of three files, one per shape a module's body can come from: an
/// inline module, a module beside its parent, and a module inside the directory
/// its parent's name spells.
const NESTED: &[(&str, &str)] = &[
    (
        ROOT,
        "bundle demo 0.1.0\nmodule Math\nlet four = Math::double 2\n",
    ),
    ("Math.hc", "module Vec\nlet double = fn x => x\n"),
    ("Math/Vec.hc", "let zero = 0\n"),
];

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
    assert_eq!(pattern, r"\B\?\d*");

    // `\?\d*`: a `?`, then digits — of which a bare `?` is the empty case.
    for (ty, tail) in [(Core::Var(4), "4"), (Core::Undecided, "")] {
        let printed = ty.to_string();
        assert_eq!(printed, format!("?{tail}"));
        assert!(tail.chars().all(|c| c.is_ascii_digit()), "{printed}");
    }

    // And a variable a scheme quantified is deliberately outside the pattern:
    // it prints as the `'a` a reader writes one with, and lighting those would
    // light every annotation on the page. The sigil is what says which letters
    // a scheme quantifies, so nothing else has to.
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
/// A letter a scheme quantifies stays dark for the reason above: it is spelled
/// exactly as a name is.
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

    // The optional fields' `?`s sit against their labels and stay dark, and so
    // do the letters the scheme beside them quantifies.
    let matches: Vec<&str> = pattern
        .find_iter("{ a when 'a: Nat, b when 'b: 'b }")
        .map(|found| found.as_str())
        .collect();
    assert_eq!(matches, Vec::<&str>::new());

    // Each variable spelling begins after space or punctuation, and still
    // matches whole: a solver's `?4` and an undecided `?`.
    let matches: Vec<&str> = pattern
        .find_iter("{ x: ?4 } -> ? -> a2")
        .map(|found| found.as_str())
        .collect();
    assert_eq!(matches, vec!["?4", "?"]);
}

/// The notation is shared, but how far one spelling reaches is not. A `?4` is
/// one entry of the solver's one table, so it is the same variable in every
/// row of the Constraints and Solve tabs. A scheme's `a` is not: generalization
/// numbers each definition's quantifiers from `a` again, so `id : a -> a`
/// and `k : a -> 'b -> a` share a spelling and nothing else — and the page
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
    // A misplaced catch-all: the arm rows carry the verdicts, the scrutinee
    // type is the match row's text, and the match stays exhaustive.
    let nodes = tab(
        "patterns",
        "let f = fn n => match n with | x => 1 | 2 => 3 | 4 => 5 end",
    );
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
    let nodes = tab(
        "patterns",
        "let bad = match {x: 1, y: 2} with {x} => {} | {y} => {} end",
    );
    let coverage = nodes[0]
        .children
        .iter()
        .find(|child| child.label == "coverage")
        .expect("a coverage row");
    assert_eq!(coverage.text, "unhandled: { x, y }");
    assert!(coverage.error);

    // An unreachable arm is marked as the error its row reports.
    let nodes = tab(
        "patterns",
        "let f = fn e => match e with | #A x => 1 | #A y => 2 end",
    );
    let unreachable = nodes[0]
        .children
        .iter()
        .find(|child| child.label == "unreachable")
        .expect("an unreachable row");
    assert!(unreachable.error);
    assert_eq!(unreachable.text, "#A y");

    // A mixed match: the checks stood aside, and the tab says so per arm and
    // for the coverage.
    let nodes = tab(
        "patterns",
        "let f = fn e => match e with | 1 => 2 | #A => 3 end",
    );
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
    // A match whose column converted: one coverage batch, satisfiable, with a
    // disjunct per arm and the labels its presences decide — and the clause
    // the definition ends up publishing.
    let nodes = tab(
        "presence",
        "let p = fn a => match a with | {x} => {} | {y} => {} end",
    );
    let labels: Vec<&str> = nodes.iter().map(|node| node.label.as_str()).collect();
    assert_eq!(labels, ["match-coverage", "let p"], "{nodes:#?}");
    assert_eq!(nodes[1].text, "where 'a != 'b");
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
    let nodes = tab(
        "presence",
        "let p = fn a => match a with | {x} => {} | {y} => {} end\nlet bad = p {}",
    );
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
    let nodes = tab(
        "presence",
        "let p = fn a => match a with | {x} => {} | {y} => {} end\n\
         let bad = p {}\n\
         let q = fn a => match a with | {x} => {} | {y} => {} end",
    );
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
    let nodes = tab("presence", "let f : { x when 'a: Nat } where 'a = { x: 1 }");
    assert!(
        nodes.iter().any(|node| node.label == "annotation"),
        "{nodes:#?}"
    );

    // What the patterns phase assumes is the other half of the definition's
    // row, and it is not the scheme's clause: a definition whose presences all
    // live in a nested binding publishes no clause at all and is still walked
    // under everything the store says about them.
    let nodes = tab(
        "presence",
        "let outer = fn z =>\n  \
           let g = fn v =>\n    \
             let w = match v with | {x} => 0 | {y} => 0 end in\n    \
             match v with | {x: 1} => 1 | {x: n} => 2 | {y} => 3 end in\n  \
           0",
    );
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
    assert_eq!(assumed.text, "'a and not 'b or not 'a and 'b", "{nodes:#?}");

    // A program that constrains nothing still says so, per definition: the
    // ordinary case is a tab full of "unconstrained" rather than an empty one,
    // promise row included.
    let nodes = tab("presence", "let id = fn x => x");
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
    let stage = stage(
        "presence",
        "let p = fn a => match a with | {x} => {} | {y} => {} end",
    );
    assert_eq!(stage.summary, "1 constraint");
    assert!(stage.micros.is_some());
}

/// The `where` clause reaches the panels that render a written type, and reads
/// as the statement list the reader wrote: each `let` with its names, each
/// constraint, `;` between them. A `_` renders as the hole it is.
#[test]
fn the_ast_tab_renders_the_where_statement_list() {
    let tree = tab(
        "ast",
        "let both : { x when 'a: 'c, y when 'b: _ } -> {} where 'a = 'b; 'a = fn v => v",
    );
    let rows = flatten(&tree);
    // The clause is a row of its own beside the type, spelled as it was
    // written — `;`s and all.
    let clause = rows
        .iter()
        .find(|node| node.label == "Where")
        .expect("the clause is a row");
    assert_eq!(clause.text, "'a = 'b; 'a");
    // One row per statement, labelled by which of the two it is.
    let stmts: Vec<&str> = clause
        .children
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(stmts, ["Constraint", "Constraint"]);
    // The hole is a leaf of its own, not a name and not an error.
    assert!(
        rows.iter()
            .any(|node| node.label == "Hole" && node.text == "_"),
        "{rows:#?}"
    );
}

/// The IR tab says what each variable turned out to stand for, which is the
/// whole of what it adds over the AST's: the name is written as its uses spell
/// it, and which of the three sorts it has follows from where the type uses it.
#[test]
fn the_ir_tab_says_what_each_declared_variable_stands_for() {
    let source = "let f : { x when 'a: 'c, ..'b } -> (#A Nat | ..'d) = fn p => p";
    let tree = tab("ir", source);
    let rows = flatten(&tree);
    let stands: Vec<(&str, &str)> = rows
        .iter()
        .filter(|node| node.label.starts_with("Variable "))
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    assert_eq!(
        stands,
        [
            ("Variable a", "when 'a (a presence)"),
            ("Variable c", "'c (a whole type)"),
            ("Variable b", "'b (a whole type)"),
            ("Variable d", "..'d (the rest of a sum's cases)"),
        ]
    );
    // Each row is spanned at the first use, which is where the variable was
    // introduced, so clicking a `..'b` in the type lights it.
    let declared = rows
        .iter()
        .find(|node| node.label == "Variable b")
        .expect("the row is there");
    let tail = source.find("..'b").expect("the tail") + 2;
    assert_eq!(declared.span, at([tail, tail + 2]));

    // A named tail keeps its as-written spelling in the type beside them.
    assert!(
        rows.iter()
            .any(|node| node.label == "Rest" && node.text == "..'b"),
        "{rows:#?}"
    );
    // And a use in a type position is a row of its own, named rather than
    // resolved to a symbol.
    assert!(
        rows.iter()
            .any(|node| node.label == "Var" && node.text == "'c"),
        "{rows:#?}"
    );
}

/// A use of a declared variable in the type is grouped with the `where 'let`
/// statement that declared it, the way a declaration parameter is grouped with
/// its header — by a link the page paints, since a variable scoped to one
/// annotation reaches no symbol table. Two variables are two groups.
#[test]
fn the_ir_tab_links_a_use_to_the_declaration_it_resolves_to() {
    let tree = tab("ir", "let f : { x: 'c, ..'b } -> Nat = fn p => 0");
    let rows = flatten(&tree);
    let link = |label: &str, text: &str| {
        rows.iter()
            .find(|node| node.label == label && node.text.starts_with(text))
            .unwrap_or_else(|| panic!("`{label}` `{text}` is a row: {rows:#?}"))
            .link
    };
    let b = link("Variable b", "'b");
    let c = link("Variable c", "'c");
    assert_eq!(link("Rest", "..'b"), b);
    assert_eq!(link("Var", "'c"), c);
    // Both are a group at all, and they are not the same group: two names that
    // stand for two things may not light each other.
    assert!(b.is_some() && c.is_some(), "{rows:#?}");
    assert_ne!(b, c);

    // A declaration declares no such variables, so nothing in one is grouped
    // this way: its parameters are symbols and light up as those.
    let tree = tab("ir", "type Pair 'a = { fst: 'a, snd: 'a }");
    let rows = flatten(&tree);
    assert!(rows.iter().all(|node| node.link.is_none()), "{rows:#?}");
}

/// The Types tab shows the published schemes in the spelling the CLI reports,
/// `where 'let` clause and all — so the tab and the compiler's own answer cannot
/// drift — and the Solve tab shows a rigid in a goal as the name it was
/// declared with.
#[test]
fn the_type_tabs_show_the_new_spelling() {
    let source = "let id : 'a -> 'a = fn x => x";
    let tree = tab("types", source);
    let rows = flatten(&tree);
    let scheme = rows
        .iter()
        .find(|node| node.label == "let id")
        .expect("the definition has a row");
    assert_eq!(scheme.text, "'a -> 'a");

    // A rigid appears in the solve as its name rather than as a `?`, which is
    // what makes a step about one readable at all.
    let tree = tab("solve", source);
    let rows = flatten(&tree);
    assert!(
        rows.iter().any(|node| node.text.contains(" ~ 'a")),
        "{rows:#?}"
    );

    // And the rule that refuses one is a step like any other, red and carrying
    // the same words the strip does.
    let tree = tab("solve", "let g : 'a -> 'a = fn x => 0");
    let rows = flatten(&tree);
    let refused = rows
        .iter()
        .find(|node| node.error)
        .expect("the refusal is a step");
    assert_eq!(refused.label, "mismatch");
    let error = refused
        .fields
        .iter()
        .find(|field| field.name == "_error")
        .expect("a failed step carries what it said");
    assert_eq!(
        error.value,
        "this is `Nat`, but `'a` stands for whatever type the caller picks"
    );
}

/// The LIR tab: one root per global and per function wearing the header line
/// the listing writes, a row per instruction under it labelled by its opcode,
/// and a dispatch's blocks behind the answer that selects each. Every row shows
/// the representation of what it assigns, since that is the whole of what LIR
/// keeps of a type.
#[test]
fn the_lir_tab_renders_the_listing_as_a_tree() {
    let nodes = tab(
        "lir",
        "let f = fn a => match a with | #A n => n | b => 0 end",
    );
    let roots: Vec<(&str, &str)> = nodes
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    assert_eq!(
        roots,
        [
            ("global", "global f:"),
            ("fn", "fn f(%0: sum):"),
            ("fn", "fn f#1(%4: sum):"),
        ],
        "{nodes:#?}"
    );

    // The dispatch owns its cases, and each case owns the block it selects.
    let switch = &nodes[1].children[0];
    assert_eq!(switch.label, "switch_tag");
    assert_eq!(switch.text, "%3: nat = switch_tag %0:");
    let cases: Vec<&str> = switch
        .children
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(cases, ["#A =>", "else =>"], "{switch:#?}");
    let inside: Vec<(&str, &str)> = switch.children[0]
        .children
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    assert_eq!(
        inside,
        [("payload", "%1: nat = payload %0"), ("yield", "yield %1")]
    );

    // A row the source wrote points back at it, so the tab and the editor
    // cross-highlight; a curried wrapper is nobody's source and says so.
    assert!(nodes[1].span.is_some(), "{:#?}", nodes[1]);
    assert!(nodes[2].generated, "{:#?}", nodes[2]);
    assert!(nodes[2].span.is_none(), "{:#?}", nodes[2]);
}

/// Evidence is plumbing rather than prose: the record of a handler's operations
/// and the bundle a call builds for an effect-polymorphic callee are rows the
/// reader never wrote, so they are marked generated and point at no source. The
/// `catch` beside them is the `handle` the reader did write, and still does.
#[test]
fn the_lir_tab_marks_the_evidence_it_plumbs_as_generated() {
    let nodes = tab(
        "lir",
        "effect Log = write : Nat -> ()\n\
         let piped : (Nat -> Nat + ..'e) -> Nat -> Nat + ..'e = fn g => fn n => g n\n\
         let logger : Nat -> Nat + !Log = fn n => let z = !Log.write n in n\n\
         let use = fn w => handle piped logger 1 with | !Log.write s => {} end\n",
    );
    let rows = flatten(&nodes);
    let row = |text: &str| {
        rows.iter()
            .find(|node| node.text.contains(text))
            .unwrap_or_else(|| panic!("`{text}` is a row of:\n{rows:#?}"))
    };

    // The handler's record of operation closures, and the bundle the call to
    // `piped` builds out of it.
    for text in ["= struct { write: %", "= struct { Log: %"] {
        let node = row(text);
        assert!(node.generated, "{node:#?}");
        assert!(node.span.is_none(), "{node:#?}");
    }

    // The `handle` itself is the reader's, and so is the perform inside the
    // function the evidence reaches.
    let caught = row("= catch %");
    assert!(!caught.generated, "{caught:#?}");
    assert!(caught.span.is_some(), "{caught:#?}");
}

/// A temp is one temp everywhere it appears, so the tab lights every occurrence
/// of `%17` in the whole listing rather than only the ones in a row's own
/// function. That is the pattern it declares, and it is deliberately not the
/// type tabs' — a temp is a value, not a question the solver has yet to answer.
#[test]
fn the_lir_tab_lights_a_temp_wherever_it_appears() {
    let spec = REGISTRY
        .iter()
        .find(|spec| spec.id == "lir")
        .expect("the lir stage is registered");
    let pattern = spec.highlight.expect("the lir tab declares a pattern");
    assert_eq!(pattern, r"%\d+");
    assert!(!spec.scoped);

    let found: Vec<&str> = Regex::new(pattern)
        .expect("the pattern compiles")
        .find_iter("%3: nat = call add, %17, %2")
        .map(|at| at.as_str())
        .collect();
    assert_eq!(found, ["%3", "%17", "%2"]);
}

/// Lowering runs on a program every earlier phase accepted, so the tab reports
/// `Skipped` the moment anything upstream complains — rather than rendering
/// half a listing of a program that does not type.
#[test]
fn the_lir_tab_skips_a_program_with_errors() {
    use ruddy_debug::wire::Status;

    let stage = |source: &str| stage("lir", source);

    let ok = stage("let f = fn a => a\n");
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.summary, "1 global · 2 functions");

    // A type error, which is one the earlier phases all survive: the program is
    // built and inferred, and lowering still stands aside.
    let bad = stage("let f : Nat = fn a => a\n");
    assert_eq!(bad.status, Status::Skipped);
    assert_eq!(bad.summary, "lowering to LIR did not run");
    assert!(bad.nodes.is_empty());
    assert!(bad.micros.is_none());
}

/// The Tokens tab is one row per file with that file's own stream under it. A
/// bundle is several streams rather than one, and a flat list would run the last
/// token of one file into the first of the next with nothing to say where the
/// seam was.
#[test]
fn the_tokens_tab_groups_its_rows_by_file() {
    let stage = named(bundle(NESTED), "tokens");

    // One row per file, in load order, each labelled with its path and what it
    // holds — so the row says how much is behind it while it is collapsed.
    let rows: Vec<(&str, &str)> = stage
        .nodes
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("File", "main.hc · 16 tokens"),
            ("File", "Math.hc · 9 tokens"),
            ("File", "Math/Vec.hc · 4 tokens"),
        ],
        "{:#?}",
        stage.nodes
    );

    // The tokens themselves are the children, and each is a span in the file it
    // is a child of.
    for (index, file) in stage.nodes.iter().enumerate() {
        assert!(!file.children.is_empty(), "{} is empty", file.text);
        for token in &file.children {
            let at = token.span.expect("a token was written somewhere");
            assert_eq!(at.file, index as u32, "{} {:?}", token.label, token.text);
        }
    }

    // And the summary counts both: the tokens are what the tab renders, and the
    // files are what it renders them under.
    assert_eq!(stage.summary, "29 tokens · 3 files");
}

/// The AST tab is one row per file holding the statements written in *that*
/// file. The tree the loader hands over is spliced, so a module whose body came
/// from another file has to be rendered as the leaf it was written as — nesting
/// its statements here as well as under their own file would show the reader one
/// program in two places.
#[test]
fn the_ast_tab_groups_its_rows_by_file() {
    let stage = named(bundle(NESTED), "ast");

    let files: Vec<(&str, &str)> = stage
        .nodes
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    assert_eq!(
        files,
        [
            ("File", "main.hc"),
            ("File", "Math.hc"),
            ("File", "Math/Vec.hc"),
        ],
        "{:#?}",
        stage.nodes
    );

    let children = |at: usize| -> Vec<&str> {
        stage.nodes[at]
            .children
            .iter()
            .map(|node| node.label.as_str())
            .collect()
    };
    // The root file leads with the header, which is the file's own first line
    // rather than a statement — there is no node in the statement list for it
    // to be.
    assert_eq!(children(0), ["Bundle", "Module", "Let"]);
    assert_eq!(stage.nodes[0].children[0].text, "bundle demo 0.1.0");
    // And the module whose body is in `Math.hc` carries its name and nothing
    // else: the body is that file's row.
    let math = &stage.nodes[0].children[1];
    assert_eq!(
        math.children
            .iter()
            .map(|node| node.label.as_str())
            .collect::<Vec<_>>(),
        ["Name"],
        "{math:#?}"
    );
    assert_eq!(children(1), ["Module", "Let"]);
    assert_eq!(children(2), ["Let"]);

    // A module written inline is the other half of the rule: its statements
    // were written in this file, so this file's row is where they go.
    let nodes = tab("ast", "module A =\n  let x = 1\nend\n");
    let inline = &nodes[0].children[1];
    assert_eq!(inline.label, "Module");
    let kids: Vec<&str> = inline
        .children
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(kids, ["Name", "Let"], "{inline:#?}");

    // The summary counts each kind of declaration the bundle holds, across
    // every file of it.
    assert_eq!(
        named(bundle(NESTED), "ast").summary,
        "2 module · 0 effect · 0 type · 3 let"
    );
}

/// The Symbols tab says where each symbol sits — which module declared it, and
/// which file that module was written in. A name is no longer enough to say
/// where a symbol came from, and the mangled name it round-trips through is the
/// only other thing on the row that knows.
#[test]
fn the_symbols_tab_says_which_module_and_file_a_symbol_came_from() {
    let stage = named(bundle(NESTED), "symbols");
    let field = |name: &str, of: &str| -> String {
        stage
            .nodes
            .iter()
            .find(|node| node.label == name)
            .unwrap_or_else(|| panic!("no row for {name}: {:#?}", stage.nodes))
            .fields
            .iter()
            .find(|field| field.name == of)
            .unwrap_or_else(|| panic!("{name} has no {of}"))
            .value
            .clone()
    };

    // Two modules deep, declared in the file its own path spells.
    assert_eq!(field("zero", "module"), "Math::Vec");
    assert_eq!(field("zero", "file"), "Math/Vec.hc");
    // One deep, in the file its parent's declaration named.
    assert_eq!(field("double", "module"), "Math");
    assert_eq!(field("double", "file"), "Math.hc");
    // And at the bundle root, which is a real position in the tree rather than
    // a missing one — so it is written as one rather than left blank.
    assert_eq!(field("four", "module"), "—");
    assert_eq!(field("four", "file"), "main.hc");

    // A local is written inside a definition rather than declared, so there is
    // no declaration to read a file off: the column is blank, and the module is
    // still the one it was written in.
    assert_eq!(field("x", "scope"), "local");
    assert_eq!(field("x", "file"), "");
    assert_eq!(field("x", "module"), "Math");

    // Every row still round-trips through the mangler, which now has an `M`
    // component per enclosing module to lose.
    for node in &stage.nodes {
        assert_eq!(field(&node.label, "demangle"), "ok", "{}", node.label);
        assert!(!node.error, "{}", node.label);
    }

    // The summary counts what a bundle made worth counting.
    assert_eq!(stage.summary, "6 symbols · 3 files · 2 modules");
}

/// A whole bundle, each file exactly as written — the request the page posts,
/// with nothing added to it.
fn bundle(files: &[(&str, &str)]) -> Snapshot {
    compile(
        &CompileRequest {
            files: files
                .iter()
                .map(|(path, source)| FileSpec {
                    path: (*path).to_string(),
                    source: (*source).to_string(),
                })
                .collect(),
            revision: 0,
        },
        0,
    )
}

/// One stage over one snippet, compiled as the whole of a bundle's root file.
fn stage(id: &'static str, snippet: &str) -> Stage {
    named(bundle(&[(ROOT, &format!("{HEADER}{snippet}"))]), id)
}

/// One stage of a snapshot, by the id it is registered under.
fn named(snapshot: Snapshot, id: &'static str) -> Stage {
    snapshot
        .stages
        .into_iter()
        .find(|stage| stage.id == id)
        .unwrap_or_else(|| panic!("{id} is registered"))
}

/// The rows of one stage over one snippet, for the tests above.
fn tab(id: &'static str, snippet: &str) -> Vec<Node> {
    stage(id, snippet).nodes
}

/// Where a range written in a snippet's own offsets ends up on the wire: in the
/// root file, [`HEADER`] bytes further in than the snippet spells it.
fn at(range: [usize; 2]) -> Option<Loc> {
    Some(Loc {
        file: 0,
        range: [range[0] + HEADER.len(), range[1] + HEADER.len()],
    })
}

/// Every row of a tree, parents before children — the shape of the tree is not
/// what these tests are about.
fn flatten(nodes: &[Node]) -> Vec<&Node> {
    let mut out = Vec::new();
    for node in nodes {
        out.push(node);
        out.extend(flatten(&node.children));
    }
    out
}
