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

#[test]
fn discard_spellings_have_the_same_types_effects_and_definition_positions() {
    for discard in ["let _", "_"] {
        let source = format!(
            "effect Log = {{ write: Nat -> () }}
             {discard} = 1n
             {discard} : Nat = 2n
             module Nested =
               {discard} = 3n
               {discard} : Nat = 4n
               let value = 5n
             end
             let nested = Nested::value
             let logged : () -> Nat + !Log = fn _ => do
               {discard} = !Log.write 1n
               let _ = !Log.write 2n
               return 3n
             end
             let unit : () -> () + !Log = fn _ => do {discard} = !Log.write 4n end
             let mutated = fn _ => do
               let cell = mut 0n
               {discard} : Nat = cell := 5n
               return ~cell
             end"
        );
        let program = accepted(&source);
        assert_eq!(scheme(&program, "nested"), "Nat");
        assert_eq!(scheme(&program, "logged"), "() -> Nat + !Log");
        assert_eq!(scheme(&program, "unit"), "() -> () + !Log");
        assert_eq!(scheme(&program, "mutated"), "'a -> Nat");
    }
}

#[test]
fn discard_spellings_enforce_the_same_annotations_and_effect_allowances() {
    for discard in ["let _", "_"] {
        for statement in [
            format!("{discard} : Nat = true"),
            format!("module Nested = {discard} : Nat = true end"),
            format!("let local = do {discard} : Nat = true end"),
        ] {
            assert_eq!(codes(&rejected(&statement)), ["type-mismatch"]);
        }
        for (statement, expected) in [
            (format!("{discard} = !Log.write 1n"), "unhandled-effect"),
            (
                format!("module Nested = {discard} = !Log.write 1n end"),
                "unhandled-effect",
            ),
            (
                format!("let pure : () -> () = fn _ => do {discard} = !Log.write 1n end"),
                "effect-not-allowed",
            ),
        ] {
            let source = format!("effect Log = {{ write: Nat -> () }}\n{statement}");
            assert_eq!(codes(&rejected(&source)), [expected]);
        }
    }
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
                    kind: ruddy::artifact::Kind::Library,
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
                        exported: true,
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

#[test]
fn private_values_and_modules_are_bundle_local() {
    let producer = accepted(
        "@private let (secret, other) = (40n, 2n)\n\
         @private extern host : Nat -> Nat = \"x => x\"\n\
         @private module Hidden =\n\
           module Nested = let value = host secret end\n\
         end\n\
         module Visible = @private let hidden = other let shown = hidden end\n\
         let answer = (Hidden::Nested::value, Visible::shown)",
    );
    let header = producer.artifact().header();
    let names: Vec<_> = header
        .values
        .iter()
        .map(|value| value.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["tests@0.1.0::Visible::shown", "tests@0.1.0::answer"]
    );
    let modules: Vec<_> = header
        .modules
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    assert_eq!(modules, ["tests@0.1.0::Visible"]);
    let dependency = ruddy::artifact::Artifact::try_parse(&producer.artifact().print()).unwrap();
    accepted_with(
        "let answer = (dep::answer, dep::Visible::shown)",
        &dependency,
    );
    for path in [
        "secret",
        "other",
        "host",
        "Hidden::Nested::value",
        "Visible::hidden",
    ] {
        let source = format!("let stolen = dep::{path}");
        let parsed = parse::parse(token::lex(&source, FileID::GENERATED).tokens);
        let partial = compile::compile_with_dependencies(
            Mint::new(Bundle::new("consumer", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Unchecked(&dependency),
            }],
            inference::Trace::Off,
        )
        .expect_err("private names must not resolve in a dependent bundle");
        assert!(
            partial
                .errors
                .iter()
                .any(|error| matches!(error, Error::Ir(_))),
            "{partial:#?}"
        );
    }
}

#[test]
fn rethrowing_signatures_round_trip_with_their_callback_remainders() {
    let producer = accepted(include_str!("../bundles/rethrowing-imports/dependency.rud"));
    let persisted = ruddy::artifact::Artifact::try_parse(&producer.artifact().print()).unwrap();
    let consumer = accepted_with(
        "effect Log = { write: Nat -> () }
        effect Ask 'a = { get: () -> 'a }
        let residual = fn action =>
          handle dep::forward action with | !Log.write _ => () end
        let forward = fn action => dep::forward action
        let result : () -> Nat + !Ask Nat = fn _ => dep::silence (fn _ => do
          let _ = !Log.write 1n
          return !Ask.get ()
        end)",
        &persisted,
    );
    assert_eq!(
        scheme(&consumer, "residual"),
        "(() -> 'a + !Log + ..'b) -> 'a + ..'b"
    );
    assert_eq!(
        scheme(&consumer, "forward"),
        "(() -> 'a + !Log + ..'b) -> 'a + !Log + ..'b"
    );
    assert_eq!(scheme(&consumer, "result"), "() -> Nat + !Ask Nat");
}

