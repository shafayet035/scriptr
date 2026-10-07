//! Loopback control API (`127.0.0.1:7378/v1/*`) behind a bearer token.
//!
//! The desktop app cannot itself be a stdio MCP server — it is a GUI process
//! behind the single-instance plugin — so the MCP speaker is a separate
//! `scriptr-mcp` bridge that calls this API. The bridge discovers the port and
//! token from `mcp.json`, written next to the database at launch.
//!
//! Everything an outside agent can reach goes through here, and tasks it files
//! land in the backlog: nothing an agent says over this socket spends money
//! until a human presses Run.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{now_ms, Project, Task};
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
#[derive(Clone)]
pub struct Hooks {
    pub running_scripts: Arc<dyn Fn() -> usize + Send + Sync>,
    pub on_task: Arc<dyn Fn(&Task) + Send + Sync>,
}

impl Hooks {
    pub fn none() -> Self {
        Self { running_scripts: Arc::new(|| 0), on_task: Arc::new(|_| {}) }
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
