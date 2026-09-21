use std::{fs, path::Path};

use ruddy_cli::workspace::Workspace;

fn project(directory: &Path, name: &str, dependencies: &str, source: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("Ruddy.toml"), format!(
        "name = {name:?}\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n{dependencies}"
    )).unwrap();
    fs::write(directory.join("main.rud"), source).unwrap();
}

fn hover(workspace: &Workspace, path: &Path) -> String {
    let (project, logical) = workspace.file(path).unwrap();
    project.analysis().hover(logical, 5).unwrap().ty
}

#[test]
fn editor_edits_reuse_acquired_files_and_filesystem_events_invalidate_only_changes() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut workspace = Workspace::new(root.clone());
    workspace.set_notification_driven(true);
    let first = workspace.refresh().unwrap();
    assert!(first.disk_reads > 0);
    assert!(first.directory_scans > 0);

    workspace.set_overlay(
        &root.join("main.rud"),
        Some("let answer = dep::value\n\n".into()),
    );
    let edit = workspace.refresh().unwrap();
    assert_eq!((edit.rebuilt, edit.reused), (1, 1));
    assert_eq!((edit.disk_reads, edit.directory_scans), (0, 0));

    fs::write(dep.join("main.rud"), "let value = false\n").unwrap();
    assert_eq!(workspace.refresh().unwrap().reused, 2);
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Nat");
    workspace.invalidate_paths([dep.join("main.rud")]);
    let changed = workspace.refresh().unwrap();
    assert_eq!(changed.disk_reads, 1);
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Bool");
}

#[test]
fn editor_observes_missing_module_creation_and_deletion() {
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    project(
        root,
        "editor",
        "",
        "module Missing\nlet answer = Missing::value\n",
    );
    let mut workspace = Workspace::new(root.to_path_buf());
    workspace.set_notification_driven(true);
    workspace.refresh().unwrap();
    let module = root.join("Missing.rud");
    assert_eq!(workspace.observed_files().get(&module), Some(&None));

    fs::write(&module, "let value = 1n\n").unwrap();
    workspace.invalidate_paths([module.clone()]);
    workspace.refresh().unwrap();
    assert!(
        workspace
            .file(&root.join("main.rud"))
            .unwrap()
            .0
            .analysis()
            .diagnostics
            .is_empty()
    );

    fs::remove_file(&module).unwrap();
    workspace.invalidate_paths([module.clone()]);
    workspace.refresh().unwrap();
    assert!(
        !workspace
            .file(&root.join("main.rud"))
            .unwrap()
            .0
            .analysis()
            .diagnostics
            .is_empty()
    );
    assert_eq!(workspace.observed_files().get(&module), Some(&None));
}

#[test]
fn closing_an_overlay_observes_a_save_before_its_filesystem_notification() {
    let tree = tempfile::tempdir().unwrap();
    project(tree.path(), "editor", "", "let value = 1n\n");
    let path = tree.path().join("main.rud");
    let mut workspace = Workspace::new(tree.path().to_path_buf());
    workspace.set_notification_driven(true);
    workspace.refresh().unwrap();
    workspace.set_overlay(&path, Some("let value = false\n".into()));
    workspace.refresh().unwrap();
    fs::write(&path, "let value = \"saved\"\n").unwrap();
    assert!(workspace.set_overlay(&path, None));
    workspace.refresh().unwrap();
    assert_eq!(hover(&workspace, &path), "String");
}

#[test]
fn changing_editor_focus_does_not_run_inference_until_requested() {
    let tree = tempfile::tempdir().unwrap();
    project(tree.path(), "editor", "", "module Other\nlet value = 1n\n");
    fs::write(tree.path().join("Other.rud"), "let bad = 1n + false\n").unwrap();
    let mut workspace = Workspace::new(tree.path().to_path_buf());
    workspace.focus(&tree.path().join("main.rud"));
    workspace.refresh().unwrap();
    assert!(workspace.projects[0].analysis().diagnostics.is_empty());
    workspace.focus(&tree.path().join("Other.rud"));
    assert!(workspace.projects[0].analysis().diagnostics.is_empty());
    assert!(workspace.request_file(&tree.path().join("Other.rud")));
    assert!(!workspace.projects[0].analysis().diagnostics.is_empty());
}

