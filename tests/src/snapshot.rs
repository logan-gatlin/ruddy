//! Tests for [`ruddy_debug::snapshot`].

use std::{collections::HashMap, fs};

use indexmap::IndexMap;

use ruddy_debug::{
    snapshot::{ROOT, compile, compile_at, guard, install_hook},
    stage::REGISTRY,
    wire::{
        CompileRequest, DependencyDetail, DependencySpec, FileSpec, InferenceCause, Loc, Node,
        Snapshot, Stage, Status, StdConfig, View,
    },
};

const DEMO: &str = include_str!("../../demo.rud");

fn clean_demo() -> &'static str {
    DEMO.split_once("\nlet bad = @")
        .expect("the demo's deliberate frontend errors")
        .0
}

/// A bundle of three files, one per shape a module's body can come from: an
/// inline module, a module beside its parent, and a module inside the directory
/// its parent's name spells.
const NESTED: &[(&str, &str)] = &[
    (ROOT, "module Math\nlet four = Math::double 2n\n"),
    ("Math.rud", "module Vec\n@private let double = fn x => x\n"),
    ("Math/Vec.rud", "let zero = 0n\n"),
];

/// One snippet, compiled as the whole of a bundle's root file.
fn snapshot(snippet: &str) -> Snapshot {
    bundle(&[(ROOT, &compiled(snippet))])
}

/// What a snippet is compiled as, for tests that inspect the exact bytes read.
fn compiled(snippet: &str) -> String {
    snippet.to_string()
}

#[test]
fn executable_contract_is_checked_and_artifact_targets_defer_node_support() {
    let request = |source: &str, target: &str| -> CompileRequest {
        serde_json::from_value(serde_json::json!({
            "name": "app", "version": "1.0.0", "kind": "executable", "target": target,
            "root": ROOT, "std": false, "files": [{"path": ROOT, "source": source}],
        }))
        .unwrap()
    };
    let missing = compile(&request("let value = ()", "js"), 0);
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|error| error.code == "invalid-entry-point")
    );
    assert!(missing.panic.is_none());
    assert_eq!(
        missing
            .stages
            .iter()
            .find(|s| s.id == "entry")
            .unwrap()
            .status,
        Status::Error
    );
    assert_ne!(
        missing
            .stages
            .iter()
            .find(|s| s.id == "artifact")
            .unwrap()
            .status,
        Status::Skipped
    );
    let source = "effect Custom = () -> ()\nlet main = fn _ => !Custom ()";
    let artifact = compile(&request(source, "artifact"), 0);
    assert_eq!(
        artifact
            .stages
            .iter()
            .find(|s| s.id == "entry")
            .unwrap()
            .status,
        Status::Ok
    );
    assert!(
        artifact.diagnostics.is_empty(),
        "{:?}",
        artifact.diagnostics
    );
    let js = compile(&request(source, "js"), 0);
    assert!(
        js.diagnostics
            .iter()
            .any(|error| error.code == "unsupported-entry-effects"),
        "{:?}",
        js.diagnostics
    );
}

/// The document's configured target and platform are what its `@if` guards
/// are judged against, so the page shows what a build of that kind would
/// compile. A request with no platform is a Node one, as every request was
/// before there was a choice.
#[test]
fn guards_follow_the_documents_configured_target_and_platform() {
    let request = |target: &str, platform: Option<&str>| -> CompileRequest {
        let mut request = serde_json::json!({
            "name": "app", "version": "1.0.0", "kind": "library", "target": target,
            "root": ROOT, "std": false,
            "files": [{"path": ROOT, "source":
                "@if {target: \"js\", platform: \"node\"} let only_js = 1n\nlet uses = only_js\n"}],
        });
        if let Some(platform) = platform {
            request["platform"] = serde_json::json!(platform);
        }
        serde_json::from_value(request).unwrap()
    };
    let undefined = |snapshot: &Snapshot| {
        snapshot
            .diagnostics
            .iter()
            .any(|error| error.code == "undefined-term")
    };
    let js = compile(&request("js", None), 0);
    assert!(js.diagnostics.is_empty(), "{:?}", js.diagnostics);
    let node = compile(&request("js", Some("node")), 0);
    assert!(node.diagnostics.is_empty(), "{:?}", node.diagnostics);
    let web = compile(&request("js", Some("web")), 0);
    assert!(undefined(&web), "{:?}", web.diagnostics);
    let artifact = compile(&request("artifact", None), 0);
    assert!(undefined(&artifact), "{:?}", artifact.diagnostics);
}

/// The debugger says at its entry stage what the command line says at the
/// manifest: an executable for the web has no launch adapter yet. A web
/// library is unaffected.
#[test]
fn a_web_executable_is_refused_at_the_entry_stage() {
    let request = |kind: &str| -> CompileRequest {
        serde_json::from_value(serde_json::json!({
            "name": "app", "version": "1.0.0", "kind": kind, "target": "js",
            "platform": "web", "root": ROOT, "std": false,
            "files": [{"path": ROOT, "source": "let main: () -> () = fn _ => ()\n"}],
        }))
        .unwrap()
    };
    let executable = compile(&request("executable"), 0);
    let refused = executable
        .diagnostics
        .iter()
        .find(|error| error.code == "platform-unsupported")
        .unwrap_or_else(|| panic!("{:?}", executable.diagnostics));
    assert_eq!(refused.stage, "entry");
    assert!(executable.panic.is_none());
    let library = compile(&request("library"), 0);
    assert!(library.diagnostics.is_empty(), "{:?}", library.diagnostics);
}

#[test]
fn library_artifact_targets_including_the_default_skip_backend_validation() {
    for target in [None, Some("artifact"), Some("js")] {
        let request: CompileRequest = serde_json::from_value(serde_json::json!({
            "name": "lib", "version": "1.0.0", "kind": "library", "target": target,
            "root": ROOT, "std": false,
            "files": [{"path": ROOT, "source": "extern invalid : Nat = \"?\""}],
        }))
        .unwrap();
        let snapshot = compile(&request, 0);
        assert_eq!(
            snapshot
                .stages
                .iter()
                .find(|s| s.id == "entry")
                .unwrap()
                .status,
            Status::Skipped
        );
        let js = snapshot.stages.iter().find(|s| s.id == "js").unwrap();
        if target == Some("js") {
            assert!(
                snapshot
                    .diagnostics
                    .iter()
                    .any(|d| d.code == "javascript-generation")
            );
        } else {
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:?}",
                snapshot.diagnostics
            );
            assert_eq!(js.status, Status::Skipped);
        }
    }
}

/// A whole bundle, each file exactly as written — the request the page posts,
/// with nothing added to it.
fn bundle(files: &[(&str, &str)]) -> Snapshot {
    compile(
        &CompileRequest {
            kind: ruddy::artifact::Kind::Library,
            target: Some(ruddy_cli::Target::Js),
            platform: None,
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            root: ROOT.to_string(),
            document: "demo".to_string(),
            files: files
                .iter()
                .map(|(path, source)| FileSpec {
                    path: (*path).to_string(),
                    source: (*source).to_string(),
                })
                .collect(),
            std: StdConfig::Disabled,
            dependencies: IndexMap::new(),
            revision: 3,
        },
        1,
    )
}

/// Where a range written in a snippet's own offsets ends up on the wire.
fn at(range: [usize; 2]) -> Option<Loc> {
    Some(Loc { file: 0, range })
}

fn nodes(stage: &Stage) -> Vec<&Node> {
    fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
        for node in nodes {
            out.push(node);
            walk(&node.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&stage.nodes, &mut out);
    out
}

#[test]
fn compile_requests_without_dependencies_remain_compatible() {
    let request: CompileRequest =
        serde_json::from_str(r#"{"files":[{"path":"main.rud","source":""}],"revision":4}"#)
            .unwrap();
    assert_eq!(request.name, "demo");
    assert_eq!(request.version, "0.1.0");
    assert_eq!(request.std, StdConfig::Default);
    assert!(!serde_json::to_string(&request).unwrap().contains("\"std\""));
    assert!(request.dependencies.is_empty());
    assert_eq!(request.revision, 4);

    let request: CompileRequest = serde_json::from_str(
        r#"{"files":[],"dependencies":{"http_core":{"bundle":"http-core","path":"../http-core"}}}"#,
    )
    .unwrap();
    assert!(matches!(
        &request.dependencies["http_core"],
        DependencySpec::Detailed(detail)
            if detail.bundle.as_deref() == Some("http-core")
                && detail.path.as_deref() == Some(std::path::Path::new("../http-core"))
    ));

    let git: CompileRequest = serde_json::from_str(
        r#"{"files":[],"dependencies":{"http_core":{"bundle":"http-core","git":"https://example.test/http-core","branch":"next"}}}"#,
    )
    .unwrap();
    assert!(matches!(
        &git.dependencies["http_core"],
        DependencySpec::Detailed(detail)
            if detail.bundle.as_deref() == Some("http-core")
                && detail.git.as_deref() == Some("https://example.test/http-core")
                && detail.branch.as_deref() == Some("next")
    ));

    let disabled: CompileRequest = serde_json::from_str(r#"{"files":[],"std":false}"#).unwrap();
    assert_eq!(disabled.std, StdConfig::Disabled);
    let custom: CompileRequest =
        serde_json::from_str(r#"{"files":[],"std":"../std-next"}"#).unwrap();
    assert_eq!(
        custom.std,
        StdConfig::Dependency(DependencySpec::from("../std-next"))
    );
    assert!(
        serde_json::from_str::<CompileRequest>(r#"{"files":[],"std":true}"#)
            .unwrap_err()
            .to_string()
            .contains("std = true")
    );

    let old = serde_json::from_str::<CompileRequest>(
        r#"{"files":[],"dependencies":{"http_core":{"package":"http-core","path":"../http-core"}}}"#,
    )
    .unwrap_err();
    assert!(old.to_string().contains("unknown field `package`"), "{old}");
}

#[test]
fn dependency_paths_without_a_scratch_root_are_recoverable() {
    let mut dependencies = IndexMap::new();
    dependencies.insert("base".to_string(), "../base".into());
    let snapshot = compile(
        &CompileRequest {
            kind: ruddy::artifact::Kind::Library,
            target: None,
            platform: None,
            name: "debugger".to_string(),
            version: "1.2.3".to_string(),
            root: ROOT.to_string(),
            document: "debugger".to_string(),
            files: vec![FileSpec {
                path: ROOT.to_string(),
                source: compiled("let main = 0n\n"),
            }],
            std: StdConfig::Disabled,
            dependencies,
            revision: 9,
        },
        1,
    );
    assert_eq!(snapshot.diagnostics[0].stage, "dependencies");
    assert_eq!(snapshot.diagnostics[0].code, "dependency-workspace-missing");
    assert_eq!(
        snapshot.diagnostics[0].help,
        ["open the debugger with a scratch directory before adding dependencies"]
    );
    assert_eq!(
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == "ir")
            .unwrap()
            .status,
        Status::Skipped
    );
    assert_eq!(
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == "artifact")
            .unwrap()
            .status,
        Status::Skipped
    );
}

#[test]
fn custom_standard_library_is_source_visible_rendered_and_sandboxed() {
    let outer = tempfile::tempdir().unwrap();
    let scratch = outer.path().join("scratch");
    let app = scratch.join("app");
    let standard = scratch.join("std-next");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&standard).unwrap();
    fs::write(standard.join("main.rud"), "let answer = 42n\n").unwrap();
    fs::write(
        standard.join("Ruddy.toml"),
        "name = \"std\"\nversion = \"2.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let request = CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".into(),
        version: "1.0.0".into(),
        root: ROOT.into(),
        document: "app".into(),
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let main = std::answer\n".into(),
        }],
        std: StdConfig::Dependency(DependencySpec::from("../std-next")),
        dependencies: IndexMap::new(),
        revision: 1,
    };
    let built = compile_at(&request, 1, &scratch);
    assert!(built.diagnostics.is_empty(), "{:#?}", built.diagnostics);
    let stage = built
        .stages
        .iter()
        .find(|stage| stage.id == "dependencies")
        .unwrap();
    assert_eq!(stage.summary, "1 declared · 1 built");
    assert_eq!(stage.nodes[0].children[0].text, "std@2.0.0");
    assert!(
        stage.nodes[0].children[0]
            .fields
            .iter()
            .any(|field| field.name == "source" && field.value == "path ../std-next")
    );

    let outside = outer.path().join("outside-std");
    fs::rename(&standard, &outside).unwrap();
    let escaped = CompileRequest {
        std: StdConfig::Dependency(DependencySpec::from("../../outside-std")),
        ..request
    };
    let failed = compile_at(&escaped, 2, &scratch);
    assert!(failed.diagnostics.iter().any(|diagnostic| {
        diagnostic.stage == "dependencies" && diagnostic.code == "project-outside-workspace"
    }));
}

/// The debugger remembers a compiled dependency graph between requests, and
/// forgets it the moment a file the graph was read from changes: a standard
/// library edited on disk is what the very next compile sees.
#[test]
fn a_remembered_dependency_graph_follows_edits_to_its_sources() {
    let outer = tempfile::tempdir().unwrap();
    let scratch = outer.path().join("scratch");
    let app = scratch.join("app");
    let standard = scratch.join("std-next");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&standard).unwrap();
    fs::write(standard.join("main.rud"), "let answer = 42n\n").unwrap();
    fs::write(
        standard.join("Ruddy.toml"),
        "name = \"std\"\nversion = \"2.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let request = CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".into(),
        version: "1.0.0".into(),
        root: ROOT.into(),
        document: "app".into(),
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let main = std::answer\n".into(),
        }],
        std: StdConfig::Dependency(DependencySpec::from("../std-next")),
        dependencies: IndexMap::new(),
        revision: 1,
    };
    for revision in 1..=2 {
        let built = compile_at(&request, revision, &scratch);
        assert!(built.diagnostics.is_empty(), "{:#?}", built.diagnostics);
    }

    fs::write(standard.join("main.rud"), "let renamed = 42n\n").unwrap();
    let stale = compile_at(&request, 3, &scratch);
    assert!(
        stale
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.stage == "ir"),
        "{:#?}",
        stale.diagnostics
    );

    let repointed = CompileRequest {
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let main = std::renamed\n".into(),
        }],
        ..request
    };
    let built = compile_at(&repointed, 4, &scratch);
    assert!(built.diagnostics.is_empty(), "{:#?}", built.diagnostics);
}

