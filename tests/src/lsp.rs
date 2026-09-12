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
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false").unwrap();
    for (path, text) in files {
        let path = tree.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let root = format!("file://{}/", tree.path().display());
    let uri = format!("{root}main.rud");
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_cli::lsp::serve(server).unwrap());
    let expect_valid = method == "textDocument/codeAction";
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
            match client
                .receiver
                .recv_timeout(Duration::from_secs(60))
                .unwrap()
            {
                Message::Response(response) => break response.response_result.unwrap(),
                Message::Notification(note)
                    if expect_valid && note.method == "textDocument/publishDiagnostics" =>
                {
                    assert!(
                        note.params["diagnostics"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|d| d["severity"] != 1),
                        "{}",
                        note.params
                    );
                }
                _ => {}
            }
        }
    };
    request(
        1,
        "initialize",
        json!({"rootUri":root,"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":true}}}}),
    );
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".into(),
            json!({}),
        )))
        .unwrap();
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".into(), json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":files.iter().find(|(path, _)| *path == "main.rud").unwrap().1}})))).unwrap();
    let mut result = request(
        2,
        method,
        json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character},"range":{"start":{"line":0,"character":0},"end":{"line":files[0].1.lines().count(),"character":0}},"context":{"diagnostics":[]}}),
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
        &[(
            "main.rud",
            "type Person = { name: String }\nlet value : Per",
        )],
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
        ("Child.rud", "let value = 1n"),
        ("Child/module.rud", ""),
        ("Child/module.rud", "// empty module\n"),
    ] {
        let result = editor_request(
            &[("main.rud", "module Child"), (target, body)],
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
            ("main.rud", "module Parent = module Child end"),
            ("Parent/Child.rud", ""),
        ],
        "textDocument/definition",
        0,
        24,
    );
    assert_eq!(result["uri"], "Parent/Child.rud");
}

#[test]
fn editor_module_navigation_does_not_guess_missing_or_ambiguous_files() {
    for files in [
        vec![("main.rud", "module Child")],
        vec![
            ("main.rud", "module Child"),
            ("Child.rud", ""),
            ("Child/module.rud", ""),
        ],
        vec![("main.rud", "module Child = end"), ("Child.rud", "")],
    ] {
        assert!(editor_request(&files, "textDocument/definition", 0, 8).is_null());
    }
}

#[test]
fn editor_can_initialize_open_hover_and_shutdown() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false").unwrap();
    std::fs::write(tree.path().join("main.rud"), "let value = 1n").unwrap();
    let root = format!("file://{}", tree.path().display());
    #[cfg(unix)]
    std::os::unix::fs::symlink(tree.path().join("main.rud"), tree.path().join("open.rud")).unwrap();
    let uri = format!(
        "{root}/{}",
        if cfg!(unix) { "open.rud" } else { "main.rud" }
    );
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_cli::lsp::serve(server).unwrap());
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
    let capabilities = response.response_result.unwrap()["capabilities"].clone();
    assert_eq!(capabilities["hoverProvider"], true);
    assert_eq!(capabilities["documentFormattingProvider"], true);
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
                    .contains("Bool")
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
        ruddy_cli::lsp::offset(source, &json!({"line":0,"character":3})),
        Some(5)
    );
    assert_eq!(
        ruddy_cli::lsp::offset(source, &json!({"line":0,"character":2})),
        None
    );
    assert_eq!(
        ruddy_cli::lsp::offset(source, &json!({"line":0,"character":999})),
        Some(6)
    );
    assert_eq!(
        ruddy_cli::lsp::position(source, 5),
        json!({"line":0,"character":3})
    );
    assert_eq!(
        ruddy_cli::lsp::position(source, 8),
        json!({"line":1,"character":0})
    );
}

