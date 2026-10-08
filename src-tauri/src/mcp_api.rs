//! Loopback control API (`127.0.0.1:7378/v1/*`) behind a bearer token.
//!
//! The desktop app cannot itself be a stdio MCP server — it is a GUI process
//! behind the single-instance plugin — so the MCP speaker is a separate
//! `scriptr-mcp` bridge that calls this API. The bridge discovers the port and
//! token from `mcp.json`, written next to the database at launch.
//!
//! Everything an outside agent can reach goes through here. The surface is
//! wide — scripts, logs, groups, task fields — with one line drawn through it:
//! *starting an agent run* spends tokens and edits code, so it needs
//! `settings.mcpAgentControl`, which is off by default. Filing a task always
//! lands it in the backlog, where a human sees it first.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{now_ms, Group, Project, RunInfo, Script, Task, TaskStatus};
use crate::{tasks, AppState};

/// Fixed on purpose: a roaming port would silently invalidate a registered
/// client. A collision is something to tell the user about, not paper over.
pub const DEFAULT_PORT: u16 = 7378;
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInfo {
    pub api_version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: i64,
    pub inbox: String,
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(crate::db::default_path()?.with_file_name("mcp.json"))
}

pub fn inbox_dir() -> Result<PathBuf, String> {
    Ok(crate::db::default_path()?.with_file_name("inbox"))
}

