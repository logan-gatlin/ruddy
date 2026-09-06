//! Tests for the parsed-bundle core compilation seam.

use ruddy::{
    compile::{self, Error},
    inference, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};

#[test]
fn failed_compilation_preserves_completed_phases_and_aggregates_checking_errors() {
    let source = "let bad = 1n 2n\nlet missing = fn truth => match truth with | true => 1n end";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);

    let partial = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .expect_err("the malformed call and incomplete match must not be accepted");

    // Each completed phase remains available, even though no accepted program
    // (and therefore no LIR) can be obtained from this result.
    assert_eq!(partial.ir.program.terms.len(), 2);
    assert_eq!(partial.inference.semantics().schemes().len(), 2);
    assert_eq!(partial.patterns.reports.len(), 1);
    assert!(!partial.inference.errors().is_empty(), "{partial:#?}");
    assert!(!partial.patterns.errors.is_empty(), "{partial:#?}");

    // Core compilation publishes the errors from every checking phase together
    // rather than making callers re-run a later phase to learn its failures.
    assert!(
        partial
            .errors
            .iter()
            .any(|error| matches!(error, Error::Inference(_)))
    );
    assert!(
        partial
            .errors
            .iter()
            .any(|error| matches!(error, Error::Patterns(_)))
    );
}

/// The core seam preserves rich errors independently of trace retention. Only
/// a debugger that asks for a complete trace receives the solver replay data.
#[test]
fn compilation_trace_retention_does_not_change_causal_errors() {
    let source = "let bad = 1n + \"text\"\n\
                  let inspect = fn value => match value with | { field } => field | {} => 0n end";
    let compile = |trace| {
        let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
        assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
        compile::compile(
            Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            trace,
        )
        .expect_err("the type mismatch must leave a partial compilation")
    };

    let off = compile(inference::Trace::Off);
    let complete = compile(inference::Trace::Complete);

    assert_eq!(
        off.inference.errors().len(),
        complete.inference.errors().len()
    );
    for (without_trace, with_trace) in off
        .inference
        .errors()
        .iter()
        .zip(complete.inference.errors())
    {
        assert_eq!(without_trace.explanation, with_trace.explanation);
        assert_eq!(
            without_trace.diagnostic(&off.ir.source).title,
            with_trace.diagnostic(&complete.ir.source).title
        );
    }
    assert!(
        off.inference
            .errors()
            .iter()
            .any(|error| error.explanation.is_some()),
        "the public compilation error keeps its causal explanation: {off:#?}"
    );

    let diagnostics = off.inference.diagnostics();
    assert_eq!(diagnostics.trace(), inference::Trace::Off);
    assert!(diagnostics.constraints().is_empty());
    assert!(diagnostics.steps().is_empty());
    assert!(diagnostics.reasons().is_empty());
    assert!(diagnostics.variables().is_empty());
    assert!(diagnostics.refinements().is_empty());

    let diagnostics = complete.inference.diagnostics();
    assert_eq!(diagnostics.trace(), inference::Trace::Complete);
    assert!(!diagnostics.constraints().is_empty());
    assert!(!diagnostics.steps().is_empty());
    assert!(!diagnostics.reasons().is_empty());
    assert!(!diagnostics.variables().is_empty());
    assert!(!diagnostics.refinements().is_empty());
}

/// Compile one source that every phase accepts.
fn accepted(source: &str) -> compile::AcceptedProgram {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{source}: {:#?}", partial.errors))
}

/// Compile one source some phase refuses, for the errors it publishes.
fn rejected(source: &str) -> compile::PartialCompilation {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .expect_err("the source is refused")
}

/// The published scheme of one top-level definition, printed.
fn scheme(accepted: &compile::AcceptedProgram, name: &str) -> String {
    accepted
        .semantics()
        .schemes()
        .iter()
        .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
        .map(|(_, scheme)| scheme.to_string())
        .unwrap_or_else(|| panic!("no definition named {name}"))
}

/// The codes of every error a refused compilation published, in order.
fn codes(partial: &compile::PartialCompilation) -> Vec<&'static str> {
    partial
        .errors
        .iter()
        .map(|error| match error {
            Error::Ir(error) => error.kind.code(),
            Error::Inference(error) => error.kind.code(),
            Error::Patterns(error) => error.kind.code(),
        })
        .collect()
}

