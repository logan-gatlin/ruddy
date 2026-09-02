//! Tests for the debugger state shared by a browser and an agent.

use ruddy_debug::{
    session::{EditError, Store},
    wire::{CompileRequest, SessionView},
};

fn request(source: &str) -> CompileRequest {
    serde_json::from_value(serde_json::json!({
        "document": "demo",
        "files": [{ "path": "main.hc", "source": source }]
    }))
    .unwrap()
}

#[test]
fn agent_edits_are_optimistic_and_stale_browser_compiles_cannot_undo_them() {
    let mut store = Store::default();
    assert!(store.observe(request("human"), 0, "browser".into(), None));

    let edited = store.edit(1, "demo", "main.hc", "agent".into()).unwrap();
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.request.files[0].source, "agent");

    assert!(!store.observe(request("stale human"), 1, "browser".into(), None));
    assert_eq!(store.current().unwrap().request.files[0].source, "agent");
    assert!(store.observe(request("human after sync"), 2, "browser".into(), None));
}

#[test]
fn a_later_overlapping_compile_from_the_same_browser_wins_its_own_race() {
    let mut store = Store::default();
    let mut first = request("first");
    first.revision = 1;
    assert!(store.observe(first, 0, "browser".into(), None));

    let mut later = request("later");
    later.revision = 2;
    assert!(store.observe(later, 0, "browser".into(), None));
    assert_eq!(store.current().unwrap().request.files[0].source, "later");
}

#[test]
fn edits_must_name_the_open_document_file_and_current_revision() {
    let mut store = Store::default();
    assert_eq!(
        store.edit(0, "demo", "main.hc", String::new()).unwrap_err(),
        EditError::NoSession
    );
    store.observe(request(""), 0, "browser".into(), None);
    assert_eq!(
        store
            .edit(1, "other", "main.hc", String::new())
            .unwrap_err(),
        EditError::WrongDocument
    );
    assert_eq!(
        store
            .edit(1, "demo", "Other.hc", String::new())
            .unwrap_err(),
        EditError::MissingFile
    );
    store.edit(1, "demo", "main.hc", "first".into()).unwrap();
    assert_eq!(
        store
            .edit(1, "demo", "main.hc", "second".into())
            .unwrap_err(),
        EditError::Conflict { current: 2 }
    );
}

#[test]
fn shared_view_tracks_what_the_human_is_inspecting() {
    let mut store = Store::default();
    let view = SessionView {
        active_file: "main.hc".into(),
        caret: 12,
        tabs: vec!["ir".into(), "solve".into()],
        split: true,
        ..Default::default()
    };
    store.observe(request(""), 0, "browser".into(), Some(view.clone()));
    assert_eq!(store.current().unwrap().view.caret, 12);

    let moved = SessionView { caret: 20, ..view };
    store.update_view(1, moved).unwrap();
    assert_eq!(store.current().unwrap().view.caret, 20);
}

/// A compile from a client that tracks no view — an agent, a stale page —
/// must not reset the panels every other collaborator is looking at.
#[test]
fn a_viewless_compile_keeps_the_shared_view() {
    let mut store = Store::default();
    let view = SessionView {
        active_file: "main.hc".into(),
        split: true,
        tabs: vec!["ir".into()],
        ..Default::default()
    };
    store.observe(request("first"), 0, "browser".into(), Some(view));
    assert!(store.observe(request("second"), 1, "browser".into(), None));
    let kept = store.current().unwrap().view;
    assert!(kept.split);
    assert_eq!(kept.active_file, "main.hc");
    assert_eq!(kept.tabs, ["ir"]);
}
