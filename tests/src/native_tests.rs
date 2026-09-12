//! Native tests through the project compiler and command interfaces.
use std::{fs, path::Path};

fn project(source: &str, kind: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("Ruddy.toml"), format!(
        "name = \"native-tests\"\nversion = \"0.1.0\"\nkind = {kind:?}\nroot = \"main.rud\"\n[dependencies]\nstd = false\n"
    )).unwrap();
    fs::write(project.path().join("main.rud"), source).unwrap();
    project
}

fn run(directory: &Path) -> Result<ruddy_cli::Outcome, ruddy_cli::CliError> {
    ruddy_cli::run(["test"], directory)
}

#[test]
fn native_tests_run_private_modules_without_main_and_exclude_dependencies() {
    let dependency = project(
        "effect Assert = String -> ()\n@test let forbidden: () -> () + .. = fn _ => !Assert \"dependency ran\"",
        "library",
    );
    let root = project(
        r#"
@private effect Assert = String -> ()
@private type Test = () -> ()
@test let alias: Test = fn _ => ()
@private module nested =
    @test let passing: () -> () + .. = fn _ => ()
    @test let handled: () -> () + .. = fn _ => handle !Assert "handled" with | !Assert _ => () end
end
"#,
        "executable",
    );
    fs::write(root.path().join("Ruddy.toml"), format!(
        "name = \"root-tests\"\nversion = \"0.1.0\"\nkind = \"executable\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\nother = {{ path = {:?}, bundle = \"native-tests\" }}\n", dependency.path()
    )).unwrap();
    run(root.path()).expect("private root tests run without main or dependency tests");
}

#[test]
fn native_tests_fail_on_escaping_assertions_and_runtime_errors() {
    for source in [
        "effect Assert = String -> ()\n@test let fails: () -> () + .. = fn _ => !Assert \"broken\"",
        "extern crash: () -> () = \"() => { throw new Error('runtime error'); }\"\n@test let fails: () -> () + .. = fn _ => crash ()",
    ] {
        let root = project(source, "library");
        assert!(run(root.path()).is_err());
    }
    run(project("let ordinary = 1n", "library").path()).expect("zero tests succeeds");
}

#[cfg(unix)]
#[test]
fn native_tests_report_order_continue_after_failures_and_use_the_configured_runner() {
    let root = project(
        r#"
effect Assert = String -> ()
extern crash: () -> () = "() => { throw new Error('host failure'); }"
@test let z_pass: () -> () = fn _ => ()
@test let a_fail: () -> () + !Assert = fn _ => do
    _ = !Assert "first assertion"
    return !Assert "must not resume"
end
@test let m_error: () -> () = fn _ => crash ()
"#,
        "library",
    );
    let manifest = root.path().join("Ruddy.toml");
    let mut source = fs::read_to_string(&manifest).unwrap();
    source.push_str("\n[run]\njs = \"node > report.txt 2>&1\"\n");
    fs::write(manifest, source).unwrap();
    let error = run(root.path()).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    let report = fs::read_to_string(root.path().join("report.txt")).unwrap();
    assert!(report.contains("first assertion"), "{report}");
    assert!(report.contains("host failure"), "{report}");
    assert!(!report.contains("must not resume"), "{report}");
    assert!(report.contains("3 tests: 1 passed; 2 failed"), "{report}");
    assert!(report.find("::a_fail").unwrap() < report.find("::m_error").unwrap());
    assert!(report.find("::m_error").unwrap() < report.find("::z_pass").unwrap());

    fs::remove_file(root.path().join("report.txt")).unwrap();
    fs::write(root.path().join("main.rud"), "@test let invalid = 1n").unwrap();
    assert!(run(root.path()).is_err());
    assert!(
        !root.path().join("report.txt").exists(),
        "checking errors prevent execution"
    );
}

#[test]
fn assertions_have_default_handlers_in_main_and_library_exports() {
    let main = project(
        "effect Assert = String -> ()\nlet main: () -> () + !Assert = fn _ => !Assert \"main assertion\"",
        "executable",
    );
    let error = ruddy_cli::run_project(main.path()).unwrap_err();
    assert!(error.to_string().contains("main assertion"), "{error}");
    for platform in ["node", "web"] {
        let library = project(
            "effect Assert = String -> ()\nlet verify: () -> () + !Assert = fn _ => !Assert \"library assertion\"",
            "library",
        );
        let manifest = library.path().join("Ruddy.toml");
        let source = fs::read_to_string(&manifest).unwrap().replace(
            "[dependencies]",
            &format!("target = \"js\"\nplatform = {platform:?}\n[dependencies]"),
        );
        fs::write(manifest, source).unwrap();
        let built = ruddy_cli::build_project(library.path()).unwrap();
        let script = format!(
            "import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({})); try {{ await app.verify({{}}); process.exitCode = 1; }} catch (error) {{ if (error.message !== 'library assertion') throw error; }}",
            serde_json::to_string(&built.with_extension("js")).unwrap()
        );
        let output = std::process::Command::new("node")
            .args(["--input-type=module", "--eval", &script])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn standard_bundle_tests_run_on_both_platforms_and_integer_domains() {
    let standard = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for platform in ["node", "web"] {
        for integers in [32, 53] {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("Ruddy.toml"), format!(
                "name = \"std\"\nversion = \"0.1.0\"\nkind = \"library\"\nroot = {:?}\nplatform = {platform:?}\nintegers = {integers}\n[dependencies]\nstd = false\n", standard.join("std/lib.rud")
            )).unwrap();
            run(root.path()).unwrap_or_else(|error| panic!("{platform}/{integers}: {error}"));
        }
    }
}
