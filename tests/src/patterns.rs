//! Tests for [`ruddy::patterns`].

use ruddy::{
    inference,
    ir::{self, Witness},
    parse,
    patterns::{self, Coverage, ErrorKind, Verdict},
    symbol::{Bundle, Mint, Version},
    token::lex,
    tracking::FileID,
};

fn dummy_mint() -> Mint {
    Mint::new(Bundle::new("test", Version::new(0, 0, 0)).expect("valid bundle"))
}

/// Parse, lower, infer and check. Only the parse is required to be clean:
/// several tests are exactly about what the checks do with a program an
/// earlier phase complained about.
fn checked(src: &str) -> (ir::Output, inference::Output, patterns::Output) {
    let parsed = parse::parse(lex(src, FileID::GENERATED).tokens);
    assert!(
        parsed.errors.is_empty(),
        "unexpected parse errors: {:#?}",
        parsed.errors
    );
    let mut mint = dummy_mint();
    let mut out = ir::build(&mut mint, parsed.stmts);
    let inferred = inference::infer(&mint, &mut out.program);
    let checks = patterns::check(&out.program, &inferred);
    (out, inferred, checks)
}

/// [`checked`] for a program every phase should accept.
fn clean(src: &str) -> patterns::Output {
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "ir errors: {:#?}", out.errors);
    assert!(
        inferred.errors.is_empty(),
        "inference errors: {:#?}",
        inferred.errors
    );
    assert!(
        checks.errors.is_empty(),
        "pattern errors: {:#?}",
        checks.errors
    );
    checks
}

/// The phase's complaints as `code@offset`, the shape every error test here
/// asserts on.
fn errors(checks: &patterns::Output) -> Vec<String> {
    checks
        .errors
        .iter()
        .map(|error| format!("{}@{}", error.kind.code(), error.span.start))
        .collect()
}

/// The one report of a program with one match.
fn sole_report(checks: &patterns::Output) -> &patterns::Report {
    assert_eq!(checks.reports.len(), 1, "{:#?}", checks.reports);
    &checks.reports[0]
}

fn verdicts(report: &patterns::Report) -> Vec<Verdict> {
    report.arms.iter().map(|arm| arm.verdict).collect()
}

/// The witness of an unhandled match, rendered.
fn witness_of(checks: &patterns::Output) -> String {
    checks
        .errors
        .iter()
        .find_map(|error| match &error.kind {
            ErrorKind::UnhandledValues { witness } => Some(witness.to_string()),
            _ => None,
        })
        .expect("an unhandled-values complaint carries the witness")
}

/// The motivating program: exact arms over every presence combination of two
/// fields. Exhaustive, every arm reachable, and — the other half being
/// inference's — no complaint from anywhere.
#[test]
fn the_motivating_program_is_exhaustive() {
    let checks = clean(
        "let p = fn a => match a with \
         | {a, b} => () | {a} => {} | {b} => {} | {} => {} end",
    );
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable; 4]);
}

/// Presences are independent: with `{a, b}` and `{}` over an otherwise
/// unconstrained scrutinee, the subsets holding exactly one of the fields go
/// unhandled, and the witness names one pun-style — the presence is the
/// information.
#[test]
fn independent_presences_leave_a_hole() {
    let src = "let p = fn a => match a with | {a, b} => 1 | {} => 2 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unhandled-values@{}",
            src.find("match").expect("the match")
        )],
        "{checks:#?}"
    );
    assert_eq!(witness_of(&checks), "{ a }");
    let report = sole_report(&checks);
    assert!(matches!(
        &report.coverage,
        Coverage::Unhandled(Witness::Struct(_))
    ));
}

/// Typed reachability, the spec's example: a `let` above the match proves
/// both fields present, so the `{}` arm can never be reached, and the match
/// is exhaustive without it.
#[test]
fn a_solved_present_field_makes_the_empty_arm_unreachable() {
    let src = "let h = fn v => let {a, b} = v in \
               match v with | {a, b} => a | {} => 0 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.find("{} => 0").expect("the empty arm")
        )],
        "{checks:#?}"
    );
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Unreachable]);
}