#[test]
fn complete_dependency_interfaces_survive_without_background_artifacts() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let cache = tree.path().join("cache");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut first = Workspace::with_artifact_cache(root.clone(), cache.clone());
    first.focus(&root.join("main.rud"));
    first.refresh().unwrap();
    // Deliberately never lower the first workspace, as when typing interrupts
    // startup background work or the user closes the editor immediately.
    let entries: Vec<_> = fs::read_dir(cache.join(ruddy::artifact::COMPILER_HASH))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(entries.iter().any(|path| {
        path.extension()
            .is_some_and(|extension| extension == "interface")
    }));
    assert!(!entries.iter().any(|path| {
        path.extension()
            .is_some_and(|extension| extension == "artifact")
    }));

    let mut fresh = Workspace::with_artifact_cache(root.clone(), cache);
    fresh.focus(&root.join("main.rud"));
    let restored = fresh.refresh().unwrap();
    assert_eq!((restored.cached, restored.rebuilt), (1, 1));
    assert!(fresh.file(&dep.join("main.rud")).is_none());
    assert_eq!(hover(&fresh, &root.join("main.rud")), "Nat");
    assert!(fresh.check_background().is_empty());
    assert!(fresh.file(&dep.join("main.rud")).is_some());
}

#[test]
fn unsaved_dependency_interfaces_do_not_poison_disk_cache_keys() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let cache = tree.path().join("cache");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut edited = Workspace::with_artifact_cache(root.clone(), cache.clone());
    edited.focus(&root.join("main.rud"));
    edited.set_overlay(&dep.join("main.rud"), Some("let value = false\n".into()));
    edited.refresh().unwrap();
    assert_eq!(hover(&edited, &root.join("main.rud")), "Bool");
    assert!(edited.check_background().is_empty());
    let mut fresh = Workspace::with_artifact_cache(root.clone(), cache);
    fresh.focus(&root.join("main.rud"));
    assert_eq!(fresh.refresh().unwrap().cached, 0);
    assert_eq!(hover(&fresh, &root.join("main.rud")), "Nat");
}

#[test]
fn newly_acquired_modules_contribute_to_persisted_cache_keys() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let cache = tree.path().join("cache");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut workspace = Workspace::with_artifact_cache(root.clone(), cache.clone());
    workspace.set_notification_driven(true);
    workspace.focus(&root.join("main.rud"));
    workspace.refresh().unwrap();

    // Creating an unreferenced module does not notify the exact-input watcher.
    let module = dep.join("Part.rud");
    fs::write(&module, "let value = false\n").unwrap();
    let importer = "module Part\nlet value = Part::value\n";
    workspace.set_overlay(&dep.join("main.rud"), Some(importer.into()));
    let changed = workspace.refresh().unwrap();
    assert_eq!(changed.directory_scans, 0);
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Bool");
    assert!(workspace.check_background().is_empty());

    // The successful interface must not be stored under the digest of just
    // the importer, which would also describe this now-missing-module state.
    fs::write(dep.join("main.rud"), importer).unwrap();
    fs::remove_file(module).unwrap();
    let mut fresh = Workspace::with_artifact_cache(root, cache);
    assert_eq!(fresh.refresh().unwrap().cached, 0);
    assert!(
        !fresh
            .file(&dep.join("main.rud"))
            .unwrap()
            .0
            .analysis()
            .diagnostics
            .is_empty()
    );
}

