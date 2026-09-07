use lsp_server::{Connection, Message, Notification, Request};
use serde_json::json;
use std::time::Duration;

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
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".into(), json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":"let value = false\nlet use = value"}})))).unwrap();
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