#[test]
fn installed_standard_library_is_the_only_trusted_external_local_root() {
    let outer = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "snapshot::installed_standard_library_child",
        ])
        .env("RUDDY_HOME", outer.path().join("home"))
        .env("RUDDY_TEST_STD_SCRATCH", outer.path().join("scratch"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "run in isolation with a dedicated installed standard library"]
fn installed_standard_library_child() {
    let home = std::path::PathBuf::from(std::env::var_os("RUDDY_HOME").unwrap());
    let scratch = std::path::PathBuf::from(std::env::var_os("RUDDY_TEST_STD_SCRATCH").unwrap());
    let app = scratch.join("app");
    let standard = home.join("std");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&standard).unwrap();
    fs::write(standard.join("main.rud"), "let installed = 1n\n").unwrap();
    fs::write(
        standard.join("Ruddy.toml"),
        "name = \"std\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let request = CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".into(),
        version: "1.0.0".into(),
        root: ROOT.into(),
        document: "app".into(),
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let main = std::installed\n".into(),
        }],
        std: StdConfig::Default,
        dependencies: IndexMap::new(),
        revision: 1,
    };
    let built = compile_at(&request, 1, &scratch);
    assert!(built.diagnostics.is_empty(), "{:#?}", built.diagnostics);
    let dependencies = built
        .stages
        .iter()
        .find(|stage| stage.id == "dependencies")
        .unwrap();
    assert_eq!(dependencies.nodes[0].children[0].text, "std@0.1.0");
    assert!(
        dependencies.nodes[0].children[0]
            .fields
            .iter()
            .any(|field| field.name == "source" && field.value == "installed default")
    );

    // Trust is attached to the Default setting, not merely to the path: the
    // same external tree named as a custom dependency remains sandboxed.
    let custom = CompileRequest {
        std: StdConfig::Dependency(DependencySpec::Path(standard)),
        ..request
    };
    let rejected = compile_at(&custom, 2, &scratch);
    assert!(rejected.diagnostics.iter().any(|diagnostic| {
        diagnostic.stage == "dependencies" && diagnostic.code == "project-outside-workspace"
    }));
}

#[test]
fn saved_dependency_projects_supply_artifact_identity_and_gate_lir() {
    let scratch = tempfile::tempdir().unwrap();
    let app = scratch.path().join("app");
    let base = scratch.path().join("base");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("main.rud"), "let base = 0n\n").unwrap();
    fs::write(
        base.join("Ruddy.toml"),
        "name = \"base\"\nversion = \"2.3.4\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    let mut dependencies = IndexMap::new();
    dependencies.insert("base".to_string(), "../base".into());
    let request = CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".to_string(),
        version: "1.0.0".to_string(),
        root: ROOT.to_string(),
        document: "app".to_string(),
        files: vec![FileSpec {
            path: ROOT.to_string(),
            source: "let app = base::base\n".to_string(),
        }],
        std: StdConfig::Disabled,
        dependencies,
        revision: 1,
    };
    let built = compile_at(&request, 1, scratch.path());
    assert!(built.diagnostics.is_empty(), "{:#?}", built.diagnostics);
    let artifact = built
        .stages
        .iter()
        .find(|stage| stage.id == "artifact")
        .unwrap();
    assert!(
        artifact
            .text
            .as_deref()
            .unwrap()
            .contains("(dependency \"base\" \"2.3.4\")")
    );
    assert!(
        artifact
            .text
            .as_deref()
            .unwrap()
            .contains("base@2.3.4::base")
    );
    let dependencies = built
        .stages
        .iter()
        .find(|stage| stage.id == "dependencies")
        .unwrap();
    assert_eq!(dependencies.status, Status::Ok);
    let dependency = &dependencies.nodes[0].children[0];
    assert_eq!(dependency.text, "base@2.3.4");
    assert!(
        dependency
            .fields
            .iter()
            .any(|field| field.name == "source" && field.value == "path ../base")
    );
    assert!(
        dependency
            .fields
            .iter()
            .any(|field| field.name == "imported values" && field.value == "1")
    );

    fs::write(base.join("main.rud"), "let bad : Nat = fn x => x\n").unwrap();
    let failed = compile_at(&request, 1, scratch.path());
    assert_eq!(failed.diagnostics[0].stage, "dependencies");
    assert_eq!(failed.diagnostics[0].code, "type-mismatch");
    assert!(
        failed.diagnostics[0]
            .notes
            .iter()
            .any(|note| note.contains("dependency `base`")),
        "{:#?}",
        failed.diagnostics[0]
    );
    let errors = &failed
        .stages
        .iter()
        .find(|stage| stage.id == "errors")
        .expect("the Errors tab renders the dependency report")
        .debug;
    assert!(errors.contains("[type-mismatch]"), "{errors}");
    assert!(errors.contains("let bad : Nat = fn x => x"), "{errors}");
    assert!(errors.contains("dependency `base`"), "{errors}");
    assert_eq!(
        failed
            .stages
            .iter()
            .find(|stage| stage.id == "lir")
            .unwrap()
            .status,
        Status::Skipped
    );
    assert_eq!(
        failed
            .stages
            .iter()
            .find(|stage| stage.id == "artifact")
            .unwrap()
            .status,
        Status::Skipped
    );

    fs::write(
        base.join("main.rud"),
        "let one : Nat = false\nlet two : String = 2n\n",
    )
    .unwrap();
    let multiple = compile_at(&request, 2, scratch.path());
    assert_eq!(multiple.diagnostics.len(), 2, "{:#?}", multiple.diagnostics);
    assert!(
        multiple
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.stage == "dependencies"
                && diagnostic.code == "type-mismatch")
    );
}

#[test]
fn dependencies_tab_correlates_same_bundle_versions_by_request_alias() {
    let scratch = tempfile::tempdir().unwrap();
    for (directory, version, source) in [
        ("old-lib", "1.0.0", "let one = 1n\n"),
        ("new-lib", "2.0.0", "let one = 1n\nlet two = 2n\n"),
    ] {
        let path = scratch.path().join(directory);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("main.rud"), source).unwrap();
        fs::write(
            path.join("Ruddy.toml"),
            format!(
                "name = \"lib\"\nversion = \"{version}\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n"
            ),
        )
        .unwrap();
    }
    fs::create_dir_all(scratch.path().join("app")).unwrap();
    let detailed = |path: &str| {
        DependencySpec::Detailed(DependencyDetail {
            bundle: Some("lib".into()),
            path: Some(path.into()),
            git: None,
            branch: None,
            tag: None,
            rev: None,
        })
    };
    let dependencies = IndexMap::from([
        ("old".into(), detailed("../old-lib")),
        ("new".into(), detailed("../new-lib")),
    ]);
    let request = CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".into(),
        version: "1.0.0".into(),
        root: ROOT.into(),
        document: "app".into(),
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let old_one = old::one\nlet new_two = new::two\n".into(),
        }],
        std: StdConfig::Disabled,
        dependencies,
        revision: 1,
    };

    let snapshot = compile_at(&request, 1, scratch.path());
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    let stage = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "dependencies")
        .expect("dependencies stage");
    let projects = &stage.nodes[0].children;
    assert_eq!(
        projects
            .iter()
            .map(|node| node.text.as_str())
            .collect::<Vec<_>>(),
        ["lib@1.0.0", "lib@2.0.0"]
    );
    let imported_values = |node: &Node| {
        node.fields
            .iter()
            .find(|field| field.name == "imported values")
            .map(|field| field.value.clone())
    };
    assert_eq!(imported_values(&projects[0]).as_deref(), Some("1"));
    assert_eq!(imported_values(&projects[1]).as_deref(), Some("2"));
}

#[test]
fn transitive_detailed_dependency_manifests_are_validated_and_compiled() {
    let scratch = tempfile::tempdir().unwrap();
    for directory in ["app", "base", "shared"] {
        fs::create_dir_all(scratch.path().join(directory)).unwrap();
    }
    fs::write(
        scratch.path().join("shared/Ruddy.toml"),
        "name = \"shared-package\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(scratch.path().join("shared/main.rud"), "let value = 1n\n").unwrap();
    fs::write(
        scratch.path().join("base/Ruddy.toml"),
        "name = \"base\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n[dependencies.shared]\nbundle = \"shared-package\"\npath = \"../shared\"\n",
    )
    .unwrap();
    fs::write(
        scratch.path().join("base/main.rud"),
        "let value = shared::value\n",
    )
    .unwrap();

    let request = CompileRequest {
        files: vec![FileSpec {
            path: ROOT.into(),
            source: "let value = base::value\n".into(),
        }],
        ..dependency_request(IndexMap::from([("base".into(), "../base".into())]))
    };
    let snapshot = compile_at(&request, 1, scratch.path());

    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    let dependencies = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "dependencies")
        .expect("dependencies stage");
    assert_eq!(dependencies.status, Status::Ok);
    // The tab lists source-visible roots only; successful compilation of
    // `base::value` proves its detailed transitive dependency was linked.
    assert_eq!(dependencies.nodes[0].children[0].text, "base@1.0.0");
}

#[test]
fn failed_graph_validation_is_reported_for_the_dependency_build() {
    let scratch = tempfile::tempdir().unwrap();
    fs::create_dir_all(scratch.path().join("app")).unwrap();
    fs::create_dir_all(scratch.path().join("base")).unwrap();
    fs::write(
        scratch.path().join("base/Ruddy.toml"),
        "name = \"base\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nmissing = \"../../outside\"\n",
    )
    .unwrap();
    fs::write(scratch.path().join("base/main.rud"), "let base = 0n\n").unwrap();
    let request = dependency_request(IndexMap::from([
        ("base".into(), "../base".into()),
        ("alias".into(), "../base".into()),
    ]));

    let snapshot = compile_at(&request, 1, scratch.path());
    let unavailable: Vec<_> = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "project-unavailable")
        .collect();
    assert_eq!(unavailable.len(), 1);
    assert!(
        unavailable[0]
            .notes
            .iter()
            .any(|note| note.contains("dependency `missing`"))
    );
}

#[test]
fn dependency_roots_cannot_be_absolute_or_escape_the_scratch_folder() {
    let outer = tempfile::tempdir().unwrap();
    let scratch = outer.path().join("scratch");
    fs::create_dir_all(scratch.join("app")).unwrap();
    fs::create_dir_all(scratch.join("base")).unwrap();
    let outside = outer.path().join("outside.rud");
    fs::write(&outside, "let outside = 0n\n").unwrap();

    for root in [
        outside.display().to_string(),
        "../../outside.rud".to_string(),
    ] {
        fs::write(
            scratch.join("base/Ruddy.toml"),
            format!("name = \"base\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = {root:?}\n[dependencies]\nstd = false\n"),
        )
        .unwrap();
        let snapshot = compile_at(
            &dependency_request(IndexMap::from([("base".into(), "../base".into())])),
            1,
            &scratch,
        );
        assert!(
            snapshot.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code,
                    "project-root-outside-workspace" | "project-outside-workspace"
                )
            }),
            "{:#?}",
            snapshot.diagnostics
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_dependency_manifests_are_confined_to_the_scratch_folder() {
    let outer = tempfile::tempdir().unwrap();
    let scratch = outer.path().join("scratch");
    let base = scratch.join("base");
    fs::create_dir_all(scratch.join("app")).unwrap();
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("main.rud"), "let base = 0n\n").unwrap();
    let manifest = "name = \"base\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n";
    let outside = outer.path().join("outside.toml");
    fs::write(&outside, manifest).unwrap();
    std::os::unix::fs::symlink(&outside, base.join("Ruddy.toml")).unwrap();

    let request = dependency_request(IndexMap::from([("base".into(), "../base".into())]));
    let snapshot = compile_at(&request, 1, &scratch);
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "manifest-outside-workspace"),
        "{:#?}",
        snapshot.diagnostics
    );
    let error =
        ruddy_cli::compile_sandboxed_dependency_graph([("base", &base)], &scratch).unwrap_err();
    assert!(
        error.to_string().contains("outside the debugger workspace"),
        "{error}"
    );

    fs::remove_file(base.join("Ruddy.toml")).unwrap();
    let shared = scratch.join("base-manifest.toml");
    fs::write(&shared, manifest).unwrap();
    std::os::unix::fs::symlink(&shared, base.join("Ruddy.toml")).unwrap();

    let snapshot = compile_at(&request, 2, &scratch);
    assert!(
        !snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.starts_with("dependency-")),
        "{:#?}",
        snapshot.diagnostics
    );
    ruddy_cli::compile_sandboxed_dependency_graph([("base", &base)], &scratch).unwrap();
}

#[cfg(unix)]
#[test]
fn symlinked_dependency_modules_cannot_escape_the_scratch_folder() {
    let outer = tempfile::tempdir().unwrap();
    let scratch = outer.path().join("scratch");
    fs::create_dir_all(scratch.join("app")).unwrap();
    fs::create_dir_all(scratch.join("base")).unwrap();
    fs::write(
        scratch.join("base/Ruddy.toml"),
        "name = \"base\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
    )
    .unwrap();
    fs::write(scratch.join("base/main.rud"), "module Escape\n").unwrap();
    let outside = outer.path().join("Escape.rud");
    fs::write(&outside, "let escaped = 0n\n").unwrap();
    std::os::unix::fs::symlink(&outside, scratch.join("base/Escape.rud")).unwrap();

    let snapshot = compile_at(
        &dependency_request(IndexMap::from([("base".into(), "../base".into())])),
        1,
        &scratch,
    );
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "module-file-missing"),
        "{:#?}",
        snapshot.diagnostics
    );
}

#[test]
fn types_stage_walks_deep_imported_aliases_on_a_small_stack() {
    std::thread::Builder::new()
        .name("deep-imported-types-stage".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 2_048;
            let scratch = tempfile::tempdir().unwrap();
            fs::create_dir_all(scratch.path().join("app")).unwrap();
            fs::create_dir_all(scratch.path().join("dep")).unwrap();
            fs::write(
                scratch.path().join("dep/Ruddy.toml"),
                "name = \"dep\"\nversion = \"1.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n",
            )
            .unwrap();
            let mut source = String::new();
            for index in 0..DEPTH {
                source.push_str(&format!(
                    "type T{index} = {}\n",
                    if index + 1 == DEPTH {
                        "Nat".to_string()
                    } else {
                        format!("T{}", index + 1)
                    }
                ));
            }
            fs::write(scratch.path().join("dep/main.rud"), source).unwrap();

            let snapshot = compile_at(
                &dependency_request(IndexMap::from([("dep".into(), "../dep".into())])),
                1,
                scratch.path(),
            );
            assert!(snapshot.panic.is_none(), "{:?}", snapshot.panic);
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:#?}",
                snapshot.diagnostics
            );
            let types = snapshot
                .stages
                .iter()
                .find(|stage| stage.id == "types")
                .expect("the Types stage is present");
            assert_eq!(types.status, Status::Ok);
            assert!(types.nodes.len() >= DEPTH, "{}", types.nodes.len());
        })
        .expect("the bounded-stack debugger regression thread starts")
        .join()
        .expect("the Types stage walks deep imported aliases without overflowing");
}

fn dependency_request(dependencies: IndexMap<String, String>) -> CompileRequest {
    let dependencies = dependencies
        .into_iter()
        .map(|(name, path)| (name, path.into()))
        .collect();
    CompileRequest {
        kind: ruddy::artifact::Kind::Library,
        target: None,
        platform: None,
        name: "app".to_string(),
        version: "1.0.0".to_string(),
        root: ROOT.to_string(),
        document: "app".to_string(),
        files: vec![FileSpec {
            path: ROOT.to_string(),
            source: "let app = 0n\n".to_string(),
        }],
        std: StdConfig::Disabled,
        dependencies,
        revision: 1,
    }
}