/// Writes `mcp.json` 0600 so only this user can read the token.
fn write_config(info: &McpInfo) -> Result<(), String> {
    let path = config_path()?;
    let body = serde_json::to_vec_pretty(info).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn read_config() -> Option<McpInfo> {
    let raw = std::fs::read(config_path().ok()?).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn remove_config() {
    if let Ok(p) = config_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// What the API needs from the running app, as plain callbacks — so the router
/// can be driven in tests without a Tauri window.
type Fut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
/// Reads recent output for a `script:<id>` or `task:<id>` key.
type LogReader = Arc<dyn Fn(&str, usize) -> Result<String, String> + Send + Sync>;

#[derive(Clone)]
pub struct Hooks {
    pub running_scripts: Arc<dyn Fn() -> usize + Send + Sync>,
    pub on_task: Arc<dyn Fn(&Task) + Send + Sync>,
    /// Live state for one script.
    pub run_info: Arc<dyn Fn(&str) -> RunInfo + Send + Sync>,
    pub start_script: Arc<dyn Fn(String) -> Result<(), String> + Send + Sync>,
    pub stop_script: Arc<dyn Fn(String) -> Fut<()> + Send + Sync>,
    pub restart_script: Arc<dyn Fn(String) -> Fut<Result<(), String>> + Send + Sync>,
    pub run_group: Arc<dyn Fn(String) -> Fut<Result<(), String>> + Send + Sync>,
    /// Recent output for `script:<id>` or `task:<id>`, ANSI already stripped.
    pub logs: LogReader,
    pub start_task: Arc<dyn Fn(String) -> Fut<Result<(), String>> + Send + Sync>,
    pub stop_task: Arc<dyn Fn(String) -> Fut<Result<(), String>> + Send + Sync>,
    /// Whether the user has allowed agent runs to be started from outside.
    pub agent_control: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Hooks {
    /// Everything inert, for tests that only exercise the read paths.
    pub fn none() -> Self {
        Self {
            running_scripts: Arc::new(|| 0),
            on_task: Arc::new(|_| {}),
            run_info: Arc::new(RunInfo::idle),
            start_script: Arc::new(|_| Ok(())),
            stop_script: Arc::new(|_| Box::pin(async {})),
            restart_script: Arc::new(|_| Box::pin(async { Ok(()) })),
            run_group: Arc::new(|_| Box::pin(async { Ok(()) })),
            logs: Arc::new(|_, _| Ok(String::new())),
            start_task: Arc::new(|_| Box::pin(async { Ok(()) })),
            stop_task: Arc::new(|_| Box::pin(async { Ok(()) })),
            agent_control: Arc::new(|| false),
        }
    }
}

struct Api {
    db: Arc<crate::db::Db>,
    hooks: Hooks,
    token: String,
}

pub fn router(db: Arc<crate::db::Db>, token: String, hooks: Hooks) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/projects", get(projects))
        .route("/v1/tasks", get(list_tasks).post(create_task))
        .route("/v1/tasks/{id}", get(get_task).patch(update_task))
        .route("/v1/tasks/{id}/start", post(start_task))
        .route("/v1/tasks/{id}/stop", post(stop_task))
        .route("/v1/scripts", get(list_scripts))
        .route("/v1/scripts/{id}/start", post(start_script))
        .route("/v1/scripts/{id}/stop", post(stop_script))
        .route("/v1/scripts/{id}/restart", post(restart_script))
        .route("/v1/groups", get(list_groups))
        .route("/v1/groups/{id}/run", post(run_group))
        .route("/v1/logs", get(logs))
        .route("/v1/health", get(|| async { "ok" }))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(256 * 1024))
        .with_state(Arc::new(Api { db, hooks, token }))
}

/// Starts the listener. A bind failure is reported, never fatal: the app must
/// open even when something else holds the port.
pub async fn serve(app: Arc<AppState>, port: u16) -> Result<McpInfo, String> {
    let hooks = Hooks {
        running_scripts: {
            let sup = app.sup.clone();
            Arc::new(move || sup.live_pids().len())
        },
        on_task: {
            let app = app.clone();
            Arc::new(move |t: &Task| app.notify_task(t))
        },
        run_info: {
            let sup = app.sup.clone();
            Arc::new(move |id: &str| sup.run_info(id))
        },
        start_script: {
            let sup = app.sup.clone();
            Arc::new(move |id: String| sup.start(&id))
        },
        stop_script: {
            let sup = app.sup.clone();
            Arc::new(move |id: String| {
                let sup = sup.clone();
                Box::pin(async move { sup.stop(&id).await }) as Fut<()>
            })
        },
        restart_script: {
            let sup = app.sup.clone();
            Arc::new(move |id: String| {
                let sup = sup.clone();
                Box::pin(async move { sup.restart(&id).await }) as Fut<Result<(), String>>
            })
        },
        run_group: {
            let sched = app.sched.clone();
            Arc::new(move |id: String| {
                let sched = sched.clone();
                Box::pin(async move { sched.run(&id).map(drop) }) as Fut<Result<(), String>>
            })
        },
        logs: {
            let sup = app.sup.clone();
            Arc::new(move |key: &str, lines: usize| {
                let terminals = sup.terminals();
                let run_key = match key.split_once(':') {
                    Some(("script", id)) => crate::pty::RunKey::script(id),
                    Some(("task", id)) => crate::pty::RunKey::task(id),
                    _ => return Err("unknown log target".to_string()),
                };
                Ok(tail_plain(&terminals.get(&run_key).snapshot(), lines))
            })
        },
        start_task: {
            let tasks = app.tasks.clone();
            Arc::new(move |id: String| {
                let tasks = tasks.clone();
                Box::pin(async move { tasks.start(&id).await }) as Fut<Result<(), String>>
            })
        },
        stop_task: {
            let tasks = app.tasks.clone();
            Arc::new(move |id: String| {
                let tasks = tasks.clone();
                Box::pin(async move { tasks.stop(&id).await }) as Fut<Result<(), String>>
            })
        },
        agent_control: {
            let db = app.db.clone();
            Arc::new(move || db.settings().map(|s| s.mcp_agent_control).unwrap_or(false))
        },
    };
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let inbox = inbox_dir()?;
    std::fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;

    let info = McpInfo {
        api_version: API_VERSION,
        port,
        token: token.clone(),
        pid: std::process::id(),
        started_at: now_ms(),
        inbox: inbox.to_string_lossy().into_owned(),
    };

    let router = router(app.db.clone(), token, hooks);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("port {port} is unavailable ({e}) — set another in Settings"))?;
    write_config(&info)?;

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            log::warn!("mcp api stopped: {e}");
        }
    });
    Ok(info)
}