/// The misplaced catch-all, at the arm and only there: the arms it starves
/// are not additionally flagged, and — per R7 — they were still lowered and
/// typed, which is what makes their consistent `Nat` demands part of the
/// solved scrutinee.
#[test]
fn a_misplaced_catch_all_is_reported_at_the_arm() {
    let src = "let f = fn n => match n with | x => 1 | 2 => 3 | 4 => 5 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "misplaced-catch-all@{}",
            src.find("x => 1").expect("the catch-all")
        )],
        "{checks:#?}"
    );
    let report = sole_report(&checks);
    assert_eq!(
        verdicts(report),
        [Verdict::Reachable, Verdict::Starved, Verdict::Starved]
    );
    // The starved arms were typed: the scrutinee solved to `Nat`.
    assert_eq!(report.scrutinee.to_string(), "Nat");
    assert!(matches!(report.coverage, Coverage::Exhaustive));

    // A second catch-all after the first is starved like anything else: one
    // complaint per match, at the first.
    let src = "let f = fn e => match e with | a => 1 | b => 2 | c => 3 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "misplaced-catch-all@{}",
            src.find("a => 1").expect("the first catch-all")
        )],
        "{checks:#?}"
    );

    // A bare `{..}` accepts everything on its face, so it is one too.
    let src = "let f = fn e => match e with | {..} => 1 | {} => 2 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "misplaced-catch-all@{}",
            src.find("{..}").expect("the open arm")
        )],
        "{checks:#?}"
    );
}

/// An open pattern naming a field is *not* irrefutable on its face — a value
/// without the field escapes it — so the spec's open example gets no
/// misplaced-catch-all and is exhaustive: present is arm 1's, absent arm 2's.
#[test]
fn an_open_pattern_with_a_field_is_not_a_catch_all() {
    let checks = clean("let f = fn v => match v with | {x, ..} => x | {} => 0 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);
}

/// Unit equivalence in a match: `{a}` then `()` — neither is a catch-all,
/// both are reachable, and together they cover the one optional field.
#[test]
fn unit_and_empty_braces_are_one_pattern() {
    let checks = clean("let g = fn v => match v with | {a} => a | () => 0 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);
}

/// The naturals cannot be listed in full, so a match testing them still needs
/// a final arm naming the rest — the check moved behind inference, wording
/// and code unchanged.
#[test]
fn naturals_still_need_a_final_catch_all() {
    let src = "let f = fn n => match n with | 0 => 1 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unhandled-numbers@{}",
            src.find("match").expect("the match")
        )],
        "{checks:#?}"
    );

    // A refutable last arm is not the rest either.
    let (_, _, checks) = checked("let f = fn n => match n with | 0 => 1 | 1 => 2 end");
    assert_eq!(checks.errors.len(), 1, "{:#?}", checks.errors);
    assert!(matches!(checks.errors[0].kind, ErrorKind::UnhandledNumbers));

    // With the final arm, clean — and a duplicate literal above it is the
    // unreachable arm it always was.
    clean("let f = fn n => match n with | 0 => 1 | k => k end");
    let src = "let f = fn n => match n with | 0 => 1 | 0 => 2 | k => 3 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.rfind("0 => 2").expect("the duplicate")
        )],
        "{checks:#?}"
    );
}

/// Sum reachability and coverage read the solved row: a duplicate case arm is
/// unreachable, a missing case is unhandled with the case as witness, and a
/// nested natural column is worded with the witness written out.
#[test]
fn sum_checks_read_the_solved_row() {
    let src = "let f = fn e => match e with | `A x => 1 | `A y => 2 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.find("`A y").expect("the shadowed arm")
        )],
        "{checks:#?}"
    );

    let src = "let f = fn e => match e with | `A 0 => 1 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "unhandled-values@{}",
            src.find("match").expect("the match")
        )],
        "{checks:#?}"
    );
    assert_eq!(witness_of(&checks), "`A 1");

    // The hole across two columns: each covered, the combination not.
    let src = "let f = fn e => match e with | { a: `A, b: `X } => 1 | { a: `B, b: `Y } => 2 end";
    let (_, _, checks) = checked(src);
    assert_eq!(checks.errors.len(), 1, "{:#?}", checks.errors);
    assert_eq!(witness_of(&checks), "{ a: `A, b: `Y }");

    // An open position's rest is worded as "anything other than" what was
    // listed.
    let src = "let f = fn e => match e with | { a: `A, b: `X } => 1 | { a: `B, b: w } => 2 end";
    let (_, _, checks) = checked(src);
    assert_eq!(witness_of(&checks), "{ a: `A, b: anything other than `X }");
}