#[test]
fn private_types_and_effects_support_public_structural_signatures() {
    let producer = accepted(
        "@private type Hidden = Nat\n\
         @private effect Secret = { get: () -> Hidden }\n\
         @private module Internal =\n\
           type Box 'a = { value: 'a }\n\
           type Chain = #End | #Next Chain\n\
           effect Ask 'a = { get: () -> Box 'a }\n\
           effect Alias 'a = !Ask 'a\n\
         end\n\
         type Visible = Hidden\n\
         type Box 'a = Internal::Box 'a\n\
         effect Ask 'a = Internal::!Alias 'a\n\
         effect Public = { echo: Hidden -> Internal::Box Nat }\n\
         let explicit : Hidden = 42n\n\
         let inferred = explicit\n\
         let chain : Internal::Chain = #Next #End\n\
         let ask : () -> Internal::Box Nat + Internal::!Ask Nat = fn _ => Internal::!Ask.get ()\n\
         let secret : () -> Hidden + !Secret = fn _ => !Secret.get ()",
    );
    let dependency = ruddy::artifact::Artifact::try_parse(&producer.artifact().print()).unwrap();
    let restored = dependency
        .clone()
        .validate()
        .expect("private semantic declarations survive serialization");
    assert_eq!(producer.artifact(), &restored);
    accepted_with(
        "effect Secret = { get: () -> Nat }\n\
         let explicit : Nat = dep::explicit\n\
         let inferred : dep::Visible = dep::inferred\n\
         let box : dep::Box Nat = { value: explicit }\n\
         let chain = dep::chain\n\
         let ask : () -> { value: Nat } + dep::!Ask Nat = dep::ask\n\
         let public = fn _ => dep::!Public.echo explicit\n\
         let run = fn _ => handle dep::secret () with | !Secret.get _ => 42n end",
        &dependency,
    );
    for source in [
        "let bad : dep::Hidden = 1n",
        "type Bad = dep::Internal::Box Nat",
        "let bad = fn _ => dep::!Secret.get ()",
        "effect Bad = dep::Internal::!Alias Nat",
    ] {
        let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
        assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
        let partial = compile::compile_with_dependencies(
            Mint::new(Bundle::new("consumer", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Unchecked(&dependency),
            }],
            inference::Trace::Off,
        )
        .expect_err("private semantic declarations must not enter source lookup");
        assert!(
            partial
                .errors
                .iter()
                .any(|error| matches!(error, Error::Ir(_))),
            "{partial:#?}"
        );
    }
}

#[test]
fn private_attribute_requires_unit_on_every_declaration_kind() {
    for declaration in [
        "let value = 1n",
        "let (a, b) = (1n, 2n)",
        "let _ = 1n",
        "extern value : Nat = \"1n\"",
        "type Value = Nat",
        "effect Read = () -> Nat",
        "module Hidden = let value = 1n end",
    ] {
        for unit in ["", "()", "{}"] {
            accepted(&format!("@private {unit} {declaration}"));
        }
        for payload in [
            "false",
            "true",
            "1n",
            "1i",
            "1.0",
            "\"reason\"",
            "[]",
            "#Flag",
            "{ x: () }",
        ] {
            let source = format!("@private {payload} {declaration}");
            let partial = rejected(&source);
            assert_eq!(codes(&partial), ["invalid-private-value"], "{source}");
            let diagnostic = partial.ir.errors[0].diagnostic(&partial.ir.source);
            assert_eq!(diagnostic.title, "`@private` requires unit");
            assert_eq!(diagnostic.primary.span.start, 9);
            assert_eq!(diagnostic.primary.span.width, payload.len());
        }
    }
}