/// Referring to an operation instantiates its effect's parameters afresh:
/// what the operation is applied to, and what its result is used as,
/// constrain the application the surrounding row carries; independent
/// occurrences of one effect coalesce when their arguments agree; and a
/// reusable operation value stays polymorphic.
#[test]
fn an_operation_reference_instantiates_its_effects_parameters() {
    let source = "effect Ask 'a = { get: () -> 'a }\n\
                  let num : () -> Nat + !Ask Nat = fn _ => !Ask.get ()\n\
                  let any = fn _ => !Ask.get ()\n\
                  let twice = fn _ => do let n = !Ask.get () let m = !Ask.get () return n + m end\n\
                  let get = !Ask.get\n\
                  let run = fn _ => handle !Ask.get () with | !Ask.get _ => 1n end\n\
                  let text = fn _ => handle !Ask.get () with | !Ask.get _ => \"s\" end";
    let accepted = accepted(source);
    assert_eq!(scheme(&accepted, "num"), "() -> Nat + !Ask Nat");
    assert_eq!(scheme(&accepted, "any"), "'a -> 'b + !Ask 'b");
    assert_eq!(scheme(&accepted, "twice"), "'a -> Real + !Ask Real");
    assert_eq!(scheme(&accepted, "get"), "() -> 'a + !Ask 'a");
    assert_eq!(scheme(&accepted, "run"), "'a -> Nat");
    assert_eq!(scheme(&accepted, "text"), "'a -> String");

    // Two occurrences whose arguments cannot agree are one computation using
    // incompatible versions of one effect, whichever branch each is on.
    for source in [
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad = fn _ => do let n : Nat = !Ask.get () let s : String = !Ask.get () end",
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad = fn b => if b then do let n : Nat = !Ask.get () end else do let s : String = !Ask.get () end end",
        "effect Ask 'a = { get: () -> 'a }\n\
         let bad : () -> Nat + !Ask String = fn _ => !Ask.get ()",
    ] {
        let partial = rejected(source);
        assert_eq!(codes(&partial), ["effect-argument-mismatch"], "{source}");
        let Some(Error::Inference(error)) = partial.errors.first() else {
            unreachable!()
        };
        assert!(
            matches!(
                &error.kind,
                inference::ErrorKind::EffectArgument { effect, position: 0, cause }
                    if effect == "Ask" && matches!(**cause, inference::ErrorKind::Mismatch { .. })
            ),
            "{source}: {:#?}",
            error.kind
        );
        assert!(
            error.explanation.is_some(),
            "{source}: the clash keeps its causal detail"
        );
    }
}

/// A block is typed as what its `return` carries, or as unit when it has
/// none, and it carries the effects of every statement in it: what a binding
/// performs is performed by the block, whether or not the value is kept.
#[test]
fn a_block_is_typed_by_its_return_and_carries_every_statements_effects() {
    let source = "effect Log = { write: Nat -> () }\n\
                  effect Tick = { tick: () -> () }\n\
                  let valued = fn _ => do let x = 1n let y = { v: x } return y end\n\
                  let unit = fn _ => do let x = 1n end\n\
                  let empty = do end\n\
                  let only = do return 1n end\n\
                  let ascribed = do let n : Nat = 1n return n end\n\
                  let effectful : () -> Nat + !Log + !Tick = fn _ => do let _ = !Log.write 1n let _ = !Tick.tick () return 2n end\n\
                  let inferred = fn _ => do let _ = !Log.write 1n let _ = !Tick.tick () return 2n end\n\
                  let quiet = fn _ => do let _ = !Log.write 1n end";
    let accepted = accepted(source);
    assert_eq!(scheme(&accepted, "valued"), "'a -> { v: Nat }");
    assert_eq!(scheme(&accepted, "unit"), "'a -> ()");
    assert_eq!(scheme(&accepted, "empty"), "()");
    assert_eq!(scheme(&accepted, "only"), "Nat");
    assert_eq!(scheme(&accepted, "ascribed"), "Nat");
    assert_eq!(scheme(&accepted, "effectful"), "() -> Nat + !Log + !Tick");
    assert_eq!(scheme(&accepted, "inferred"), "'a -> Nat + !Log + !Tick");
    assert_eq!(scheme(&accepted, "quiet"), "'a -> () + !Log");

    // An ascription inside a block is held to, the way a definition's is.
    let partial = rejected("let bad = do let n : Nat = \"s\" return n end");
    assert_eq!(codes(&partial), ["type-mismatch"]);
}

/// Compile one source to the artifact another compilation may depend on.
fn exported(source: &str) -> ruddy::artifact::UncheckedArtifact {
    accepted(source).artifact().to_unchecked()
}

