use ruddy::{
    inference::{self, Trace},
    ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
};

fn program(text: &str) -> (Mint, ir::Program) {
    let mut files = ruddy::tracking::FileManager::new();
    let file = files.register_new_file("main.rud".into(), text.into());
    let parsed = parse::parse(token::lex(text, file).tokens);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let mut mint = Mint::new(Bundle::new("queries", Version::new(0, 0, 0)).unwrap());
    let built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty(), "{:?}", built.errors);
    (mint, built.program)
}

#[test]
fn edits_reuse_unaffected_groups_and_match_fresh_analysis() {
    let mut session = inference::Session::default();
    let (mint, before) = program("let id = fn x => x\nlet use = id 1n\nlet apart = true");
    session.infer(&mint, &before, Trace::Complete);
    let initial = session.solved_groups();
    let (mint, after) = program("let id = fn x => x\nlet use = id false\nlet apart = true");
    let edited = session.infer(&mint, &after, Trace::Complete);
    assert_eq!(session.solved_groups() - initial, 1);
    let fresh = inference::infer(&mint, &after, Trace::Complete);
    assert_eq!(
        format!("{:?}", edited.semantics()),
        format!("{:?}", fresh.semantics())
    );
    assert_eq!(
        format!("{:?}", edited.errors()),
        format!("{:?}", fresh.errors())
    );
}

#[test]
fn unchanged_interfaces_reuse_callers_and_refresh_explanations() {
    let mut session = inference::Session::default();
    let (mint, before) = program("let id = fn x => x\nlet bad = (id 1n) + false");
    let initial = session.infer(&mint, &before, Trace::Complete);
    assert!(!initial.errors().is_empty());
    let count = session.solved_groups();
    let (mint, after) =
        program("let id = fn x => do let same = x return same end\nlet bad = (id 1n) + false");
    let edited = session.infer(&mint, &after, Trace::Complete);
    assert_eq!(
        session.solved_groups() - count,
        1,
        "the caller's interface dependencies did not change"
    );
    let mut fresh = inference::Session::default();
    let fresh = fresh.infer(&mint, &after, Trace::Complete);
    assert_eq!(
        format!("{:?}", edited.semantics()),
        format!("{:?}", fresh.semantics())
    );
    assert_eq!(
        format!("{:?}", edited.errors()),
        format!("{:?}", fresh.errors())
    );
    assert_eq!(
        format!("{:?}", edited.diagnostics().reasons()),
        format!("{:?}", fresh.diagnostics().reasons())
    );
}

#[test]
fn current_broken_buffers_keep_unaffected_definitions() {
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some("let good = 1n\nlet broken =".into()));
    let bundle = Bundle::new("editor", Version::new(0, 0, 0)).unwrap();
    let analysis = host.analyze(
        bundle,
        "main.rud",
        &ruddy::bundle::Environment::new([("target", "js"), ("platform", "node")]),
    );
    assert!(!analysis.diagnostics.is_empty());
    assert_eq!(analysis.hover("main.rud", 5).unwrap().ty, "Nat");
}