/// The empty match stays silent: the scrutinee is the empty sum, which has no
/// values to leave unhandled.
#[test]
fn an_empty_match_stays_silent() {
    let checks = clean("let absurd = fn v => match v with end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert!(report.arms.is_empty());
}

/// R11: a match whose scrutinee — or any tested position — failed to solve is
/// skipped whole. A mixed match gets exactly one complaint, the solver's, and
/// a match on a name that never resolved gets only the resolution error.
#[test]
fn a_failed_typing_skips_the_checks() {
    // Numbers and cases at one position: the solver's mismatch is the whole
    // story, and every arm reports as skipped.
    let src = "let f = fn e => match e with | 1 => 2 | `A => 3 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(inferred.errors.len(), 1, "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Skipped));
    assert_eq!(verdicts(report), [Verdict::Skipped, Verdict::Skipped]);

    // At a nested position too: the payloads of one tag are one position.
    let src = "let f = fn e => match e with | `A 0 => 1 | `A `X => 2 | r => 3 end";
    let (_, inferred, checks) = checked(src);
    assert_eq!(inferred.errors.len(), 1, "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);

    // A scrutinee that never resolved: the undefined name is the one
    // complaint, and the natural match around it says nothing.
    let src = "let f = match oops with | 0 => 1 end";
    let (out, _, checks) = checked(src);
    assert_eq!(out.errors.len(), 1, "{:#?}", out.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(matches!(sole_report(&checks).coverage, Coverage::Skipped));

    // And an empty match over something that is not the empty sum: the
    // mismatch speaks, the report stays honest.
    let (_, inferred, checks) = checked("let f = match 5 with end");
    assert_eq!(inferred.errors.len(), 1, "{:#?}", inferred.errors);
    assert!(matches!(sole_report(&checks).coverage, Coverage::Skipped));
}

/// A match nested inside an arm's body is found and checked like the outer
/// one: the walk reaches every match in the program.
#[test]
fn nested_matches_are_checked_too() {
    let src = "let f = fn e => match e with \
               | k => match k with | 0 => 1 end end";
    let (_, _, checks) = checked(src);
    assert_eq!(checks.reports.len(), 2, "{:#?}", checks.reports);
    assert_eq!(
        errors(&checks),
        [format!(
            "unhandled-numbers@{}",
            src.rfind("match k").expect("the inner match")
        )],
        "{checks:#?}"
    );
}

/// A field the solved type proves absent has no present case: a value of the
/// fieldless unit type has exactly the fields the type names, so an arm
/// demanding one it does not name is unreachable — the other half of the
/// typed presence rules.
#[test]
fn a_solved_absent_field_starves_its_arm() {
    let src = "let f = match {} with | {a} => 1 | {} => 2 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.find("{a}").expect("the demanding arm")
        )],
        "{checks:#?}"
    );
    assert!(matches!(
        sole_report(&checks).coverage,
        Coverage::Exhaustive
    ));
}

/// The report carries what the debugger renders: the match's span, the solved
/// scrutinee type, and the scrutinee's own span for cross-highlighting.
#[test]
fn a_report_names_the_match_and_its_scrutinee() {
    let src = "let get = fn opt => match opt with | `Some x => x | `None => 0 end";
    let checks = clean(src);
    let report = sole_report(&checks);
    assert_eq!(report.span.start, src.find("match").expect("the match"));
    assert_eq!(
        report.scrutinee_span.start,
        src.find("opt with").expect("the scrutinee")
    );
    assert_eq!(report.scrutinee.to_string(), "`Some Nat | `None");
}

/// A scrutinee behind a declared name: the checks look through the alias, and
/// a row parameter's spliced tail is flattened on the way — the `NoErr`-style
/// alias is the one place a solved row still arrives as a chain.
#[test]
fn an_aliased_scrutinee_is_unfolded() {
    let checks = clean(
        "type Fallible r = `Err Nat | ..r\n\
         let f : Fallible (`Ok Nat) -> Nat = \
         fn t => match t with | `Err n => n | `Ok k => k end",
    );
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);
}

/// A listed number can itself be the witness, when the hole is in another
/// field beside it: `a: 0` is covered for `b` present and for nothing else.
#[test]
fn a_listed_number_can_witness_the_hole() {
    let src = "let f = fn v => match v with | {a: 0, b} => 1 | {} => 2 end";
    let (_, inferred, checks) = checked(src);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(checks.errors.len(), 1, "{:#?}", checks.errors);
    assert_eq!(witness_of(&checks), "{ a: 0 }");
}