#[test]
fn rapid_changes_publish_current_diagnostics_and_disk_creation_is_observed() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false").unwrap();
    std::fs::write(tree.path().join("main.rud"), "module Added\nlet value = 1n").unwrap();
    let root = format!("file://{}", tree.path().display());
    let uri = format!("{root}/main.rud");
    let (server, client) = Connection::memory();
    let worker = std::thread::spawn(move || ruddy_cli::lsp::serve(server).unwrap());
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
                        .contains("Bool")
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
    // missing candidates, including the competing Added/module.rud form.
    std::fs::write(tree.path().join("Added.rud"), "let answer : Nat = false").unwrap();
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
    let added_uri = format!("{root}/Added.rud");
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
    std::fs::write(tree.path().join("Added.rud"), "let answer = 1n").unwrap();
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

/// Formatting is one edit over the whole buffer, or none when the buffer is
/// already formatted; a buffer with a syntax error is formatted around it.
#[test]
fn editor_formats_the_whole_document() {
    let edits = editor_request(
        &[("main.rud", "let   value = 1n\nlet  other = 2n")],
        "textDocument/formatting",
        0,
        0,
    );
    let edits = edits.as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["newText"], "let value = 1n\nlet other = 2n\n");
    assert_eq!(edits[0]["range"]["start"], json!({"line":0,"character":0}));
    assert_eq!(edits[0]["range"]["end"], json!({"line":1,"character":15}));

    let edits = editor_request(
        &[("main.rud", "let value = 1n\n")],
        "textDocument/formatting",
        0,
        0,
    );
    assert_eq!(edits, json!([]));

    let edits = editor_request(
        &[("main.rud", "let   value = 1n\nlet = \nlet  other = 2n")],
        "textDocument/formatting",
        0,
        0,
    );
    assert_eq!(
        edits[0]["newText"],
        "let value = 1n\nlet =\nlet other = 2n\n"
    );
}

#[test]
fn editor_offers_tail_recursion_for_counting() {
    let source =
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 + count rest end\n";
    let actions = tail_actions(source);
    assert_eq!(actions.as_array().unwrap().len(), 1, "{actions}");
    assert_eq!(actions[0]["title"], "Convert to tail recursion");
    assert_eq!(actions[0]["kind"], "quickfix");
    assert_eq!(
        actions[0]["diagnostics"][0]["code"],
        "tail-recursion-opportunity"
    );
    assert_eq!(actions[0]["diagnostics"][0]["severity"], 3);
    let rewritten = apply_tail_action(source, &actions[0]);
    assert!(
        tail_actions(&rewritten).as_array().unwrap().is_empty(),
        "{rewritten}"
    );
}

fn tail_actions(source: &str) -> serde_json::Value {
    editor_request(&[("main.rud", source)], "textDocument/codeAction", 0, 0)
}

fn apply_tail_action(source: &str, action: &serde_json::Value) -> String {
    let document = &action["edit"]["documentChanges"][0];

    let byte = |position: &serde_json::Value| {
        let line = position["line"].as_u64().unwrap() as usize;
        let column = position["character"].as_u64().unwrap() as usize;
        let start: usize = source.split_inclusive('\n').take(line).map(str::len).sum();
        let mut utf16 = 0;
        for (offset, ch) in source[start..].char_indices() {
            if utf16 == column {
                return start + offset;
            }
            utf16 += ch.len_utf16();
        }
        assert_eq!(utf16, column);
        source.len()
    };
    let raw_edits = if document.is_null() {
        action["edit"]["changes"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
    } else {
        &document["edits"]
    };
    let mut edits: Vec<_> = raw_edits
        .as_array()
        .unwrap()
        .iter()
        .map(|edit| {
            (
                byte(&edit["range"]["start"]),
                byte(&edit["range"]["end"]),
                edit["newText"].as_str().unwrap(),
            )
        })
        .collect();
    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut result = source.to_owned();
    for (start, end, text) in edits {
        result.replace_range(start..end, text);
    }
    result
}

fn tail_manifest() -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    format!(
        "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = {root:?}\n"
    )
}

