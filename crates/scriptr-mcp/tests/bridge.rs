//! The bridge end of the MCP integration: real process, real stdio, real HTTP
//! against a mock Scriptr. No desktop app involved.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

/// A temp HOME so the bridge reads our mcp.json and never the real one.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("scriptr-mcp-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Library/Application Support/scriptr")).unwrap();
        Home(dir)
    }

    fn data(&self) -> PathBuf {
        self.0.join("Library/Application Support/scriptr")
    }

    fn write_config(&self, port: u16) {
        std::fs::write(
            self.data().join("mcp.json"),
            serde_json::json!({
                "apiVersion": 1, "port": port, "token": "tok-123", "pid": 1, "startedAt": 0,
                "inbox": self.data().join("inbox").to_string_lossy()
            })
            .to_string(),
        )
        .unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Answers every request with `reply` and `status`, handing each raw request
/// back. It keeps
/// serving rather than answering once: an ephemeral port on a developer's
/// machine attracts stray probes, and the bridge's request may not be first.
fn mock_scriptr(reply: &'static str) -> (u16, mpsc::Receiver<String>) {
    mock_scriptr_status(reply, "200 OK")
}

fn mock_scriptr_status(reply: &'static str, status: &'static str) -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let mut stream = stream;
            std::thread::spawn(move || {
                // Headers and body can arrive in separate reads: keep going
                // until content-length bytes of body are in hand.
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                    let want: usize = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    if body.len() >= want {
                        break;
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&raw).into_owned());
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    (port, rx)
}

/// The next request whose first line starts with `prefix`, ignoring stray probes.
fn request_matching(requests: &mpsc::Receiver<String>, prefix: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match requests.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(raw) if raw.starts_with(prefix) => return raw,
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(e) => panic!("mock server stopped: {e}"),
        }
    }
    panic!("the bridge never sent a request starting with {prefix:?}");
}

struct Bridge(Child);

impl Bridge {
    fn start(home: &Home) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_scriptr-mcp"))
            .env("HOME", &home.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the bridge binary should run");
        Bridge(child)
    }

    /// Sends one request and returns its reply.
    fn call(&mut self, line: &str) -> serde_json::Value {
        let stdin = self.0.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let stdout = self.0.stdout.as_mut().unwrap();
        let mut reply = String::new();
        BufReader::new(stdout).read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).expect("the bridge must answer with one JSON message per line")
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tool_text(reply: &serde_json::Value) -> String {
    reply["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn file_task_reaches_the_app_with_the_token() {
    let home = Home::new("online");
    let (port, requests) = mock_scriptr(r#"{"id":"t1","title":"Fix the flaky webhook test","status":"backlog"}"#);
    home.write_config(port);

    let mut bridge = Bridge::start(&home);
    let reply = bridge.call(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"file_task","arguments":{"project":"/code/acme","title":"Fix the flaky webhook test","goal":"test_replay flakes in CI"}}}"#,
    );

    let raw = request_matching(&requests, "POST /v1/tasks ");
    assert!(
        raw.to_ascii_lowercase().contains("authorization: bearer tok-123"),
        "the call must carry the token from mcp.json: {raw}"
    );
    let body: serde_json::Value =
        serde_json::from_str(raw.split_once("\r\n\r\n").expect("a body").1).expect("the body must be JSON");
    assert_eq!(body["project"], "/code/acme");
    assert_eq!(body["title"], "Fix the flaky webhook test");
    assert_eq!(body["goal"], "test_replay flakes in CI");

    let text = tool_text(&reply);
    assert!(text.contains("Added"), "{text}");
    assert!(text.contains("backlog"), "the model should learn which column it landed in: {text}");
    assert_eq!(reply["result"]["isError"], false);
}

#[test]
fn a_task_filed_while_scriptr_is_closed_is_spooled_for_later() {
    // No mcp.json at all: the app has never run, or is shut down.
    let home = Home::new("offline");
    let mut bridge = Bridge::start(&home);

    let reply = bridge.call(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"file_task","arguments":{"project":"acme","title":"Add OpenAPI docs","goal":"Generate a spec for the billing routes"}}}"#,
    );
    let text = tool_text(&reply);
    assert_eq!(reply["result"]["isError"], false, "{text}");
    assert!(text.contains("queued"), "the model should learn it was spooled: {text}");

    let inbox = home.data().join("inbox");
    let files: Vec<_> = std::fs::read_dir(&inbox).unwrap().flatten().collect();
    assert_eq!(files.len(), 1, "exactly one spooled task");
    let spooled: serde_json::Value = serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(spooled["title"], "Add OpenAPI docs");
    assert_eq!(spooled["project"], "acme");
}

