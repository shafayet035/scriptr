//! MCP bridge between a coding agent and the Scriptr desktop app.
//!
//! Scriptr itself is a GUI process and cannot be a stdio server, so this small
//! binary speaks MCP on stdin/stdout and plain HTTP to the app's loopback API,
//! discovering the port and token from `mcp.json`.
//!
//! Dual-era by design: the 2026-07-28 revision dropped the `initialize`
//! handshake for per-request metadata plus `server/discover`, but clients in
//! the wild still open with `initialize`. We answer both.
//!
//! When Scriptr isn't running, `file_task` spools to an inbox the app drains on
//! launch — filing a task must not require the app to be open.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

/// Revisions we can speak. Newest first.
const VERSIONS: [&str; 3] = ["2026-07-28", "2025-11-25", "2025-06-18"];
const SERVER_NAME: &str = "scriptr";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpInfo {
    port: u16,
    token: String,
    inbox: String,
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("scriptr").join("mcp.json"))
}

fn read_info() -> Option<McpInfo> {
    serde_json::from_slice(&std::fs::read(config_path()?).ok()?).ok()
}

// ---- HTTP to the app ------------------------------------------------------

enum Call {
    Ok(Value),
    /// The app is not running (or not answering).
    Offline(String),
    Failed(String),
}

fn request(method: &str, path: &str, body: Option<Value>) -> Call {
    let Some(info) = read_info() else {
        return Call::Offline("Scriptr is not running".into());
    };
    let url = format!("http://127.0.0.1:{}{path}", info.port);
    let auth = format!("Bearer {}", info.token);
    let sent = match (method, body) {
        ("POST", Some(b)) => ureq::post(&url).header("authorization", &auth).send_json(b),
        _ => ureq::get(&url).header("authorization", &auth).call(),
    };
    match sent {
        Ok(mut res) => match res.body_mut().read_json::<Value>() {
            Ok(v) => Call::Ok(v),
            Err(e) => Call::Failed(format!("unreadable reply from Scriptr: {e}")),
        },
        Err(ureq::Error::StatusCode(code)) => {
            Call::Failed(format!("Scriptr refused the request ({code})"))
        }
        Err(e) => Call::Offline(format!("Scriptr is not reachable on port {} ({e})", info.port)),
    }
}