fn standard_tail_actions(source: &str) -> serde_json::Value {
    editor_request(
        &[("main.rud", source), ("Ruddy.toml", &tail_manifest())],
        "textDocument/codeAction",
        0,
        0,
    )
}

fn tail_results(source: &str) -> (Vec<String>, Vec<String>) {
    let tree = tempfile::tempdir().unwrap();
    let manifest = if source.contains("::") {
        tail_manifest()
    } else {
        "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\ntarget = \"js\"\n[dependencies]\nstd = false\n".to_owned()
    };
    std::fs::write(tree.path().join("Ruddy.toml"), manifest).unwrap();
    std::fs::write(tree.path().join("main.rud"), source).unwrap();
    let linked =
        ruddy_cli::compile(tree.path()).unwrap_or_else(|error| panic!("{error:?}\n{source}"));
    let program = ruddy_interp::Program::load(&linked).unwrap();
    let interpreted = ["empty", "sample"]
        .iter()
        .map(|name| ruddy_interp::render::json(&program.export(name).unwrap()))
        .collect();
    let javascript_path = tree.path().join("program.mjs");
    std::fs::write(
        &javascript_path,
        ruddy::backend::js::generate(&linked).unwrap(),
    )
    .unwrap();
    let script = format!(
        "import {{pathToFileURL}} from 'node:url'; const app = await import(pathToFileURL({})); console.log(JSON.stringify([app.empty, app.sample], (_, v) => typeof v === 'bigint' ? Number(v) : v));",
        serde_json::to_string(&javascript_path).unwrap()
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
    let values: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    (
        interpreted,
        values.iter().map(ToString::to_string).collect(),
    )
}

#[test]
fn editor_tail_recursion_rewrites_standard_numeric_addition() {
    for (ty, suffix, add) in [
        ("Nat", "n", "std::nat::add"),
        ("Int", "i", "std::int::add"),
        ("Nat8", "n8", "std::nat::add8"),
        ("Int64", "i64", "std::int::add64"),
        ("Real", "", "std::real::add"),
    ] {
        let source = format!(
            "let sum: [{ty}] -> {ty} = fn xs => match xs with | [] => 0{suffix} | [x, ..rest] => {add} x (sum rest) end\nlet empty = sum []\nlet sample = sum [1{suffix}, 2{suffix}, 3{suffix}]\n"
        );
        let actions = standard_tail_actions(&source);
        assert_eq!(actions.as_array().unwrap().len(), 1, "{ty}: {actions}");
        let rewritten = apply_tail_action(&source, &actions[0]);
        for program in [&source, &rewritten] {
            let (interpreted, javascript) = tail_results(program);
            let expected = if ty == "Int64" {
                ["\"0\"", "\"6\""]
            } else {
                ["0", "6"]
            };
            assert_eq!(interpreted, expected, "{program}");
            assert_eq!(javascript, ["0", "6"], "{program}");
        }
    }
}

#[test]
fn editor_tail_recursion_obeys_effect_rows() {
    for annotation in [
        "",
        ": (Real -> Real + ..'e) -> [Real] -> Real + ..'e",
        ": (Real -> Real + !Ask) -> [Real] -> Real + !Ask",
    ] {
        let source = format!(
            "effect Ask = {{ ask: () -> Real }}\nlet sum{annotation} = fn f => fn xs => match xs with | [] => 0 | [x, ..rest] => f x + sum f rest end\n"
        );
        assert!(
            tail_actions(&source).as_array().unwrap().is_empty(),
            "{source}"
        );
    }
    for source in [
        "let sum: (Real -> Real) -> [Real] -> Real = fn f => fn xs => match xs with | [] => 0 | [x, ..rest] => f x + sum f rest end\n",
        "let pure = fn x => x + 1\nlet sum = fn xs => match xs with | [] => 0 | [x, ..rest] => pure x + sum rest end\n",
        "effect Ask = { ask: () -> Real }\nlet sum = fn xs => match xs with | [] => 0 | [x, ..rest] => (fn _ => handle !Ask.ask () with | !Ask.ask _ => 1 end) () + sum rest end\n",
        "effect Ask = { ask: () -> Real }\nlet sum = fn xs => match xs with | [] => 0 | [x, ..rest] => (handle !Ask.ask () with | !Ask.ask _ => 1 end) + sum rest end\n",
        "effect Ask = { ask: () -> Real }\nlet sum = fn xs => match xs with | [] => 0 | [x, ..rest] => (handle !Ask.ask () with | !Ask.ask _ => raise 1 | return value => value + 1 end) + sum rest end\n",
        "effect Ask = { ask: () -> Real }\nlet sum = fn xs => match xs with | [] => 0 | [x, ..rest] => (handle (handle !Ask.ask () with | !Ask.ask _ => !Ask.ask () end) with | !Ask.ask _ => 1 end) + sum rest end\n",
    ] {
        assert_eq!(
            tail_actions(source).as_array().unwrap().len(),
            1,
            "{source}"
        );
    }
}

#[test]
fn editor_tail_recursion_preserves_local_captures_and_source() {
    let source = "-- 🦀 keep this header\nlet make = fn bias => do\n  let tail_loop = bias\n  let count: Bool -> [Real] -> Real = fn enabled => fn xs => match xs with\n  -- keep the base case\n  | [] => tail_loop\n  | [x, ..rest] => if enabled then x + count enabled rest else count true rest end\n  end\n  return count\nend\nlet empty = make 10 true []\nlet sample = make 10 false [1, 2, 3]\n".replace('\n', "\r\n");
    let actions = tail_actions(&source);
    assert_eq!(actions.as_array().unwrap().len(), 1, "{actions}");
    let rewritten = apply_tail_action(&source, &actions[0]);
    assert!(rewritten.starts_with("-- 🦀 keep this header\r\nlet make"));
    assert_eq!(rewritten.matches("-- keep the base case").count(), 1);
    assert!(!rewritten.replace("\r\n", "").contains('\n'), "{rewritten}");
    assert!(
        tail_actions(&rewritten).as_array().unwrap().is_empty(),
        "{rewritten}"
    );
    for program in [&source, &rewritten] {
        let (interpreted, javascript) = tail_results(program);
        assert_eq!(interpreted, ["10", "15"], "{program}");
        assert_eq!(javascript, ["10", "15"], "{program}");
    }
}

#[test]
fn editor_tail_recursion_preserves_effect_order() {
    let source = r#"
effect Tick = { tick: Real -> () }
@private let count = fn xs => match xs with
| [] => 0
| [x, ..rest] => do
  _ = !Tick.tick x
  return 1 + count rest
end
end
let run = fn xs => do
  let trace = mut 0
  let value = handle count xs with
  | !Tick.tick x => do _ = trace := ~trace * 10 + x end
  end
  return value * 1000 + ~trace
end
let empty = run []
let sample = run [1, 2, 3]
"#;
    let actions = tail_actions(source);
    assert_eq!(actions.as_array().unwrap().len(), 1);
    let rewritten = apply_tail_action(source, &actions[0]);
    for program in [source, &rewritten] {
        let (interpreted, javascript) = tail_results(program);
        assert_eq!(interpreted, ["0", "3123"], "{program}");
        assert_eq!(javascript, ["0", "3123"], "{program}");
    }
}

#[test]
fn editor_tail_recursion_skips_unsupported_patterns() {
    for source in [
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => count rest end\n",
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => count rest + count rest end\n",
        "let first = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 + second rest end\nlet second = fn xs => first xs\n",
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 - count rest end\n",
        "let count = fn xs => do let count = fn ys => 1 return 1 + count xs end\n",
        "let add = fn a => fn b => a - b\nlet count = fn xs => match xs with | [] => 0 | [_, ..rest] => add 1 (count rest) end\n",
        "let apply = fn f => fn x => f x\nlet count = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 + apply count rest end\n",
        "effect Ask = { ask: () -> Real }\nlet count = fn xs => match xs with | [] => 0 | [_, ..rest] => count rest + !Ask.ask () end\n",
        "effect Ask = { ask: () -> Real }\nlet count = fn xs => match xs with | [] => 0 | [_, ..rest] => (handle !Ask.ask () with | !Ask.ask _ => !Ask.ask () end) + count rest end\n",
        "effect Ask = { ask: () -> Real }\nlet count = fn xs => match xs with | [] => 0 | [_, ..rest] => (handle !Ask.ask () with | !Ask.ask _ => 1 | return value => !Ask.ask () end) + count rest end\n",
    ] {
        assert!(
            tail_actions(source).as_array().unwrap().is_empty(),
            "{source}"
        );
    }
}

#[test]
fn editor_tail_recursion_actions_follow_document_revisions() {
    for versioned in [true, false] {
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n").unwrap();
        std::fs::write(tree.path().join("main.rud"), "").unwrap();
        let root = format!("file://{}/", tree.path().display());
        let uri = format!("{root}main.rud");
        let (server, client) = Connection::memory();
        let worker = std::thread::spawn(move || ruddy_cli::lsp::serve(server).unwrap());
        let notify = |method: &str, params| {
            client
                .sender
                .send(Message::Notification(Notification::new(
                    method.into(),
                    params,
                )))
                .unwrap()
        };
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
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                {
                    return response.response_result.unwrap();
                }
            }
        };
        let diagnostics = |version: i64| {
            loop {
                if let Message::Notification(note) = client
                    .receiver
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    && note.method == "textDocument/publishDiagnostics"
                    && note.params["version"] == version
                {
                    return note.params["diagnostics"].clone();
                }
            }
        };
        let initialized = request(
            1,
            "initialize",
            json!({"rootUri":root,"capabilities":{"workspace":{"workspaceEdit":{"documentChanges":versioned}}}}),
        );
        assert_eq!(
            initialized["capabilities"]["codeActionProvider"]["codeActionKinds"],
            json!(["quickfix"])
        );
        notify("initialized", json!({}));
        let source = "let label = \"🦀\" let count = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 + count rest end\n";
        notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"ruddy","version":1,"text":source}}),
        );
        let original = diagnostics(1);
        assert_eq!(original.as_array().unwrap().len(), 1, "{original}");
        assert_eq!(original[0]["severity"], 3);
        assert_eq!(
            original[0]["range"]["start"]["character"],
            source[..source.find("count").unwrap()]
                .encode_utf16()
                .count()
        );
        let mut params = json!({"textDocument":{"uri":uri},"range":original[0]["range"],"context":{"diagnostics":original,"only":["refactor"]}});
        assert_eq!(
            request(2, "textDocument/codeAction", params.clone()),
            json!([])
        );
        params["context"]["only"] = json!(["quickfix"]);
        params["range"] = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}});
        assert_eq!(
            request(3, "textDocument/codeAction", params.clone()),
            json!([])
        );
        params["range"] = original[0]["range"].clone();
        let actions = request(4, "textDocument/codeAction", params.clone());
        assert_eq!(actions.as_array().unwrap().len(), 1);
        if versioned {
            assert_eq!(
                actions[0]["edit"]["documentChanges"][0]["textDocument"]["version"],
                1
            );
        } else {
            assert!(actions[0]["edit"]["documentChanges"].is_null());
            assert!(
                actions[0]["edit"]["changes"][uri.as_str()].is_array(),
                "{actions}"
            );
        }
        let rewritten = apply_tail_action(source, &actions[0]);
        let shifted = format!("-- moved\n{source}");
        notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":shifted}]}),
        );
        let current = diagnostics(2);
        // A request carrying an old diagnostic range cannot edit the new location.
        assert_eq!(
            request(5, "textDocument/codeAction", params.clone()),
            json!([])
        );
        params["range"] = current[0]["range"].clone();
        let fresh = request(6, "textDocument/codeAction", params.clone());
        assert_eq!(fresh.as_array().unwrap().len(), 1);
        if versioned {
            assert_eq!(
                fresh[0]["edit"]["documentChanges"][0]["textDocument"]["version"],
                2
            );
        }
        notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":format!("-- moved\n{rewritten}")}]}),
        );
        assert_eq!(diagnostics(3), json!([]));
        assert_eq!(
            request(7, "textDocument/codeAction", params.clone()),
            json!([])
        );
        notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":"let count = fn\n"}]}),
        );
        assert!(
            diagnostics(4)
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["severity"] == 1)
        );
        params["range"] = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":14}});
        assert_eq!(request(8, "textDocument/codeAction", params), json!([]));
        request(9, "shutdown", json!(null));
        notify("exit", json!(null));
        worker.join().unwrap();
    }
}