/// A terminal ring buffer is bytes meant for a screen. A model reading logs
/// wants the text, so strip the escapes with the same stripper the readiness
/// gates use, then keep the last `lines` lines.
pub fn tail_plain(bytes: &[u8], lines: usize) -> String {
    let text = String::from_utf8_lossy(&crate::pty::AnsiStripper::default().strip(bytes)).into_owned();
    let kept: Vec<&str> = text.lines().collect();
    let from = kept.len().saturating_sub(lines);
    kept[from..].join("\n")
}

type ApiState = State<Arc<Api>>;
type ApiResult<T> = Result<Json<T>, (StatusCode, String)>;

fn authed(api: &Api, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        // Length check first; the compare is not constant-time, and the token
        // is local-user attribution rather than a secret against a network.
        .is_some_and(|t| t.len() == api.token.len() && t == api.token);
    ok.then_some(()).ok_or((StatusCode::UNAUTHORIZED, "bad or missing token".into()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    app: &'static str,
    version: &'static str,
    api_version: u32,
    projects: usize,
    running_scripts: usize,
    working_tasks: usize,
}

async fn status(State(api): ApiState, headers: HeaderMap) -> ApiResult<Status> {
    authed(&api, &headers)?;
    let db = &api.db;
    let projects = db.projects().map_err(bad)?;
    let working = projects
        .iter()
        .filter_map(|p| db.project_tasks(&p.id).ok())
        .flatten()
        .filter(|t| matches!(t.status, crate::model::TaskStatus::Working | crate::model::TaskStatus::Queued))
        .count();
    Ok(Json(Status {
        app: "scriptr",
        version: env!("CARGO_PKG_VERSION"),
        api_version: API_VERSION,
        projects: projects.len(),
        running_scripts: (api.hooks.running_scripts)(),
        working_tasks: working,
    }))
}

async fn projects(State(api): ApiState, headers: HeaderMap) -> ApiResult<Vec<Project>> {
    authed(&api, &headers)?;
    api.db.projects().map(Json).map_err(bad)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskQuery {
    project: Option<String>,
}

async fn list_tasks(State(api): ApiState, headers: HeaderMap, Query(q): Query<TaskQuery>) -> ApiResult<Vec<Task>> {
    authed(&api, &headers)?;
    let db = &api.db;
    match q.project {
        Some(name) => {
            let p = resolve_project(&api, &name)?;
            db.project_tasks(&p.id).map(Json).map_err(bad)
        }
        None => {
            let mut all = Vec::new();
            for p in db.projects().map_err(bad)? {
                all.extend(db.project_tasks(&p.id).map_err(bad)?);
            }
            Ok(Json(all))
        }
    }
}

async fn create_task(
    State(api): ApiState,
    headers: HeaderMap,
    Json(input): Json<tasks::NewTask>,
) -> ApiResult<Task> {
    authed(&api, &headers)?;
    let project = resolve_project(&api, &input.project)?;
    let task = tasks::create(&api.db, &project.id, &input, "mcp").map_err(bad)?;
    (api.hooks.on_task)(&task);
    Ok(Json(task))
}

// ---- tasks ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskDetail {
    #[serde(flatten)]
    task: Task,
    runs: Vec<crate::model::TaskRun>,
    /// Where the agent's checkout is, when the task has one.
    workspace_path: Option<String>,
}

async fn get_task(State(api): ApiState, headers: HeaderMap, Path(id): Path<String>) -> ApiResult<TaskDetail> {
    authed(&api, &headers)?;
    let task = api.db.task(&id).map_err(not_found)?;
    let runs = api.db.task_runs(&id).unwrap_or_default();
    let workspace_path = (task.workspace == crate::model::WorkspaceMode::Worktree)
        .then(|| crate::worktree::path_for(&id).ok())
        .flatten()
        .filter(|p| p.join(".git").exists())
        .map(|p| p.to_string_lossy().into_owned());
    Ok(Json(TaskDetail { task, runs, workspace_path }))
}

/// Everything an outside agent may change about a filed task. Absent fields
/// are left alone, so a caller can nudge one thing without reading first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskPatch {
    status: Option<TaskStatus>,
    title: Option<String>,
    goal: Option<String>,
    priority: Option<i64>,
    effort: Option<crate::model::Effort>,
    base: Option<String>,
    workspace: Option<crate::model::WorkspaceMode>,
    labels: Option<Vec<String>>,
    issue_url: Option<String>,
}

async fn update_task(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(patch): Json<TaskPatch>,
) -> ApiResult<Task> {
    authed(&api, &headers)?;
    let mut task = api.db.task(&id).map_err(not_found)?;

    // A running task's status belongs to the runner, not to a caller.
    if let Some(status) = patch.status {
        if matches!(task.status, TaskStatus::Working | TaskStatus::Queued | TaskStatus::Verifying) {
            return Err(bad("this task is running — stop it before setting its status"));
        }
        task.status = status;
    }
    if let Some(t) = patch.title {
        if t.trim().is_empty() {
            return Err(bad("a task needs a title"));
        }
        task.title = t.trim().to_string();
    }
    if let Some(g) = patch.goal {
        if g.trim().is_empty() {
            return Err(bad("a task needs a goal — it is the prompt the agent gets"));
        }
        task.goal = g.trim().to_string();
    }
    if let Some(p) = patch.priority {
        task.priority = p.clamp(0, 3);
    }
    if patch.effort.is_some() {
        task.effort = patch.effort;
    }
    if let Some(b) = patch.base {
        task.base_branch = Some(b).filter(|b| !b.trim().is_empty());
    }
    if let Some(w) = patch.workspace {
        task.workspace = w;
    }
    if let Some(l) = patch.labels {
        task.labels = l;
    }
    if patch.issue_url.is_some() {
        task.issue_url = patch.issue_url;
    }
    task.updated_at = now_ms();
    api.db.upsert_task(&task).map_err(bad)?;
    (api.hooks.on_task)(&task);
    Ok(Json(task))
}

/// The one gated verb: starting a run spends tokens and writes code.
///
/// `_body` is taken on every action route although none of them has a payload:
/// hyper does not drain a body no extractor asked for, and resets the
/// connection instead — which would throw away the error text the caller needs.
async fn start_task(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<Task> {
    authed(&api, &headers)?;
    if !(api.hooks.agent_control)() {
        return Err((
            StatusCode::FORBIDDEN,
            "starting agent runs from outside is off — turn on \"Let agents start runs\" in Scriptr's              Settings, or press Run on the board"
                .into(),
        ));
    }
    api.db.task(&id).map_err(not_found)?;
    (api.hooks.start_task)(id.clone()).await.map_err(bad)?;
    api.db.task(&id).map(Json).map_err(bad)
}

async fn stop_task(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<Task> {
    authed(&api, &headers)?;
    api.db.task(&id).map_err(not_found)?;
    (api.hooks.stop_task)(id.clone()).await.map_err(bad)?;
    api.db.task(&id).map(Json).map_err(bad)
}

// ---- scripts and groups --------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScriptState {
    #[serde(flatten)]
    script: Script,
    #[serde(flatten)]
    run: RunInfo,
}

async fn list_scripts(
    State(api): ApiState,
    headers: HeaderMap,
    Query(q): Query<TaskQuery>,
) -> ApiResult<Vec<ScriptState>> {
    authed(&api, &headers)?;
    let all = api.db.scripts().map_err(bad)?;
    let wanted = match q.project {
        Some(name) => Some(resolve_project(&api, &name)?.id),
        None => None,
    };
    Ok(Json(
        all.into_iter()
            .filter(|s| wanted.as_ref().is_none_or(|id| &s.project_id == id))
            .map(|s| {
                let run = (api.hooks.run_info)(&s.id);
                ScriptState { script: s, run }
            })
            .collect(),
    ))
}

/// Accepts a script id or its name, scoped by project when given.
fn resolve_script(api: &Api, needle: &str) -> Result<Script, (StatusCode, String)> {
    let scripts = api.db.scripts().map_err(bad)?;
    scripts
        .iter()
        .find(|s| s.id == needle)
        .or_else(|| scripts.iter().find(|s| s.name == needle))
        .or_else(|| scripts.iter().find(|s| s.name.eq_ignore_ascii_case(needle)))
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = scripts.iter().map(|s| s.name.as_str()).collect();
            (StatusCode::NOT_FOUND, format!("no script \"{needle}\" in Scriptr — known: {}", known.join(", ")))
        })
}

