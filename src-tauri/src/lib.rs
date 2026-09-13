//! Scriptr core: runs a project's dev processes in real PTYs with dependency
//! ordering, readiness gates and restart policies.

mod commands;
pub mod config;
pub mod db;
pub mod detect;
pub mod graph;
mod menu;
pub mod model;
pub mod pty;
pub mod scheduler;
mod stats;
pub mod supervisor;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

use crate::db::Db;
use crate::model::{GroupProgress, OnQuit, RunInfo, Snapshot};
use crate::scheduler::Scheduler;
use crate::supervisor::{EventSink, Supervisor};
use crate::watch::ProjectWatcher;

/// Upper bound on how long quitting waits for processes to stop.
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Locks a mutex, ignoring poisoning (all guarded state stays consistent).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct TauriEvents(AppHandle);

impl EventSink for TauriEvents {
    fn run_changed(&self, info: &RunInfo) {
        let _ = self.0.emit("script:state", info);
    }

    fn group_progress(&self, progress: &GroupProgress) {
        let _ = self.0.emit("group:progress", progress);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectChanged {
    project_id: String,
}

/// Shared state behind every command.
pub struct AppState {
    pub db: Arc<Db>,
    pub sup: Arc<Supervisor>,
    pub sched: Arc<Scheduler>,
    watcher: Mutex<Option<ProjectWatcher>>,
}

impl AppState {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let scripts = self.db.scripts()?;
        Ok(Snapshot {
            projects: self.db.projects()?,
            runs: scripts.iter().map(|s| self.sup.run_info(&s.id)).collect(),
            scripts,
            groups: self.db.groups()?,
            settings: self.db.settings()?,
            db_path: self.db.path().to_string_lossy().into_owned(),
        })
    }

    fn export_toml(&self, project_id: &str) -> Result<PathBuf, String> {
        let project = self.db.project(project_id)?;
        if let Some(w) = lock(&self.watcher).as_ref() {
            w.suppress(&Path::new(&project.path).join(config::FILE_NAME));
        }
        config::export(&self.db, project_id)
    }

    /// Re-exports `scriptr.toml` after a mutation when keepTomlInSync is on.
    fn sync_toml(&self, project_id: &str) {
        if self.db.settings().is_ok_and(|s| s.keep_toml_in_sync) {
            if let Err(e) = self.export_toml(project_id) {
                log::warn!("keepTomlInSync: {e}");
            }
        }
    }

    fn sync_watcher(&self) {
        match (lock(&self.watcher).as_mut(), self.db.projects()) {
            (Some(w), Ok(projects)) => w.sync(&projects),
            (_, Err(e)) => log::warn!("watcher sync: {e}"),
            _ => {}
        }
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let db = Arc::new(Db::open(&db::default_path()?)?);
    let events: Arc<dyn EventSink> = Arc::new(TauriEvents(handle.clone()));
    let sup = Supervisor::new(db.clone(), events.clone());
    let sched = Scheduler::new(db.clone(), sup.clone(), events);

    let emitter = handle.clone();
    let watcher = ProjectWatcher::new(move |project_id| {
        let _ = emitter.emit("project:changed", ProjectChanged { project_id });
    })
    .map_err(|e| log::warn!("file watching disabled: {e}"))
    .ok();

    let state = Arc::new(AppState { db, sup: sup.clone(), sched, watcher: Mutex::new(watcher) });
    state.sync_watcher();
    app.manage(state);

    let emitter = handle;
    stats::spawn_sampler(sup, move |stats| {
        let _ = emitter.emit("stats", stats);
    })?;

    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("main") {
        use tauri::window::{Effect, EffectState, EffectsBuilder};
        window.set_effects(
            EffectsBuilder::new().effect(Effect::Sidebar).state(EffectState::FollowsWindowActiveState).build(),
        )?;
    }
    Ok(())
}

/// Stops every run (bounded) before the process exits, unless onQuit = leave.
fn stop_on_quit(app: &AppHandle) {
    let Some(state) = app.try_state::<Arc<AppState>>() else { return };
    if state.db.settings().is_ok_and(|s| s.on_quit == OnQuit::Leave) {
        return;
    }
    let sup = state.sup.clone();
    tauri::async_runtime::block_on(async move {
        let _ = tokio::time::timeout(QUIT_TIMEOUT, sup.stop_all()).await;
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first so a second launch focuses this one.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .menu(menu::build)
        .on_menu_event(|app, event| {
            let id = event.id().0.as_str();
            if menu::is_custom(id) {
                let _ = app.emit(menu::EVENT, id);
            }
        })
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::app_state,
            commands::project_scan,
            commands::project_add,
            commands::project_remove,
            commands::script_save,
            commands::script_delete,
            commands::group_save,
            commands::group_delete,
            commands::script_start,
            commands::script_stop,
            commands::script_restart,
            commands::retry_cancel,
            commands::group_plan,
            commands::group_run,
            commands::group_stop,
            commands::group_continue,
            commands::stop_everything,
            commands::pty_attach,
            commands::pty_write,
            commands::pty_resize,
            commands::config_export,
            commands::config_import,
            commands::settings_set,
            commands::db_backup,
            commands::run_history,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Scriptr application")
        .run(|app, event| {
            // Covers window close, ⌘Q (NSApp terminate) and app.exit().
            if let RunEvent::Exit = event {
                stop_on_quit(app);
            }
        });
}