#[test]
fn hover_and_navigation_follow_current_expression_bindings() {
    let mut host = ruddy::analysis::Host::default();
    host.set_file(
        "main.rud",
        Some("let id = fn x => x\nlet use = id 1n".into()),
    );
    let analysis = host.analyze(
        Bundle::new("editor", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(analysis.diagnostics.is_empty());
    let target = analysis.definition("main.rud", 29).unwrap();
    assert_eq!(target.start, 4);
    assert!(analysis.hover("main.rud", 29).unwrap().ty.contains("Nat"));
    let local = analysis.definition("main.rud", 17).unwrap();
    assert_eq!(local.start, 12);
}

#[test]
fn completion_uses_lexical_scope_and_known_record_fields() {
    let mut host = ruddy::analysis::Host::default();
    let text =
        "let record = { count: 1n, label: \"x\" }\nlet use = fn item => item\nlet field = record.";
    host.set_file("main.rud", Some(text.into()));
    let analysis = host.analyze(
        Bundle::new("editor", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    let fields = analysis.completions("main.rud", text.len());
    assert_eq!(
        fields
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["count", "label"]
    );
    let names = analysis.completions("main.rud", text.find("=> item").unwrap() + 5);
    assert!(names.iter().any(|item| item.label == "item"));
    let outside = analysis.completions("main.rud", 0);
    assert!(!outside.iter().any(|item| item.label == "item"));
}

#[test]
fn cancellation_interrupts_difficult_solver_work() {
    use ruddy::{cancellation::Cancellation, inference::sat, types::Formula};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let mut clauses = Vec::new();
    for pigeon in 0..20 {
        clauses.push(Formula::any(
            (0..19).map(|hole| Formula::var(pigeon * 19 + hole)),
        ));
        for other in 0..pigeon {
            for hole in 0..19 {
                clauses.push(
                    Formula::var(pigeon * 19 + hole)
                        .not()
                        .or(Formula::var(other * 19 + hole).not()),
                );
            }
        }
    }
    let formula = Formula::all(clauses);
    let cancellation = Cancellation::default();
    let worker_token = cancellation.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = worker_token.run(|| sat::satisfiable(&formula));
        done_tx.send(result.is_err()).unwrap();
    });
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let started = Instant::now();
    cancellation.cancel();
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("obsolete SAT work stops")
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    worker.join().unwrap();
    assert!(
        Cancellation::default()
            .run(|| sat::satisfiable(&Formula::True))
            .unwrap()
    );
}

#[test]
fn unrelated_declaration_edits_do_not_repeat_value_inference() {
    let mut session = inference::Session::default();
    let (mint, before) = program("type Unused = Nat\nlet id = fn x => x\nlet answer = id 1n");
    session.infer(&mint, &before, Trace::Off);
    let count = session.solved_groups();
    let (mint, after) = program("type Unused = String\nlet id = fn x => x\nlet answer = id 1n");
    let updated = session.infer(&mint, &after, Trace::Off);
    assert_eq!(session.solved_groups(), count);
    let fresh = inference::infer(&mint, &after, Trace::Off);
    assert_eq!(
        format!("{:?}", updated.semantics()),
        format!("{:?}", fresh.semantics())
    );
}

#[test]
fn unsaved_dependency_interfaces_flow_without_lowering() {
    let mut dependency = ruddy::analysis::Host::default();
    let mut root = ruddy::analysis::Host::default();
    root.set_file("main.rud", Some("let answer = dep::value".into()));
    let environment = ruddy::bundle::Environment::new([]);
    for (source, expected) in [("let value = 1n", "Nat"), ("let value = false", "Bool")] {
        dependency.set_file("main.rud", Some(source.into()));
        let analysis = dependency.analyze(
            Bundle::new("dep", Version::new(0, 0, 0)).unwrap(),
            "main.rud",
            &environment,
        );
        let interface = analysis.interface();
        let analysis = root.analyze_with_interfaces(
            Bundle::new("root", Version::new(0, 0, 0)).unwrap(),
            "main.rud",
            &environment,
            &[ir::InterfaceImport {
                alias: "dep",
                header: &interface,
            }],
            &[&interface],
        );
        assert!(
            analysis.diagnostics.is_empty(),
            "{:?}",
            analysis.diagnostics
        );
        assert_eq!(analysis.hover("main.rud", 5).unwrap().ty, expected);
    }
}

#[test]
fn workspace_tracks_unsaved_local_dependencies_and_navigation() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    std::fs::create_dir_all(&dep).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(dep.join("Ruddy.toml"), "name = \"dep\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n").unwrap();
    std::fs::write(root.join("Ruddy.toml"), "name = \"root\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\ndep = \"../dep\"\n").unwrap();
    std::fs::write(dep.join("main.rud"), "let value = 1n").unwrap();
    std::fs::write(root.join("main.rud"), "let answer = dep::value").unwrap();
    let mut workspace = ruddy_cli::workspace::Workspace::new(root.clone());
    workspace.refresh().unwrap();
    let (project, logical) = workspace.file(&root.join("main.rud")).unwrap();
    assert_eq!(project.analysis.hover(logical, 5).unwrap().ty, "Nat");
    let (target, span) = workspace.definition(&root.join("main.rud"), 19).unwrap();
    assert_eq!(target, dep.join("main.rud"));
    assert_eq!(span.start, 4);
    workspace.set_overlay(&dep.join("main.rud"), Some("let value = false".into()));
    workspace.refresh().unwrap();
    let (project, logical) = workspace.file(&root.join("main.rud")).unwrap();
    assert_eq!(project.analysis.hover(logical, 5).unwrap().ty, "Bool");
}

#[test]
fn qualified_completion_uses_imported_source_names() {
    let mut dep = ruddy::analysis::Host::default();
    dep.set_file("main.rud", Some("let value = 1n".into()));
    let environment = ruddy::bundle::Environment::new([]);
    let dep = dep
        .analyze(
            Bundle::new("dependency", Version::new(0, 0, 0)).unwrap(),
            "main.rud",
            &environment,
        )
        .interface();
    let mut root = ruddy::analysis::Host::default();
    root.set_file("main.rud", Some("let answer = lib::va".into()));
    let root = root.analyze_with_interfaces(
        Bundle::new("root", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &environment,
        &[ir::InterfaceImport {
            alias: "lib",
            header: &dep,
        }],
        &[&dep],
    );
    assert!(
        root.completions("main.rud", 20)
            .iter()
            .any(|item| item.label == "value")
    );
}

#[test]
fn renaming_a_parameter_does_not_change_its_published_interface() {
    let mut session = inference::Session::default();
    let (mint, before) = program("let id = fn x => x\nlet answer = id 1n");
    session.infer(&mint, &before, Trace::Off);
    let count = session.solved_groups();
    let (mint, after) = program("let id = fn renamed => renamed\nlet answer = id 1n");
    session.infer(&mint, &after, Trace::Off);
    assert_eq!(session.solved_groups() - count, 1);
}

#[test]
fn equivalent_effect_origins_refresh_without_reinferring_callers() {
    let mut session = inference::Session::default();
    let source = |module| {
        format!(
            "module A = effect Log = {{ write: Nat -> () }} end\nmodule B = effect Log = {{ write: Nat -> () }} end\nlet write = {module}::!Log.write\nlet bad : Nat -> () = fn n => write n"
        )
    };
    let (mint, before) = program(&source("A"));
    session.infer(&mint, &before, Trace::Complete);
    let count = session.solved_groups();
    let (mint, after) = program(&source("B"));
    let updated = session.infer(&mint, &after, Trace::Complete);
    assert_eq!(session.solved_groups() - count, 1);
    let fresh = inference::infer(&mint, &after, Trace::Complete);
    assert!(!updated.errors().is_empty());
    assert_eq!(
        format!("{:?}", updated.errors()),
        format!("{:?}", fresh.errors())
    );
}

fn editor_source(text: &str) -> ruddy::analysis::Analysis {
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some(text.into()));
    host.analyze(
        Bundle::new("editor", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    )
}

#[test]
fn completion_understands_annotations_effect_qualifiers_and_pattern_binders() {
    for (text, expected) in [
        (
            "type Person = { name: String }\nlet value : Person -> Per",
            "Person",
        ),
        ("let value : Nat -> Str", "String"),
        (
            "module M = effect Log = { write: Nat -> () } end\nlet value = M::!Lo",
            "Log",
        ),
        ("let unwrap = fn | #Some value => val", "value"),
        (
            "let nested = { inner: { count: 1n } }\nlet value = nested.inner.",
            "count",
        ),
    ] {
        let analysis = editor_source(text);
        let items = analysis.completions("main.rud", text.len());
        assert!(
            items.iter().any(|item| item.label == expected),
            "{text}: {items:?}"
        );
    }
}

#[test]
fn active_file_requests_and_background_completion_share_one_revision() {
    let mut host = ruddy::analysis::Host::default();
    host.set_file("main.rud", Some("module Other\nlet value = 1n".into()));
    host.set_file(
        "Other.rud",
        Some("let identity = fn x => x\nlet problem : Nat = false".into()),
    );
    host.focus(Some("main.rud"));
    let mut analysis = host.analyze(
        Bundle::new("editor", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(analysis.diagnostics.is_empty());
    assert_eq!(host.solved_groups(), 1);
    host.request_file(&mut analysis, "Other.rud");
    assert!(!analysis.diagnostics.is_empty());
    let count = host.solved_groups();
    host.complete(&mut analysis);
    assert_eq!(host.solved_groups(), count);
    assert!(!analysis.diagnostics.is_empty());
    assert!(analysis.lower(&[]).0.is_none());
}

#[test]
fn navigation_follows_written_type_and_effect_references() {
    let source = "type Item = Nat\neffect Log = { write: Nat -> () }\nlet write : Item -> () + !Log = fn item => !Log.write item";
    let analysis = editor_source(source);
    assert_eq!(
        analysis
            .definition("main.rud", source.find("Item ->").unwrap())
            .unwrap()
            .start,
        5
    );
    assert_eq!(
        analysis
            .definition("main.rud", source.find("!Log =").unwrap() + 1)
            .unwrap()
            .start,
        source.find("Log =").unwrap()
    );
}

#[test]
fn recursive_group_edits_and_undo_match_fresh_inference() {
    let mut session = inference::Session::default();
    for source in [
        "let a = fn x => b x\nlet b = fn x => x\nlet use = a false",
        "let a = fn x => b x\nlet b = fn x => a x\nlet use = a false",
        "let b = fn x => x\nlet a = fn x => b x\nlet use = a false",
        "let a = fn x => b x\nlet b = fn x => x\nlet use = a false",
    ] {
        let (mint, program) = program(source);
        let updated = session.infer(&mint, &program, Trace::Complete);
        let fresh = inference::infer(&mint, &program, Trace::Complete);
        assert_eq!(
            format!("{:?}", updated.semantics()),
            format!("{:?}", fresh.semantics())
        );
        assert_eq!(
            format!("{:?}", updated.errors()),
            format!("{:?}", fresh.errors())
        );
    }
}

#[test]
fn file_candidates_configuration_and_moved_diagnostics_use_current_inputs() {
    let mut host = ruddy::analysis::Host::default();
    let identity = Bundle::new("editor", Version::new(0, 0, 0)).unwrap();
    let original = "module Added\n@if {platform: \"node\"} let value : Nat = false\n@if {platform: \"web\"} let value = true";
    for (main, added, competing, platform) in [
        (original.to_owned(), None, None, "node"),
        (original.to_owned(), Some("let answer = 1n"), None, "node"),
        (
            format!("\n\n{original}"),
            Some("let answer = 1n"),
            None,
            "node",
        ),
        (
            original.to_owned(),
            Some("let answer = 1n"),
            Some("let answer = false"),
            "node",
        ),
        (original.to_owned(), None, Some("let answer = false"), "web"),
        (original.to_owned(), None, None, "node"),
    ] {
        host.set_file("main.rud", Some(main.clone()));
        host.set_file("Added.rud", added.map(str::to_owned));
        host.set_file("Added/module.rud", competing.map(str::to_owned));
        let environment = ruddy::bundle::Environment::new([("platform", platform)]);
        let updated = host.analyze(identity.clone(), "main.rud", &environment);
        let mut fresh = ruddy::analysis::Host::default();
        fresh.set_file("main.rud", Some(main));
        fresh.set_file("Added.rud", added.map(str::to_owned));
        fresh.set_file("Added/module.rud", competing.map(str::to_owned));
        let fresh = fresh.analyze(identity.clone(), "main.rud", &environment);
        assert_eq!(updated.diagnostics, fresh.diagnostics);
        assert_eq!(updated.interface(), fresh.interface());
    }
}

#[test]
fn cancelling_a_query_revision_does_not_poison_the_next_revision() {
    use ruddy::cancellation::Cancellation;
    use std::{sync::mpsc, time::Duration};
    let source: String = (0..1000)
        .map(|i| format!("let value{i} = fn x => do let y = x + 1.0 return {{ value: y }} end\n"))
        .collect();
    let (mint, program) = program(&source);
    let cancellation = Cancellation::default();
    let worker_token = cancellation.clone();
    let (ready, wait) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut session = inference::Session::default();
        ready.send(()).unwrap();
        assert!(
            worker_token
                .run(|| session.infer(&mint, &program, Trace::Off))
                .is_err()
        );
        let resumed = session.infer(&mint, &program, Trace::Off);
        let fresh = inference::infer(&mint, &program, Trace::Off);
        assert_eq!(
            format!("{:?}", resumed.semantics()),
            format!("{:?}", fresh.semantics())
        );
    });
    wait.recv().unwrap();
    std::thread::sleep(Duration::from_millis(10));
    cancellation.cancel();
    worker.join().unwrap();
}

#[test]
fn repairing_an_annotated_body_keeps_its_caller_contract() {
    let mut session = inference::Session::default();
    let (mint, before) = program("let value : Nat = false\nlet bad = value + false");
    session.infer(&mint, &before, Trace::Complete);
    let count = session.solved_groups();
    let (mint, after) = program("let value : Nat = 1n\nlet bad = value + false");
    let updated = session.infer(&mint, &after, Trace::Complete);
    assert_eq!(session.solved_groups() - count, 1);
    let fresh = inference::infer(&mint, &after, Trace::Complete);
    assert_eq!(
        format!("{:?}", updated.errors()),
        format!("{:?}", fresh.errors())
    );
}

#[test]
fn invalid_manifest_revisions_do_not_keep_old_workspace_semantics() {
    let tree = tempfile::tempdir().unwrap();
    let manifest = "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false";
    std::fs::write(tree.path().join("Ruddy.toml"), manifest).unwrap();
    std::fs::write(tree.path().join("main.rud"), "let value = 1n").unwrap();
    let mut workspace = ruddy_cli::workspace::Workspace::new(tree.path().to_path_buf());
    workspace.refresh().unwrap();
    assert!(workspace.check_background().is_empty());
    workspace.set_overlay(
        &tree.path().join("Ruddy.toml"),
        Some("not valid toml".into()),
    );
    assert!(workspace.refresh().is_err());
    assert!(workspace.file(&tree.path().join("main.rud")).is_none());
    workspace.set_overlay(&tree.path().join("Ruddy.toml"), None);
    workspace.refresh().unwrap();
    assert!(workspace.file(&tree.path().join("main.rud")).is_some());
}

#[test]
fn reordering_record_fields_preserves_the_consumed_interface() {
    let mut session = inference::Session::default();
    let (mint, before) = program("let record = { a: 1n, b: false }\nlet use = record.a");
    session.infer(&mint, &before, Trace::Off);
    let count = session.solved_groups();
    let (mint, after) = program("let record = { b: false, a: 1n }\nlet use = record.a");
    let fresh = inference::infer(&mint, &after, Trace::Off);
    let updated = session.infer(&mint, &after, Trace::Off);
    assert_eq!(session.solved_groups() - count, 1);
    assert_eq!(
        format!("{:?}", updated.semantics().schemes()),
        format!("{:?}", fresh.semantics().schemes())
    );
}

#[test]
fn reordered_fields_refresh_cached_diagnostic_evidence() {
    let before = "let record = { a: 1n, b: false }\nlet use : String = record.a";
    let after = "let record = { b: false, a: 1n }\nlet use : String = record.a";
    let mut host = ruddy::analysis::Host::default();
    let bundle = Bundle::new("editor", Version::new(0, 0, 0)).unwrap();
    let environment = ruddy::bundle::Environment::new([]);
    host.set_file("main.rud", Some(before.into()));
    host.analyze(bundle.clone(), "main.rud", &environment);
    let count = host.solved_groups();
    host.set_file("main.rud", Some(after.into()));
    let updated = host.analyze(bundle, "main.rud", &environment);
    let fresh = editor_source(after);
    assert_eq!(host.solved_groups() - count, 1);
    assert!(!updated.diagnostics.is_empty());
    assert_eq!(updated.diagnostics, fresh.diagnostics);
}

#[test]
fn incomplete_annotations_complete_in_their_module_scope() {
    let mut host = ruddy::analysis::Host::default();
    let child = "type Person = { name: String }\nlet value : Per";
    host.set_file("main.rud", Some("module Child".into()));
    host.set_file("Child.rud", Some(child.into()));
    let analysis = host.analyze(
        Bundle::new("editor", Version::new(0, 0, 0)).unwrap(),
        "main.rud",
        &ruddy::bundle::Environment::new([]),
    );
    assert!(
        analysis
            .completions("Child.rud", child.len())
            .iter()
            .any(|item| item.label == "Person")
    );
    let inline = "module Child = type Person = { name: String } let value : Per end";
    let analysis = editor_source(inline);
    assert!(
        analysis
            .completions("main.rud", inline.find("Per end").unwrap() + 3)
            .iter()
            .any(|item| item.label == "Person")
    );
}

#[cfg(unix)]
#[test]
fn symlinked_document_paths_share_unsaved_source_identity() {
    let tree = tempfile::tempdir().unwrap();
    let real = tree.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tree.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    std::fs::write(real.join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false").unwrap();
    std::fs::write(real.join("main.rud"), "let value = 1n").unwrap();
    let mut workspace = ruddy_cli::workspace::Workspace::new(link.clone());
    workspace.focus(&link.join("main.rud"));
    workspace.set_overlay(
        &link.join("main.rud"),
        Some("module Added\nlet value = Added::answer".into()),
    );
    workspace.set_overlay(&link.join("Added.rud"), Some("let answer = false".into()));
    workspace.refresh().unwrap();
    let (project, logical) = workspace.file(&link.join("main.rud")).unwrap();
    assert_eq!(project.analysis.hover(logical, 17).unwrap().ty, "Bool");
    assert!(project.analysis.diagnostics.is_empty());
    assert!(workspace.file(&real.join("Added.rud")).is_some());
    assert!(workspace.definition(&link.join("main.rud"), 31).is_some());
}

#[test]
fn hover_includes_extern_declaration_names() {
    let text =
        "extern add : fn(Nat, Nat) -> Nat = \"(left, right) => left + right\"\nlet local = 1n";
    let analysis = editor_source(text);
    assert!(analysis.diagnostics.is_empty());
    assert!(
        analysis
            .hover("main.rud", text.find("local").unwrap() + 1)
            .is_some()
    );
    assert!(
        analysis
            .hover("main.rud", text.find("add").unwrap() + 1)
            .is_some(),
        "extern declaration names need hover types just like let declarations"
    );
}

#[test]
fn hover_includes_type_effect_and_module_declarations() {
    let text = "type Box 'value = { value: 'value }\neffect Read 'value = { read: () -> 'value }\neffect Alias 'value = !Read 'value\nmodule Helpers = end";
    let analysis = editor_source(text);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    for (name, expected) in [
        ("Box", "value:"),
        ("Read", "read:"),
        ("Alias", "!Read"),
        ("Helpers", "module Helpers"),
    ] {
        let start = text.find(name).unwrap();
        let hover = analysis
            .hover("main.rud", start)
            .unwrap_or_else(|| panic!("missing hover for {name}"));
        assert_eq!(hover.span.start, start);
        assert_eq!(hover.span.width, name.len());
        assert!(hover.ty.contains(expected), "{name}: {}", hover.ty);
    }
}