#[test]
fn editor_tail_recursion_does_not_break_polymorphic_recursion() {
    let source = "let count: 'a -> [Real] -> Real = fn item => fn xs => match xs with | [] => 0 | [_, ..rest] => 1 + count [item] rest end\n";
    assert!(tail_actions(source).as_array().unwrap().is_empty());
}

#[test]
fn editor_tail_recursion_forwards_the_recursive_continuation() {
    use ruddy::artifact::{End, Op, Rep};
    for (operator, base, expected) in [("+", "0", "4096"), ("*", "1", "1")] {
        let source = &format!(
            "let count: [Real] -> Real = fn xs => match xs with | [] => {base} | [_, ..rest] => 1 {operator} count rest end\n"
        );
        let rewritten = apply_tail_action(source, &tail_actions(source)[0]);
        let tree = tempfile::tempdir().unwrap();
        std::fs::write(tree.path().join("Ruddy.toml"), "name = \"editor\"\nversion = \"0.0.0\"\nkind = \"library\"\nroot = \"main.rud\"\n[dependencies]\nstd = false\n").unwrap();
        for (text, tail) in [(source.as_str(), false), (rewritten.as_str(), true)] {
            std::fs::write(tree.path().join("main.rud"), text).unwrap();
            let artifact = ruddy_cli::compile(tree.path()).unwrap();
            // Select the numeric reduction body, then its calls receiving the
            // remaining array. Curried helper setup may itself need continuations;
            // the complete recursive invocation must forward the caller's one.
            let recursive: Vec<_> = artifact
                .lir()
                .functions
                .iter()
                .filter(|function| {
                    function
                        .blocks
                        .iter()
                        .flat_map(|block| &block.instrs)
                        .any(|instr| {
                            matches!(
                                (&instr.op, operator),
                                (Op::Add { .. }, "+") | (Op::Mul { .. }, "*")
                            )
                        })
                })
                .flat_map(|function| {
                    let reps: std::collections::HashMap<_, _> = function
                        .params
                        .iter()
                        .map(|p| (p.temp, p.rep))
                        .chain(function.blocks.iter().flat_map(|block| {
                            block
                                .params
                                .iter()
                                .map(|p| (p.temp, p.rep))
                                .chain(block.instrs.iter().map(|i| (i.temp, i.rep)))
                        }))
                        .collect();
                    function
                        .blocks
                        .iter()
                        .filter_map(move |block| match &block.end {
                            End::Call {
                                args, continuation, ..
                            } if args
                                .last()
                                .is_some_and(|arg| reps.get(arg) == Some(&Rep::Array)) =>
                            {
                                Some(*continuation == function.continuation)
                            }
                            _ => None,
                        })
                })
                .collect();
            assert!(
                !recursive.is_empty(),
                "the reduction calls itself with the remaining array"
            );
            assert!(
                recursive.iter().all(|forwarded| *forwarded == tail),
                "{recursive:?}\n{text}"
            );
            if tail {
                let mut program = ruddy_interp::Program::load(&artifact).unwrap();
                let count = program.export("count").unwrap();
                let value = program
                    .call(
                        &count,
                        ruddy_interp::Value::array(vec![ruddy_interp::Value::Real(0.0); 4096]),
                    )
                    .unwrap();
                assert_eq!(ruddy_interp::render::json(&value), expected);
            }
        }
    }
}