async fn start_script(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<RunInfo> {
    authed(&api, &headers)?;
    let script = resolve_script(&api, &id)?;
    (api.hooks.start_script)(script.id.clone()).map_err(bad)?;
    Ok(Json((api.hooks.run_info)(&script.id)))
}

async fn stop_script(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<RunInfo> {
    authed(&api, &headers)?;
    let script = resolve_script(&api, &id)?;
    (api.hooks.stop_script)(script.id.clone()).await;
    Ok(Json((api.hooks.run_info)(&script.id)))
}

async fn restart_script(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<RunInfo> {
    authed(&api, &headers)?;
    let script = resolve_script(&api, &id)?;
    (api.hooks.restart_script)(script.id.clone()).await.map_err(bad)?;
    Ok(Json((api.hooks.run_info)(&script.id)))
}

async fn list_groups(State(api): ApiState, headers: HeaderMap, Query(q): Query<TaskQuery>) -> ApiResult<Vec<Group>> {
    authed(&api, &headers)?;
    let all = api.db.groups().map_err(bad)?;
    let wanted = match q.project {
        Some(name) => Some(resolve_project(&api, &name)?.id),
        None => None,
    };
    Ok(Json(all.into_iter().filter(|g| wanted.as_ref().is_none_or(|id| &g.project_id == id)).collect()))
}

async fn run_group(
    State(api): ApiState,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: axum::body::Bytes,
) -> ApiResult<serde_json::Value> {
    authed(&api, &headers)?;
    let groups = api.db.groups().map_err(bad)?;
    let group = groups
        .iter()
        .find(|g| g.id == id || g.name == id || g.name.eq_ignore_ascii_case(&id))
        .ok_or_else(|| {
            let known: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            (StatusCode::NOT_FOUND, format!("no group \"{id}\" — known: {}", known.join(", ")))
        })?;
    (api.hooks.run_group)(group.id.clone()).await.map_err(bad)?;
    Ok(Json(json!({ "group": group.name, "scripts": group.script_ids.len() })))
}

// ---- logs ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogQuery {
    /// `script:<id-or-name>` or `task:<id>`
    target: String,
    lines: Option<usize>,
}

async fn logs(State(api): ApiState, headers: HeaderMap, Query(q): Query<LogQuery>) -> ApiResult<serde_json::Value> {
    authed(&api, &headers)?;
    let lines = q.lines.unwrap_or(200).clamp(1, 2000);
    // Names are friendlier than ids for a caller, so resolve them here.
    let key = match q.target.split_once(':') {
        Some(("script", needle)) => format!("script:{}", resolve_script(&api, needle)?.id),
        Some(("task", id)) => {
            api.db.task(id).map_err(not_found)?;
            format!("task:{id}")
        }
        _ => return Err(bad("target must be \"script:<name-or-id>\" or \"task:<id>\"")),
    };
    let text = (api.hooks.logs)(&key, lines).map_err(bad)?;
    Ok(Json(json!({ "target": key, "lines": text.lines().count(), "text": text })))
}

/// Accepts a project id, its name, or its path — an outside agent knows the
/// directory it is working in, not Scriptr's ids.
fn resolve_project(api: &Api, needle: &str) -> Result<Project, (StatusCode, String)> {
    let projects = api.db.projects().map_err(bad)?;
    let needle_trimmed = needle.trim_end_matches('/');
    projects
        .iter()
        .find(|p| p.id == needle || p.name == needle || p.path.trim_end_matches('/') == needle_trimmed)
        .or_else(|| projects.iter().find(|p| p.name.eq_ignore_ascii_case(needle)))
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            (StatusCode::NOT_FOUND, format!("no project \"{needle}\" in Scriptr — known: {}", known.join(", ")))
        })
}

