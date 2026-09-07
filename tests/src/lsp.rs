use lsp_server::{Connection, Message, Notification, Request};
use serde_json::json;
use std::time::Duration;

fn editor_request(
    files: &[(&str, &str)],
    method: &str,
    line: u32,
    character: u32,
) -> serde_json::Value {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.hc\"\n[dependencies]\nstd = false").unwrap();
    for (path, text) in files {
        let path = tree.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let root = format!("file://{}/", tree.path().display());
    let uri = format!("{root}main.hc");
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_lsp::serve(server).unwrap());
    let request = |id: i32, method: &str, params| {
        client
            .sender
            .send(Message::Request(Request::new(
                id.into(),
                method.into(),
                params,
            )))
            .unwrap();
        loop {
            if let Message::Response(response) = client
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
            {
                break response.response_result.unwrap();
            }
        }
    };
    request(1, "initialize", json!({"rootUri":root,"capabilities":{}}));
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".into(),
            json!({}),
        )))
        .unwrap();
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".into(), json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":files.iter().find(|(path, _)| *path == "main.hc").unwrap().1}})))).unwrap();
    let mut result = request(
        2,
        method,
        json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
    );
    request(3, "shutdown", json!(null));
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".into(),
            json!(null),
        )))
        .unwrap();
    worker.join().unwrap();
    if let Some(uri) = result.get_mut("uri") {
        *uri = json!(uri.as_str().unwrap().strip_prefix(root.as_str()).unwrap());
    }
    result
}

#[test]
fn editor_type_completions_do_not_claim_to_be_classes() {
    let items = editor_request(
        &[("main.hc", "type Person = { name: String }\nlet value : Per")],
        "textDocument/completion",
        1,
        15,
    );
    let person = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "Person")
        .unwrap();
    assert!(
        person.get("kind").is_none(),
        "LSP has no general type kind: {person}"
    );
    assert!(person["detail"].as_str().unwrap().starts_with("type "));
}

#[test]
fn editor_module_declarations_navigate_to_the_loaded_file() {
    for (target, body) in [
        ("Child.hc", "let value = 1n"),
        ("Child/module.hc", ""),
        ("Child/module.hc", "// empty module\n"),
    ] {
        let result = editor_request(
            &[("main.hc", "module Child"), (target, body)],
            "textDocument/definition",
            0,
            8,
        );
        assert_eq!(
            result["uri"], target,
            "module declaration should open its source file"
        );
        assert_eq!(result["range"]["start"], json!({"line":0,"character":0}));
    }
    let result = editor_request(
        &[
            ("main.hc", "module Parent = module Child end"),
            ("Parent/Child.hc", ""),
        ],
        "textDocument/definition",
        0,
        24,
    );
    assert_eq!(result["uri"], "Parent/Child.hc");
}

#[test]
fn editor_module_navigation_does_not_guess_missing_or_ambiguous_files() {
    for files in [
        vec![("main.hc", "module Child")],
        vec![
            ("main.hc", "module Child"),
            ("Child.hc", ""),
            ("Child/module.hc", ""),
        ],
        vec![("main.hc", "module Child = end"), ("Child.hc", "")],
    ] {
        assert!(editor_request(&files, "textDocument/definition", 0, 8).is_null());
    }
}

#[test]
fn editor_can_initialize_open_hover_and_shutdown() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.hc\"\n[dependencies]\nstd = false").unwrap();
    std::fs::write(tree.path().join("main.hc"), "let value = 1n").unwrap();
    let root = format!("file://{}", tree.path().display());
    #[cfg(unix)]
    std::os::unix::fs::symlink(tree.path().join("main.hc"), tree.path().join("open.hc")).unwrap();
    let uri = format!("{root}/{}", if cfg!(unix) { "open.hc" } else { "main.hc" });
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_lsp::serve(server).unwrap());
    client
        .sender
        .send(Message::Request(Request::new(
            1.into(),
            "initialize".into(),
            json!({"rootUri": root, "capabilities": {}}),
        )))
        .unwrap();
    let Message::Response(response) = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
    else {
        panic!("initialize response");
    };
    assert_eq!(
        response.response_result.unwrap()["capabilities"]["hoverProvider"],
        true
    );
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".into(),
            json!({}),
        )))
        .unwrap();
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".into(), json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":"let value = false\nlet use = value\nextern add : fn(Nat, Nat) -> Nat = \"(left, right) => left + right\""}})))).unwrap();
    client
        .sender
        .send(Message::Request(Request::new(
            2.into(),
            "textDocument/hover".into(),
            json!({"textDocument":{"uri":uri},"position":{"line":0,"character":5}}),
        )))
        .unwrap();
    loop {
        if let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        {
            assert!(
                response.response_result.unwrap()["contents"]["value"]
                    .as_str()
                    .unwrap()
                    .contains("Boolean")
            );
            break;
        }
    }
    client
        .sender
        .send(Message::Request(Request::new(
            5.into(),
            "textDocument/hover".into(),
            json!({"textDocument":{"uri":uri},"position":{"line":2,"character":8}}),
        )))
        .unwrap();
    loop {
        if let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        {
            assert_eq!(
                response.response_result.unwrap()["contents"]["value"],
                "```ruddy\nNat -> Nat -> Nat\n```",
                "extern declaration hover reaches the editor"
            );
            break;
        }
    }
    client
        .sender
        .send(Message::Request(Request::new(
            4.into(),
            "textDocument/definition".into(),
            json!({"textDocument":{"uri":uri},"position":{"line":1,"character":12}}),
        )))
        .unwrap();
    loop {
        if let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        {
            assert_eq!(
                response.response_result.unwrap()["uri"],
                uri,
                "navigation preserves the open buffer URI"
            );
            break;
        }
    }
    client
        .sender
        .send(Message::Request(Request::new(
            3.into(),
            "shutdown".into(),
            json!(null),
        )))
        .unwrap();
    loop {
        if matches!(
            client
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            Message::Response(_)
        ) {
            break;
        }
    }
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".into(),
            json!(null),
        )))
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn positions_use_utf16_and_respect_crlf() {
    let source = "a😀z\r\nnext\n";
    assert_eq!(
        ruddy_lsp::offset(source, &json!({"line":0,"character":3})),
        Some(5)
    );
    assert_eq!(
        ruddy_lsp::offset(source, &json!({"line":0,"character":2})),
        None
    );
    assert_eq!(
        ruddy_lsp::offset(source, &json!({"line":0,"character":999})),
        Some(6)
    );
    assert_eq!(
        ruddy_lsp::position(source, 5),
        json!({"line":0,"character":3})
    );
    assert_eq!(
        ruddy_lsp::position(source, 8),
        json!({"line":1,"character":0})
    );
}