#[test]
fn private_semantic_support_survives_transitive_public_aliases() {
    let producer = accepted(
        "@private type Hidden = Nat\n\
         @private effect Read = { get: () -> Hidden }\n\
         let value : Hidden = 42n\n\
         let read : () -> Hidden + !Read = fn _ => !Read.get ()",
    );
    let middle = accepted_with(
        "let value = dep::value\nlet read = dep::read",
        &producer.artifact().to_unchecked(),
    );
    let middle = ruddy::artifact::Artifact::try_parse(&middle.artifact().print())
        .unwrap()
        .validate()
        .unwrap();
    let source = "effect Read = { get: () -> Nat }\n\
                  let value : Nat = middle::value\n\
                  let read = fn _ => handle middle::read () with | !Read.get _ => value end";
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty(), "{:#?}", parsed.errors);
    compile::compile_with_dependencies(
        Mint::new(Bundle::new("consumer", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        &[
            compile::Dependency {
                alias: None,
                artifact: compile::DependencyArtifact::Checked(producer.artifact()),
            },
            compile::Dependency {
                alias: Some("middle"),
                artifact: compile::DependencyArtifact::Checked(&middle),
            },
        ],
        inference::Trace::Off,
    )
    .unwrap_or_else(|partial| panic!("{:#?}", partial.errors));
}

#[test]
fn foreign_protocols_and_sync_export_eligibility_are_checked_during_compilation() {
    let pure = accepted("@export \"sync\" let identity = fn n => n");
    assert_eq!(
        pure.artifact().lir().globals[0].callable,
        Some(ruddy::lir::Suspension::Synchronous)
    );
    for source in [
        "@export \"sync\" let apply = fn f => f ()",
        "@async extern wait : Nat -> Nat = \"host.wait\"\n@export \"sync\" let run = fn n => wait n",
        "@async extern value : Nat = \"host.value\"",
        "@export \"sometimes\" let identity = fn n => n",
    ] {
        let partial = rejected(source);
        assert!(
            codes(&partial).contains(&"foreign-protocol"),
            "{source}: {partial:#?}"
        );
        assert!(
            !partial.ir.errors[0]
                .diagnostic(&partial.ir.source)
                .title
                .is_empty()
        );
    }
    accepted("@export \"promise\" let identity = fn n => n");
}

#[test]
fn async_boundary_annotations_are_local_and_preserve_outer_shorthand() {
    for source in [
        "@async extern wait : fn(Nat) -> Nat = \"host.wait\"",
        "extern wait : @async fn(Nat) -> Nat = \"host.wait\"",
        "extern wait : (@async fn(Nat) -> Nat) = \"host.wait\"",
        "extern wait : @async (fn(Nat) -> Nat) = \"host.wait\"",
        "@async extern wait : @async fn(Nat) -> Nat = \"host.wait\"",
        "type Wait = Nat -> Nat\nextern wait : @async Wait = \"host.wait\"",
    ] {
        let program = accepted(&format!("{source}\nlet run = fn n => wait n"));
        assert_eq!(
            program.artifact().lir().globals[0].callable,
            Some(ruddy::lir::Suspension::MaySuspend)
        );
    }
    for source in [
        "extern apply : fn(@async fn(Nat) -> Nat) -> Nat = \"host.apply\"",
        "extern factory : fn() -> @async fn(Nat) -> Nat = \"host.factory\"",
        "extern factory : fn(fn() -> @async fn(Nat) -> Nat) -> () = \"host.factory\"",
    ] {
        accepted(source);
    }
    // An async returned function does not make its factory async.
    accepted(
        "extern factory : fn() -> @async fn(Nat) -> Nat = \"host.factory\"\n@export \"sync\" let run = fn _ => factory ()",
    );
    accepted("extern call : Nat -> Nat = \"host.call\"\n@export \"sync\" let run = fn n => call n");
}

#[test]
fn invalid_boundary_metadata_is_rejected() {
    for source in [
        "@async false extern call : Nat -> Nat = \"host.call\"",
        "extern call : @async false fn(Nat) -> Nat = \"host.call\"",
        "extern call : fn(@async Nat) -> Nat = \"host.call\"",
        "extern call : fn(Nat) -> @async Nat = \"host.call\"",
        "type Record = { value: Nat }\nextern call : @async Record = \"host.call\"",
        "extern call : fn(@unknown fn(Nat) -> Nat) -> Nat = \"host.call\"",
        "extern call : fn(Nat) -> @encoding \"utf8\" String = \"host.call\"",
    ] {
        assert!(
            codes(&rejected(source)).contains(&"foreign-protocol"),
            "{source}"
        );
    }
    let duplicate = rejected("extern call : @async @async fn(Nat) -> Nat = \"host.call\"");
    assert!(
        duplicate
            .ir
            .errors
            .iter()
            .any(|e| matches!(e.kind, ruddy::ir::ErrorKind::DuplicateAttribute { .. }))
    );
}

#[test]
fn imported_callable_summaries_control_sync_export_eligibility_before_linking() {
    let source = "@export \"sync\" let run = fn n => dep::identity n";
    let mut dependency = exported("let identity = fn n => n");
    let persisted =
        ruddy::artifact::Artifact::try_parse(&dependency.clone().validate().unwrap().print())
            .unwrap();
    let consumer = accepted_with(source, &persisted);
    assert_eq!(
        consumer.artifact().lir().globals[0].callable,
        Some(ruddy::lir::Suspension::Synchronous)
    );
    // A valid producer can omit its callable proof. The consumer must
    // remain conservative even though the imported function has an empty row.
    dependency.lir.globals[0].callable = None;
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    let partial = compile::compile_with_dependencies(
        Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        &[compile::Dependency {
            alias: Some("dep"),
            artifact: compile::DependencyArtifact::Unchecked(&dependency),
        }],
        inference::Trace::Off,
    )
    .expect_err("missing imported proof is potentially suspending");
    assert!(codes(&partial).contains(&"foreign-protocol"));
}

#[test]
fn local_mutation_is_isolated_at_a_function_body() {
    let program = accepted(
        "let counter = fn _ => do let cell = mut 0n let alias = cell let _ = alias := 2n return ~cell end",
    );
    assert_eq!(scheme(&program, "counter"), "'a -> Nat");
    let _ = program.artifact();
}

#[test]
fn mutation_signatures_forward_region_kinds_through_aliases() {
    let program = accepted(
        "type cell 'r = mut 'r Nat
        effect state 'r = !mut 'r
        let make: Nat -> cell 'r + !state 'r = fn x => mut x
        let read: cell 'r -> Nat + !state 'r = fn cell => ~cell",
    );
    assert!(scheme(&program, "make").contains("!mut"));
    assert!(scheme(&program, "read").contains("!mut"));
    let artifact = program.artifact().to_unchecked();
    let consumer = accepted_with("let run = fn _ => dep::read (dep::make 3n)", &artifact);
    assert_eq!(scheme(&consumer, "run"), "'a -> Nat");
}

#[test]
fn mutation_rejects_wrong_region_kinds_and_incompatible_writes() {
    for source in [
        "type cell 'r = mut 'r Nat type bad = cell Nat",
        "type bad 'r = (mut 'r Nat, 'r)",
        "type bad = mut Nat Nat",
        "let bad = fn _ => do let cell = mut (fn x => x) let alias = cell let _ = (~cell) 1n return (~alias) true end",
        "let bad = fn _ => do let cell = mut [] let alias = cell let _ = cell := [1n] return alias := [true] end",
        "let bad: mut 'r Nat -> Nat = fn cell => ~cell",
        "let bad: (mut 'r Nat, mut 's Nat) -> Nat + !mut 'r = fn pair => pair.0 := ~pair.1",
    ] {
        let partial = rejected(source);
        assert!(!partial.errors.is_empty(), "{source}");
    }
}

#[test]
fn mutation_isolation_respects_pure_annotations_and_other_effects() {
    let program = accepted("effect Log = { write: Nat -> () }
        let local: () -> Nat = fn _ => do let cell = mut 1n return cell := 2n end
        let logged: () -> Nat + !Log = fn _ => do let cell = mut 1n let _ = !Log.write (~cell) return ~cell end
        let factory = fn x => mut x
        let uses = fn _ => do let a = factory 1n let b = factory true return (~a, ~b) end");
    assert_eq!(scheme(&program, "local"), "() -> Nat");
    assert_eq!(scheme(&program, "logged"), "() -> Nat + !Log");
    assert_eq!(scheme(&program, "uses"), "'a -> (Nat, Boolean)");
}

#[test]
fn mutation_value_restriction_preserves_pure_factories_and_shared_unknowns() {
    let program = accepted(
        "let factory = fn x => mut x
      let run = fn _ => do
        let number = factory 1n
        let truth = factory true
        let pure = (fn _ => fn x => x) ()
        return (pure (~number), pure (~truth))
      end",
    );
    assert_eq!(scheme(&program, "run"), "'a -> (Nat, Boolean)");
    for source in [
        "let bad = fn _ => do let box = { cell: mut (fn x => x) } let { cell: alias } = box let _ = (~box.cell) 1n return (~alias) true end",
        "let bad = fn _ => do let cell = mut (fn x => x) let use = fn x => (~cell) x let _ = use 1n return use true end",
        "effect Get 'a = { get: () -> ('a -> 'a) } let bad = fn _ => do let id = !Get.get () let alias = id let _ = id 1n return alias true end",
        "let global = mut 0n",
    ] {
        assert!(!rejected(source).errors.is_empty(), "{source}");
    }
}

#[test]
fn mutation_regions_remain_visible_through_residual_effects_and_captures() {
    let program = accepted(
        "effect Export 'r = { send: mut 'r Nat -> () }
      let export_cell = fn _ => do let cell = mut 0n return !Export.send cell end
      let capture = fn cell => fn _ => ~cell
      let copy = fn target => fn source => target := ~source
      let local = fn _ => do let cell = mut 2n return ~cell end
      let use = fn cell => cell := local ()",
    );
    assert!(scheme(&program, "export_cell").contains("!mut"));
    assert!(scheme(&program, "export_cell").contains("!Export"));
    assert!(scheme(&program, "capture").contains("!mut"));
    assert!(scheme(&program, "copy").contains("!mut"));
    assert!(scheme(&program, "use").contains("!mut"));
}

#[test]
fn mutation_syntax_rejects_incomplete_forms_and_user_handlers() {
    for source in [
        "let x = mut",
        "let x = ~",
        "let x = fn cell => cell :=",
        "type Cell = mut Nat",
        "effect State = !mut",
        "effect State 'r = !mut 'r Nat",
        "let bad = fn _ => do let cell = mut 0n cell := 1n return ~cell end",
        "effect mut 'r = Nat -> Nat",
        "let bad = fn cell => handle ~cell with | !mut value => value end",
    ] {
        let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
        if parsed.errors.is_empty() {
            rejected(source);
        }
    }
}

#[test]
fn mutation_identity_cannot_be_forged_by_an_imported_handler() {
    let mut dependency = exported("effect Evil 'r = { fake: mut 'r Nat -> Nat }");
    let builtin = dependency
        .header
        .effects
        .iter()
        .find(|effect| effect.name.ends_with("::mut"))
        .unwrap()
        .identity
        .clone();
    dependency
        .header
        .effects
        .iter_mut()
        .find(|effect| effect.name.ends_with("::Evil"))
        .unwrap()
        .identity = builtin;
    assert!(dependency.clone().validate().is_err());
    let parsed = parse::parse(token::lex(
        "let bad: mut 'r Nat -> Nat = fn cell => handle ~cell with | dep::!Evil.fake _ => 0n end",
        FileID::GENERATED,
    ).tokens);
    assert!(parsed.errors.is_empty());
    assert!(
        compile::compile_with_dependencies(
            Mint::new(Bundle::new("app", Version::new(0, 1, 0)).unwrap()),
            parsed.stmts,
            &[compile::Dependency {
                alias: Some("dep"),
                artifact: compile::DependencyArtifact::Unchecked(&dependency)
            }],
            inference::Trace::Off,
        )
        .is_err(),
        "recovery cannot expose a handler for builtin mutation"
    );
}

#[test]
fn mutation_region_kinds_forward_from_imported_effects() {
    let dependency = exported("effect Access 'r = { read: mut 'r Nat -> Nat }");
    let direct = accepted_with(
        "let run: mut 'r Nat -> Nat + dep::!Access 'r = fn cell => dep::!Access.read cell",
        &dependency,
    );
    direct.artifact().to_unchecked().validate().unwrap();
    let program = accepted_with(
        "effect Copy 'r = dep::!Access 'r
        type Reader 'r = () -> Nat + dep::!Access 'r
        let run: mut 'r Nat -> Nat + !Copy 'r = fn cell => dep::!Access.read cell",
        &dependency,
    );
    let artifact = program.artifact();
    for declaration in &artifact.header().types {
        assert_eq!(declaration.params[0].sense, ruddy::artifact::Sense::Region);
        assert!(declaration.params[0].relevant);
    }
    let copy = artifact
        .header()
        .effects
        .iter()
        .find(|effect| effect.name.ends_with("::Copy"))
        .unwrap();
    assert_eq!(copy.params[0].sense, ruddy::artifact::Sense::Region);
}

#[test]
fn reification_callable_interfaces_separate_initialization_and_curried_demands() {
    use ruddy::reification::conventions::Shape;
    let source = r#"
extern box: 'a -> Any = "$anyUpcast"
let pair = fn first => fn second => (box first, box second)
let partial = pair 1n
let erased = fn value => value
let apply = fn callback => fn value => callback value
let dynamic = apply box
let pure = apply (fn value => value)
let token = box 1n
let select: ('a -> Any) -> ('a -> Any) -> ('a -> Any) = fn first => fn second => second
let use_selector: (('a -> Any) -> ('a -> Any) -> ('a -> Any)) -> ('a -> Any) =
  fn selector => selector box (fn _ => token)
let selected = use_selector select
let first: 'a -> 'a -> 'a = fn left => fn right => left
let retained = first (fn _ => token) box
"#;
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    let accepted = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|failed| panic!("{:?}", failed.errors));
    let plan = &accepted.reification().callables;
    let requirements = plan.graph.solve();
    let binding = |name| {
        plan.bindings
            .iter()
            .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
            .map(|(symbol, binding)| (*symbol, binding))
            .unwrap()
    };
    let (_, pair) = binding("pair");
    let Shape::Arrow {
        needs: first,
        result,
        ..
    } = plan.graph.exposed(pair.value)
    else {
        panic!("pair function")
    };
    let Shape::Arrow { needs: second, .. } = plan.graph.exposed(*result) else {
        panic!("curried result")
    };
    assert_eq!(requirements[*first as usize].len(), 1);
    assert_eq!(requirements[*second as usize].len(), 1);
    assert_ne!(
        requirements[*first as usize],
        requirements[*second as usize]
    );
    for name in ["erased", "apply"] {
        let (_, binding) = binding(name);
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("function")
        };
        assert!(
            requirements[*needs as usize].is_empty(),
            "{name} needs no evidence on its first invocation"
        );
    }
    let (_, apply) = binding("apply");
    let Shape::Arrow { result, .. } = plan.graph.exposed(apply.value) else {
        unreachable!()
    };
    let Shape::Arrow { needs, .. } = plan.graph.exposed(*result) else {
        unreachable!()
    };
    assert!(
        requirements[*needs as usize]
            .iter()
            .all(|need| need.port.is_some())
    );
    assert!(
        !requirements[*needs as usize].is_empty(),
        "higher-order call needs remain quantified"
    );
    for (name, demanded) in [
        ("dynamic", true),
        ("pure", false),
        ("selected", false),
        ("retained", false),
    ] {
        let (symbol, binding) = binding(name);
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("returned function")
        };
        assert_eq!(
            !requirements[*needs as usize].is_empty(),
            demanded,
            "{name}: {:?}",
            requirements[*needs as usize]
        );
        let initializer = &accepted.semantics().typed()[&symbol].value;
        assert!(requirements[plan.occurrences[&initializer.at].evaluation as usize].is_empty());
    }
    let (symbol, partial) = binding("partial");
    let initializer = &accepted.semantics().typed()[&symbol].value;
    let flow = plan.occurrences[&initializer.at];
    assert!(
        requirements[flow.evaluation as usize].is_empty(),
        "constructing a partial application does not require its future argument's type"
    );
    let Shape::Arrow { needs, .. } = plan.graph.exposed(partial.value) else {
        panic!("partial function")
    };
    assert_eq!(requirements[*needs as usize].len(), 1);
}

#[test]
fn reification_callable_interfaces_solve_polymorphic_recursive_groups() {
    use ruddy::reification::conventions::Shape;
    let source = r#"
extern box: 'a -> Any = "$anyUpcast"
let left: 'a -> Any = fn value => right [value]
let right: 'a -> Any = fn value => match true with
  | true => box value
  | false => left [value]
end
"#;
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    let accepted = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|failed| panic!("{:?}", failed.errors));
    let plan = &accepted.reification().callables;
    let requirements = plan.graph.solve();
    for name in ["left", "right"] {
        let (_, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
            .unwrap();
        let Shape::Arrow {
            argument, needs, ..
        } = plan.graph.exposed(binding.value)
        else {
            panic!("recursive function")
        };
        assert!(
            matches!(
                plan.graph.exposed(*argument),
                Shape::Parameter(_) | Shape::Sealed
            ),
            "{name}: recursive instantiation must not change the declared argument shape"
        );
        assert_eq!(
            requirements[*needs as usize].len(),
            1,
            "{name}: recursive evidence reaches every member"
        );
        assert!(
            requirements[*needs as usize]
                .iter()
                .all(|need| need.port.is_none()),
            "{name}: known boxing needs are mandatory"
        );
    }
}

#[test]
fn reification_callable_interfaces_defer_native_returned_functions() {
    use ruddy::reification::conventions::Shape;
    let source = r#"
extern make: () -> ('a -> 'a) = "() => value => value"
extern nested: () -> { functions: ['a -> 'a] } = "() => ({ functions: [value => value] })"
let direct = make ()
let indirect = nested ()
"#;
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    let accepted = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|failed| panic!("{:?}", failed.errors));
    let plan = &accepted.reification().callables;
    let requirements = plan.graph.solve();
    for name in ["make", "nested"] {
        let (_, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
            .unwrap();
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("foreign function")
        };
        assert!(
            requirements[*needs as usize].is_empty(),
            "{name}: obtaining a function does not call it"
        );
    }
    for name in ["direct", "indirect"] {
        let (symbol, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
            .unwrap();
        let initializer = &accepted.semantics().typed()[symbol].value;
        assert!(
            requirements[plan.occurrences[&initializer.at].evaluation as usize].is_empty(),
            "{name}: initialization needs no future argument descriptor"
        );
        let value = if name == "indirect" {
            let Shape::Record(fields) = plan.graph.exposed(binding.value) else {
                panic!("returned record")
            };
            let Shape::Array(element) = plan.graph.exposed(fields["functions"]) else {
                panic!("returned array")
            };
            *element
        } else {
            binding.value
        };
        let Shape::Arrow { needs, .. } = plan.graph.exposed(value) else {
            panic!("returned function")
        };
        assert_eq!(
            requirements[*needs as usize].len(),
            1,
            "{name}: native invocation converts its argument and result"
        );
        assert!(
            requirements[*needs as usize]
                .iter()
                .all(|need| need.port.is_none())
        );
    }
}

#[test]
fn reification_callable_interfaces_follow_array_values_and_evaluation() {
    use ruddy::reification::conventions::Shape;
    let source = r#"
extern box: 'a -> Any = "$anyUpcast"
let array_box = fn value => [box value]
let through_spread = fn value => [..array_box value]
let return_rest = fn callbacks => match callbacks with
  | [_, ..rest] => rest
  | [] => callbacks
end
let retained = return_rest [fn value => box value, box]
"#;
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    let accepted = compile::compile(
        Mint::new(Bundle::new("tests", Version::new(0, 1, 0)).unwrap()),
        parsed.stmts,
        inference::Trace::Off,
    )
    .unwrap_or_else(|failed| panic!("{:?}", failed.errors));
    let plan = &accepted.reification().callables;
    let requirements = plan.graph.solve();
    for name in ["array_box", "through_spread"] {
        let (_, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| accepted.mint().name(**symbol) == name)
            .unwrap();
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("function")
        };
        assert_eq!(
            requirements[*needs as usize].len(),
            1,
            "{name}: array construction executes its elements and spreads"
        );
    }
    let (_, binding) = plan
        .bindings
        .iter()
        .find(|(symbol, _)| accepted.mint().name(**symbol) == "retained")
        .unwrap();
    let Shape::Array(element) = plan.graph.exposed(binding.value) else {
        panic!("array result")
    };
    let Shape::Arrow { needs, .. } = plan.graph.exposed(*element) else {
        panic!("callable elements")
    };
    assert_eq!(
        requirements[*needs as usize].len(),
        1,
        "rest patterns preserve callable elements"
    );
    assert!(
        requirements[*needs as usize]
            .iter()
            .all(|need| need.port.is_none())
    );
}