#[test]
fn every_stage_reports_on_the_demo() {
    let snapshot = bundle(&[(ROOT, DEMO)]);
    let ids: Vec<_> = snapshot.stages.iter().map(|stage| stage.id).collect();
    assert_eq!(
        ids,
        [
            "errors",
            "tokens",
            "dependencies",
            "ast",
            "externs",
            "ir",
            "constraints",
            "solve",
            "types",
            "reification",
            "presence",
            "patterns",
            "lir",
            "artifact",
            "entry",
            "linked",
            "js",
            "symbols",
            "types-ir"
        ]
    );
    assert_eq!(snapshot.revision, 3);
    assert!(snapshot.panic.is_none());
    for stage in &snapshot.stages {
        if stage.id == "errors" {
            assert_eq!(stage.status, Status::Partial);
            assert!(stage.text.as_deref().is_some_and(|text| !text.is_empty()));
            assert!(!stage.summary.is_empty());
            continue;
        }
        // Recovery remains visible through semantic stages; lowering still
        // requires a fully accepted program.
        if matches!(stage.id, "lir" | "artifact" | "entry" | "linked" | "js") {
            assert_eq!(stage.status, Status::Skipped, "{}", stage.id);
            assert!(stage.nodes.is_empty(), "a skipped stage rendered rows");
            assert!(!stage.summary.is_empty(), "{} counted nothing", stage.id);
            continue;
        }
        if matches!(
            stage.id,
            "externs"
                | "ir"
                | "constraints"
                | "solve"
                | "types"
                | "reification"
                | "presence"
                | "patterns"
                | "symbols"
                | "types-ir"
        ) {
            assert_eq!(stage.status, Status::Partial, "{}", stage.id);
            assert!(!stage.summary.is_empty(), "{} counted nothing", stage.id);
            continue;
        }
        assert!(!stage.nodes.is_empty(), "{} produced nothing", stage.id);
        // The pane bar prints this beside the tab strip, so a stage that
        // counted nothing leaves a blank there rather than a count.
        assert!(!stage.summary.is_empty(), "{} counted nothing", stage.id);
    }

    // The tab strip is the stages that annotate nothing, so the annotator at
    // the end of the registry adds no second "Types" tab.
    let titles: Vec<_> = snapshot
        .stages
        .iter()
        .filter(|stage| stage.annotates.is_none())
        .map(|stage| stage.title)
        .collect();
    assert_eq!(
        titles,
        [
            "Errors",
            "Tokens",
            "Dependencies",
            "AST",
            "Externs",
            "IR",
            "Constraints",
            "Solve",
            "Types",
            "Runtime types",
            "Presence",
            "Patterns",
            "LIR",
            "Artifact",
            "Entry",
            "Linked Artifact",
            "JS",
            "Symbols"
        ]
    );
}

/// A span hygiene check on the compiler, not on the debugger: every offset
/// a stage hands out has to be a real position in the file it came from.
/// Bad `merge` arithmetic shows up here rather than as a mangled highlight.
#[test]
fn every_span_lies_inside_the_source() {
    let snapshot = bundle(&[(ROOT, DEMO)]);
    // The demo is one file, and a `Loc` naming a file the snapshot does not
    // list is nowhere the reader could be shown — so the index is checked
    // against the list before the range is checked against the file.
    let files: Vec<&str> = snapshot.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, [ROOT]);
    assert_eq!(snapshot.files[0].len, DEMO.len());

    for stage in &snapshot.stages {
        for node in nodes(stage) {
            let Some(Loc {
                file,
                range: [start, end],
            }) = node.span
            else {
                continue;
            };
            let info = snapshot
                .files
                .get(file as usize)
                .unwrap_or_else(|| panic!("{}: {} names file {file}", stage.id, node.label));
            assert!(
                start <= end && end <= info.len,
                "{}: {} {:?} has span {start}..{end} in {} bytes",
                stage.id,
                node.label,
                node.text,
                info.len
            );
            assert!(
                DEMO.get(start..end).is_some(),
                "{}: {} has a span that splits a character",
                stage.id,
                node.label
            );
        }
    }
    for diagnostic in &snapshot.diagnostics {
        if let Some(at) = diagnostic.span {
            assert_eq!(at.file, 0, "{}", diagnostic.code);
            assert!(
                DEMO.get(at.range[0]..at.range[1]).is_some(),
                "{}",
                diagnostic.code
            );
        }
    }
}

/// `Node::symbol` is what the page paints occurrences from: it walks every
/// stage, collects the span of every node carrying the symbol, and highlights
/// all of them in the editor as uses of that name. So a node may only claim it
/// when its span really is somewhere the name was written — which is a
/// property of the whole snapshot, checkable here, and not one any single
/// stage can be trusted to have remembered. A stage that wants the association
/// without the span has `Node::owner` for it.
#[test]
fn a_node_naming_a_symbol_is_spanned_at_the_name() {
    let source = clean_demo();
    let snapshot = bundle(&[(ROOT, source)]);
    let symbols = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage is registered");
    // The symbols stage is the index every `symbol` points into, and its own
    // rows are labelled with the names.
    let names: HashMap<u32, &str> = symbols
        .nodes
        .iter()
        .map(|node| (node.id, node.label.as_str()))
        .collect();

    let mut checked = 0;
    for stage in &snapshot.stages {
        for node in nodes(stage) {
            let (
                Some(index),
                Some(Loc {
                    range: [start, end],
                    ..
                }),
            ) = (node.symbol, node.span)
            else {
                continue;
            };
            let name = names
                .get(&index)
                .unwrap_or_else(|| panic!("{}: {} points at no symbol row", stage.id, node.label));
            // A span covers the whole lexeme, and a sigil is not part of the
            // name it heads: a type parameter is written `'a` and named `a`,
            // the way a tag's `#` and an effect's `!` are no part of theirs.
            assert_eq!(
                source[start..end].trim_start_matches(['\'', '#', '!']),
                *name,
                "{}: {} claims to name {name} at {start}..{end}",
                stage.id,
                node.label
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no stage claimed a symbol at all");

    // And the association a solve step does want is the one that costs it no
    // occurrence: its span is whatever sub-expression the constraint came
    // from, which is nowhere the definition's name appears.
    let solve = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    assert!(!solve.nodes.is_empty());
    assert!(
        solve.nodes.iter().all(|node| node.symbol.is_none()),
        "a solve step claimed its span as an occurrence"
    );
    assert!(
        solve.nodes.iter().any(|node| node.owner.is_some()),
        "no solve step says which definition it belongs to"
    );
}

#[test]
fn diagnostics_are_reported_in_source_order() {
    let snapshot = bundle(&[(ROOT, DEMO)]);
    let codes: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    for code in [
        "attribute-needs-name",
        "number-joined-to-name",
        "expected-name",
        "undefined-term",
    ] {
        assert!(codes.contains(&code), "{codes:?}");
    }

    // Where the reader would look for them: the file first, then the offset
    // inside it, since a bundle's diagnostics come from more than one file.
    let offsets: Vec<_> = snapshot
        .diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic
                .span
                .map(|at| (at.file, at.range[0]))
                .unwrap_or((0, 0))
        })
        .collect();
    assert!(
        offsets.windows(2).all(|pair| pair[0] <= pair[1]),
        "{offsets:?}"
    );
}

/// A literal has to show up in every panel that renders a term, and to be
/// coloured as a literal in the editor — which the tokens stage is what
/// drives.
#[test]
fn a_natural_reaches_every_stage() {
    let snapshot = snapshot("let n = 42n\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    for id in ["tokens", "ast", "ir"] {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        // By what it says rather than by being the first of its kind.
        let node = nodes(stage)
            .into_iter()
            .find(|node| node.label == "Natural" && node.text == "42n")
            .unwrap_or_else(|| panic!("{id} rendered no natural"));

        assert_eq!(node.span, at([8, 11]), "{id}");
        // A literal names nothing, so no panel may point it at a symbol.
        assert_eq!(node.symbol, None, "{id}");
    }

    let tokens = nodes(
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == "tokens")
            .expect("the tokens stage"),
    );
    let literal = tokens
        .iter()
        .find(|node| node.label == "Natural" && node.text == "42n")
        .expect("the tokens tab renders the literal");
    let class = literal
        .fields
        .iter()
        .find(|field| field.name == "_class")
        .expect("the editor is told what to paint it");
    assert_eq!(class.value, "number");
}

/// A positional projection is numeric syntax too, even though its token is
/// contextual rather than a literal expression.
#[test]
fn a_numeric_field_is_coloured_as_a_number() {
    let snapshot = snapshot("@private let first = fn p => p.0\n");
    assert!(snapshot.diagnostics.is_empty());

    let field = nodes(
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == "tokens")
            .expect("the tokens stage"),
    )
    .into_iter()
    .find(|node| node.label == "NumericField" && node.text == "0")
    .expect("the tokens tab renders the numeric field");
    let class = field
        .fields
        .iter()
        .find(|field| field.name == "_class")
        .expect("the editor is told what to paint it");
    assert_eq!(class.value, "number");
}

/// The three forms phase 0 adds, in one line, checked through every panel
/// that renders them. A stage that stopped matching on one of them would
/// fail to compile; this is the check that it renders it too.
#[test]
fn the_surface_prerequisites_reach_every_stage() {
    let source = "let fst : { x: Nat, y: Nat } -> Nat = fn p => p.x\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let labelled = |id: &str, label: &str| -> Vec<String> {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        nodes(stage)
            .into_iter()
            .filter(|node| node.label == label)
            .map(|node| node.text.clone())
            .collect()
    };

    for id in ["ast", "ir"] {
        // The ascription is a child of the declaration in both trees, and the
        // node carrying it is the arrow's own: one node, labelled with its role
        // and its kind, rather than a wrapper repeating the type below it.
        assert_eq!(
            labelled(id, "Ascribed Arrow"),
            ["{ x: Nat, y: Nat } -> Nat"],
            "{id}"
        );
        assert_eq!(labelled(id, "Arrow"), [] as [&str; 0], "{id}");
        assert_eq!(labelled(id, "Project"), ["p.x"], "{id}");
        assert_eq!(labelled(id, "Field"), ["x"], "{id}");
    }

    // `Nat` is an ordinary identifier until lowering, which is the one
    // place the two trees are meant to differ.
    assert_eq!(labelled("ir", "Prim"), ["Nat", "Nat", "Nat"]);
    assert_eq!(labelled("tokens", "Arrow"), ["->"]);
    assert_eq!(labelled("tokens", "Dot"), ["."]);
}

/// The row forms — a `when`-named presence, and a named tail — checked through
/// every panel that renders them: the tokens they lex to, the field and tail
/// rows of both trees, the constraint the projection becomes, and the scheme
/// the definition ends with.
#[test]
fn rows_reach_every_stage() {
    let source = "@private let f : { x when 'a: Nat, y: Nat, ..'r } -> Nat = fn p => p.y\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered")
    };
    let labelled = |id: &str, label: &str| -> Vec<String> {
        nodes(stage(id))
            .into_iter()
            .filter(|node| node.label == label)
            .map(|node| node.text.clone())
            .collect()
    };

    assert_eq!(labelled("tokens", "DotDot"), [".."]);
    assert_eq!(
        labelled("tokens", "Identifier"),
        ["f", "x", "when", "Nat", "y", "Nat", "Nat", "p", "p", "y"]
    );

    for id in ["ast", "ir"] {
        assert_eq!(labelled(id, "x when 'a:"), ["Nat"], "{id}");
        assert_eq!(labelled(id, "Rest"), ["..'r"], "{id}");
        assert_eq!(labelled(id, "Project"), ["p.y"], "{id}");
    }

    let constraints: Vec<&str> = stage("constraints")
        .nodes
        .iter()
        .flat_map(|group| &group.children)
        .map(|node| node.text.as_str())
        .collect();
    assert!(
        constraints
            .iter()
            .any(|text| text.contains("..'r") && text.contains(".y")),
        "{constraints:?}"
    );

    let solve = stage("solve");
    assert!(
        nodes(solve)
            .iter()
            .any(|node| node.label == "struct" && node.text.contains("..'r")),
        "{solve:#?}"
    );

    let types: Vec<&str> = stage("types")
        .nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(types, ["{ x when 'a: Nat, y: Nat, ..'b } -> Nat"]);
}

/// `()` is one piece of punctuation in the surface syntax, and the AST keeps
/// it that way — a term position and a type position both read back as `()`.
/// Lowering folds both into the struct with no fields, so the IR reads every
/// occurrence back as `{}` instead, regardless of which namespace it came
/// from. The two trees therefore say different things about the same three
/// characters — which is the sort of difference the panels exist to show.
#[test]
fn unit_is_the_empty_struct_in_the_ir() {
    let snapshot = snapshot("type T = ()\nlet u : () = ()\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    // Every node the source spelled `()`, by label and by how the stage renders
    // it back — in the IR the two are no longer the same string.
    let unit_nodes = |id: &str| -> Vec<(String, String)> {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered");
        nodes(stage)
            .into_iter()
            .filter(|node| node.text == "()" || node.text == "{}")
            .map(|node| (node.label.clone(), node.text.clone()))
            .collect()
    };

    // Two type positions and one term; the parse tree calls all three Unit and
    // renders all three as the punctuation they were written as.
    assert_eq!(
        unit_nodes("ast"),
        [
            ("Unit".into(), "()".into()),
            ("Ascribed Unit".into(), "()".into()),
            ("Unit".into(), "()".into()),
        ] as [(String, String); 3]
    );
    // Lowering folds the unit value and the unit type alike into a fieldless
    // `Struct`, which reads back as `{}` rather than as what was written.
    assert_eq!(
        unit_nodes("ir"),
        [
            ("Struct".into(), "()".into()),
            ("Ascribed Struct".into(), "()".into()),
            ("Struct".into(), "()".into()),
        ] as [(String, String); 3]
    );
}

#[test]
fn a_bad_literal_is_a_diagnostic_of_its_own() {
    let codes = |source: &str| {
        snapshot(source)
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>()
    };
    // An invalid lexeme is retained as one invalid token. The parser consumes
    // that placeholder without inventing a second complaint about the same
    // bytes, and semantic phases never inspect its recovery tree.
    assert_eq!(codes("let n = 1x\n"), ["number-joined-to-name"]);
    assert_eq!(
        codes(&format!("let n = {}0n\n", u64::MAX)),
        ["whole-number-too-large"]
    );
}

#[test]
fn a_missing_match_end_points_to_the_match_without_repeating_the_fix() {
    let source = "let _ = match 1 with | f => ()\n\nlet a = 1";
    let snapshot = snapshot(source);
    let [diagnostic] = snapshot.diagnostics.as_slice() else {
        panic!("expected one diagnostic: {:#?}", snapshot.diagnostics);
    };
    assert_eq!(diagnostic.code, "expected-end");
    assert_eq!(diagnostic.message, "this match needs a closing `end`");
    assert_eq!(diagnostic.label, "write `end` before this");
    assert_eq!(
        diagnostic.span.unwrap().range[0],
        source.rfind("let").unwrap()
    );
    let [start] = diagnostic.related.as_slice() else {
        panic!("expected only the match location: {diagnostic:#?}");
    };
    let match_at = source.find("match").unwrap();
    assert_eq!(start.span.unwrap().range, [match_at, match_at + 5]);
    assert_eq!(start.message, "this match starts here");
}

#[test]
fn frontend_errors_keep_reader_advice_and_recovered_semantic_stages() {
    let snapshot = snapshot("let n = 1x\n");
    let [diagnostic] = snapshot.diagnostics.as_slice() else {
        panic!("expected one diagnostic: {:#?}", snapshot.diagnostics);
    };
    assert_eq!(diagnostic.stage, "lex");
    assert_eq!(diagnostic.code, "number-joined-to-name");
    assert!(
        diagnostic
            .message
            .starts_with("a number cannot run directly into a name"),
        "{}",
        diagnostic.message
    );
    assert_eq!(diagnostic.label, "the number and name are joined");
    assert_eq!(
        diagnostic.help,
        ["add a space, or start the whole name with a letter"]
    );

    for id in [
        "ir",
        "constraints",
        "solve",
        "types",
        "presence",
        "patterns",
        "lir",
        "artifact",
        "entry",
        "linked",
        "js",
        "symbols",
        "types-ir",
    ] {
        let stage = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"));
        let expected = if matches!(id, "lir" | "artifact" | "entry" | "linked" | "js") {
            Status::Skipped
        } else {
            Status::Partial
        };
        assert_eq!(stage.status, expected, "{id}: {stage:#?}");
    }
}

/// A repeat is only legible next to what it repeats, so it has to arrive as
/// one diagnostic with a related span rather than as two loose lines.
#[test]
fn a_duplicate_carries_the_definition_it_repeats() {
    let snapshot = snapshot("let x = ()\nlet x = ()\n");
    let duplicate = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate-term")
        .expect("the repeat is reported");
    assert_eq!(duplicate.related.len(), 1);
    assert_eq!(duplicate.related[0].span, at([4, 5]));
    assert_eq!(duplicate.related[0].message, "first defined here");
}

/// A name given two rests of different shapes is the same kind of complaint:
/// half of what went wrong is the `..` somewhere 'else on the page, so the strip
/// highlights that one too rather than leaving the reader to find it.
#[test]
fn a_mixed_tail_carries_the_use_it_clashes_with() {
    let source = "let f : { x: Nat, ..'r } -> (#A Nat | ..'r) -> Nat = fn a => fn b => 1n\n";
    let snapshot = snapshot(source);
    let mixed = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "mixed-tail")
        .unwrap_or_else(|| panic!("{:#?}", snapshot.diagnostics));
    // At the name rather than at the `..` in front of it: the name is the one
    // thing the reader can change, and is what the second use points back to.
    let first = source.find("..'r").expect("the first tail") + 2;
    assert_eq!(mixed.related.len(), 1);
    assert_eq!(mixed.related[0].span, at([first, first + 2]));
    // Worded as a use rather than as a definition: nothing here was defined
    // twice.
    assert_eq!(
        mixed.related[0].message,
        "first used as the rest of a struct's fields here"
    );
}