/// Writes a task into the inbox the app drains at launch.
fn spool(input: &Value) -> Result<String, String> {
    let dir = read_info()
        .map(|i| PathBuf::from(i.inbox))
        .or_else(|| config_path().map(|p| p.with_file_name("inbox")))
        .ok_or("cannot locate Scriptr's data directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // No uuid dependency: the pid plus a nanosecond stamp is unique enough for
    // a per-call file name.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{}-{stamp}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_vec_pretty(input).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

// ---- tools ----------------------------------------------------------------

fn tools() -> Value {
    json!([
        {
            "name": "file_task",
            "title": "File a task in Scriptr",
            "description": "Add a task to a Scriptr project's backlog, to be run later by a coding agent. \
The task is NOT started: a human reviews and runs it. Use this to hand work to the developer's \
Scriptr board instead of doing it yourself. Works even when Scriptr is closed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Scriptr project name, path, or id. Use the repository directory you are working in."},
                    "title": {"type": "string", "description": "Short imperative title, e.g. 'Rate-limit the checkout endpoint'"},
                    "goal": {"type": "string", "description": "The prompt the coding agent will receive: what to do and what done looks like. Be specific."},
                    "agentId": {"type": "string", "description": "Optional agent: claude-code, opencode, cursor-agent, gemini, aider"},
                    "model": {"type": "string"},
                    "effort": {"type": "string", "enum": ["low", "medium", "high", "extra", "max", "ultracode"]},
                    "priority": {"type": "integer", "minimum": 0, "maximum": 3, "description": "0 none, 1 low, 2 medium, 3 high"},
                    "labels": {"type": "array", "items": {"type": "string"}},
                    "issueUrl": {"type": "string"}
                },
                "required": ["project", "title", "goal"]
            },
            "annotations": {"title": "File a task in Scriptr", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false}
        },
        {
            "name": "list_projects",
            "title": "List Scriptr projects",
            "description": "Projects the developer has added to Scriptr, with their paths. Call this first if unsure what to pass as `project`.",
            "inputSchema": {"type": "object", "additionalProperties": false},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "list_tasks",
            "title": "List Scriptr tasks",
            "description": "Tasks and their status (backlog, working, review, done, failed), optionally for one project.",
            "inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string", "description": "Optional project name, path or id"}}
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "scriptr_status",
            "title": "Scriptr status",
            "description": "Whether Scriptr is running, how many projects it manages, how many scripts are up and how many agents are working.",
            "inputSchema": {"type": "object", "additionalProperties": false},
            "annotations": {"readOnlyHint": true}
        }
    ])
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({ "resultType": "complete", "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn call_tool(name: &str, args: &Value) -> Value {
    match name {
        "scriptr_status" => match request("GET", "/v1/status", None) {
            Call::Ok(v) => text_result(
                format!(
                    "Scriptr {} is running: {} projects, {} scripts up, {} agents working.",
                    v["version"].as_str().unwrap_or("?"),
                    v["projects"],
                    v["runningScripts"],
                    v["workingTasks"]
                ),
                false,
            ),
            Call::Offline(why) => text_result(format!("{why}. Tasks filed now will arrive when it next opens."), false),
            Call::Failed(e) => text_result(e, true),
        },
        "list_projects" => match request("GET", "/v1/projects", None) {
            Call::Ok(v) => {
                let list = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| format!("- {} ({})", p["name"].as_str().unwrap_or("?"), p["path"].as_str().unwrap_or("?")))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                text_result(if list.is_empty() { "No projects in Scriptr yet.".into() } else { list }, false)
            }
            Call::Offline(why) | Call::Failed(why) => text_result(why, true),
        },
        "list_tasks" => {
            let path = match args.get("project").and_then(Value::as_str) {
                Some(p) => format!("/v1/tasks?project={}", urlencode(p)),
                None => "/v1/tasks".to_string(),
            };
            match request("GET", &path, None) {
                Call::Ok(v) => {
                    let list = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|t| {
                                    format!(
                                        "- [{}] {} ({})",
                                        t["status"].as_str().unwrap_or("?"),
                                        t["title"].as_str().unwrap_or("?"),
                                        t["agentId"].as_str().unwrap_or("?")
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    text_result(if list.is_empty() { "No tasks.".into() } else { list }, false)
                }
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "file_task" => {
            for key in ["project", "title", "goal"] {
                if args.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").is_empty() {
                    return text_result(format!("`{key}` is required and must not be empty"), true);
                }
            }
            match request("POST", "/v1/tasks", Some(args.clone())) {
                Call::Ok(v) => text_result(
                    format!(
                        "Filed \"{}\" in Scriptr's backlog. It will not run until the developer starts it.",
                        v["title"].as_str().unwrap_or("task")
                    ),
                    false,
                ),
                Call::Offline(why) => match spool(args) {
                    Ok(_) => text_result(
                        format!("{why}, so the task was queued and will appear on the board when Scriptr next opens."),
                        false,
                    ),
                    Err(e) => text_result(format!("{why}, and the task could not be queued: {e}"), true),
                },
                Call::Failed(e) => text_result(e, true),
            }
        }
        other => text_result(format!("unknown tool: {other}"), true),
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---- JSON-RPC over stdio --------------------------------------------------

fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn capabilities() -> Value {
    json!({ "tools": { "listChanged": false } })
}

fn server_info() -> Value {
    json!({ "name": SERVER_NAME, "title": "Scriptr", "version": SERVER_VERSION })
}

/// Returns the reply for one request, or `None` for a notification.
fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let Some(id) = id else {
        return None; // notification: nothing to answer
    };

    Some(match method {
        // Legacy handshake (2025-11-25 and earlier).
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or(VERSIONS[0]);
            let version = if VERSIONS.contains(&asked) { asked } else { VERSIONS[0] };
            result(
                &id,
                json!({ "protocolVersion": version, "capabilities": capabilities(), "serverInfo": server_info() }),
            )
        }
        // Modern discovery (2026-07-28+).
        "server/discover" => result(
            &id,
            json!({
                "resultType": "complete",
                "protocolVersions": VERSIONS,
                "capabilities": capabilities(),
                "serverInfo": server_info()
            }),
        ),
        "ping" => result(&id, json!({})),
        "tools/list" => result(&id, json!({ "resultType": "complete", "tools": tools() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            result(&id, call_tool(name, &args))
        }
        "resources/list" => result(&id, json!({ "resultType": "complete", "resources": [] })),
        "prompts/list" => result(&id, json!({ "resultType": "complete", "prompts": [] })),
        _ => error(&id, -32601, &format!("method not found: {method}")),
    })
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // stdio framing is one JSON message per line; stdout carries protocol only,
    // so anything diagnostic goes to stderr.
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle(&msg),
            Err(e) => {
                eprintln!("scriptr-mcp: unparseable message: {e}");
                Some(error(&Value::Null, -32700, "parse error"))
            }
        };
        if let Some(reply) = reply {
            if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
                break;
            }
        }
    }
}