#[test]
fn reification_callable_interfaces_survive_separate_compilation() {
    use ruddy::reification::conventions::Shape;
    let producer = exported(
        r#"
extern box: 'a -> Any = "$anyUpcast"
let apply = fn callback => fn value => callback value
let identity = fn value => value
"#,
    );
    let imported = accepted_with(
        r#"
let dynamic = dep::apply dep::box
let erased = dep::apply (fn value => value)
let forwarded = dep::identity dep::box
"#,
        &producer,
    );
    let plan = &imported.reification().callables;
    let solved = plan.graph.solve();
    for (name, demanded) in [("dynamic", true), ("erased", false), ("forwarded", true)] {
        let (_, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| imported.mint().name(**symbol) == name)
            .unwrap();
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("imported callable result")
        };
        assert_eq!(
            !solved[*needs as usize].is_empty(),
            demanded,
            "{name}: imports preserve higher-order requirements and value forwarding"
        );
        assert!(
            solved[*needs as usize]
                .iter()
                .all(|need| need.port.is_none())
        );
    }
}

#[test]
fn reification_recursive_callable_interfaces_preserve_forwarded_profiles() {
    use ruddy::reification::conventions::Shape;
    let program = accepted(
        r#"
extern box: 'a -> Any = "$anyUpcast"
let token = box 1n
let left: ('a -> Any) -> ('a -> Any) = fn callback => right callback
let right: ('a -> Any) -> ('a -> Any) = fn callback => match true with
  | true => callback
  | false => left callback
end
let erased = left (fn _ => token)
let dynamic = left box
"#,
    );
    let plan = &program.reification().callables;
    let solved = plan.graph.solve();
    for (name, demanded) in [("erased", false), ("dynamic", true)] {
        let (_, binding) = plan
            .bindings
            .iter()
            .find(|(symbol, _)| program.mint().name(**symbol) == name)
            .unwrap();
        let Shape::Arrow { needs, .. } = plan.graph.exposed(binding.value) else {
            panic!("forwarded callback")
        };
        assert_eq!(
            !solved[*needs as usize].is_empty(),
            demanded,
            "{name}: mutually recursive forwarding preserves the supplied profile"
        );
        assert!(
            solved[*needs as usize]
                .iter()
                .all(|need| need.port.is_none())
        );
    }
}