#[test]
fn editor_tail_recursion_reorders_pure_composite_values() {
    for (expression, expected) in [
        ("(-x)", "-6"),
        ("do let y = x return y + 1 end", "9"),
        ("do return do let y = x return y end end", "6"),
        ("if not false then x else 0 end", "6"),
        ("({ value: x, ..{ extra: true } }).value", "6"),
        (
            "(fn values => match values with | [y] => y | [..] => 0 end) [x]",
            "6",
        ),
        (
            "(fn tag => match tag with | #Some y => y end) (#Some x)",
            "6",
        ),
        ("(fn tag => match tag with | #Empty => x end) #Empty", "6"),
    ] {
        let source = format!(
            "let sum: [Real] -> Real = fn xs => match xs with | [] => 0 | [x, ..rest] => ({expression}) + sum rest end\nlet empty = sum []\nlet sample = sum [1, 2, 3]\n"
        );
        let actions = tail_actions(&source);
        assert_eq!(actions.as_array().unwrap().len(), 1, "{source}");
        let rewritten = apply_tail_action(&source, &actions[0]);
        let (interpreted, javascript) = tail_results(&rewritten);
        assert_eq!(interpreted, ["0", expected], "{rewritten}");
        assert_eq!(javascript, ["0", expected], "{rewritten}");
    }
}