/// The types stage reports what inference concluded, and the annotating
/// stage carries the same conclusions keyed by the IR stage's own node ids.
/// That keying is the whole contract behind `annotates`, and the page's badge
/// painting is silently wrong rather than broken if it ever slips — which is
/// why it is asserted here rather than left to the eye.
#[test]
fn inferred_types_reach_the_panels() {
    let snapshot = snapshot("@private let id = fn x => x\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    let types = stage("types");
    assert_eq!(types.annotates, None);
    let scheme = nodes(types)
        .into_iter()
        .find(|node| node.label == "let id")
        .expect("the definition has a row");
    assert_eq!(scheme.text, "'a -> 'a");

    let badges = stage("types-ir");
    assert_eq!(badges.annotates, Some("ir"));
    assert!(!badges.nodes.is_empty());
    // Every badge decorates a row the IR stage actually renders.
    let ir_ids: std::collections::HashSet<_> =
        nodes(stage("ir")).into_iter().map(|node| node.id).collect();
    for badge in &badges.nodes {
        assert!(ir_ids.contains(&badge.id), "badge {} has no row", badge.id);
    }
    // The declaration row wears the scheme.
    let texts: Vec<_> = badges.nodes.iter().map(|node| node.text.as_str()).collect();
    assert!(texts.contains(&"'a -> 'a"), "{texts:?}");
}

/// Every row the IR stage renders a term on wears the type inference gave it.
/// The badges used to come from a second walk that mirrored the IR's child
/// layout by hand, so a change to one could silently stop the other; both now
/// fall out of the same walk, and this is the check that they still line up
/// across every shape a term can take.
#[test]
fn every_term_row_in_the_ir_wears_its_type() {
    let snapshot = snapshot("let f : { x: Nat } -> Nat = fn p => p.x\nlet n = f { x: 1n }\n");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };
    let badges: HashMap<u32, &str> = stage("types-ir")
        .nodes
        .iter()
        .map(|node| (node.id, node.text.as_str()))
        .collect();

    // Labels no type node ever carries, so every row reached here is a term.
    const TERMS: [&str; 5] = ["Apply", "Fn", "Project", "Natural", "Arg"];
    let rows = nodes(stage("ir"));
    let terms: Vec<&&Node> = rows
        .iter()
        .filter(|node| TERMS.contains(&node.label.as_str()))
        .collect();
    // `fn p => p.x`, its `p`, `p.x`, `f { x: 1 }` and the `1` inside it.
    assert_eq!(terms.len(), 5, "{terms:#?}");
    for node in &terms {
        assert!(
            badges.contains_key(&node.id),
            "{} {:?} wears no type",
            node.label,
            node.text
        );
    }

    let badge = |label: &str, text: &str| -> &str {
        let node = rows
            .iter()
            .find(|node| node.label == label && node.text == text)
            .unwrap_or_else(|| panic!("the IR renders no {label} {text:?}"));
        badges[&node.id]
    };
    // The bound name has no term of its own to carry a type; the lambda's
    // arrow is where it comes from.
    assert_eq!(badge("Arg", "p"), "{ x: Nat }");
    assert_eq!(badge("Project", "p.x"), "Nat");
    assert_eq!(badge("Apply", "f { x: 1n }"), "Nat");
    assert_eq!(badge("Natural", "1n"), "Nat");
}

/// A declared type is shown as what it stands for, one step deep, and says
/// under itself which declarations it leads back through. The row above can
/// only show the name coming back; whether that name leads anywhere is what
/// two types declared in terms of each other make impossible to read off.
#[test]
fn the_types_tab_says_which_declarations_are_recursive() {
    let snapshot = snapshot(
        "type list = { val: Nat, next: list }\n\
         type forest = { head: tree }\n\
         type tree = { val: Nat, kids: forest }\n\
         type Endo = Nat -> Nat\n\
         type Ptr 'a = Nat\n\
         type PhantomLoop = Ptr PhantomLoop\n",
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let types = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage is registered");
    let row = |name: &str| {
        types
            .nodes
            .iter()
            .find(|node| node.label == format!("type {name}"))
            .unwrap_or_else(|| panic!("no row for {name}"))
    };

    // One step deep: the name inside is still a name, which is the only way a
    // type that names itself can be rendered at all.
    assert_eq!(row("list").text, "{ val: Nat, next: list }");
    assert_eq!(row("Endo").text, "Nat -> Nat");

    let recursion = |name: &str| -> Option<&str> {
        row(name)
            .children
            .iter()
            .find(|child| child.label == "recursive")
            .map(|child| child.text.as_str())
    };
    // A declaration that names itself is a loop of one.
    assert_eq!(recursion("list"), Some("list"));
    // And one that does not is the case the row cannot show on its own:
    // neither of these mentions itself, and the loop runs through a
    // declaration written below the first of them.
    assert_eq!(recursion("forest"), Some("forest, tree"));
    assert_eq!(recursion("tree"), Some("tree, forest"));
    // And an alias that leads nowhere 'says nothing.
    assert_eq!(recursion("Endo"), None);
    // Merely spelling a name in a discarded/phantom alias argument is not a
    // semantic recursion edge.
    assert_eq!(recursion("Ptr"), None);
    assert_eq!(recursion("PhantomLoop"), None);
}

/// A definition says the same thing under itself, off the binding groups. A
/// scheme shows none of it — a definition solved with its group prints exactly
/// like one solved alone — so without the row the tab cannot say which
/// definitions had to be typed at once.
#[test]
fn the_types_tab_says_which_definitions_are_recursive() {
    let snapshot = snapshot(
        "@private let loops = fn x => loops x\n\
         @private let even = fn n => odd n\n\
         @private let odd = fn n => even n\n\
         @private let plain = fn x => x\n",
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let types = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage is registered");
    let recursion = |name: &str| -> Option<&str> {
        types
            .nodes
            .iter()
            .find(|node| node.label == format!("let {name}"))
            .unwrap_or_else(|| panic!("no row for {name}"))
            .children
            .iter()
            .find(|child| child.label == "recursive")
            .map(|child| child.text.as_str())
    };
    // A definition that names itself is a group of one, which the members
    // alone could not tell from the group below.
    assert_eq!(recursion("loops"), Some("loops"));
    // A pair typed together names itself first and then the rest of its group.
    assert_eq!(recursion("even"), Some("even, odd"));
    assert_eq!(recursion("odd"), Some("odd, even"));
    // And a definition that refers back into its group nowhere 'says nothing.
    assert_eq!(recursion("plain"), None);
}

/// A lambda's argument badge comes from the arrow the lambda has, which is an
/// arrow just as much when the annotation was a name for one.
#[test]
fn an_argument_wears_its_type_through_a_declared_type() {
    let badge = |source: &str| -> String {
        let snapshot = snapshot(source);
        assert!(
            snapshot.diagnostics.is_empty(),
            "{:#?}",
            snapshot.diagnostics
        );
        let stage = |id: &str| {
            snapshot
                .stages
                .iter()
                .find(|stage| stage.id == id)
                .unwrap_or_else(|| panic!("{id} is registered"))
        };
        let badges: HashMap<u32, &str> = stage("types-ir")
            .nodes
            .iter()
            .map(|node| (node.id, node.text.as_str()))
            .collect();
        let arg = nodes(stage("ir"))
            .into_iter()
            .find(|node| node.label == "Arg")
            .expect("the IR renders the bound name");
        badges[&arg.id].to_string()
    };

    assert_eq!(
        badge("type Endo = Nat -> Nat\nlet id : Endo = fn x => x\n"),
        "Nat"
    );

    // A declaration taking a row is where the badge is a type nobody wrote out:
    // the argument is spliced into the tail the declaration left open, and the
    // badge shows what that came to. `{}` allows nothing more, so the row is
    // closed and no `..` belongs on it — `∅` is the solver's mark for a row with
    // nothing left to come, and never part of a type a reader is shown.
    assert_eq!(
        badge("type F 'r = { x: Nat, ..'r } -> Nat\nlet f : F {} = fn p => p.x\n"),
        "{ x: Nat }"
    );
    // And a sum's tail reads in cases, because it is the row of a sum. Spelled
    // in braces it would show a reader a case list as if it were fields.
    assert_eq!(
        badge("type G 'r = (#Err Nat | ..'r) -> Nat\nlet g : G (#Ok Nat) = fn p => 1n\n"),
        "#Err Nat | #Ok Nat"
    );
}

/// The strip's messages are inference's own words. They were a copy of the
/// CLI driver's, and the two had already drifted apart on this very sentence —
/// `tests/src/inference.rs` pins the other end of it.
#[test]
fn extern_boundary_explanations_reach_the_debugger_with_cli_vocabulary() {
    let callback = snapshot(
        "effect Fail = () -> ()\n\
         extern install : fn(fn(()) -> () + !Fail) -> () = \"host.install\"\n",
    );
    let diagnostic = callback
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "callback-effects-not-covered")
        .expect("callback diagnostic");
    assert!(diagnostic.message.contains("extern declaration"));
    let explanation = diagnostic
        .inference_explanation
        .as_ref()
        .expect("callback explanation on wire");
    assert_eq!(
        explanation.contradiction.kind,
        "callback-effects-not-covered"
    );
    assert_eq!(
        explanation
            .full
            .iter()
            .map(|fact| fact.payload)
            .collect::<Vec<_>>(),
        [
            "callback-requirement",
            "extern-capability",
            "extern-declaration"
        ]
    );
    assert_eq!(explanation.full.len(), explanation.abridged.len());

    let polymorphic = snapshot("@private extern run : fn('a) -> Nat = \"host.run\"\n");
    assert!(
        polymorphic.diagnostics.is_empty(),
        "{:#?}",
        polymorphic.diagnostics
    );
    let unsupported = snapshot(
        "effect Tick = Nat -> Nat\nextern run: 'a -> Nat = \"host.run\"\nlet value = run (fn n => !Tick n)\n",
    );
    assert!(
        unsupported
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "runtime-type-information")
    );
}