#[test]
fn reification_graphs_and_forwarding_use_bounded_stack_and_artifact_space() {
    std::thread::Builder::new().name("runtime-type-graphs".into()).stack_size(256 * 1024)
        .spawn(|| {
            let mut source = String::from("@private extern box: 'a -> Any = \"$anyUpcast\"\nlet apply = fn call value => call value\nlet forward0 = apply box\n");
            for at in 1..128 {
                source.push_str(&format!("let forward{at} = fn value => forward{} value\n", at - 1));
            }
            source.push_str("type Recursive 'a = #End | #Next ('a, Recursive 'a)\nlet recursive: Recursive Nat = #Next (1n, #End)\nlet token = forward127 recursive\n");
            let program = accepted(&source);
            let artifact = program.artifact();
            let printed = artifact.print();
            assert!(printed.len() < 4_000_000, "finite forwarding must not expand source bodies");
            assert!(ruddy::artifact::parse(&printed).validate().is_ok());
        }).unwrap().join().expect("finite descriptor and callable graphs use bounded stack");
}

#[test]
fn reification_shared_generic_aliases_do_not_expand_unused_callable_fields() {
    let mut source = String::from("type N0 'a = 'a -> 'a\n");
    for level in 1..=24 {
        source.push_str(&format!(
            "type N{level} 'a = {{left: N{} 'a, right: N{} 'a}}\n",
            level - 1,
            level - 1
        ));
    }
    source.push_str("@private let ignore: N24 'a -> () = fn _ => ()\nlet result = ()\n");
    source.push_str("let forward: N24 'a -> N24 'a = fn value => value\n");
    source.push_str(&format!(
        "let select: N24 'a -> ('a -> 'a) = fn value => value{}\n",
        ".left".repeat(24)
    ));
    source.push_str("extern native: () -> N24 'a = \"() => ({})\"\n");
    let program = accepted(&source);
    assert!(
        program.reification().callables.graph.shapes.len() < 1000,
        "unused generic alias DAGs must not expand into callable trees"
    );
    assert!(
        ruddy::artifact::parse(&program.artifact().print())
            .validate()
            .is_ok()
    );
}