#[test]
fn the_handshake_and_tool_list_are_stable() {
    let home = Home::new("handshake");
    let mut bridge = Bridge::start(&home);

    // Legacy clients open with initialize…
    let init = bridge.call(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "echo a version we support");
    assert_eq!(init["result"]["serverInfo"]["name"], "scriptr");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // …modern ones probe instead. Both must work from the same binary.
    let discover = bridge.call(r#"{"jsonrpc":"2.0","id":2,"method":"server/discover"}"#);
    let versions = discover["result"]["protocolVersions"].as_array().unwrap();
    assert!(versions.iter().any(|v| v == "2026-07-28"), "{versions:?}");

    let tools = bridge.call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "file_task", "list_projects", "list_tasks", "scriptr_status", "get_task", "update_task",
        "move_task", "list_scripts", "control_script", "run_group", "get_logs", "list_groups",
    ] {
        assert!(names.contains(&want), "{want} is missing from tools/list: {names:?}");
    }
    // Nothing on the board surface is destructive: no tool deletes anything.
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_ne!(tool["annotations"]["destructiveHint"], true, "{} claims to be destructive", tool["name"]);
    }
    // Every tool needs a schema a model can fill in.
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        assert!(tool["description"].as_str().unwrap_or("").len() > 20, "{} needs a real description", tool["name"]);
    }

    let unknown = bridge.call(r#"{"jsonrpc":"2.0","id":4,"method":"nonsense/method"}"#);
    assert_eq!(unknown["error"]["code"], -32601);
}

#[test]
fn a_refusal_reaches_the_model_with_its_reason() {
    // A refusal's body is the only actionable part of it; a bare status code
    // would leave the model unable to tell the user what went wrong.
    let home = Home::new("refused");
    let (port, requests) = mock_scriptr_status("no project \"ghost\" in Scriptr — known: acme", "404 Not Found");
    home.write_config(port);

    let mut bridge = Bridge::start(&home);
    let reply = bridge.call(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tasks","arguments":{"project":"ghost"}}}"#,
    );
    request_matching(&requests, "GET /v1/tasks?project=ghost ");

    let text = tool_text(&reply);
    assert_eq!(reply["result"]["isError"], true, "{text}");
    assert!(text.contains("known: acme"), "the reason must survive the hop: {text}");
}

#[test]
fn update_task_sends_a_patch_with_only_the_fields_given() {
    let home = Home::new("patch");
    let (port, requests) = mock_scriptr(r#"{"id":"t1","title":"Ship it","status":"todo","priority":3}"#);
    home.write_config(port);

    let mut bridge = Bridge::start(&home);
    let reply = bridge.call(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"update_task","arguments":{"taskId":"t1","status":"todo","labels":["urgent"]}}}"#,
    );

    let raw = request_matching(&requests, "PATCH /v1/tasks/t1 ");
    let body: serde_json::Value = serde_json::from_str(raw.split_once("\r\n\r\n").expect("a body").1).unwrap();
    assert_eq!(body["status"], "todo");
    assert_eq!(body["labels"][0], "urgent");
    assert!(body.get("taskId").is_none(), "the id travels in the path, not the body: {body}");
    assert!(body.get("priority").is_none(), "an unset field must not be sent: {body}");
    assert_eq!(reply["result"]["isError"], false, "{}", tool_text(&reply));
}

#[test]
fn a_tool_called_without_its_required_argument_says_which() {
    let home = Home::new("required");
    let mut bridge = Bridge::start(&home);
    for (tool, key) in [("get_task", "taskId"), ("get_logs", "target"), ("run_group", "group")] {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
        );
        let reply = bridge.call(&line);
        let text = tool_text(&reply);
        assert_eq!(reply["result"]["isError"], true, "{tool}: {text}");
        assert!(text.contains(key), "{tool} should name the missing `{key}`: {text}");
    }
}
