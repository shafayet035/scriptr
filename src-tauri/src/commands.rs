//! `#[tauri::command]` surface. Thin wrappers over db / supervisor /
//! scheduler / config. All commands are async so they run on the tokio
//! runtime (the supervisor spawns tasks).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::model::{
    AddProjectInput, Group, HistoryEntry, ImportMode, ImportReport, Plan, Project, RestartPolicy, ScanResult,
    Script, Settings, Snapshot,
};
use crate::{config, detect, graph, supervisor, AppState};

type Res<T> = Result<T, String>;
type AppStateRef<'a> = State<'a, Arc<AppState>>;

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tauri::command]
pub async fn app_state(state: AppStateRef<'_>) -> Res<Snapshot> {
    state.snapshot()
}

#[tauri::command]
pub async fn project_scan(path: String) -> Res<ScanResult> {
    tokio::task::spawn_blocking(move || detect::scan(Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn project_add(state: AppStateRef<'_>, input: AddProjectInput) -> Res<Snapshot> {
    let db = &state.db;
    let root = PathBuf::from(&input.path);
    if !root.is_dir() {
        return Err(format!("{} is not a directory", input.path));
    }
    if db.projects()?.iter().any(|p| p.path == input.path) {
        return Err("this folder has already been added".into());
    }
    let name = match input.name.trim() {
        "" => root.file_name().map_or_else(|| input.path.clone(), |n| n.to_string_lossy().into_owned()),
        n => n.to_string(),
    };
    let project = Project {
        id: new_id(),
        name,
        path: input.path.clone(),
        branch: detect::git_branch(&root),
        sort_order: db.next_project_order()?,
    };
    db.insert_project(&project)?;

    let populated = if input.import_toml {
        config::import(db, &project.id, &root.join(config::FILE_NAME), ImportMode::Merge).map(drop)
    } else {
        let mut used = HashSet::new();
        input.scripts.iter().enumerate().try_for_each(|(i, d)| {
            let name = (1..)
                .map(|n| if n == 1 { d.name.clone() } else { format!("{}-{n}", d.name) })
                .find(|c| !used.contains(c))
                .unwrap_or_else(|| d.name.clone());
            used.insert(name.clone());
            db.upsert_script(&Script {
                id: new_id(),
                project_id: project.id.clone(),
                name,
                label: d.label.clone(),
                cmd: d.cmd.clone(),
                cwd: d.cwd.clone(),
                shell: None,
                env: Default::default(),
                env_file: None,
                after: vec![],
                ready: d.ready.clone(),
                restart: RestartPolicy::default(),
                port: d.port,
                // key is "<dir><file>:<task>"
                source: d.key.split_once(':').map(|(file, _)| file.trim_start_matches("./").to_string()),
                sort_order: i as i64,
            })
        })
    };
    if let Err(e) = populated {
        let _ = db.delete_project(&project.id);
        return Err(e);
    }
    state.sync_watcher();
    state.snapshot()
}

#[tauri::command]
pub async fn project_remove(state: AppStateRef<'_>, project_id: String) -> Res<()> {
    let ids: Vec<String> = state.db.project_scripts(&project_id)?.into_iter().map(|s| s.id).collect();
    state.sup.stop_many(&ids).await;
    ids.iter().for_each(|id| state.sup.forget(id));
    state.db.delete_project(&project_id)?;
    state.sync_watcher();
    Ok(())
}

#[tauri::command]
pub async fn script_save(state: AppStateRef<'_>, script: Script) -> Res<Script> {
    let db = &state.db;
    let mut script = script;
    db.project(&script.project_id)?;
    script.name = script.name.trim().to_string();
    if script.name.is_empty() || script.cmd.trim().is_empty() {
        return Err("a script needs a name and a command".into());
    }
    let mut siblings = db.project_scripts(&script.project_id)?;
    if script.id.is_empty() {
        script.id = new_id();
        script.sort_order = siblings.iter().map(|s| s.sort_order + 1).max().unwrap_or(0);
    }
    if siblings.iter().any(|s| s.id != script.id && s.name == script.name) {
        return Err(format!("a script named \"{}\" already exists", script.name));
    }
    if let Some(bad) = script.after.iter().find(|a| **a == script.id || !siblings.iter().any(|s| &s.id == *a)) {
        return Err(format!("invalid dependency {bad}"));
    }
    siblings.retain(|s| s.id != script.id);
    siblings.push(script.clone());
    let nodes: Vec<graph::Node> = siblings.iter().map(|s| graph::Node { id: &s.id, after: &s.after }).collect();
    graph::waves(&nodes).map_err(|edge| supervisor::cycle_error(&siblings, &edge))?;

    db.upsert_script(&script)?;
    state.sync_toml(&script.project_id);
    Ok(script)
}

#[tauri::command]
pub async fn script_delete(state: AppStateRef<'_>, script_id: String) -> Res<()> {
    let script = state.db.script(&script_id)?;
    state.sup.stop(&script_id).await;
    state.sup.forget(&script_id);
    state.db.delete_script(&script_id)?;
    state.sync_toml(&script.project_id);
    Ok(())
}

#[tauri::command]
pub async fn group_save(state: AppStateRef<'_>, group: Group) -> Res<Group> {
    let mut group = group;
    group.name = group.name.trim().to_string();
    if group.name.is_empty() {
        return Err("a group needs a name".into());
    }
    let scripts = state.db.project_scripts(&group.project_id)?;
    if let Some(bad) = group.script_ids.iter().find(|id| !scripts.iter().any(|s| &s.id == *id)) {
        return Err(format!("script {bad} is not part of this project"));
    }
    if group.id.is_empty() {
        group.id = new_id();
    }
    state.db.upsert_group(&group)?;
    state.sync_toml(&group.project_id);
    Ok(group)
}

#[tauri::command]
pub async fn group_delete(state: AppStateRef<'_>, group_id: String) -> Res<()> {
    let group = state.db.group(&group_id)?;
    state.db.delete_group(&group_id)?;
    state.sync_toml(&group.project_id);
    Ok(())
}

#[tauri::command]
pub async fn script_start(state: AppStateRef<'_>, script_id: String) -> Res<()> {
    state.sup.start(&script_id)
}

#[tauri::command]
pub async fn script_stop(state: AppStateRef<'_>, script_id: String) -> Res<()> {
    state.sup.stop(&script_id).await;
    Ok(())
}

#[tauri::command]
pub async fn script_restart(state: AppStateRef<'_>, script_id: String) -> Res<()> {
    state.sup.restart(&script_id).await
}

#[tauri::command]
pub async fn retry_cancel(state: AppStateRef<'_>, script_id: String) -> Res<()> {
    state.sup.cancel_retry(&script_id);
    Ok(())
}

#[tauri::command]
pub async fn group_plan(state: AppStateRef<'_>, group_id: String) -> Res<Plan> {
    state.sched.plan(&group_id)
}

#[tauri::command]
pub async fn group_run(state: AppStateRef<'_>, group_id: String) -> Res<Plan> {
    state.sched.run(&group_id)
}

#[tauri::command]
pub async fn group_stop(state: AppStateRef<'_>, group_id: String) -> Res<()> {
    state.sched.stop(&group_id).await
}

#[tauri::command]
pub async fn group_continue(state: AppStateRef<'_>, group_id: String) -> Res<()> {
    state.sched.resume(&group_id)
}

#[tauri::command]
pub async fn stop_everything(state: AppStateRef<'_>) -> Res<()> {
    state.sup.stop_all().await;
    Ok(())
}

/// Streams raw PTY bytes (JS receives `ArrayBuffer`s). The first message is
/// the ring-buffer snapshot (possibly empty); a new attach replaces the old.
#[tauri::command]
pub async fn pty_attach(
    state: AppStateRef<'_>,
    script_id: String,
    on_data: Channel<InvokeResponseBody>,
) -> Res<()> {
    state.sup.attach(&script_id, Box::new(move |bytes| on_data.send(InvokeResponseBody::Raw(bytes)).is_ok()));
    Ok(())
}

#[tauri::command]
pub async fn pty_write(state: AppStateRef<'_>, script_id: String, data: String) -> Res<()> {
    state.sup.write_input(&script_id, data.as_bytes())
}

#[tauri::command]
pub async fn pty_resize(state: AppStateRef<'_>, script_id: String, cols: u16, rows: u16) -> Res<()> {
    state.sup.resize(&script_id, cols, rows)
}

#[tauri::command]
pub async fn config_export(state: AppStateRef<'_>, project_id: String) -> Res<String> {
    state.export_toml(&project_id).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn config_import(
    state: AppStateRef<'_>,
    project_id: String,
    path: String,
    mode: ImportMode,
) -> Res<ImportReport> {
    config::import(&state.db, &project_id, Path::new(&path), mode)
}

#[tauri::command]
pub async fn settings_set(state: AppStateRef<'_>, settings: Settings) -> Res<Settings> {
    if settings.default_shell.trim().is_empty() {
        return Err("default shell cannot be empty".into());
    }
    state.db.set_settings(&settings)?;
    state.db.settings()
}

#[tauri::command]
pub async fn db_backup(state: AppStateRef<'_>, dest: String) -> Res<()> {
    state.db.backup(&dest)
}

#[tauri::command]
pub async fn run_history(state: AppStateRef<'_>, script_id: String) -> Res<Vec<HistoryEntry>> {
    state.db.history(&script_id)
}
