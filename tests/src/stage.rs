//! Tests for [`ruddy_debug::stage`].

use ruddy::types::Ty;
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
    let declared: Vec<Option<&str>> = ["constraints", "solve", "types"]
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
    assert_eq!(pattern, r"\?\d*|'[a-z]\d*");

    // `\?\d*`: a `?`, then digits — of which a bare `?` is the empty case.
    for (ty, tail) in [(Ty::Var(4), "4"), (Ty::Undecided, "")] {
        let printed = ty.to_string();
        assert_eq!(printed, format!("?{tail}"));
        assert!(tail.chars().all(|c| c.is_ascii_digit()), "{printed}");
    }

    // `'[a-z]\d*`: a quote, a letter, then the digits that appear once the
    // alphabet runs out.
    for index in [0, 25, 26] {
        let printed = Ty::Bound(index).to_string();
        let mut chars = printed.chars();
        assert_eq!(chars.next(), Some('\''), "{printed}");
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_lowercase()),
            "{printed}"
        );
        assert!(chars.all(|c| c.is_ascii_digit()), "{printed}");
    }
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