#[test]
fn editor_tail_recursion_declines_unsafe_or_unrepresentable_edits() {
    for source in [
        "let count = fn _ => 1 + count ()\n",
        "let count = fn xs => match count xs with | value => 1 + value end\n",
        "let count = fn xs => do let value = count xs return 1 + value end\n",
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => 1 (* retain this comment *) + count rest end\n",
        "let count = fn xs => match xs with | [] => 0 | [_, ..rest] => (do let cell = mut 1 return ~cell end) + count rest end\n",
        "let count = fn cell => fn xs => match xs with | [] => 0 | [_, ..rest] => (cell := 1) + count cell rest end\n",
    ] {
        assert!(
            tail_actions(source).as_array().unwrap().is_empty(),
            "{source}"
        );
    }
}

#[test]
fn editor_tail_recursion_preserves_function_blocks() {
    for body in [
        "do let bias = 1 return match xs with | [] => 0 | [_, ..rest] => bias + count rest end end",
        "do return match xs with | [] => 0 | [_, ..rest] => 1 + count rest end end",
        "do let (bias, extra) = (1, 0) return match xs with | [] => extra | [_, ..rest] => bias + count rest end end",
    ] {
        let source = format!(
            "let count: [Real] -> Real = fn xs => {body}\nlet empty = count []\nlet sample = count [1, 2, 3]\n"
        );
        let actions = tail_actions(&source);
        assert_eq!(actions.as_array().unwrap().len(), 1, "{source}");
        let rewritten = apply_tail_action(&source, &actions[0]);
        let (interpreted, javascript) = tail_results(&rewritten);
        assert_eq!(interpreted, ["0", "3"], "{rewritten}");
        assert_eq!(javascript, ["0", "3"], "{rewritten}");
    }
}