#[test]
fn rapid_changes_publish_current_diagnostics_and_disk_creation_is_observed() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.hc\"\n[dependencies]\nstd = false").unwrap();
    std::fs::write(tree.path().join("main.hc"), "module Added\nlet value = 1n").unwrap();
    let root = format!("file://{}", tree.path().display());
    let uri = format!("{root}/main.hc");
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_lsp::serve(server).unwrap());
    client
        .sender
        .send(Message::Request(Request::new(
            1.into(),
            "initialize".into(),
            json!({"rootUri":root,"capabilities":{}}),
        )))
        .unwrap();
    client
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".into(),
            json!({}),
        )))
        .unwrap();
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".into(), json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":"module Added\nlet value = 1n"}})))).unwrap();
    for version in 2..=30 {
        let text = if version == 30 {
            "module Added\nlet value = false"
        } else {
            "module Added\nlet value ="
        };
        client.sender.send(Message::Notification(Notification::new("textDocument/didChange".into(), json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]})))).unwrap();
    }
    client
        .sender
        .send(Message::Request(Request::new(
            2.into(),
            "textDocument/hover".into(),
            json!({"textDocument":{"uri":uri},"position":{"line":1,"character":5}}),
        )))
        .unwrap();
    let mut newest = 0;
    let mut hover = false;
    while !hover || newest != 30 {
        match client
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            Message::Response(response) => {
                assert!(
                    response.response_result.unwrap()["contents"]["value"]
                        .as_str()
                        .unwrap()
                        .contains("Boolean")
                );
                hover = true;
            }
            Message::Notification(notification)
                if notification.method == "textDocument/publishDiagnostics" =>
            {
                let version = notification.params["version"].as_i64().unwrap();
                assert!(version >= newest);
                newest = version;
                assert!(
                    !notification.params["diagnostics"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
            }
            _ => {}
        }
    }
    // No client watched-file notification: the server tracks the loader's
    // missing candidates, including the competing Added/module.hc form.
    std::fs::write(tree.path().join("Added.hc"), "let answer : Nat = false").unwrap();
    loop {
        if let Message::Notification(notification) = client
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            && notification.method == "textDocument/publishDiagnostics"
        {
            assert_eq!(notification.params["version"], 30);
            if notification.params["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
            {
                break;
            }
        }
    }
    let added_uri = format!("{root}/Added.hc");
    loop {
        if let Message::Notification(notification) = client
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            && notification.method == "textDocument/publishDiagnostics"
            && notification.params["uri"] == added_uri
        {
            assert!(
                !notification.params["diagnostics"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            break;
        }
    }
    std::fs::write(tree.path().join("Added.hc"), "let answer = 1n").unwrap();
    loop {
        if let Message::Notification(notification) = client
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            && notification.method == "textDocument/publishDiagnostics"
            && notification.params["uri"] == added_uri
            && notification.params["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        {
            break;
        }
    }
    client
        .sender
        .send(Message::Request(Request::new(
            3.into(),
            "shutdown".into(),
            json!(null),
        )))
        .unwrap();
    loop {
        if matches!(
            client
                .receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap(),
            Message::Response(_)
        ) {
            break;
        }
    }
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".into(),
            json!(null),
        )))
        .unwrap();
    worker.join().unwrap();
}