#[test]
fn using_module_aliases_are_hoisted_and_do_not_add_exports() {
    let program = accepted(
        "let answer = short::value\nusing Source as short\nmodule Source = let value = 42n end",
    );
    let header = program.artifact().header();
    assert_eq!(header.values.len(), 2);
    assert!(
        header
            .values
            .iter()
            .any(|value| value.name.ends_with("::answer"))
    );
    assert!(
        header
            .values
            .iter()
            .all(|value| !value.name.contains("::short"))
    );
}

#[test]
fn using_groups_import_all_namespaces_and_preserve_public_signatures() {
    let producer = accepted(
        "module Source =
           @private type Count = Nat
           effect Ask = { get: () -> Count }
           let value: Count = 42n
           module Child = let flag = true end
         end
         using Source::{self as source, Count as Number, Ask as Query, value, Child::{self as child, *}}
         let answer: Number = value
         let query: () -> Number + !Query = fn _ => !Query.get ()
         let nested = (flag, child::flag, source::Child::flag)",
    );
    assert_eq!(scheme(&producer, "answer"), "Count");
    let header = producer.artifact().header();
    assert!(
        header
            .types
            .iter()
            .all(|item| !item.name.ends_with("::Number"))
    );
    assert!(
        header
            .effects
            .iter()
            .all(|item| !item.name.ends_with("::Query"))
    );
    assert!(
        header
            .modules
            .iter()
            .all(|item| !item.name.ends_with("::child") && !item.name.ends_with("::source"))
    );
    let artifact = ruddy::artifact::Artifact::try_parse(&producer.artifact().print()).unwrap();
    let consumer = accepted_with(
        "using dep::{answer, query}\nlet result: Nat = answer\nlet ask = query",
        &artifact,
    );
    assert_eq!(scheme(&consumer, "result"), "Nat");
}