#[test]
fn editor_tail_recursion_rewrites_products() {
    let source = "let product: [Real] -> Real = fn xs => match xs with | [] => 1 | [x, ..rest] => x * product rest end\nlet empty = product []\nlet sample = product [2, 3, 4]\n";
    let actions = tail_actions(source);
    assert_eq!(actions.as_array().unwrap().len(), 1);
    let rewritten = apply_tail_action(source, &actions[0]);
    for program in [source, &rewritten] {
        let (interpreted, javascript) = tail_results(program);
        assert_eq!(interpreted, ["1", "24"], "{program}");
        assert_eq!(javascript, ["1", "24"], "{program}");
    }
    assert!(tail_actions(&rewritten).as_array().unwrap().is_empty());
}

#[test]
fn editor_tail_recursion_rewrites_standard_numeric_multiplication() {
    for (ty, suffix, multiply) in [
        ("Nat", "n", "std::nat::multiply"),
        ("Int", "i", "std::int::multiply"),
        ("Nat8", "n8", "std::nat::multiply8"),
        ("Int64", "i64", "std::int::multiply64"),
        ("Real", "", "std::real::multiply"),
    ] {
        let source = format!(
            "let product: [{ty}] -> {ty} = fn xs => match xs with | [] => 1{suffix} | [x, ..rest] => {multiply} x (product rest) end\nlet empty = product []\nlet sample = product [2{suffix}, 3{suffix}, 4{suffix}]\n"
        );
        let actions = standard_tail_actions(&source);
        assert_eq!(actions.as_array().unwrap().len(), 1, "{ty}: {actions}");
        let rewritten = apply_tail_action(&source, &actions[0]);
        for program in [&source, &rewritten] {
            let (interpreted, javascript) = tail_results(program);
            let expected = if ty == "Int64" {
                ["\"1\"", "\"24\""]
            } else {
                ["1", "24"]
            };
            assert_eq!(interpreted, expected, "{program}");
            assert_eq!(javascript, ["1", "24"], "{program}");
        }
    }
}