#[test]
fn a_type_error_is_a_diagnostic() {
    let mismatch = snapshot("let n : Nat = fn x => x\n");
    let codes: Vec<_> = mismatch
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(codes, ["type-mismatch"]);
    let explanation = mismatch.diagnostics[0]
        .inference_explanation
        .as_ref()
        .expect("debugger mismatch explanation");
    assert!(!explanation.full.is_empty());
    assert!(!explanation.abridged.is_empty());
    assert_eq!(explanation.contradiction.kind, "incompatible-types");
    assert!(
        explanation
            .abridged
            .iter()
            .all(|fact| fact.span.is_some() && !fact.origin.is_empty())
    );

    let missing = snapshot("let f : { x: Nat } -> Nat = fn p => p.y\n");
    let [diagnostic] = missing.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", missing.diagnostics);
    };
    assert_eq!(diagnostic.code, "missing-field");
    assert_eq!(diagnostic.message, "no field `y` on `{ x: Nat }`");
    assert!(
        diagnostic.inference_explanation.is_none(),
        "one projection cannot supply its own opposing limiter"
    );

    let repeated_source = concat!(
        "let split : { x: Nat, ..'r } -> { ..'r } -> Nat = fn whole => fn rest => 0n\n",
        "let bad = fn value => split value { x: 1n }\n",
    );
    let repeated = snapshot(repeated_source);
    let explanation = repeated.diagnostics[0]
        .inference_explanation
        .as_ref()
        .expect("repeated row explanation crosses the full debugger path");
    assert_eq!(explanation.contradiction.kind, "repeated-label");
    assert_eq!(
        explanation.contradiction.repairs,
        ["change-first-use", "change-second-use"]
    );
    assert_eq!(
        explanation
            .abridged
            .iter()
            .map(|fact| fact.payload)
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(["label-introduction", "label-forbidden"]),
    );
    assert!(explanation.abridged.iter().all(|fact| fact.span.is_some()));

    let calls = snapshot("let repeated = fn use => { first: use 1n, second: use false }\n");
    let diagnostic = &calls.diagnostics[0];
    let explanation = diagnostic
        .inference_explanation
        .as_ref()
        .expect("repeated calls keep the shared debugger explanation");
    let pivot = explanation.pivot.as_ref().expect("named shared input");
    assert_eq!(pivot.kind, "function-input");
    assert_eq!(pivot.name, "Input");
    assert_eq!(
        explanation.omitted_facts,
        explanation.full.len() - explanation.abridged.len()
    );
    assert_eq!(diagnostic.label.matches("Let’s call").count(), 1);
    assert_eq!(
        diagnostic
            .notes
            .iter()
            .filter(|note| note.contains("omitted"))
            .count(),
        usize::from(explanation.omitted_facts > 0),
        "the shared diagnostic prose and debugger account must agree"
    );

    let projection = snapshot("let value = { x: 1n }\nlet bad : Boolean = value.x\n");
    let projection = projection.diagnostics[0]
        .inference_explanation
        .as_ref()
        .expect("projection keeps its debugger explanation");
    assert_eq!(
        projection.pivot.as_ref().map(|pivot| pivot.kind),
        Some("projected-field")
    );
    assert!(
        projection
            .pivot
            .as_ref()
            .is_some_and(|pivot| pivot.references.len() >= 2)
    );

    let effects = snapshot(concat!(
        "effect Log = { write: Nat -> () }\n",
        "let split : (() -> () + !Log + ..'r) -> (() -> () + ..'r) -> Nat = fn whole => fn rest => 0n\n",
        "let bad = fn action => split action (fn _ => !Log.write 0n)\n",
    ));
    let effects = effects.diagnostics[0]
        .inference_explanation
        .as_ref()
        .expect("effect contradiction keeps its debugger explanation");
    assert!(effects.pivot.is_none());
    assert!(effects.full.windows(2).all(|facts| {
        let [one, two] = facts else { return true };
        one.span.as_ref().map(|span| (span.file, span.range))
            <= two.span.as_ref().map(|span| (span.file, span.range))
    }));

    let recursive = snapshot("let bad = fn f => f f\n");
    let explanation = recursive.diagnostics[0]
        .inference_explanation
        .as_ref()
        .expect("recursive cycle keeps its debugger explanation");
    assert_eq!(explanation.contradiction.kind, "recursive-value");
    assert!((2..=4).contains(&explanation.abridged.len()));
    assert!(explanation.full.len() >= explanation.abridged.len());
    assert!(explanation.abridged.iter().all(|fact| fact.span.is_some()));
    assert!(!explanation.cause.constraint_ids.is_empty());
    assert!(!explanation.cause.reason_ids.is_empty());
    assert_eq!(
        explanation.contradiction.repairs,
        ["change-first-use", "change-second-use"]
    );
}

/// Projection failures keep their shape-specific diagnostics and spans in the
/// debugger, and the failed solver steps carry the same errors as the strip.
#[test]
fn a_row_error_reaches_the_strip_and_the_solve_tab() {
    let not_struct_source = "let bad = 1n.x\n";
    let not_struct = snapshot(not_struct_source);
    let [diagnostic] = not_struct.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", not_struct.diagnostics);
    };
    assert_eq!(diagnostic.code, "not-a-struct");
    let base = not_struct_source.find("1n").unwrap();
    assert_eq!(diagnostic.span, at([base, base + 2]));

    let missing_source = "let bad : { x: Nat } -> Nat = fn p => p.y\n";
    let missing = snapshot(missing_source);
    let [diagnostic] = missing.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", missing.diagnostics);
    };
    assert_eq!(diagnostic.code, "missing-field");
    let field = missing_source.rfind('y').unwrap();
    assert_eq!(diagnostic.span, at([field, field + 1]));

    let rigid_source = "let bad : 'a -> Nat = fn p => p.x\n";
    let rigid = snapshot(rigid_source);
    let [diagnostic] = rigid.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", rigid.diagnostics);
    };
    assert_eq!(diagnostic.code, "rigid-field");
    assert_eq!(
        diagnostic.message,
        "the body cannot assume caller-chosen field `x`"
    );
    assert_eq!(
        diagnostic.label,
        "this annotation leaves the choice to each caller"
    );
    assert_eq!(
        diagnostic.help,
        [
            "read caller-chosen struct fields only when named explicitly before the annotation's `..` remainder",
            "or add this field explicitly to the annotation"
        ]
    );
    assert_eq!(
        diagnostic.related[0].message,
        "this reads field `x` from the caller's choice"
    );
    let declared = rigid_source.find("'a").unwrap();
    assert_eq!(diagnostic.span, at([declared, declared + 2]));
    let field = rigid_source.rfind('x').unwrap();
    assert_eq!(diagnostic.related[0].span, at([field, field + 1]));
    let explanation = diagnostic
        .inference_explanation
        .as_ref()
        .expect("structured caller-choice explanation");
    assert_eq!(explanation.contradiction.kind, "caller-choice");
    assert_eq!(explanation.abridged.len(), 2);

    for snapshot in [&not_struct, &missing, &rigid] {
        let constraints = stage_named(snapshot, "constraints");
        assert!(
            nodes(constraints)
                .iter()
                .any(|node| node.label == "project"),
            "{constraints:#?}"
        );
        let solve = stage_named(snapshot, "solve");
        assert!(nodes(solve).iter().any(|node| node.error), "{solve:#?}");
    }
}

/// The solver assumes a goal it is already in the middle of, and what it is
/// keyed on is the whole goal — both declared types with their arguments. The
/// Solve tab shows that key without being given anything of its own: a step's
/// goal is the row's text, so the assumption stack has nothing left to say.
/// A key a reader cannot see is one nobody can check the assumption against.
#[test]
fn an_assumed_step_shows_the_goal_it_was_keyed_on() {
    let snapshot = snapshot(
        "type Tree 'a = { value: 'a, kids: Forest }\n\
         type Forest = { head: Tree Nat, tail: Forest }\n\
         type Wood 'a = { value: 'a, kids: Grove }\n\
         type Grove = { head: Wood Nat, tail: Grove }\n\
         let f : Forest -> Grove = fn x => x\n",
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    let solve = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    let texts: Vec<&str> = nodes(solve)
        .iter()
        .filter(|node| node.label == "assume")
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(texts, ["Grove ~ Forest", "Grove ~ Forest"]);

    // And the row where the arguments are what is being decided reads as the
    // types the reader wrote, applications and all.
    let unfolded: Vec<&str> = nodes(solve)
        .iter()
        .filter(|node| node.label == "unfold")
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(unfolded, ["Grove ~ Forest", "Wood Nat ~ Tree Nat"]);
}

#[test]
fn symbols_round_trip_through_the_mangler() {
    let snapshot = snapshot("let f = fn x => x\ntype T = ()\n");
    let symbols = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage is registered");
    // The counts a bundle made worth reporting: how many files the symbols were
    // declared across, and how many of them are modules.
    assert_eq!(symbols.summary, "3 symbols · 1 file · 0 modules");
    for node in nodes(symbols) {
        let demangle = node
            .fields
            .iter()
            .find(|field| field.name == "demangle")
            .expect("every row checks itself");
        assert_eq!(demangle.value, "ok", "{} did not round-trip", node.label);
        assert!(!node.error);
    }

    // Each row's text is the path, and the lambda's `x` is a local: it belongs
    // to no module, so the panel shows it one segment in rather than beside
    // the top-level `f` it is not addressable alongside.
    let paths: Vec<(&str, &str)> = nodes(symbols)
        .iter()
        .map(|node| (node.label.as_str(), node.text.as_str()))
        .collect();
    // In mint order, which is the row order: types are lowered first, and every
    // definition's name is minted before any body is lowered — so `f` comes
    // before the `x` its own lambda binds, and `x` is shown inside `f`.
    assert_eq!(
        paths,
        [("T", "demo::T"), ("f", "demo::f"), ("x", "demo::f::_::x")]
    );
}

/// The compiler will panic while it is being worked on. When it does, the
/// panic has to become a result rather than take the server with it.
#[test]
fn a_panic_becomes_a_result() {
    install_hook();
    let mut slot = None;
    let value = guard("ir", &mut slot, || panic!("the name table lied"));

    assert!(value.is_none());
    let panicked = slot.expect("the panic was captured");
    assert_eq!(panicked.stage, "ir");
    assert_eq!(panicked.message, "the name table lied");
    assert!(panicked.location.contains("snapshot.rs"));

    // Only the first panic is kept: the ones after it are the same bug seen
    // from a stage that was handed nothing.
    let mut slot = Some(panicked);
    guard("ast", &mut slot, || panic!("second"));
    assert_eq!(slot.expect("still the first").stage, "ir");
}

#[test]
fn a_snapshot_survives_the_wire() {
    let source = clean_demo();
    let snapshot = bundle(&[(ROOT, source)]);
    let json = serde_json::to_string(&snapshot).expect("serializes");
    let back: serde_json::Value = serde_json::from_str(&json).expect("parses");

    // Every registered stage reaches the page, annotators included: the count
    // comes from the registry so that adding a stage cannot quietly leave one
    // off the wire without also being noticed here.
    assert_eq!(
        back["stages"].as_array().expect("stages").len(),
        REGISTRY.len()
    );
    assert_eq!(back["stages"][0]["view"], "terminal");
    assert_eq!(back["stages"][1]["view"], "list");
    assert_eq!(back["stages"][2]["view"], "tree");
    let artifact = back["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|stage| stage["id"] == "artifact")
        .expect("the artifact stage is registered");
    assert_eq!(
        artifact["views"]
            .as_array()
            .expect("artifact views")
            .iter()
            .map(|view| view.as_str())
            .collect::<Vec<_>>(),
        [Some("text"), Some("tree")]
    );
    // The file strip and everything a `Loc` is read against: one entry per file
    // the loader read, each carrying what the page turns an offset into a line
    // and a column with.
    let files = back["files"].as_array().expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], ROOT);
    assert_eq!(files[0]["len"], source.len());
    assert!(
        files[0]["line_starts"]
            .as_array()
            .expect("line starts")
            .len()
            > 1
    );
    // And the externally supplied identity, for the chip.
    assert_eq!(back["bundle"], "demo@0.1.0");
    // Underscored fields are the page's, and have to survive too: the
    // editor's colouring is built from them.
    assert!(json.contains("_class"));

    // Everything the pane bar and the highlighter read off a stage. A field
    // the page branches on is no use to it left behind on this side.
    let types = back["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|stage| stage["id"] == "types")
        .expect("the types stage is registered");
    assert_eq!(types["scoped"], true);
    // A stage owning no phase says so on the wire rather than reporting a zero
    // the page has to guess the meaning of.
    let solve = back["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .find(|stage| stage["id"] == "solve")
        .expect("the solve stage is registered");
    assert!(solve["micros"].is_null());
    assert!(types["micros"].as_u64().is_some());
    assert!(types["summary"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(json.contains("\"owner\""));
}

/// An empty root is a program with nothing in it, not a program with something
/// wrong with it; identity is supplied independently.
#[test]
fn an_empty_buffer_is_not_an_error() {
    let snapshot = snapshot("");
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    assert!(snapshot.panic.is_none());
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].len, 0);
    assert_eq!(snapshot.files[0].line_starts, vec![0]);
}

/// Bad externally supplied configuration is not a reason to stop compiling:
/// it is reported, the mint falls back, and every later phase still runs.
#[test]
fn a_bad_bundle_is_reported_rather_than_fatal() {
    for (name, version, code) in [
        ("_x", "0.1.0", "project-name-invalid"),
        ("demo", "not-a-version", "project-version-invalid"),
        ("demo", "1.2.3+unsupported", "project-version-build-suffix"),
    ] {
        let snapshot = compile(
            &CompileRequest {
                kind: ruddy::artifact::Kind::Library,
                target: None,
                platform: None,
                name: name.to_string(),
                version: version.to_string(),
                root: ROOT.to_string(),
                document: "demo".to_string(),
                files: vec![FileSpec {
                    path: ROOT.to_string(),
                    source: "let x = ()".to_string(),
                }],
                std: StdConfig::Disabled,
                dependencies: IndexMap::new(),
                revision: 3,
            },
            1,
        );
        assert_eq!(snapshot.diagnostics[0].code, code);
        assert!(!snapshot.diagnostics[0].help.is_empty());
        // And the chip has nothing to show because the supplied identity was
        // invalid.
        assert_eq!(snapshot.bundle, None);
        // The fallback bundle still lowers the program.
        let ir = snapshot
            .stages
            .iter()
            .find(|stage| stage.id == "ir")
            .expect("ir stage");
        assert_eq!(ir.nodes.len(), 1);
    }
}

/// Module file candidates are relative to the configured root's directory,
/// including when the in-memory debugger filesystem names that directory.
#[test]
fn a_nested_debugger_root_resolves_module_files_beside_its_root() {
    let snapshot = compile(
        &CompileRequest {
            kind: ruddy::artifact::Kind::Library,
            target: None,
            platform: None,
            name: "demo".into(),
            version: "0.1.0".into(),
            root: "src/main.rud".into(),
            document: "demo".into(),
            files: vec![
                FileSpec {
                    path: "src/main.rud".into(),
                    source: "module Math\n".into(),
                },
                FileSpec {
                    path: "src/Math.rud".into(),
                    source: "let four = 4n\n".into(),
                },
            ],
            std: StdConfig::Disabled,
            dependencies: IndexMap::new(),
            revision: 1,
        },
        1,
    );
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    assert_eq!(
        snapshot
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["src/main.rud", "src/Math.rud"]
    );
}

/// The file list is the index every `Loc` on the wire points into, so it has to
/// be the files the *loader* read rather than the files the request carried:
/// root first, then depth-first through the modules, which is also the order the
/// page shows its file strip in.
#[test]
fn a_bundle_lists_every_file_the_loader_read() {
    let snapshot = bundle(NESTED);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let files: Vec<(&str, usize)> = snapshot
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.len))
        .collect();
    assert_eq!(
        files,
        [
            (ROOT, NESTED[0].1.len()),
            ("Math.rud", NESTED[1].1.len()),
            ("Math/Vec.rud", NESTED[2].1.len()),
        ]
    );
    // Each with what the page turns an offset in it into a line and a column.
    assert_eq!(snapshot.files[2].line_starts, vec![0, 14]);

    // And the chip reports the identity supplied independently of all files.
    assert_eq!(snapshot.bundle.as_deref(), Some("demo@0.1.0"));
}

/// A range means nothing without the file it is a range in. A node written in a
/// module file has to name that file, and so does a complaint about one —
/// otherwise the page reveals the right offset of the wrong file, which is
/// worse than revealing nothing.
#[test]
fn a_span_from_a_module_file_names_that_file() {
    let snapshot = bundle(&[(ROOT, "module Math\n"), ("Math.rud", "let double = nope\n")]);

    // Every token of a file's row is a span in that file, which is the whole of
    // what the index has to get right.
    let tokens = stage_named(&snapshot, "tokens");
    assert_eq!(tokens.nodes.len(), 2);
    for (index, file) in tokens.nodes.iter().enumerate() {
        for token in &file.children {
            let at = token.span.expect("a token was written somewhere");
            assert_eq!(at.file, index as u32, "{} {:?}", token.label, token.text);
        }
    }

    // And the name the module file could not resolve is reported in the module
    // file, at the offsets that file spells it with rather than the root's.
    let [diagnostic] = snapshot.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", snapshot.diagnostics);
    };
    assert_eq!(diagnostic.code, "undefined-term");
    assert_eq!(
        diagnostic.span,
        Some(Loc {
            file: 1,
            range: [13, 17]
        })
    );
}