/// Compile one source against a dependency, under the alias `dep`.
fn accepted_with(
    source: &str,
    dependency: &ruddy::artifact::UncheckedArtifact,
) -> compile::AcceptedProgram {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{source}: {:#?}", parsed.errors);
    compile::compile_with_dependencies(
        Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: compile::DependencyArtifact::Unchecked(dependency),
        }],
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{source}: {:#?}", partial.errors))
}

/// An imported effect keeps its parameters and generic interface, an imported
/// alias expands with the consumer's arguments, and a local declaration with
/// the same generic interface is the same effect whatever its parameters are
/// called.
#[test]
fn imported_effects_and_aliases_behave_like_local_declarations() {
    let dependency = exported(
        "effect Log = { write: Nat -> () }\n\
         effect Ask 'a = { get: () -> 'a }\n\
         effect Both 'a 'e = !Ask 'a + !Log + ..'e\n\
         let ask : () -> Nat + !Ask Nat = fn _ => !Ask.get ()",
    );
    let accepted = accepted_with(
        "effect IO = { print: Nat -> () }\n\
         effect Ask 'b = { get: () -> 'b }\n\
         let num : () -> Nat + dep::!Ask Nat = fn _ => dep::!Ask.get ()\n\
         let any = fn _ => dep::!Ask.get ()\n\
         let same : () -> Nat + !Ask Nat = fn _ => dep::!Ask.get ()\n\
         let both : () -> Nat + dep::!Both Nat (!IO) = fn _ => do let _ = !IO.print 1n return dep::!Ask.get () end\n\
         let open = fn _ => do let _ = dep::!Log.write 1n return dep::!Ask.get () end\n\
         let run = fn _ => handle dep::!Ask.get () with | dep::!Ask.get _ => 1n end",
        &dependency,
    );
    assert_eq!(scheme(&accepted, "num"), "() -> Nat + !Ask Nat");
    assert_eq!(scheme(&accepted, "any"), "'a -> 'b + !Ask 'b");
    assert_eq!(scheme(&accepted, "same"), "() -> Nat + !Ask Nat");
    assert_eq!(
        scheme(&accepted, "both"),
        "() -> Nat + !Ask Nat + !Log + !IO"
    );
    assert_eq!(scheme(&accepted, "open"), "'a -> 'b + !Log + !Ask 'b");
    assert_eq!(scheme(&accepted, "run"), "'a -> Nat");
    // An imported open alias splices the consumer's tail, and the effects
    // the alias supplies may not be handed back through it.
    let accepted = accepted_with(
        "effect IO = { print: Nat -> () }\n\
         let tail : () -> Nat + dep::!Both Nat (!IO + ..'e) = fn _ => dep::!Ask.get ()",
        &dependency,
    );
    assert_eq!(
        scheme(&accepted, "tail"),
        "() -> Nat + !Ask Nat + !Log + !IO + ..'a"
    );
    let partial = {
        let parsed = parse::parse(
            token::lex(
                "let clash : () -> Nat + dep::!Both Nat (dep::!Log) = fn _ => 0n\n\
                 let bad = fn _ => do let n : Nat = dep::!Ask.get () let s : String = dep::!Ask.get () end",
                FileID::GENERATED,
            )
            .tokens,
        );
        compile::compile_with_dependencies(
            Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Unchecked(&dependency),
            }],
            inference::Trace::Off,
        )
        .expect_err("the clashes are refused")
    };
    assert_eq!(
        codes(&partial),
        ["repeated-row-field", "effect-argument-mismatch"]
    );

    // Diagnostics involving an imported application are the local ones.
    let partial = {
        let parsed = parse::parse(
            token::lex(
                "let bad : () -> Nat + dep::!Ask = fn _ => 0n\n\
                 let worse : () -> Nat + dep::!Both Nat = fn _ => 0n",
                FileID::GENERATED,
            )
            .tokens,
        );
        compile::compile_with_dependencies(
            Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Unchecked(&dependency),
            }],
            inference::Trace::Off,
        )
        .expect_err("the applications are refused")
    };
    assert_eq!(codes(&partial), ["effect-arity", "effect-arity"]);
}

