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
    // A refusal's body is the only actionable part of it — "no script \"api\",
    // known: web, worker", or that the user has not allowed agent runs to be
    // started from outside — so take 4xx/5xx as a response to read, not an Err.
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let sent = match (method, body) {
        ("POST", Some(b)) => agent.post(&url).header("authorization", &auth).send_json(b),
        ("PATCH", Some(b)) => agent.patch(&url).header("authorization", &auth).send_json(b),
        _ => agent.get(&url).header("authorization", &auth).call(),
    };
    match sent {
        Ok(mut res) => {
            let status = res.status();
            let body = res.body_mut().read_to_string().unwrap_or_default();
            if status.is_success() {
                return match serde_json::from_str::<Value>(&body) {
                    Ok(v) => Call::Ok(v),
                    Err(e) => Call::Failed(format!("unreadable reply from Scriptr: {e}")),
                };
            }
            let why = body.trim();
            Call::Failed(if why.is_empty() {
                format!("Scriptr refused the request ({})", status.as_u16())
            } else {
                why.to_string()
            })
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
                    "issueUrl": {"type": "string"},
                    "workspace": {"type": "string", "enum": ["in-place", "worktree"], "description": "worktree gives the agent its own checkout on its own branch; in-place uses the project directory. Default in-place."},
                    "base": {"type": "string", "description": "Branch the work is cut from and its PR will target, e.g. main or staging. Omit for whatever the repository is on."}
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
        },
        {
            "name": "get_task",
            "title": "Read one Scriptr task",
            "description": "Everything about one task: its goal, status, branch, agent, every run with exit code, token cost and duration, and where its worktree is. Use it to find out how a task went before deciding what to do next.",
            "inputSchema": {
                "type": "object",
                "properties": {"taskId": {"type": "string", "description": "Task id, as list_tasks reports it"}},
                "required": ["taskId"]
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "update_task",
            "title": "Update a Scriptr task",
            "description": "Change a filed task: move it between board columns, re-prioritise it, raise its effort, retarget its base branch, rewrite its goal, or add labels. Only the fields you pass are touched. A running task's status cannot be changed — stop it first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "taskId": {"type": "string"},
                    "status": {"type": "string", "enum": ["backlog", "queued", "working", "verifying", "review", "done", "failed", "cancelled"], "description": "Board column. Use 'queued' to mark it ready, 'done' when it is finished."},
                    "title": {"type": "string"},
                    "goal": {"type": "string", "description": "Rewrites the prompt the agent will receive."},
                    "priority": {"type": "integer", "minimum": 0, "maximum": 3},
                    "effort": {"type": "string", "enum": ["low", "medium", "high", "extra", "max", "ultracode"]},
                    "base": {"type": "string", "description": "Branch the work targets, e.g. main or staging."},
                    "workspace": {"type": "string", "enum": ["in-place", "worktree"]},
                    "labels": {"type": "array", "items": {"type": "string"}},
                    "issueUrl": {"type": "string"}
                },
                "required": ["taskId"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true}
        },
        {
            "name": "start_task",
            "title": "Start a Scriptr agent run",
            "description": "Runs a task's coding agent now. This spends tokens and edits the developer's code, so it is refused unless they have turned on 'Let agents start runs' in Scriptr's settings. Prefer file_task and let them press Run.",
            "inputSchema": {"type": "object", "properties": {"taskId": {"type": "string"}}, "required": ["taskId"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
        },
        {
            "name": "stop_task",
            "title": "Stop a Scriptr agent run",
            "description": "Stops a task's running agent, signalling its whole process group. The task is recorded as cancelled.",
            "inputSchema": {"type": "object", "properties": {"taskId": {"type": "string"}}, "required": ["taskId"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true}
        },
        {
            "name": "list_scripts",
            "title": "List Scriptr scripts",
            "description": "The processes a project runs — server, worker, database, migrations — with whether each is up, its pid, port, uptime and readiness. Use it to find out what is running before starting or stopping anything.",
            "inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string", "description": "Optional project name, path or id"}}
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "control_script",
            "title": "Start, stop or restart a script",
            "description": "Starts, stops or restarts one of the project's processes. These are commands the developer configured themselves. Starting respects the script's readiness gate, so it returns once the process is up, not merely spawned.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Script name or id, as list_scripts reports it"},
                    "action": {"type": "string", "enum": ["start", "stop", "restart"]}
                },
                "required": ["script", "action"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false}
        },
        {
            "name": "run_group",
            "title": "Run a Scriptr group",
            "description": "Starts a named group of scripts in dependency order, waiting for each wave's readiness gates before the next. This is how you bring a whole stack up in one call.",
            "inputSchema": {
                "type": "object",
                "properties": {"group": {"type": "string", "description": "Group name or id"}},
                "required": ["group"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false}
        },
        {
            "name": "get_logs",
            "title": "Read script or agent output",
            "description": "Recent terminal output for a running or finished script, or for a task's agent, with ANSI escapes stripped. This is how you find out WHY something failed — read the logs before guessing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": {"type": "string", "description": "'script:<name-or-id>' or 'task:<id>'"},
                    "lines": {"type": "integer", "minimum": 1, "maximum": 2000, "description": "How many trailing lines, default 200"}
                },
                "required": ["target"]
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "publish_task",
            "title": "Open a pull request for a task",
            "description": "Commits whatever the agent left uncommitted on the task's branch, pushes it, and opens a pull request against the task's base branch. Only works for a task running on its own branch. Safe to call twice: an existing PR is reused rather than duplicated.",
            "inputSchema": {"type": "object", "properties": {"taskId": {"type": "string"}}, "required": ["taskId"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}
        },
        {
            "name": "list_groups",
            "title": "List Scriptr groups",
            "description": "Named groups of scripts a project can bring up together, with how many scripts each holds.",
            "inputSchema": {
                "type": "object",
                "properties": {"project": {"type": "string"}}
            },
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
        "get_task" => {
            let Some(id) = req_str(args, "taskId") else { return missing("taskId") };
            match request("GET", &format!("/v1/tasks/{}", urlencode(&id)), None) {
                Call::Ok(v) => text_result(describe_task(&v), false),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "update_task" => {
            let Some(id) = req_str(args, "taskId") else { return missing("taskId") };
            let mut patch = args.clone();
            if let Some(obj) = patch.as_object_mut() {
                obj.remove("taskId");
            }
            if patch.as_object().is_some_and(serde_json::Map::is_empty) {
                return text_result("nothing to change — pass at least one field besides taskId".into(), true);
            }
            match request("PATCH", &format!("/v1/tasks/{}", urlencode(&id)), Some(patch)) {
                Call::Ok(v) => text_result(
                    format!(
                        "Updated \"{}\" — now [{}], priority {}.",
                        v["title"].as_str().unwrap_or("task"),
                        v["status"].as_str().unwrap_or("?"),
                        v["priority"]
                    ),
                    false,
                ),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "start_task" | "stop_task" => {
            let Some(id) = req_str(args, "taskId") else { return missing("taskId") };
            let verb = if name == "start_task" { "start" } else { "stop" };
            match request("POST", &format!("/v1/tasks/{}/{verb}", urlencode(&id)), Some(json!({}))) {
                Call::Ok(v) => text_result(
                    format!(
                        "{} \"{}\" — now [{}].",
                        if verb == "start" { "Started" } else { "Stopped" },
                        v["title"].as_str().unwrap_or("task"),
                        v["status"].as_str().unwrap_or("?")
                    ),
                    false,
                ),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "publish_task" => {
            let Some(id) = req_str(args, "taskId") else { return missing("taskId") };
            match request("POST", &format!("/v1/tasks/{}/publish", urlencode(&id)), Some(json!({}))) {
                Call::Ok(v) => text_result(
                    match v["prUrl"].as_str() {
                        Some(url) => format!("{}\n\nPull request: {url}", v["did"].as_str().unwrap_or("published")),
                        None => v["did"].as_str().unwrap_or("published").to_string(),
                    },
                    false,
                ),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "list_scripts" => {
            let path = match args.get("project").and_then(Value::as_str) {
                Some(p) => format!("/v1/scripts?project={}", urlencode(p)),
                None => "/v1/scripts".to_string(),
            };
            match request("GET", &path, None) {
                Call::Ok(v) => {
                    let list = v
                        .as_array()
                        .map(|a| a.iter().map(describe_script).collect::<Vec<_>>().join("\n"))
                        .unwrap_or_default();
                    text_result(if list.is_empty() { "No scripts.".into() } else { list }, false)
                }
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "control_script" => {
            let Some(script) = req_str(args, "script") else { return missing("script") };
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            if !["start", "stop", "restart"].contains(&action) {
                return text_result("`action` must be start, stop or restart".into(), true);
            }
            let path = format!("/v1/scripts/{}/{action}", urlencode(&script));
            match request("POST", &path, Some(json!({}))) {
                Call::Ok(v) => text_result(
                    format!("{script}: {} ({})", action_past(action), v["state"].as_str().unwrap_or("?")),
                    false,
                ),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "run_group" => {
            let Some(group) = req_str(args, "group") else { return missing("group") };
            match request("POST", &format!("/v1/groups/{}/run", urlencode(&group)), Some(json!({}))) {
                Call::Ok(v) => text_result(
                    format!(
                        "Running group \"{}\" — {} scripts, in dependency order.",
                        v["group"].as_str().unwrap_or(&group),
                        v["scripts"]
                    ),
                    false,
                ),
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "list_groups" => {
            let path = match args.get("project").and_then(Value::as_str) {
                Some(p) => format!("/v1/groups?project={}", urlencode(p)),
                None => "/v1/groups".to_string(),
            };
            match request("GET", &path, None) {
                Call::Ok(v) => {
                    let list = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|g| {
                                    format!(
                                        "- {} ({} scripts)",
                                        g["name"].as_str().unwrap_or("?"),
                                        g["scriptIds"].as_array().map(Vec::len).unwrap_or(0)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    text_result(if list.is_empty() { "No groups.".into() } else { list }, false)
                }
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        "get_logs" => {
            let Some(target) = req_str(args, "target") else { return missing("target") };
            let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(200);
            let path = format!("/v1/logs?target={}&lines={lines}", urlencode(&target));
            match request("GET", &path, None) {
                Call::Ok(v) => {
                    let text = v["text"].as_str().unwrap_or("");
                    text_result(
                        if text.trim().is_empty() {
                            format!("No output recorded for {target} — it may not have run yet.")
                        } else {
                            format!("{target}, last {} lines:\n\n{text}", v["lines"])
                        },
                        false,
                    )
                }
                Call::Offline(why) | Call::Failed(why) => text_result(why, true),
            }
        }
        other => text_result(format!("unknown tool: {other}"), true),
    }
}

fn req_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn missing(key: &str) -> Value {
    text_result(format!("`{key}` is required and must not be empty"), true)
}

fn action_past(action: &str) -> &'static str {
    match action {
        "start" => "started",
        "stop" => "stopped",
        _ => "restarted",
    }
}

/// One script as a line a model can act on: state first, then why it matters.
fn describe_script(s: &Value) -> String {
    let state = s["state"].as_str().unwrap_or("?");
    let mut line = format!("- {} [{state}]", s["name"].as_str().unwrap_or("?"));
    if let Some(pid) = s["pid"].as_u64() {
        line += &format!(" pid {pid}");
    }
    if let Some(port) = s["port"].as_u64() {
        line += &format!(" port {port}");
    }
    if let Some(cmd) = s["cmd"].as_str() {
        line += &format!(" · {cmd}");
    }
    line
}

/// A task plus its run history, since "how did it go" is the usual question.
fn describe_task(v: &Value) -> String {
    let mut out = format!(
        "{}\n  status: {}\n  goal: {}\n  agent: {}",
        v["title"].as_str().unwrap_or("?"),
        v["status"].as_str().unwrap_or("?"),
        v["goal"].as_str().unwrap_or("?"),
        v["agentId"].as_str().unwrap_or("?"),
    );
    for (label, key) in [("effort", "effort"), ("branch", "branch"), ("base", "baseBranch"), ("pull request", "prUrl")] {
        if let Some(x) = v[key].as_str() {
            out += &format!("\n  {label}: {x}");
        }
    }
    if let Some(path) = v["workspacePath"].as_str() {
        out += &format!("\n  workspace: {path}");
    }
    let runs = v["runs"].as_array().map(Vec::as_slice).unwrap_or_default();
    if runs.is_empty() {
        out += "\n  never run";
        return out;
    }
    out += &format!("\n  {} run(s):", runs.len());
    for r in runs {
        out += &format!("\n    - {}", r["state"].as_str().unwrap_or("?"));
        if let Some(code) = r["exitCode"].as_i64() {
            out += &format!(", exit {code}");
        }
        if let Some(turns) = r["turns"].as_u64() {
            out += &format!(", {turns} turns");
        }
        if let Some(cost) = r["costUsd"].as_f64() {
            out += &format!(", ${cost:.2}");
        }
    }
    out += "\n  (use get_logs with target \"task:<id>\" for the agent's output)";
    out
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