/// Open arms over disjoint fields: each demands its own field present and
/// says nothing about the other, and the bare `{..}` takes the rest.
#[test]
fn open_arms_cover_by_their_own_fields() {
    let checks =
        clean("let f = fn v => match v with | {a, ..} => 1 | {b, ..} => 2 | {..} => 3 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable; 3].to_vec());
}

/// A struct arm before a binder: the binder covers whatever the struct's
/// exactness leaves — both reachable, nothing unhandled.
#[test]
fn a_binder_after_a_struct_arm_takes_the_rest() {
    let checks = clean("let f = fn v => match v with | {a} => 1 | w => 2 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);
}

/// An annotation's `\a` proves the field absent in the scrutinee's own type,
/// so the arm demanding it is unreachable — absence read out of the field
/// map, not only out of a closed row's silence.
#[test]
fn an_annotated_absence_starves_the_demanding_arm() {
    let src = "let f : { \\a, .. } -> Nat = \
               fn v => match v with | {a, ..} => 1 | {..} => 2 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.find("{a, ..}").expect("the demanding arm")
        )],
        "{checks:#?}"
    );
}

/// A field's presence abandoned by a failure elsewhere in the group reads as
/// undecided, and the checks stand aside rather than reason from it.
#[test]
fn an_abandoned_presence_skips_the_checks() {
    let src = "let f = fn v => match v with | {a} => 1 | {} => g v end\n\
               let g = fn w => let n : Nat = w in f w";
    let (_, inferred, checks) = checked(src);
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(
        checks
            .reports
            .iter()
            .any(|report| matches!(report.coverage, Coverage::Skipped)),
        "{:#?}",
        checks.reports
    );
}

/// A sum position whose every case is ruled out and whose rest stays open:
/// what escapes is anything at all, and the witness says so rather than
/// being "anything other than" nothing.
#[test]
fn an_open_sum_with_no_cases_witnesses_as_anything() {
    let src = "let f : { a: Nat, b: (\\`X | ..r), .. } -> Nat = \
               fn v => match v with | {a: 0, ..} => 1 end";
    let (out, _, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert_eq!(checks.errors.len(), 1, "{:#?}", checks.errors);
    assert_eq!(witness_of(&checks), "{ a: 1, b }");
}

/// The three ways a tag test can find the typing failed, each skipping the
/// match: a scrutinee that is no sum at all, a sum that never acquired the
/// case, and a case the row settled absent — the demand said present, so the
/// mismatch already spoke.
#[test]
fn a_failed_tag_test_skips_the_checks() {
    // No sum at all: the scrutinee solved to `Nat`.
    let (_, inferred, checks) = checked("let f = match 5 with | `A => 1 end");
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(matches!(sole_report(&checks).coverage, Coverage::Skipped));

    // A sum without the case: the literal's own row never acquired `A`.
    let (_, inferred, checks) = checked("let f = match `B 1 with | `A x => x end");
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(matches!(sole_report(&checks).coverage, Coverage::Skipped));

    // A case the refinement settled absent: the binder's view has no `Some`
    // left, so testing it again is the mismatch — and only the mismatch.
    let src = "let f = fn v => match v with \
               | `Some x => 1 \
               | r => match r with | `Some y => 2 | w => 3 end end";
    let (_, inferred, checks) = checked(src);
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(
        checks
            .reports
            .iter()
            .any(|report| matches!(report.coverage, Coverage::Skipped)),
        "{:#?}",
        checks.reports
    );
}

/// A sum row abandoned from outside the match: the group-mate's conflict
/// gives the scrutinee's rest up, and the checks stand aside rather than
/// reason from a row whose remainder is nobody's answer.
#[test]
fn an_abandoned_sum_rest_skips_the_checks() {
    let src = "let f = fn v => match v with | `A x => 1 | r => g v end\n\
               let g = fn w => let n : Nat = w in f w";
    let (_, inferred, checks) = checked(src);
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    assert!(
        checks
            .reports
            .iter()
            .any(|report| matches!(report.coverage, Coverage::Skipped)),
        "{:#?}",
        checks.reports
    );
}

/// A refutable arm above the misplaced catch-all is its own arm still: it is
/// checked for reachability like any other, and only the arms after the
/// catch-all are starved.
#[test]
fn arms_above_a_misplaced_catch_all_keep_their_verdicts() {
    let src = "let f = fn n => match n with | 2 => 3 | x => 1 | 4 => 5 end";
    let (_, _, checks) = checked(src);
    assert_eq!(
        errors(&checks),
        [format!(
            "misplaced-catch-all@{}",
            src.find("x => 1").expect("the catch-all")
        )],
        "{checks:#?}"
    );
    assert_eq!(
        verdicts(sole_report(&checks)),
        [Verdict::Reachable, Verdict::Reachable, Verdict::Starved]
    );
}

/// An empty match over a scrutinee that turned out to have cases: the
/// eliminator's demand failed, and the report says the checks stood aside
/// rather than calling a sum with values exhausted by nothing.
#[test]
fn an_empty_match_over_a_real_sum_is_skipped() {
    let (_, inferred, checks) =
        checked("let f = fn v => let w : (`A Nat | ..r) = v in match v with end");
    assert!(!inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert!(checks.errors.is_empty(), "{:#?}", checks.errors);
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Skipped));
    assert!(report.arms.is_empty());
}