/// An alias may apply an effect to an array of its own parameter. The array
/// is substituted through like any other type, both when the alias is local
/// and when its body is read back from a dependency's artifact.
#[test]
fn alias_arguments_substitute_through_arrays_locally_and_when_imported() {
    let declarations = "effect Ask 'a = { get: () -> 'a }\n\
                        effect Many 'a = !Ask ['a]\n";
    let accepted = accepted(&format!(
        "{declarations}let f : () -> [Nat] + !Many Nat = fn _ => !Ask.get ()"
    ));
    assert_eq!(scheme(&accepted, "f"), "() -> [Nat] + !Ask [Nat]");

    let dependency = exported(declarations);
    let accepted = accepted_with(
        "let f : () -> [Nat] + dep::!Many Nat = fn _ => dep::!Ask.get ()",
        &dependency,
    );
    assert_eq!(scheme(&accepted, "f"), "() -> [Nat] + !Ask [Nat]");
}

/// A parameter standing for a row is instantiated as a fresh row: a fields
/// parameter from what an operation's struct carries beyond the fields the
/// declaration names, and an effects parameter from what a callback may
/// perform.
#[test]
fn row_parameters_are_inferred_from_their_uses() {
    let source = "effect Log = { write: Nat -> () }\n\
                  effect State 'r = { get: () -> { x: Nat, ..'r }, put: { x: Nat, ..'r } -> () }\n\
                  effect Run 'e = { run: (() -> () + ..'e) -> () }\n\
                  let read = fn _ => (!State.get ()).x\n\
                  let write = fn _ => !State.put { x: 1n, y: 2n }\n\
                  let both = fn _ => do let s = !State.get () return !State.put s end\n\
                  let run = fn _ => !Run.run (fn _ => !Log.write 1n)\n\
                  let pure = fn _ => !Run.run (fn _ => ())";
    let accepted = accepted(source);
    assert_eq!(scheme(&accepted, "read"), "'a -> Nat + !State { ..'b }");
    assert_eq!(scheme(&accepted, "write"), "'a -> () + !State { y: Nat }");
    assert_eq!(scheme(&accepted, "both"), "'a -> () + !State { ..'b }");
    // A callback's row that links nothing else is closed, as any lone effect
    // row is.
    assert_eq!(scheme(&accepted, "run"), "'a -> () + !Run (!Log)");
    assert_eq!(scheme(&accepted, "pure"), "'a -> () + !Run (|)");
}

/// A written row names a constructor once, whatever its arguments: repeating
/// one is a duplicate before any argument is compared, however the repeat is
/// written — plainly, absent, under a condition, or through aliases that reach
/// the same effect. An argument handed to an alias may not reintroduce what
/// the alias supplies.
#[test]
fn written_duplicates_are_refused_before_arguments_are_compared() {
    let base = "effect Log = { write: Nat -> () }\n\
                effect Ask 'a = { get: () -> 'a }\n\
                effect AskNat = !Ask Nat\n\
                effect AskText = !Ask String\n\
                effect Both 'a 'e = !Ask 'a + !Log + ..'e\n";
    for row in [
        "!Ask Nat + !Ask Nat",
        "!Ask Nat + !Ask String",
        "\\!Ask Nat + !Ask Nat + ..'e",
        "!Ask Nat (when 'p) + !Ask String + ..'e",
        "!AskNat + !AskText",
        "!Ask Nat + !AskText",
    ] {
        let partial = rejected(&format!("{base}let f : () -> Nat + {row} = fn _ => 0n"));
        assert_eq!(codes(&partial), ["duplicate-case"], "{row}");
    }
    for (row, code) in [
        ("!Both Nat (!Log)", "repeated-row-field"),
        ("!Both Nat (!Ask String)", "repeated-row-field"),
        ("!Both Nat (!Log + ..'e)", "repeated-row-field"),
    ] {
        let partial = rejected(&format!("{base}let f : () -> Nat + {row} = fn _ => 0n"));
        assert_eq!(codes(&partial), [code], "{row}");
    }
    // Two absent applications still name one application, so their
    // arguments have to agree.
    let partial = rejected(&format!(
        "{base}let f : (() -> Nat + \\!Ask Nat + ..'e) -> () -> Nat + \\!Ask String + ..'e = fn g => g"
    ));
    assert_eq!(codes(&partial), ["effect-argument-mismatch"]);
}

