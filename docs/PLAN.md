# Scriptr — Implementation Plan

Source of truth for design: Figma `MHi04ViIvvTJr1bZ0pKkyZ` (frames 01–08: nodes 2:2, 10:2, 2:6, 8:2, 8:239, 8:476, 15:2, 2:16).

Deliberate deviations from Figma: system UI font instead of Inter; native traffic lights; a crash waiting to retry is painted red (as the frames show) rather than queued-purple; "Exit 0" added to the Ready-when gate options; Name/Label fields added to the inspector; a Dependencies shortcut icon in the tab bar.

Browser dev: `pnpm dev` runs the UI against an in-memory mock (`src/lib/mock.ts`) seeded with the Figma sample data; `?empty` starts with no projects.

## Stack (decided)

| Layer | Pick | Notes |
|---|---|---|
| Shell | Tauri **2.11.x** | overlay title bar (native traffic lights), native menu, dialogs, single-instance, window-state, sidebar vibrancy |
| Core | Rust + tokio | supervisor task per run, scheduler task per group run |
| PTY | `portable-pty` 0.9 | one PtyPair per run; Unix: child is session leader → `killpg` on stop |
| Storage | `rusqlite` (bundled) | `~/Library/Application Support/scriptr/scriptr.db` |
| Streaming | `tauri::ipc::Channel<Vec<u8>>` | batched in Rust: flush on 16 ms or 8 KB. Tauri *events* only for low-rate state |
| Terminal | `@xterm/xterm` 6 + webgl (runtime check → DOM fallback), fit, search, serialize | one Terminal per run, hidden not destroyed |
| Frontend | **SolidJS** + Vite + TypeScript, plain CSS tokens (no Tailwind) | fine-grained reactivity for status updates |
| Supporting | `notify`, `sysinfo`, `regex`, `serde`+`toml`, `nix` | |

Design overrides: UI font = system (`-apple-system` / Segoe UI Variable) instead of Inter. JetBrains Mono (bundled via @fontsource) for anything a shell produced. Traffic lights are native, not drawn.

## Performance budget
Cold start → interactive < 400 ms · idle RAM < 150 MB with 5 PTYs · 10k lines/s without dropped frames · Stop all reaped < 500 ms.

## Repo layout
```
src/                      Solid frontend
  styles/tokens.css       Figma variables → CSS custom props
  assets/icons/*.svg      exact Figma exports, recoloured via CSS mask
  lib/ipc.ts              typed invoke/Channel wrappers + browser mock
  lib/types.ts            IPC contract (mirrors src-tauri/src/model.rs)
  store/                  Solid stores (projects, runs, ui)
  components/             TitleBar, Sidebar, Toolbar, TabBar, CommandBar, TerminalView, StatusBar, …
  screens/                Welcome, AddProjectModal, Workspace, Inspector, DependencyView, CrashBanner, CommandPalette, Settings
src-tauri/src/
  main.rs / lib.rs        builder, plugins, menu, vibrancy
  model.rs                Project/Script/Group/Gate/RestartPolicy/RunState (serde, camelCase)
  db.rs                   rusqlite schema + CRUD
  detect.rs               package.json / pyproject / Makefile / compose / Procfile / *.sh
  graph.rs                Kahn waves + cycle edge
  pty.rs                  spawn, reader thread, batcher, ring buffer
  supervisor.rs           per-run state machine, gates, restart/backoff, process-group kill
  scheduler.rs            group run by waves, reverse shutdown
  config.rs               scriptr.toml export/import
  stats.rs                sysinfo sampler
  commands.rs             #[tauri::command] surface
```

## Milestones & status
- [x] **M0** Tauri shell, window chrome, tokens, icons
- [x] **M1** real PTY + xterm both ways, process-tree stop
- [x] **M2** SQLite projects/scripts/groups; sidebar, tabs, status bar (screen 03)
- [x] **M3** folder picker + detection + import checklist (screens 01, 02)
- [x] **M4** groups, DAG, waves, gates; dependency view (screen 05); inspector (screen 04)
- [x] **M5** restart policy, crash banner (screen 06), exit history
- [x] **M6** command palette (07), terminal search, export/import + settings (08)
- [ ] **M7** packaging, signing, auto-update

Known gaps: Windows stop kills only the direct child (Job Objects + CTRL_BREAK TODO) · HTTP gate is http:// only · `onQuit: "leave"` can't survive PTY close (SIGHUP) · lazy-load xterm to trim the 520 KB main chunk.

## IPC contract
Commands (all return `Result<T, String>`):
`app_state() -> Snapshot` · `project_scan(path) -> ScanResult` · `project_add(input) -> Snapshot` · `project_remove(projectId)` ·
`script_save(script) -> Script` · `script_delete(scriptId)` · `group_save(group) -> Group` · `group_delete(groupId)` ·
`script_start|script_stop|script_restart|retry_cancel(scriptId)` · `group_plan|group_run(groupId) -> Plan` · `group_stop|group_continue(groupId)` · `stop_everything()` ·
`pty_attach(scriptId, onData: Channel)` — raw ArrayBuffer messages, first = ring-buffer snapshot · `pty_write(scriptId, data)` · `pty_resize(scriptId, cols, rows)` ·
`config_export(projectId) -> path` · `config_import(projectId, path, mode) -> ImportReport` · `settings_set(settings)` · `db_backup(dest)` · `run_history(scriptId)`

Events (low rate): `script:state` (RunInfo) · `group:progress` · `project:changed` · `stats` (1 Hz, per-PID cpu/mem)