fn bad(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}

fn not_found(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, e.to_string())
}

// ---- offline inbox --------------------------------------------------------

/// Tasks filed while the app was closed. The bridge spools them here; we drain
/// on launch. Treated as untrusted input: validated like any other task and
/// forced into the backlog.
pub fn drain_inbox(db: &crate::db::Db, on_task: &dyn Fn(&Task)) {
    let Ok(dir) = inbox_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let outcome = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|raw| serde_json::from_slice::<tasks::NewTask>(&raw).map_err(|e| e.to_string()))
            .and_then(|input| {
                let projects = db.projects().map_err(|e| e.to_string())?;
                let project = projects
                    .iter()
                    .find(|p| {
                        p.id == input.project
                            || p.name == input.project
                            || p.path.trim_end_matches('/') == input.project.trim_end_matches('/')
                    })
                    .ok_or_else(|| format!("no project \"{}\"", input.project))?;
                tasks::create(db, &project.id, &input, "mcp-offline")
            });
        match outcome {
            Ok(task) => {
                on_task(&task);
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                log::warn!("inbox {}: {e}", path.display());
                let rejected = dir.join("rejected");
                let _ = std::fs::create_dir_all(&rejected);
                let _ = std::fs::rename(&path, rejected.join(path.file_name().unwrap_or_default()));
            }
        }
    }
}