#[test]
fn using_local_imports_are_sequential_nested_and_do_not_escape() {
    let program = accepted(
        "module Source = let value = true type T = Boolean end
         let value = 1n
         let result = do
           let before = value
           using Source::{value, T}
           let after: T = value
           let nested = do using bundle as root return root::value end
           return (before, after, nested)
         end
         let outside = value
         let same_module = do using self::* return value end",
    );
    assert_eq!(scheme(&program, "outside"), "Nat");
    assert_eq!(scheme(&program, "same_module"), "Nat");
    assert_eq!(scheme(&program, "result"), "(Nat, T, Nat)");
    for source in [
        "module M = let x = 1n end let y = do let z = x using M::x return z end",
        "module M = let x = 1n end let y = do using M::x return x end let z = x",
        "module M = let x = 1n end let y = do using later::x using M as later return x end",
    ] {
        assert!(!rejected(source).ir.errors.is_empty(), "{source}");
    }
}

#[test]
fn using_alias_dependencies_resolve_forward_and_cycles_are_rejected() {
    let program = accepted(
        "using short::value as answer
         using middle as short
         using Source as middle
         module Source = let value = true end
         module Child = let inherited = answer end
         let result = Child::inherited",
    );
    assert_eq!(scheme(&program, "result"), "Boolean");
    for source in [
        "using a as b using b as a",
        "module Seed = module x = module y = module y = module y = let v = 3n end let v = 2n end let v = 1n end end end using Seed::* using x::y as x let got = x::v",
        "module A = end module Child = using B as A using A as B end",
        "using Missing::value",
        "using Missing::{}",
    ] {
        assert!(codes(&rejected(source)).contains(&"using"), "{source}");
    }
}