/// An arm only the values with extra fields reach is reachable: exactness is
/// the earlier arm's demand, not the later one's ceiling. `{}` takes the
/// fieldless value and `{..}` everything carrying a field, so neither is
/// unreachable — and the same reasoning reaches a binder behind the exact
/// empty pattern.
#[test]
fn an_open_arm_behind_an_exact_one_is_reachable() {
    // Bare `{..}` after the exact empty pattern.
    let checks = clean("let f = fn v => match v with | {} => 1 | {..} => 2 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);

    // An open arm naming the same field the exact one names: a value with
    // `a` and anything else gets past the first arm.
    let checks = clean("let f = fn v => match v with | {a} => 1 | {a, ..} => 2 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);

    // A binder behind the exact empty pattern takes the values with fields.
    let checks = clean("let f = fn v => match v with | {} => 1 | x => 2 end");
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Reachable]);
}

/// A column of exact arms closes the row, and over the closed row a duplicate
/// exact arm has nothing left to take: the rest column with no nonempty half.
#[test]
fn a_duplicate_exact_arm_over_a_closed_row_is_unreachable() {
    let src = "let f = fn v => match v with | {a} => 1 | {a} => 2 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.rfind("{a} => 2").expect("the duplicate")
        )],
        "{checks:#?}"
    );
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Unreachable]);
}

/// The other direction: an exact arm behind an open one covering its field is
/// unreachable — every value with `a`, extra fields or none, is the open
/// arm's, so the exact demand for an empty rest finds nothing left.
#[test]
fn an_exact_arm_behind_a_covering_open_one_is_unreachable() {
    let src = "let f = fn v => match v with | {a, ..} => 1 | {a} => 2 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.rfind("{a} => 2").expect("the exact arm")
        )],
        "{checks:#?}"
    );
    let report = sole_report(&checks);
    assert!(matches!(report.coverage, Coverage::Exhaustive));
    assert_eq!(verdicts(report), [Verdict::Reachable, Verdict::Unreachable]);
}

/// A nested position closed by its own exact sub-column keeps its rest shut
/// even when the outer row is open: with `b` proved present everywhere, a
/// second `{b, ..}` arm is left nothing — not even a value whose `a` carries
/// fields the inner exact `{}` never named, because the inner row has none to
/// carry.
#[test]
fn a_closed_nested_rest_offers_no_escape() {
    let src = "let f = fn v => match v with \
               | {a: {}, b} => 1 | {b, ..} => 2 | {b, ..} => 3 end";
    let (out, inferred, checks) = checked(src);
    assert!(out.errors.is_empty(), "{:#?}", out.errors);
    assert!(inferred.errors.is_empty(), "{:#?}", inferred.errors);
    assert_eq!(
        errors(&checks),
        [format!(
            "unreachable-arm@{}",
            src.rfind("{b, ..} => 3").expect("the shadowed arm")
        )],
        "{checks:#?}"
    );
    let report = sole_report(&checks);
    assert_eq!(
        verdicts(report),
        [Verdict::Reachable, Verdict::Reachable, Verdict::Unreachable]
    );
}