#[test]
fn unwatched_sources_are_reacquired_before_accepting_cached_interfaces() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let importer = "module Part\nlet value = Part::value\n";
    project(&dep, "dep", "", importer);
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let module = dep.join("Part.rud");
    fs::write(&module, "let value = 1n\n").unwrap();
    let mut workspace = Workspace::with_artifact_cache(root.clone(), tree.path().join("cache"));
    workspace.set_notification_driven(true);
    workspace.focus(&root.join("main.rud"));
    workspace.refresh().unwrap();

    fs::write(dep.join("main.rud"), "let value = 1n\n").unwrap();
    workspace.invalidate_paths([dep.join("main.rud")]);
    workspace.refresh().unwrap();
    // Failed reuse may conservatively observe an old input before encountering
    // the changed importer. Settle on the new graph before dropping its watch.
    workspace.refresh().unwrap();
    assert!(!workspace.observed_files().contains_key(&module));

    // Part has left the graph and its watcher subscription has been removed.
    // Reintroducing it initially finds the original fingerprint and cached
    // interface, but replaying the input index must observe its new contents.
    fs::write(&module, "let value = false\n").unwrap();
    fs::write(dep.join("main.rud"), importer).unwrap();
    workspace.invalidate_paths([dep.join("main.rud")]);
    assert_eq!(workspace.refresh().unwrap().cached, 0);
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Bool");

    workspace.set_overlay(
        &root.join("main.rud"),
        Some("let answer = dep::value\n\n".into()),
    );
    let edit = workspace.refresh().unwrap();
    assert_eq!((edit.disk_reads, edit.directory_scans), (0, 0));
}

#[cfg(unix)]
#[test]
fn first_module_acquisition_refreshes_unwatched_symlink_identities() {
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path().join("root");
    project(&root, "root", "", "let value = 1n\n");
    let previous = tree.path().join("previous.rud");
    let current = tree.path().join("current.rud");
    fs::write(&previous, "let value = 1n\n").unwrap();
    fs::write(&current, "let value = \"current\"\n").unwrap();
    let module = root.join("Part.rud");
    std::os::unix::fs::symlink(&previous, &module).unwrap();
    let mut workspace = Workspace::new(root.clone());
    workspace.set_notification_driven(true);
    workspace.refresh().unwrap();

    workspace.set_overlay(&previous, Some("let value = false\n".into()));
    fs::remove_file(&module).unwrap();
    std::os::unix::fs::symlink(&current, &module).unwrap();
    let importer = "module Part\nlet value = Part::value\n";
    workspace.set_overlay(&root.join("main.rud"), Some(importer.into()));
    workspace.refresh().unwrap();
    let (project, logical) = workspace.file(&root.join("main.rud")).unwrap();
    assert_eq!(
        project
            .analysis()
            .hover(logical, importer.find("value").unwrap() + 1)
            .unwrap()
            .ty,
        "String"
    );
}

#[test]
fn cancelled_refresh_retains_unchanged_projects_and_recovers_host_revisions() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut workspace = Workspace::new(root.clone());
    workspace.set_notification_driven(true);
    workspace.refresh().unwrap();
    let large: String = (0..10_000)
        .map(|index| {
            format!("let value{index} = fn x => do let y = x + 1.0 return {{ value: y }} end\n")
        })
        .collect();
    workspace.set_overlay(&root.join("main.rud"), Some(large));
    let cancellation = ruddy::cancellation::Cancellation::default();
    let timer = cancellation.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        timer.cancel();
    });
    assert!(cancellation.run(|| workspace.refresh()).is_err());
    timer.join().unwrap();
    assert!(workspace.file(&dep.join("main.rud")).is_some());
    workspace.set_overlay(
        &root.join("main.rud"),
        Some("let answer = dep::value\n\n".into()),
    );
    let recovered = workspace.refresh().unwrap();
    assert_eq!((recovered.rebuilt, recovered.reused), (1, 1));
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Nat");
    assert!(workspace.check_background().is_empty());
}

#[test]
fn cached_interfaces_validate_structure_and_regions() {
    let tree = tempfile::tempdir().unwrap();
    project(tree.path(), "editor", "", "let value = 1n\n");
    let mut workspace = Workspace::new(tree.path().to_path_buf());
    workspace.refresh().unwrap();
    let header = workspace.projects[0].interface.clone();
    let text = ruddy::artifact::text::print_header(&header);
    assert_eq!(
        ruddy::artifact::text::try_parse_header(&text).unwrap(),
        header
    );
    assert!(ruddy::artifact::text::try_parse_header("(").is_err());
    assert!(ruddy::artifact::text::try_parse_header("(header)").is_err());
    assert!(ruddy::artifact::text::try_parse_header(&format!("{text} trailing")).is_err());
    let mut invalid = header;
    invalid.values[0].scheme.body = ruddy::artifact::Type::Mut(
        Box::new(ruddy::artifact::Type::Nat),
        Box::new(ruddy::artifact::Type::Nat),
    );
    assert!(
        ruddy::artifact::text::try_parse_header(&ruddy::artifact::text::print_header(&invalid))
            .is_err()
    );
}