#[test]
fn editor_tail_recursion_products_preserve_base_cases_and_tail_branches() {
    for (branch, values) in [
        ("product rest * x", "2, 3, 4"),
        (
            "match rest with | [] => product rest | [..] => x * product rest end",
            "2, 3, 4, 99",
        ),
        (
            "(do let factor = x return factor end) * product rest",
            "2, 3, 4",
        ),
    ] {
        let source = format!(
            "let product: [Real] -> Real = fn xs => match xs with | [] => 2 | [x, ..rest] => {branch} end\nlet empty = product []\nlet sample = product [{values}]\n"
        );
        let actions = tail_actions(&source);
        assert_eq!(actions.as_array().unwrap().len(), 1, "{source}");
        let rewritten = apply_tail_action(&source, &actions[0]);
        for program in [&source, &rewritten] {
            let (interpreted, javascript) = tail_results(program);
            assert_eq!(interpreted, ["2", "48"], "{program}");
            assert_eq!(javascript, ["2", "48"], "{program}");
        }
    }
}

#[test]
fn editor_tail_recursion_products_require_known_operations_and_closed_effects() {
    for source in [
        "let product = fn xs => match xs with | [] => 1 | [x, ..rest] => match rest with | [] => x + product rest | [..] => x * product rest end end\n",
        "let product = fn xs => match xs with | [] => 1 | [x, ..rest] => x * product rest + 1 end\n",
        "let multiply = fn x => fn y => x - y\nlet product = fn xs => match xs with | [] => 1 | [x, ..rest] => multiply x (product rest) end\n",
        "let product: (Real -> Real + ..'e) -> [Real] -> Real + ..'e = fn f => fn xs => match xs with | [] => 1 | [x, ..rest] => f x * product f rest end\n",
        "effect Ask = { ask: Real -> Real }\nlet product: [Real] -> Real + !Ask = fn xs => match xs with | [] => 1 | [x, ..rest] => !Ask.ask x * product rest end\n",
    ] {
        assert!(
            tail_actions(source).as_array().unwrap().is_empty(),
            "{source}"
        );
    }
    let source = "let product: (Real -> Real) -> [Real] -> Real = fn f => fn xs => match xs with | [] => 1 | [x, ..rest] => f x * product f rest end\n";
    assert_eq!(tail_actions(source).as_array().unwrap().len(), 1);
}