/// A handler infers one application from its computation and every arm of
/// the handled effect: arms that cannot agree are refused, the handled
/// application is removed, and unrelated effects survive.
#[test]
fn a_handler_infers_one_application_across_its_arms() {
    let base = "effect Log = { write: Nat -> () }\n\
                effect State 's = { get: () -> 's, put: 's -> () }\n";
    let accepted = accepted(&format!(
        "{base}let counted = fn _ => handle do let n = !State.get () let _ = !Log.write 1n return !State.put n end with\n\
             | !State.get _ => 0n\n\
             | !State.put _ => ()\n\
         end\n\
         let typed = fn _ => handle !State.put 1n with | !State.get _ => 2n | !State.put _ => () end"
    ));
    assert_eq!(scheme(&accepted, "counted"), "'a -> () + !Log");
    assert_eq!(scheme(&accepted, "typed"), "'a -> ()");
    // One arm decides the state is a natural number, the other a text: the
    // handler cannot implement both.
    let partial = rejected(&format!(
        "{base}let bad = fn _ => handle !State.put 1n with | !State.get _ => \"s\" | !State.put _ => () end"
    ));
    assert_eq!(codes(&partial), ["effect-argument-mismatch"]);
}

/// An extern may declare an applied effect, which its callers then perform.
#[test]
fn an_extern_may_declare_an_applied_effect() {
    let accepted = accepted(
        "effect Ask 'a = { get: () -> 'a }\n\
         extern host : () -> Nat + !Ask Nat = \"host\"\n\
         let use = fn _ => host ()",
    );
    let host = accepted
        .semantics()
        .externs()
        .iter()
        .find(|(symbol, _)| accepted.mint().name(**symbol) == "host")
        .map(|(_, scheme)| scheme.to_string())
        .expect("the extern is published");
    assert_eq!(host, "() -> Nat + !Ask Nat");
    assert_eq!(scheme(&accepted, "use"), "'a -> Nat + !Ask Nat");
}

/// A chain of aliases as long as a bundle cares to write, and an interface as
/// deep as an artifact cares to publish, are data rather than native frames:
/// both go through on a small stack.
#[test]
fn deep_alias_chains_and_generic_interfaces_use_bounded_stack() {
    std::thread::Builder::new()
        .name("deep-effect-aliases".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const LENGTH: usize = 2_000;
            let mut source =
                String::from("effect Log = { write: Nat -> () }\neffect A0 'e = ..'e\n");
            for at in 1..LENGTH {
                source.push_str(&format!("effect A{at} 'e = !A{} (..'e)\n", at - 1));
            }
            source.push_str(&format!(
                "let f : () -> Nat + !A{} (!Log) = fn _ => 0n",
                LENGTH - 1
            ));
            let accepted = accepted(&source);
            assert_eq!(scheme(&accepted, "f"), "() -> Nat + !Log");

            // And a published generic interface nested deeper than any
            // native walk should follow, imported and applied.
            const DEPTH: usize = 20_000;
            let mut result = ruddy::artifact::Type::Bound(0);
            for _ in 0..DEPTH {
                result = ruddy::artifact::Type::Arrow(
                    Box::new(ruddy::artifact::Type::Nat),
                    Box::new(result),
                    ruddy::artifact::Row {
                        labels: Vec::new(),
                        rest: ruddy::artifact::Rest::Closed,
                    },
                );
            }
            let dependency = ruddy::artifact::UncheckedArtifact {
                header: ruddy::artifact::Header {
                    compiler: ruddy::artifact::Stamp::current(),
                    modules: Vec::new(),
                    identity: ruddy::artifact::Identity {
                        name: "dep".into(),
                        version: "1.0.0".into(),
                    },
                    dependencies: Vec::new(),
                    values: Vec::new(),
                    types: Vec::new(),
                    effects: vec![ruddy::artifact::DeclaredEffect {
                        metadata: Default::default(),
                        name: "dep@1.0.0::Deep".into(),
                        params: vec![ruddy::artifact::Parameter {
                            sense: ruddy::artifact::Sense::Type,
                            lacks: Vec::new(),
                            relevant: true,
                        }],
                        identity: Some(ruddy::artifact::EffectIdentity {
                            name: "Deep".into(),
                            interface: "deep".into(),
                        }),
                        kind: ruddy::artifact::EffectKind::Operations(vec![
                            ruddy::artifact::Operation {
                                selector: ruddy::artifact::OperationSelector::Named("op".into()),
                                from: ruddy::artifact::Type::Bound(0),
                                to: result,
                            },
                        ]),
                    }],
                },
                lir: ruddy::artifact::Lir {
                    externs: Vec::new(),
                    functions: Vec::new(),
                    globals: Vec::new(),
                },
            };
            let accepted = accepted_with(
                "let g : () -> Nat + dep::!Deep Nat = fn _ => 0n",
                &dependency,
            );
            assert_eq!(scheme(&accepted, "g"), "() -> Nat + !Deep Nat");
        })
        .expect("the bounded-stack regression thread starts")
        .join()
        .expect("deep alias chains and generic interfaces use bounded stack");
}