#[test]
fn corrupt_or_foreign_interface_cache_entries_are_rebuilt() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let cache = tree.path().join("cache");
    project(&dep, "dep", "", "let value = 1n\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut first = Workspace::with_artifact_cache(root.clone(), cache.clone());
    first.refresh().unwrap();
    let interface = fs::read_dir(cache.join(ruddy::artifact::COMPILER_HASH))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "interface")
        })
        .unwrap();
    let valid = fs::read_to_string(&interface).unwrap();
    for invalid in [
        "(header".to_string(),
        valid.replace(ruddy::artifact::COMPILER_HASH, "old-compiler"),
    ] {
        fs::write(&interface, invalid).unwrap();
        let mut fresh = Workspace::with_artifact_cache(root.clone(), cache.clone());
        let refresh = fresh.refresh().unwrap();
        assert_eq!((refresh.cached, refresh.rebuilt), (0, 2));
        assert_eq!(hover(&fresh, &root.join("main.rud")), "Nat");
    }
}

#[test]
fn focusing_a_dependency_body_keeps_all_exports_available_to_consumers() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    project(&dep, "dep", "", "let first = 1n\nlet other = false\n");
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::other\n",
    );
    let mut workspace = Workspace::with_artifact_cache(root.clone(), tree.path().join("cache"));
    workspace.focus_at(&dep.join("main.rud"), Some(5));
    workspace.refresh().unwrap();
    assert_eq!(hover(&workspace, &root.join("main.rud")), "Bool");
    workspace.set_overlay(
        &dep.join("main.rud"),
        Some("let first = 2n\nlet other = \"changed\"\n".into()),
    );
    workspace.refresh().unwrap();
    assert_eq!(hover(&workspace, &root.join("main.rud")), "String");
}

#[test]
fn cached_dependency_inputs_include_absent_competing_module_paths() {
    let tree = tempfile::tempdir().unwrap();
    let dep = tree.path().join("dep");
    let root = tree.path().join("root");
    let cache = tree.path().join("cache");
    project(&dep, "dep", "", "module Part\nlet value = Part::value\n");
    fs::write(dep.join("Part.rud"), "let value = 1n\n").unwrap();
    project(
        &root,
        "root",
        "dep = \"../dep\"\n",
        "let answer = dep::value\n",
    );
    let mut first = Workspace::with_artifact_cache(root.clone(), cache.clone());
    first.refresh().unwrap();
    let candidate = dep.join("Part/module.rud");
    assert_eq!(first.observed_files().get(&candidate), Some(&None));

    // Exercise both a current cache's precise index and migration of an older
    // cache lacking it. Neither path may forget the absent candidate.
    for migrate in [false, true] {
        if migrate {
            for entry in fs::read_dir(cache.join(ruddy::artifact::COMPILER_HASH)).unwrap() {
                let path = entry.unwrap().path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "inputs")
                {
                    fs::remove_file(path).unwrap();
                }
            }
        }
        let mut fresh = Workspace::with_artifact_cache(root.clone(), cache.clone());
        fresh.set_notification_driven(true);
        assert_eq!(fresh.refresh().unwrap().cached, 1);
        assert_eq!(fresh.observed_files().get(&candidate), Some(&None));
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        fs::write(&candidate, "let value = false\n").unwrap();
        fresh.invalidate_paths([candidate.clone()]);
        fresh.refresh().unwrap();
        assert!(
            !fresh
                .file(&dep.join("main.rud"))
                .unwrap()
                .0
                .analysis()
                .diagnostics
                .is_empty()
        );
        fs::remove_file(&candidate).unwrap();
    }
}