/// The loader's own complaints reach the strip like every other phase's, worded
/// by `ruddy::ui` and coded the same way the driver codes them — and pointed at
/// the declaration that named the file, which is the one place the reader can
/// fix it.
#[test]
fn a_missing_module_file_reaches_the_strip() {
    let root = "module Math\nlet four = 4n\n";
    let snapshot = bundle(&[(ROOT, root)]);

    let [diagnostic] = snapshot.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", snapshot.diagnostics);
    };
    assert_eq!(diagnostic.stage, "bundle");
    assert_eq!(diagnostic.code, "module-file-missing");
    assert_eq!(diagnostic.message, "this module needs a file");
    assert_eq!(diagnostic.label, "no file was found for this module");
    assert_eq!(diagnostic.help, ["create `Math.rud` or `Math/module.rud`"]);
    assert_eq!(diagnostic.notes.len(), 1);
    // At the name, which is what the file's path was spelled from.
    let at = root.find("Math").expect("the declaration");
    assert_eq!(
        diagnostic.span,
        Some(Loc {
            file: 0,
            range: [at, at + "Math".len()]
        })
    );

    // Missing modules preserve the current recovery tree for semantic tooling.
    // Its unresolved names remain explicit; backend lowering stays gated.
    let ast = stage_named(&snapshot, "ast");
    assert!(!ast.nodes.is_empty(), "{ast:#?}");
    let ir = stage_named(&snapshot, "ir");
    assert_eq!(ir.status, Status::Partial, "{ir:#?}");
    let types = stage_named(&snapshot, "types");
    assert_eq!(types.status, Status::Partial, "{types:#?}");
}

#[test]
fn two_module_files_explain_how_to_choose_one() {
    let root = "module Math\n";
    let snapshot = bundle(&[
        (ROOT, root),
        ("Math.rud", "let beside = 1n\n"),
        ("Math/module.rud", "let inside = 2n\n"),
    ]);
    let [diagnostic] = snapshot.diagnostics.as_slice() else {
        panic!("expected one error: {:#?}", snapshot.diagnostics);
    };
    assert_eq!(diagnostic.code, "module-file-ambiguous");
    assert_eq!(diagnostic.message, "this module has two possible files");
    assert_eq!(
        diagnostic.label,
        "Ruddy cannot choose which file defines this module"
    );
    assert_eq!(
        diagnostic.help,
        ["keep one of `Math.rud` or `Math/module.rud` and delete the other"]
    );
}

/// A document is a bundle, and a bundle starts somewhere. Only the debugger
/// knows the page was meant to have put a root file in the request — the loader
/// reads a file that is not there as an empty one — so the debugger is what says
/// so, rather than leaving the reader with a blank page and no reason for it.
#[test]
fn a_request_without_a_root_file_is_told_so() {
    let snapshot = bundle(&[("Math.rud", "let double = fn x => x\n")]);
    let codes: Vec<&str> = snapshot
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(codes.contains(&"project-root-missing"), "{codes:?}");
    let missing = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "project-root-missing")
        .expect("the complaint is there");
    assert_eq!(missing.stage, "bundle");
    assert_eq!(
        missing.message,
        "this document needs its root file `main.rud`"
    );
    assert_eq!(
        missing.help,
        ["create `main.rud` or choose another root file"]
    );
    // Nowhere to point at: there is no file for the missing one to be missing
    // from.
    assert_eq!(missing.span, None);

    // The root the loader read in its place is the empty one it invented, so
    // the file strip has one tab and the orphan module file is not in it.
    let files: Vec<&str> = snapshot.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, [ROOT]);
    assert_eq!(snapshot.bundle.as_deref(), Some("demo@0.1.0"));
    assert!(snapshot.panic.is_none());
}

/// The `Solve` tab is a timeline the page walks with a cursor, building its two
/// state panels by appending each step's `_bind` and `_error` as it passes
/// them. That only works if those fields appear exactly on the steps that
/// changed something, and if a binding is only ever added — never rewritten,
/// because stepping backwards is dropping the tail of the list.
#[test]
fn a_solver_step_declares_what_it_added_to_the_state() {
    let snapshot = snapshot(
        "let fst : { x: Nat } -> Nat = fn p => p.x\nlet miss : { x: Nat } -> Nat = fn p => p.y\n",
    );
    let stage = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("the solve stage is registered");
    assert!(matches!(stage.view, View::Steps), "{:?}", stage.view);
    assert!(!stage.nodes.is_empty());

    let field = |node: &Node, name: &str| {
        node.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };

    let mut bound = Vec::new();
    let mut failed = Vec::new();
    for node in &stage.nodes {
        if field(node, "_record").as_deref() == Some("metadata") {
            continue;
        }
        // Everything the page reads off every step, on every step.
        for name in ["_rule", "_effect", "_depth", "_def"] {
            assert!(field(node, name).is_some(), "{} has no {name}", node.label);
        }
        let depth = field(node, "_depth").expect("a depth");
        assert!(depth.parse::<u32>().is_ok(), "depth {depth:?}");

        // A step that failed is the red one, and the only one carrying an
        // error: the page paints from `error` and accumulates from `_error`,
        // so the two cannot be allowed to disagree.
        assert_eq!(
            node.error,
            field(node, "_error").is_some(),
            "{} is {} but {} an error",
            node.label,
            if node.error { "red" } else { "not red" },
            if node.error { "carries no" } else { "carries" },
        );
        if let Some(bind) = field(node, "_bind") {
            // The solution panel is the `_effect` column's bindings collected,
            // so a step's `_bind` is its `_effect` and not a second wording of
            // it. The two had been written out separately, identically, which
            // is two places for one notation to drift from.
            assert_eq!(field(node, "_effect"), Some(bind.clone()), "{}", node.label);
            bound.push((field(node, "_def").unwrap(), bind));
        }
        if let Some(error) = field(node, "_error") {
            failed.push(error);
        }
    }

    // Appended, never rewritten.
    let mut once = bound.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), bound.len(), "{bound:?}");

    // What the reader would have collected by the end is what inference
    // reported: one error, said the same way in both places.
    let reported: Vec<&str> = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.stage == "types")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(failed, reported);
}

/// The pane bar prints one timing chip per compiler phase, and which stages own
/// a phase is a fact about the registry rather than something to be read off a
/// number. `Constraints` and `Solve` are two views of the one `infer` call and
/// the annotator does no phase at all, so all three report nothing; everyone
/// else reports what their phase took. The page filtered on `micros > 0`
/// instead, which is a measurement standing in for that fact — and the
/// measurement truncates to whole microseconds, so a phase quick enough to
/// round to zero silently lost its chip.
#[test]
fn only_the_stages_that_own_a_phase_report_a_time() {
    let clean = snapshot("@private let f = fn a => a\n");
    let snapshot = snapshot("let bad : Nat = fn x => x\n");
    let ids = |timed: bool| -> Vec<&str> {
        snapshot
            .stages
            .iter()
            .filter(|stage| stage.micros.is_some() == timed)
            .map(|stage| stage.id)
            .collect()
    };
    assert_eq!(
        ids(true),
        [
            "tokens",
            "dependencies",
            "ast",
            "ir",
            "types",
            "presence",
            "patterns",
            "symbols"
        ]
    );
    // LIR owns a phase too, and reports nothing here for the other reason a
    // stage can: inference rejected this source, so lowering never ran and
    // there is no duration to report rather than no phase to have one.
    assert_eq!(
        ids(false),
        [
            "errors",
            "externs",
            "constraints",
            "solve",
            "reification",
            "lir",
            "artifact",
            "entry",
            "linked",
            "js",
            "types-ir"
        ]
    );

    // On a program with nothing wrong with it, it reports one like everybody
    // else — which is what makes the line above about the demo rather than
    // about the registry.
    let lir = clean
        .stages
        .iter()
        .find(|stage| stage.id == "lir")
        .expect("the lir stage is registered");
    assert_eq!(lir.status, Status::Ok);
    assert!(lir.micros.is_some());
    let artifact = clean
        .stages
        .iter()
        .find(|stage| stage.id == "artifact")
        .expect("the artifact stage is registered");
    assert_eq!(artifact.status, Status::Ok);
    assert!(artifact.micros.is_some());
    let linked = clean
        .stages
        .iter()
        .find(|stage| stage.id == "linked")
        .expect("the link stage is registered");
    assert_eq!(linked.status, Status::Ok);
    assert!(linked.micros.is_some());
    let javascript = clean
        .stages
        .iter()
        .find(|stage| stage.id == "js")
        .expect("the JavaScript stage is registered");
    assert_eq!(javascript.status, Status::Ok);
    assert!(javascript.micros.is_some());
    assert!(
        javascript
            .text
            .as_deref()
            .is_some_and(|source| !source.is_empty())
    );
    assert!(
        artifact
            .text
            .as_ref()
            .is_some_and(|text| text.starts_with("(artifact\n  (header"))
    );
}

/// A tab's raw view dumps what that tab owns, and no more.
/// `inference::Output` is one struct carrying three tabs' worth of material,
/// and `Types` had been dumping the whole of it — so the constraint list and
/// every solver step crossed the wire twice, on every keystroke, since each one
/// posts a fresh snapshot.
#[test]
fn a_raw_dump_carries_only_its_own_tab() {
    let snapshot = snapshot("let fst : { x: Nat } -> Nat = fn p => p.x\n");
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    // Each of the three payloads, dumped by the one tab that shows it.
    let types = stage("types");
    assert!(types.debug.contains("schemes"), "{}", types.debug);
    assert!(stage("constraints").debug.contains("Constraint"));
    assert!(stage("solve").debug.contains("Step"));

    // And not by the other two.
    assert!(
        !types.debug.contains("Step"),
        "the Types dump repeats the solve"
    );
    assert!(
        !types.debug.contains("Constraint"),
        "the Types dump repeats the constraints"
    );
}

#[test]
fn zero_step_solves_still_publish_machine_readable_arenas() {
    let snapshot = snapshot("");
    let solve = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "solve")
        .expect("solve stage");
    assert_eq!(solve.summary, "0 steps");
    assert_eq!(solve.nodes.len(), 1);
    let metadata = &solve.nodes[0];
    let field = |name| {
        metadata
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.as_str())
    };
    assert_eq!(field("_record"), Some("metadata"));
    for arena in ["_variables", "_reasons"] {
        let value: serde_json::Value = serde_json::from_str(field(arena).unwrap()).unwrap();
        assert!(value.is_array());
    }
}

#[test]
fn inference_stage_rows_serialize_compiler_identities() {
    let source = "effect Fail = { abort: () -> () }\n\
                  type Callback = () -> () + !Fail\n\
                  extern install : fn(Callback) -> () + !Fail = \"host.install\"\n\
                  let choose : { x when 'a: Nat, y when 'b: Nat } -> Nat where 'a != 'b = fn v => match v with | {x} => x | {y} => y end\n\
                  let local = fn tag => match tag with | {a} => do let g = fn w => choose w return g {} end | {b} => 0n end\n\
                  let bad = (1n).missing\n";
    let snapshot = snapshot(source);
    let stage = |id| snapshot.stages.iter().find(|stage| stage.id == id).unwrap();
    let field = |node: &Node, name| {
        node.fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
    };

    let constraint_rows = nodes(stage("constraints"));
    let constraint_ids: std::collections::HashSet<_> = constraint_rows
        .iter()
        .filter_map(|node| field(node, "_constraint_id"))
        .collect();
    assert!(!constraint_ids.is_empty());
    assert!(constraint_rows.iter().all(|node| {
        field(node, "_constraint_id").is_none()
            || (field(node, "_reason_id").is_some()
                && field(node, "_origin").is_some()
                && field(node, "_primary_subject").is_some())
    }));
    assert!(constraint_rows.iter().any(|node| {
        field(node, "_origin").as_deref() == Some("callback-boundary")
            && field(node, "_primary_subject").as_deref() == Some("callback-required")
            && field(node, "_secondary_subject").as_deref() == Some("callback-available")
    }));

    let solve = nodes(stage("solve"));
    let metadata = solve
        .iter()
        .find(|node| field(node, "_record").as_deref() == Some("metadata"))
        .expect("always-present inference metadata record");
    let steps: Vec<_> = solve
        .iter()
        .copied()
        .filter(|node| field(node, "_record").is_none())
        .collect();
    let step_ids: std::collections::HashSet<_> = steps
        .iter()
        .filter_map(|node| field(node, "_step_id"))
        .collect();
    assert_eq!(step_ids.len(), steps.len());
    let reason_ids: std::collections::HashSet<_> = steps
        .iter()
        .filter_map(|node| field(node, "_reason_id"))
        .collect();
    assert_eq!(reason_ids.len(), steps.len());
    assert!(
        steps
            .iter()
            .filter(|node| field(node, "_bind").is_some())
            .all(|node| field(node, "_bind_by") == field(node, "_reason_id"))
    );
    assert!(
        steps
            .iter()
            .any(|node| field(node, "_recovery_because").is_some())
    );
    assert!(
        steps.iter().all(
            |node| field(node, "_constraint_id").is_some_and(|id| constraint_ids.contains(&id))
        )
    );
    let variables: serde_json::Value = serde_json::from_str(
        &field(metadata, "_variables").expect("machine-readable variable arena"),
    )
    .unwrap();
    assert!(variables.as_array().unwrap().iter().all(|variable| {
        variable.get("sort").is_some()
            && variable.get("subject").is_some()
            && variable.get("minted_by").is_some()
    }));
    let reasons: serde_json::Value =
        serde_json::from_str(&field(metadata, "_reasons").expect("machine-readable reason arena"))
            .unwrap();
    assert!(reasons.as_array().unwrap().iter().all(|reason| {
        reason.get("parents").is_some()
            && reason.get("origin").is_some()
            && reason.get("reachable").is_some()
    }));
    let guarded_results: Vec<_> = constraint_rows
        .iter()
        .filter(|node| field(node, "_origin").as_deref() == Some("match-arm"))
        .collect();
    assert!(!guarded_results.is_empty());
    assert!(guarded_results.iter().any(|result| {
        result
            .span
            .is_some_and(|span| matches!(&source[span.range[0]..span.range[1]], "x" | "y"))
    }));
    for result in guarded_results {
        assert_eq!(
            field(result, "_primary_subject").as_deref(),
            Some("match-result")
        );
        assert_eq!(
            field(result, "_secondary_subject").as_deref(),
            Some("match-arm")
        );
        let span = result
            .span
            .expect("a guarded result points at its arm body");
        assert!(!source[span.range[0]..span.range[1]].is_empty());
        let id = field(result, "_constraint_id").expect("guarded result identity");
        assert!(
            solve
                .iter()
                .any(|step| field(step, "_constraint_id").as_deref() == Some(id.as_str()))
        );
    }

    let batches = nodes(stage("presence"))
        .into_iter()
        .filter_map(|node| field(node, "_batch_id"))
        .collect::<std::collections::HashSet<_>>();
    assert!(!batches.is_empty());
    let deferred = nodes(stage("constraints"))
        .into_iter()
        .filter_map(|node| field(node, "_batch_id"))
        .collect::<std::collections::HashSet<_>>();
    assert!(!deferred.is_empty());
    assert!(deferred.is_subset(&batches));

    let diagnostic = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| {
            matches!(
                &diagnostic.inference_cause,
                Some(InferenceCause::Step { .. })
            )
        })
        .expect("a solve-caused inference diagnostic");
    let error_id = diagnostic
        .inference_error_id
        .clone()
        .expect("stable inference error identity");
    let Some(InferenceCause::Step { step_id }) = diagnostic.inference_cause.as_ref() else {
        panic!("ordinary inference error should carry its step cause");
    };
    let linked = solve
        .iter()
        .find(|node| field(node, "_step_id").as_deref() == Some(&step_id.to_string()))
        .expect("diagnostic cause resolves to a solve row");
    assert_eq!(field(linked, "_error_id"), Some(error_id.to_string()));

    let wire = serde_json::to_value(&snapshot).expect("snapshot serializes");
    let serialized = wire["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["inference_error_id"] == error_id.as_str())
        .expect("stable inference metadata reaches the wire");
    assert!(serialized.get("id").is_some(), "display id remains present");
    assert_eq!(serialized["inference_cause"]["kind"], "step");
    assert_eq!(serialized["inference_cause"]["step_id"], *step_id);
}