#[test]
fn using_conflicts_are_namespace_sensitive_and_globs_are_lazy() {
    for source in [
        "module A = let x = 1n end module B = let x = true end using A::* using B::* let ok = 2n",
        "module A = let x = 1n end using A::* using A::* let y = x",
        "module A = let x = 1n end module B = let x = true end using A::* using B::* let x = false let y = x",
        "module A = let x = 1n end module B = let x = true end using A::* using B::x let y = x",
        "module A = let X = 1n end type X = Nat using A::X let y: X = X",
        "module A = let x = 1n end using A::x let y = do using A::x return x end",
        "module A = let x = 1n end let y = do using A::* let x = true return x end",
    ] {
        accepted(source);
    }
    for (source, code) in [
        (
            "module A = let x = 1n end module B = let x = true end using A::* using B::* let y = x",
            "using",
        ),
        (
            "module A = let x = 1n end using A::x using A::x",
            "duplicate-term",
        ),
        (
            "module A = let x = 1n end using A::x let x = true",
            "duplicate-term",
        ),
        (
            "module A = module B = end end using A::B module B = end",
            "duplicate-module",
        ),
        (
            "module A = let x = 1n end let y = do using A::x let x = true return x end",
            "duplicate-term",
        ),
        (
            "module A = let x = 1n end let y = do let x = true using A::x return x end",
            "using",
        ),
        (
            "module A = let x = 1n end let y = do using A::x using A::x return x end",
            "duplicate-term",
        ),
    ] {
        assert!(codes(&rejected(source)).contains(&code), "{source}");
    }
}

#[test]
fn using_bindings_never_become_qualified_members_or_transitive_globs() {
    for source in [
        "module B = let x = 1n end module A = using B::x end let y = A::x",
        "module B = let x = 1n end module A = using B::x end using A::* let y = x",
        "module B = end module A = using B as C end let y = A::C::x",
    ] {
        assert!(!rejected(source).ir.errors.is_empty(), "{source}");
    }
}

#[test]
fn using_path_anchors_resolve_values_types_effects_and_module_aliases() {
    let program = accepted(
        "type T = Nat effect E = { get: () -> T } let x = 1n
         module Outer =
           let x = true
           module Inner =
             using bundle as root
             using super as parent
             using self as here
             using root::{T, E}
             let a: bundle::T = bundle::x
             let b = super::x
             let c = super::super::x
             let d = parent::x
             let e = here::a
             let f: () -> root::T + bundle::!E = fn _ => root::!E.get ()
           end
         end
         let result = Outer::Inner::a",
    );
    assert_eq!(scheme(&program, "result"), "T");
    for source in [
        "using bundle",
        "using self",
        "using super",
        "using self::*",
        "let x = super::x",
        "module A = let x = super::super::x end",
    ] {
        assert!(codes(&rejected(source)).contains(&"using"), "{source}");
    }
}

#[test]
fn using_pending_value_aliases_do_not_block_resolved_module_aliases() {
    let program = accepted(
        "module A = module M = let x = 1n end end\nusing A::M as foo\nusing foo::x as foo\nlet result = foo\nlet qualified = foo::x",
    );
    assert_eq!(scheme(&program, "result"), "Nat");
    assert_eq!(scheme(&program, "qualified"), "Nat");
}

#[test]
fn using_same_name_imports_can_copy_an_inherited_module() {
    let program = accepted(
        "module Source = let x = 1n end\nmodule Child = using Source let result = Source::x end\nmodule Other = using Source as Source let result = Source::x end\nlet result = Child::result",
    );
    assert_eq!(scheme(&program, "result"), "Nat");
    assert!(codes(&rejected("using missing")).contains(&"using"));
}

#[test]
fn using_value_alias_can_share_a_glob_imported_module_name() {
    let program = accepted(
        "module A = module M = let x = 1n end end using A::* using M::x as M let result = M let qualified = M::x",
    );
    assert_eq!(scheme(&program, "result"), "Nat");
    assert_eq!(scheme(&program, "qualified"), "Nat");
}

#[test]
fn using_grouped_self_cannot_hide_a_later_glob_ambiguity() {
    let partial = rejected(
        "module M = module A = end end module N = module A = end end using M::* using later::* using N as later using A::{self}",
    );
    assert!(codes(&partial).contains(&"using"));
}

#[test]
fn using_explicit_imports_win_over_globs_in_either_order() {
    for imports in [
        "using A::* using B::x using A::*",
        "using B::x using A::* using A::*",
    ] {
        let program = accepted(&format!(
            "module A = let x = 1n end module B = let x = true end {imports} let result = x"
        ));
        assert_eq!(scheme(&program, "result"), "Boolean");
    }
    accepted("module Source = module A = end end using Source::* using A using A::{}");
    let empty = accepted("module M = end using M::{} using {}");
    assert!(empty.artifact().header().values.is_empty());
}

#[test]
fn using_local_effect_alias_chains_keep_the_original_effect_identity() {
    accepted(
        "module M = effect E = { get: () -> Nat } end
        let result: () -> Nat + M::!E = do
          using M::E as Local
          using Local as Alias
          let query: () -> Nat + !Alias = fn _ => !Alias.get ()
          return query
        end",
    );
}