/// What Settings shows: whether the socket is up and how to register it.
pub fn info_json(info: Option<&McpInfo>, bin: Option<String>) -> serde_json::Value {
    let command = bin.unwrap_or_else(|| "scriptr-mcp".into());
    json!({
        "running": info.is_some(),
        "port": info.map(|i| i.port),
        "command": format!("claude mcp add --scope user scriptr -- {command}"),
        "configPath": config_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        "bin": command,
    })
}

#[cfg(test)]
mod tests {
    use super::tail_plain;

    #[test]
    fn logs_lose_their_escapes_and_keep_the_tail() {
        // Colour codes and a cursor move: a model wants none of it.
        let raw = b"\x1b[32mok\x1b[0m line one\r\nline two\x1b[2K\r\nline three\r\n";
        assert_eq!(tail_plain(raw, 10), "ok line one\nline two\nline three");
        assert_eq!(tail_plain(raw, 1), "line three", "only the tail is kept");
    }

    #[test]
    fn a_progress_bar_does_not_become_a_line_each() {
        // A spinner rewrites one line with bare CRs, which the stripper drops,
        // so the frames collapse instead of becoming a line apiece.
        let raw = b"10%\r50%\r100%\ndone\n";
        let out = tail_plain(raw, 10);
        assert_eq!(out.lines().count(), 2, "{out:?}");
        assert!(out.ends_with("done"), "{out:?}");
    }

    #[test]
    fn an_empty_buffer_is_empty_not_a_blank_line() {
        assert_eq!(tail_plain(b"", 100), "");
    }
}