#[test]
fn the_types_raw_dump_asserts_package_and_owned_metadata() {
    let snapshot = snapshot(
        "extern choose: { left when 'a: Nat, right when 'b: Nat } where 'a != 'b = \"host.choose\"\n",
    );
    let types = snapshot
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .unwrap();
    assert!(types.debug.contains("packages=1"), "{}", types.debug);
    assert!(types.debug.contains("owned=[0]"), "{}", types.debug);
}

/// Summaries sit in the pane bar as a phrase somebody reads, so the count and
/// its noun agree. Three stages had spelled that out for themselves and the
/// rest had not, which is how `1 schemes` reached the bar of a tool whose whole
/// subject is getting the details right.
#[test]
fn a_count_of_one_is_said_in_the_singular() {
    let snapshot = snapshot("let a : Nat = 1n\n");
    let summary = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
            .summary
            .as_str()
    };
    assert_eq!(summary("constraints"), "1 constraint");
    assert_eq!(summary("solve"), "1 step");
    assert_eq!(summary("types"), "1 scheme");
    assert_eq!(summary("symbols"), "1 symbol · 1 file · 0 modules");
    assert_eq!(summary("ir"), "0 effects · 0 types · 1 term · 1 group");
}

/// A parameterized declaration's meaning prints its parameters as `a`, `'b` —
/// which says nothing on its own about which is which. The Types tab carries a
/// row per parameter mapping each letter back to the name it was written as,
/// and without them the tab is unreadable for exactly the declarations that
/// most need reading.
#[test]
fn the_types_tab_maps_each_letter_back_to_its_parameter() {
    let snap = snapshot("type Pair 'A 'B = { first: 'A, second: 'B }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage");

    let rows = nodes(stage);
    let pair = rows
        .iter()
        .find(|node| node.label == "type Pair")
        .expect("a row for the declaration");
    assert_eq!(pair.text, "{ first: 'a, second: 'b }");

    let letters: Vec<(&str, &str)> = pair
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(letters, vec![("'a", "'A"), ("'b", "'B")]);
}

/// The IR tab shows a `type` declaration's binders, as the AST tab and the
/// Types tab both do. Without them the tab was the one panel where 'clicking a
/// parameter lit nothing up — the body's uses of it carry the symbol, and the
/// binder they point back at had no row to be pointed at.
#[test]
fn the_ir_tab_shows_a_declarations_parameters() {
    let snap = snapshot("type Ghost 'a = Nat\ntype WithX 'r = { x: Nat, ..'r }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "ir")
        .expect("the ir stage");

    let params: Vec<(&str, Option<Loc>)> = nodes(stage)
        .iter()
        .filter(|node| node.label == "Param")
        .map(|node| (node.text.as_str(), node.span))
        .collect();
    // What each stands for, beside the span it was written at — a row says so,
    // and says what it may not stand for.
    assert_eq!(
        params,
        [
            ("'a", at([11, 13])),
            ("..'r (struct) without x", at([31, 33]))
        ]
    );

    // And each cross-highlights, which is the whole reason the row is here.
    for node in nodes(stage).iter().filter(|node| node.label == "Param") {
        assert!(node.symbol.is_some(), "{node:#?}");
    }

    // A parameter that is never used still gets a row: the binder is what was
    // written, whatever the body did with it.
    let ghost = nodes(stage)
        .into_iter()
        .find(|node| node.label == "type Ghost")
        .expect("a row for the declaration");
    let kids: Vec<&str> = ghost
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(kids, ["Param", "Prim"]);
}

/// A repeated parameter is a repeat like any other, so it arrives as one
/// diagnostic pointing at the name it repeats. The span was carried and no
/// reporter rendered it, so the panel could not cross-highlight the two names
/// the complaint is about.
#[test]
fn a_duplicate_parameter_carries_the_name_it_repeats() {
    let snapshot = snapshot("type Pair 'A 'A = { first: 'A, second: 'A }\n");
    let duplicate = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "duplicate-parameter")
        .expect("the repeat is reported");
    assert_eq!(duplicate.span, at([13, 15]));
    assert_eq!(duplicate.related.len(), 1);
    assert_eq!(duplicate.related[0].span, at([10, 12]));
}

/// The IR tab shows an application as its head and its arguments, and the head
/// carries the declaration's symbol so it cross-highlights with the row that
/// declares it.
#[test]
fn the_ir_tab_takes_an_application_apart() {
    let snap = snapshot("type Box 'A = { it: 'A }\ntype N = Box Nat");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "ir")
        .expect("the ir stage");

    let rows = nodes(stage);
    let apply = rows
        .iter()
        .find(|node| node.label == "Apply")
        .expect("a row for the application");
    assert_eq!(apply.text, "Box Nat");

    let kids: Vec<&str> = apply
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(kids, vec!["Head", "Prim"]);
    assert!(
        apply.children[0].symbol.is_some(),
        "the head should cross-highlight: {:#?}",
        apply.children[0]
    );
}

/// The Types tab says which parameters stand for a set of fields, and which
/// fields that set may not contain, because nothing else on the row does: a row
/// is the only reason a declared type can be open, and the meaning column
/// spells every parameter `a` alike.
#[test]
fn the_types_tab_says_which_parameters_are_rows() {
    let snap = snapshot("type Both 'A 'r = { it: 'A, ..'r }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage");

    let both = nodes(stage)
        .into_iter()
        .find(|node| node.label == "type Both")
        .expect("a row for the declaration");
    let letters: Vec<(&str, &str)> = both
        .children
        .iter()
        .map(|child| (child.label.as_str(), child.text.as_str()))
        .collect();
    assert_eq!(
        letters,
        vec![("'a", "'A"), ("'b", "..'r (struct) without it")]
    );

    // A struct's `..` beside no fields at all forbids nothing, and there is
    // nothing else about it to show: the rest of a struct *is* a whole type, so
    // a parameter with an empty lacks set is a type parameter and the row says
    // so by saying only the name.
    let snap = snapshot("type Bare 'r = { ..'r }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "types")
        .expect("the types stage");
    let bare = nodes(stage)
        .into_iter()
        .find(|node| node.label == "type Bare")
        .expect("a row for the declaration");
    assert_eq!(bare.children[0].text, "..'r (struct)");
}

/// A type parameter is a symbol like any other — minted as a local, the way a
/// lambda's argument is — so it reaches the Symbols tab with no special case,
/// and the path it is listed under is one `demangle` can read back.
#[test]
fn a_type_parameter_is_a_symbol_like_any_other() {
    let snap = snapshot("type Pair 'A 'B = { first: 'A, second: 'B }");
    let stage = snap
        .stages
        .iter()
        .find(|stage| stage.id == "symbols")
        .expect("the symbols stage");

    let listed: Vec<&str> = nodes(stage)
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    for name in ["Pair", "A", "B"] {
        assert!(listed.contains(&name), "{name} is missing from {listed:?}");
    }
}

/// A sum through every panel that renders one: the tokens it lexes to, the
/// case and tail rows of both trees, the badge the term wears, and the scheme
/// the definition ends with.
#[test]
fn sums_reach_every_stage() {
    let source = "type Fallible 'r = #Err Nat | ..'r\nlet e : Fallible (#Ok Nat) = #Err 1n\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .expect("the stage is registered")
    };
    let labelled = |id: &str, label: &str| -> Vec<String> {
        nodes(stage(id))
            .into_iter()
            .filter(|node| node.label == label)
            .map(|node| node.text.clone())
            .collect()
    };

    // A tag is one token with a class of its own, so the editor paints a case
    // differently from the names around it.
    assert_eq!(
        labelled("tokens", "Tag"),
        ["#Err", "#Ok", "#Err"],
        "{:#?}",
        nodes(stage("tokens"))
    );
    assert_eq!(labelled("tokens", "Pipe"), ["|"]);
    let tag = nodes(stage("tokens"))
        .into_iter()
        .find(|node| node.label == "Tag")
        .expect("a tag row");
    let class = tag
        .fields
        .iter()
        .find(|field| field.name == "_class")
        .map(|field| field.value.as_str());
    assert_eq!(class, Some("tag"));

    for id in ["ast", "ir"] {
        // A case is a row of the sum's, wearing its `#` in the label, and
        // the tail is a row of its own — the same two shapes a struct has.
        assert_eq!(labelled(id, "#Err"), ["Nat"], "{id}");
        assert_eq!(labelled(id, "Rest"), ["..'r"], "{id}");
        // And the tag in the term is a node that names no symbol, the way a
        // field name is.
        assert_eq!(labelled(id, "Tag"), ["#Err 1n"], "{id}");
    }

    // The Types tab says which shape the parameter stands for, since the
    // meaning column spells it `a` like any other.
    let fallible = nodes(stage("types"))
        .into_iter()
        .find(|node| node.label == "type Fallible")
        .expect("a row for the declaration");
    assert_eq!(fallible.text, "#Err Nat | ..'a");
    assert_eq!(fallible.children[0].text, "..'r (sum) without #Err");

    // And the definition's scheme is the declared type, applied to the row the
    // use site handed it.
    let types: Vec<&str> = stage("types")
        .nodes
        .iter()
        .map(|node| node.text.as_str())
        .collect();
    assert!(types.contains(&"Fallible (#Ok Nat)"), "{types:?}");
}

/// A type carrying fields reaches every tab that shows a type, because they all
/// print through the compiler's own printer.
///
/// Two shapes of it, and one program each. `fn p => p.x` carries fields on a
/// *variable* core, which is the commonest type in the language and prints with
/// a `..` — the spelling a reader could have written back. And a declaration
/// whose `..` was handed a known type carries them on that: `WithX Nat` unfolds
/// to `Nat with { x: Nat }`, which no source syntax writes and only the solve
/// can show.
/// A nested `let` reaches all four tabs that had to keep up with it: the AST
/// and IR trees show it as a node of its own, the Constraints tab shows the two
/// lists it carries as rows beneath it, and the Types tab says what the name it
/// binds was generalized to.
#[test]
fn a_nested_let_reaches_every_stage() {
    let source = "let a = do let id : Nat -> Nat = fn x => x return id 1n end\n";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );

    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    // The parse tree has the block: its row prints the whole of it, and under
    // it sit the statement's row — the same row a definition gets, with the
    // written type between the name and the value where it was written — and
    // the returned expression under its role.
    let block = nodes(stage("ast"))
        .into_iter()
        .find(|node| node.label == "Do")
        .expect("the ast renders the block");
    assert_eq!(
        block.text,
        "do let id : Nat -> Nat = fn x => x return id 1n end"
    );
    let labels: Vec<&str> = block
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(labels, ["Let", "Return"]);
    let binding = &block.children[0];
    assert_eq!(binding.text, "let id : Nat -> Nat = fn x => x");
    let labels: Vec<&str> = binding
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(labels, ["Bind", "Ascribed Arrow", "Function"]);

    // The IR has the binding the block lowered to, labelled with the name it
    // binds and printed back as the block it stands for: the resolved name,
    // the written type, the curried lambda, and the body it is in scope for.
    let node = nodes(stage("ir"))
        .into_iter()
        .find(|node| node.label == "Let id")
        .expect("the ir renders the binding");
    assert_eq!(
        node.text,
        "do let id : Nat -> Nat = fn x => x return id 1n end"
    );
    let labels: Vec<&str> = node
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert_eq!(labels, ["Name", "Ascribed Arrow", "Fn", "Apply"]);

    // The Constraints tab is a tree: the `let` is a row, and what its value and
    // its body require are rows beneath it, in the order the solver runs them.
    let bound = nodes(stage("constraints"))
        .into_iter()
        .find(|node| node.label == "let")
        .expect("the constraints tab renders the let");
    assert!(!bound.children.is_empty(), "{bound:#?}");
    let codes: Vec<&str> = bound
        .children
        .iter()
        .map(|child| child.label.as_str())
        .collect();
    assert!(codes.contains(&"instance"), "{codes:?}");

    // And the Types tab says what the local was generalized to, under the
    // definition it was written in.
    let definition = stage("types")
        .nodes
        .iter()
        .find(|node| node.label == "let a")
        .expect("the types tab renders the definition");
    let local = definition
        .children
        .iter()
        .find(|child| child.label == "local id")
        .unwrap_or_else(|| panic!("no local row: {definition:#?}"));
    assert_eq!(local.text, "Nat -> Nat");
}

#[test]
fn discard_shortcuts_reach_the_debugger_as_lets_with_their_written_ranges() {
    let source = "_ = 1n\nmodule Nested = _ : Nat = 2n end\n\
                  let value = do _ = 3n let _ = 4n return 5n end";
    let snapshot = snapshot(source);
    assert!(snapshot.panic.is_none());
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    for id in ["ast", "ir", "types", "constraints", "solve", "lir", "js"] {
        assert_eq!(stage_named(&snapshot, id).status, Status::Ok, "{id}");
    }
    let ast = stage_named(&snapshot, "ast");
    for (written, printed) in [
        ("_ = 1n", "let _ = 1n"),
        ("_ : Nat = 2n", "let _ : Nat = 2n"),
        ("_ = 3n", "let _ = 3n"),
        ("let _ = 4n", "let _ = 4n"),
    ] {
        let node = nodes(ast)
            .into_iter()
            .find(|node| node.label == "Let" && node.text == printed)
            .unwrap_or_else(|| panic!("the AST has no {printed}: {:#?}", ast.nodes));
        let start = source.find(written).unwrap();
        assert_eq!(node.span, at([start, start + written.len()]));
        let wildcard = &node.children[0];
        assert_eq!(wildcard.label, "Wildcard");
        let start = start + written.find('_').unwrap();
        assert_eq!(wildcard.span, at([start, start + 1]));
        assert!(wildcard.symbol.is_none());
    }
}

/// Every tab renders a program using explicit absence without error: the
/// Tokens tab shows the backslash like any other punctuation token, the AST
/// and IR tabs render `\y` and `\#Err` as written, and the type tabs take
/// the absent entries in stride.
#[test]
fn every_stage_reports_on_explicit_absence() {
    let source = "@private let f : { x: Nat, \\y, .. } -> Nat = fn a => a.x\n\
                  type NoErr 'r = #Ok Nat | \\#Err | ..'r\n\
                  let ok : NoErr (#Warn Nat) = #Ok 1n\n";
    let snapshot = snapshot(source);
    assert!(snapshot.panic.is_none());
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    for stage in &snapshot.stages {
        if stage.id == "entry" {
            assert_eq!(stage.status, Status::Skipped);
            continue;
        }
        // The Patterns tab has one section per match, and this program
        // matches nothing — an honest emptiness rather than a failure.
        if stage.id == "patterns" {
            continue;
        }
        assert!(
            !stage.nodes.is_empty() || stage.text.as_ref().is_some_and(|text| !text.is_empty()),
            "{} produced nothing",
            stage.id
        );
    }
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("no {id} stage"))
    };

    let tokens: Vec<_> = nodes(stage("tokens"))
        .into_iter()
        .filter(|node| node.label == "Backslash")
        .collect();
    assert_eq!(tokens.len(), 2, "{tokens:#?}");
    assert!(tokens.iter().all(|node| node.text == "\\"), "{tokens:#?}");

    let ast = nodes(stage("ast"));
    assert!(
        ast.iter().any(|node| node.label == "\\y"),
        "the AST tab renders the absent field"
    );
    assert!(
        ast.iter().any(|node| node.label == "\\#Err"),
        "the AST tab renders the absent case"
    );

    let ir = nodes(stage("ir"));
    assert!(
        ir.iter().any(|node| node.label == "\\y"),
        "the IR tab renders the absent field"
    );
    assert!(
        ir.iter().any(|node| node.label == "\\#Err"),
        "the IR tab renders the absent case"
    );
}

/// A program with a match and a pattern `let` reaches every stage: the AST
/// tab shows the written arms and patterns, the IR tab shows the one matrix
/// `Match` — scrutinee, each arm's normalized pattern with its binder
/// symbols, and body — beside the definitions the pattern `let` desugared
/// into, and the `let`'s fresh temporary sits in the symbols tab like any
/// other. No tree exists to show: the lowering-visible transformation is the
/// desugar and the normalization.
#[test]
fn a_match_and_a_pattern_let_reach_every_stage() {
    let source = "let {x, y} = { x: 1n, y: 2n }\n\
                  let f = fn e => match e with | #A #X a => x | { g: #G p } => p | r => y end\n";
    let snapshot = snapshot(source);
    assert!(snapshot.panic.is_none());
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    // The AST tab renders the match as written: the scrutinee, one wrapper
    // per arm, and the patterns inside them.
    let ast: Vec<&str> = nodes(stage("ast"))
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    for label in ["Match", "Arm", "Tag", "Bind", "Struct"] {
        assert!(ast.contains(&label), "ast lacks {label}: {ast:?}");
    }

    // The IR tab renders the matrix node: the arms' normalized patterns —
    // tags, struct fields, binders — and the definitions the struct pattern
    // `let` became. No node of any compiled tree is on the page.
    let ir = nodes(stage("ir"));
    let labels: Vec<&str> = ir.iter().map(|node| node.label.as_str()).collect();
    for label in [
        "Match",
        "Tag",
        "Bind",
        "Struct",
        "g:",
        "let x",
        "let y",
        "let %struct",
    ] {
        assert!(labels.contains(&label), "ir lacks {label}: {labels:?}");
    }
    for artifact in ["Case", "Default", "Let %join"] {
        assert!(!labels.contains(&artifact), "{labels:?}");
    }

    // The arm binders carry their symbols, so they cross-highlight with the
    // uses in their bodies.
    assert!(
        ir.iter()
            .any(|node| node.label == "Bind" && node.symbol.is_some()),
        "{labels:?}"
    );

    // Every span the two tabs hand out is a real position in this source — in
    // the file it was written in, which for a snippet is the one the source
    // sits at the top of.
    let text = compiled(source);
    for id in ["ast", "ir"] {
        for node in nodes(stage(id)) {
            if let Some(at) = node.span {
                assert_eq!(at.file, 0, "{id}: {}", node.label);
                assert!(
                    text.get(at.range[0]..at.range[1]).is_some(),
                    "{id}: {} at {:?}",
                    node.label,
                    at.range
                );
            }
        }
    }

    // The pattern `let`'s fresh temporary prints recognizably — the `%` no
    // identifier can spell — and sits in the symbols tab beside the source's
    // own names; the match minted nothing beyond what its patterns bind.
    let symbols: Vec<&str> = stage("symbols")
        .nodes
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    for name in ["%struct", "x", "y", "a", "p", "r"] {
        assert!(symbols.contains(&name), "symbols lack {name}: {symbols:?}");
    }
    for artifact in ["%join", "%scrut", "%fall", "%case"] {
        assert!(!symbols.contains(&artifact), "{symbols:?}");
    }
}

/// A program full of wildcards reaches every affected tab: the token tab
/// shows the `Underscore` token, the AST tab renders `_` patterns and `fn _`
/// arguments as the `_` they are, the IR tab renders the IR wildcard pattern
/// as `_`, and the hidden fresh definitions of `let _` and a wildcard's
/// projection sit beside the pattern-`let` temporaries the way they always
/// have. No new tab.
#[test]
fn a_wildcard_reaches_every_stage() {
    let source = "let _ = 1n\n\
                  @private let const = fn x _ => x\n\
                  @private let use_y = fn p => do let { x: _, y } = p return y end\n\
                  @private let f = fn e => match e with | #Some _ => 1n | _ => 0n end\n";
    let snapshot = snapshot(source);
    assert!(snapshot.panic.is_none());
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("{id} is registered"))
    };

    // The token tab shows the new kind, spelled as the `_` it lexed from.
    let tokens = nodes(stage("tokens"));
    assert!(
        tokens
            .iter()
            .any(|node| node.label == "Underscore" && node.text == "_"),
        "no underscore token"
    );

    // The AST tab renders wildcard patterns as `_` — the arm, the payload,
    // the struct sub-pattern, the `let` — and the `fn _` argument as one.
    let ast = nodes(stage("ast"));
    assert!(
        ast.iter()
            .any(|node| node.label == "Wildcard" && node.text == "_"),
        "ast lacks a wildcard leaf"
    );
    assert!(
        ast.iter()
            .any(|node| node.label == "Arg" && node.text == "_"),
        "ast lacks the `_` argument"
    );

    // The IR tab renders the surviving wildcard patterns as `_`, and the
    // hidden definitions the way pattern-`let` temporaries appear.
    let ir = nodes(stage("ir"));
    assert!(
        ir.iter()
            .any(|node| node.label == "Wildcard" && node.text == "_"),
        "ir lacks a wildcard leaf"
    );
    let labels: Vec<&str> = ir.iter().map(|node| node.label.as_str()).collect();
    for label in ["let %discard", "Let %struct", "Let %discard"] {
        assert!(labels.contains(&label), "ir lacks {label}: {labels:?}");
    }

    // A wildcard names nothing, so no wildcard row claims a symbol.
    for id in ["ast", "ir"] {
        for node in nodes(stage(id)) {
            if node.label == "Wildcard" {
                assert!(node.symbol.is_none(), "{id}: a wildcard claims a symbol");
            }
        }
    }

    // The hidden names sit in the symbols tab like any other local, and every
    // span every tab hands out is a real position in this source.
    let symbols: Vec<&str> = stage("symbols")
        .nodes
        .iter()
        .map(|node| node.label.as_str())
        .collect();
    assert!(symbols.contains(&"%discard"), "{symbols:?}");
    let text = compiled(source);
    for stage in &snapshot.stages {
        for node in nodes(stage) {
            if let Some(at) = node.span {
                assert_eq!(at.file, 0, "{}: {}", stage.id, node.label);
                assert!(
                    text.get(at.range[0]..at.range[1]).is_some(),
                    "{}: {} at {:?}",
                    stage.id,
                    node.label,
                    at.range
                );
            }
        }
    }
}

/// The misplaced-wildcard complaint reaches the strip coded and worded, at
/// the `_` it is about.
#[test]
fn a_misplaced_wildcard_reaches_the_strip() {
    let source = "let x = _\n";
    let snapshot = snapshot(source);
    let diagnostic = snapshot
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "misplaced-discard")
        .unwrap_or_else(|| panic!("{:#?}", snapshot.diagnostics));
    assert_eq!(diagnostic.stage, "parse");
    assert!(
        diagnostic
            .message
            .starts_with("`_` throws a value away, so it cannot be read here"),
        "{}",
        diagnostic.message
    );
    assert_eq!(diagnostic.label, "`_` does not provide a name here");
    let wildcard = source.find('_').expect("the `_`");
    assert_eq!(diagnostic.span, at([wildcard, wildcard + 1]));
}

/// A source using effects reaches every stage: the declarations get rows, the
/// handler shows what it discharges, and the types the tabs render carry the
/// effect rows the compiler prints them with.
#[test]
fn every_stage_reports_on_a_source_using_effects() {
    let source = "effect Log = { write: Nat -> () }\n\
                  effect IO = { print: Nat -> () }\n\
                  effect Console = !Log + !IO\n\
                  type Logger = Nat -> Nat + !Log\n\
                  type Runner 'e = (Nat -> Nat + ..'e) -> Nat + ..'e\n\
                  @private let greet : () -> Nat + !Log = fn _ => do let _ = !Log.write 1n return 0n end\n\
                  let quiet : () -> Nat = fn _ =>\n\
                    handle greet () with | !Log.write s => () | return x => x end\n\
                  @private let loud : () -> Nat + !IO = fn _ =>\n\
                    handle greet () with | !Log.write s => !IO.print s end\n\
                  @private let choose = fn v => match v with | #A x => x | _ => 0n end\n";
    let snapshot = snapshot(source);
    assert!(snapshot.panic.is_none());
    for stage in &snapshot.stages {
        if stage.id == "entry" {
            assert_eq!(stage.status, Status::Skipped);
            continue;
        }
        assert!(
            !stage.nodes.is_empty() || stage.text.as_ref().is_some_and(|text| !text.is_empty()),
            "{} produced nothing",
            stage.id
        );
        assert!(!stage.summary.is_empty(), "{} counted nothing", stage.id);
    }

    // The tokens tab groups by variant, so the three reserved words, the `+`
    // and the effect itself each own a label there.
    let tokens = stage_named(&snapshot, "tokens");
    for label in ["Effect", "Handle", "Plus", "EffectLabel"] {
        assert!(
            nodes(tokens).iter().any(|node| node.label == label),
            "the tokens tab shows no {label}"
        );
    }

    // The IR tab counts the effects beside the types and terms, and gives each
    // declaration a row with its operations under it.
    let ir = stage_named(&snapshot, "ir");
    assert!(ir.summary.starts_with("3 effects · "), "{}", ir.summary);
    assert!(
        nodes(ir).iter().any(|node| node.label == "effect Log"),
        "the IR tab shows no effect declaration"
    );
    // A handler's row says which effects it discharges, which is what R15's
    // coverage check decided and what nothing else on the page shows.
    let discharges: Vec<&str> = nodes(ir)
        .iter()
        .filter(|node| node.label == "Discharges")
        .map(|node| node.text.as_str())
        .collect();
    assert_eq!(discharges, ["!Log", "!Log"], "{discharges:?}");
    // And the row an arrow carries is a child beside the two sides.
    assert!(
        nodes(ir).iter().any(|node| node.label == "Effects"),
        "the IR tab shows no effect row"
    );

    // The Types tab renders the schemes, effect rows and all.
    let types = stage_named(&snapshot, "types");
    let meanings: Vec<&str> = nodes(types).iter().map(|node| node.text.as_str()).collect();
    assert!(
        meanings.contains(&"() -> Nat + !Log"),
        "the Types tab lost the row: {meanings:?}"
    );
    // An alias does not survive into the type language, so `Runner`'s
    // parameter is shown as what the body read it to be.
    assert!(
        nodes(types)
            .iter()
            .any(|node| node.text.contains("..'e (effects)")),
        "the Types tab does not say what `e` stands for"
    );

    // The Symbols tab lists an effect in its own namespace.
    let symbols = stage_named(&snapshot, "symbols");
    let log = nodes(symbols)
        .into_iter()
        .find(|node| node.label == "Log")
        .expect("the symbols tab lists the effect");
    assert!(
        log.fields
            .iter()
            .any(|field| field.name == "namespace" && field.value == "effect"),
        "{:?}",
        log.fields
    );
}

/// One stage by its id, for the tests that ask about a particular tab.
fn stage_named<'a>(snapshot: &'a Snapshot, id: &str) -> &'a Stage {
    snapshot
        .stages
        .iter()
        .find(|stage| stage.id == id)
        .unwrap_or_else(|| panic!("{id} is registered"))
}

#[test]
fn extern_values_reach_every_import_and_artifact_view() {
    let source = "extern answer : Nat = \"host.answer\"\nlet next = answer";
    let snapshot = snapshot(source);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:#?}",
        snapshot.diagnostics
    );
    let stage = |id: &str| {
        snapshot
            .stages
            .iter()
            .find(|stage| stage.id == id)
            .unwrap_or_else(|| panic!("no {id} stage"))
    };
    let externs = stage("externs");
    assert_eq!(externs.summary, "1 extern");
    assert!(
        nodes(externs)
            .iter()
            .any(|node| node.text == "\"host.answer\"")
    );
    assert!(
        nodes(stage("types"))
            .iter()
            .any(|node| node.label == "extern answer" && node.text == "Nat")
    );
    assert!(
        nodes(stage("ir"))
            .iter()
            .any(|node| node.label == "extern answer")
    );
    assert!(
        nodes(stage("lir"))
            .iter()
            .any(|node| node.label == "extern" && node.text.contains("host.answer"))
    );
    for id in ["artifact", "linked"] {
        let artifact = stage(id);
        assert!(
            artifact.summary.contains("1 externs"),
            "{id}: {}",
            artifact.summary
        );
        assert!(
            nodes(artifact)
                .iter()
                .any(|node| node.label == "extern" && node.text.contains("host.answer")),
            "{id}: {:#?}",
            artifact.nodes
        );
        assert!(
            artifact
                .text
                .as_deref()
                .is_some_and(|text| text.contains(r#"\"target\":\"host.answer\""#)),
            "{id}: {:?}",
            artifact.text
        );
    }
}

#[test]
fn recovered_buffers_keep_semantics_and_requested_solver_traces() {
    let snapshot = snapshot("let good = fn x => x\nlet broken =");
    assert!(snapshot.panic.is_none());
    assert!(!snapshot.diagnostics.is_empty());
    for id in ["types", "constraints", "solve"] {
        let stage = stage_named(&snapshot, id);
        assert_eq!(stage.status, Status::Partial);
    }
    assert_eq!(stage_named(&snapshot, "lir").status, Status::Skipped);
}
