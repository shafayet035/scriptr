//! Task runner: an agent run is a PTY run like any other.
//!
//! ```text
//! queued → working → review   (agent exited 0)
//!                  → failed   (non-zero, or the budget ran out)
//!                  → cancelled (you stopped it)
//! ```
//!
//! Slice A keeps this deliberately thin: the agent works in the project
//! directory, Scriptr never commits or pushes, and the agent's own permission
//! prompts are hosted in the PTY rather than bypassed. Worktrees (B) and
//! verification (C) hang off the same record — see `docs/AI-PM.md`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agents;
use crate::db::Db;
use crate::model::{now_ms, AgentAdapter, RunState, Task, TaskRun, TaskStatus, WorkspaceMode};
use crate::pty::{self, DataSink, ProcessGroup, RunKey, SpawnSpec, Terminal, Terminals};
use crate::stream::StreamParser;
use crate::supervisor::{EventSink, STOP_GRACE};

/// How long to wait for trailing output after the agent exits.
const DRAIN_WAIT: Duration = Duration::from_millis(250);

fn notice(msg: &str) -> Vec<u8> {
    format!("\r\n\x1b[35m[scriptr]\x1b[0m {msg}\r\n").into_bytes()
}

struct Live {
    group: Arc<ProcessGroup>,
    /// Set when the stop came from the user, so the exit maps to `cancelled`.
    stopping: Arc<AtomicBool>,
}

pub struct TaskRunner {
    db: Arc<Db>,
    events: Arc<dyn EventSink>,
    terminals: Arc<Terminals>,
    live: Mutex<HashMap<String, Live>>,
}

impl TaskRunner {
    /// Shares the supervisor's terminal registry so script and agent output
    /// travel the same batching and ring-buffer path.
    pub fn new(db: Arc<Db>, events: Arc<dyn EventSink>, terminals: Arc<Terminals>) -> Arc<Self> {
        Arc::new(Self { db, events, terminals, live: Mutex::default() })
    }

    pub fn terminal(&self, task_id: &str) -> Arc<Terminal> {
        self.terminals.get(&RunKey::task(task_id))
    }

    /// Replaces the task's output sink; the ring buffer snapshot goes first.
    pub fn attach(&self, task_id: &str, sink: DataSink) {
        self.terminal(task_id).attach(sink);
    }

    pub fn write_input(&self, task_id: &str, data: &[u8]) -> Result<(), String> {
        match self.terminal(task_id).io() {
            Some(io) => io.write(data),
            None => Err("this task has no agent running".into()),
        }
    }

    pub fn resize(&self, task_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        self.terminal(task_id).resize(cols, rows)
    }

    pub fn is_running(&self, task_id: &str) -> bool {
        crate::lock(&self.live).contains_key(task_id)
    }

    /// `(task_id, pid)` for every live agent.
    pub fn live_pids(&self) -> Vec<(String, u32)> {
        crate::lock(&self.live).iter().map(|(id, l)| (id.clone(), l.group.pid())).collect()
    }

    fn emit(&self, task: &Task, run: Option<&TaskRun>) {
        self.events.task_changed(task, run);
    }

    /// Persists both halves of the state and emits `task:state`.
    fn save(&self, task: &mut Task, run: &TaskRun) {
        task.updated_at = now_ms();
        if let Err(e) = self.db.upsert_task(task) {
            log::warn!("task {}: {e}", task.id);
        }
        if let Err(e) = self.db.upsert_task_run(run) {
            log::warn!("task run {}: {e}", run.id);
        }
        self.emit(task, Some(run));
    }

    /// Looks up the adapter and checks it can actually run.
    async fn adapter_for(&self, task: &Task, project_path: &std::path::Path) -> Result<AgentAdapter, String> {
        let mut all = agents::load(Some(project_path));
        agents::resolve(&mut all).await;
        let adapter = agents::find(all, &task.agent_id)?;
        if !adapter.available {
            let hint = adapter.docs_url.as_deref().unwrap_or("");
            return Err(format!(
                "{} is not installed — `{}` was not found on PATH{}{hint}",
                adapter.name,
                adapter.bin,
                if hint.is_empty() { "" } else { ". See " },
            ));
        }
        Ok(adapter)
    }

    /// Starts the agent for `task_id`. Returns once the process is spawned;
    /// the run itself is supervised by a tokio task.
    pub async fn start(self: &Arc<Self>, task_id: &str) -> Result<(), String> {
        let mut task = self.db.task(task_id)?;
        if self.is_running(task_id) {
            return Err(format!("“{}” is already running", task.title));
        }
        if task.workspace != WorkspaceMode::InPlace {
            return Err("worktree isolation isn't implemented yet — set the task to run in place".into());
        }
        let project = self.db.project(&task.project_id)?;
        let cwd = PathBuf::from(&project.path);
        if !cwd.is_dir() {
            return Err(format!("{} no longer exists", project.path));
        }
        let adapter = self.adapter_for(&task, &cwd).await?;

        // Some levels are a prompt keyword rather than a flag (ultracode).
        let goal = match agents::effort_prompt(&adapter, task.effort) {
            Some(extra) => format!("{}\n\n{extra}", task.goal),
            None => task.goal.clone(),
        };
        let vars = agents::Vars { prompt: &goal, model: task.model.as_deref(), session: None };
        let args = agents::argv(&adapter, &vars, task.autonomy, task.effort, false)?;
        let program = agents::resolve_bin(&adapter.bin)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| adapter.bin.clone());

        let mut run = TaskRun::queued(uuid::Uuid::new_v4().to_string(), task_id);
        task.status = TaskStatus::Queued;
        self.save(&mut task, &run);

        let terminal = self.terminal(task_id);
        terminal.write(&notice(&format!(
            "{}{}{} · {}",
            adapter.name,
            task.model.as_deref().map(|m| format!(" · {m}")).unwrap_or_default(),
            task.effort.map(|e| format!(" · {} effort", e.as_str())).unwrap_or_default(),
            task.autonomy.as_str(),
        )));
        terminal.write(format!("\x1b[32m$\x1b[0m {} {}\r\n", adapter.bin, args.join(" ")).as_bytes());

        let parser = Arc::new(Mutex::new(StreamParser::new(adapter.stream)));
        let feed = parser.clone();
        let sink = terminal.clone();
        let spec = SpawnSpec {
            program,
            args,
            cwd,
            env: vec![
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("FORCE_COLOR".into(), "1".into()),
            ],
            size: terminal.size(),
        };

        let spawned = pty::spawn(spec, move |batch| {
            sink.write(batch);
            // Harvesting cost/session must never alter the bytes the terminal
            // renders, so the parser only ever sees a copy.
            crate::lock(&feed).feed(batch);
        });
        let mut spawned = match spawned {
            Ok(s) => s,
            Err(e) => {
                terminal.write(&notice(&format!("failed to start: {e}")));
                run.state = RunState::Crashed;
                run.ended_at = Some(now_ms());
                task.status = TaskStatus::Failed;
                self.save(&mut task, &run);
                return Err(e);
            }
        };

        let group = Arc::new(spawned.group);
        let stopping = Arc::new(AtomicBool::new(false));
        crate::lock(&self.live)
            .insert(task_id.to_string(), Live { group: group.clone(), stopping: stopping.clone() });

        terminal.set_io(Some(spawned.io.clone()));
        run.state = RunState::Running;
        run.pid = Some(group.pid());
        run.started_at = Some(now_ms());
        task.status = TaskStatus::Working;
        self.save(&mut task, &run);

        let this = self.clone();
        let id = task_id.to_string();
        let budget = task.budget_seconds.filter(|s| *s > 0).map(|s| Duration::from_secs(s as u64));
        tokio::spawn(async move {
            let mut over_budget = false;
            let code = match budget {
                Some(limit) => match tokio::time::timeout(limit, &mut spawned.exited).await {
                    Ok(code) => code.unwrap_or(None),
                    Err(_) => {
                        over_budget = true;
                        this.terminal(&id)
                            .write(&notice(&format!("budget of {}s reached — stopping the agent", limit.as_secs())));
                        this.halt(&group).await;
                        (&mut spawned.exited).await.unwrap_or(None)
                    }
                },
                None => (&mut spawned.exited).await.unwrap_or(None),
            };
            // Let the reader drain whatever the agent printed on its way out.
            let _ = tokio::time::timeout(DRAIN_WAIT, spawned.drained).await;
            let harvest = {
                let mut p = crate::lock(&parser);
                p.finish();
                p.harvest()
            };
            this.finish(&id, code, stopping.load(Ordering::SeqCst), over_budget, harvest).await;
        });
        Ok(())
    }

    /// SIGTERM to the process group, then SIGKILL after the grace period.
    async fn halt(&self, group: &ProcessGroup) {
        group.terminate();
        let deadline = tokio::time::Instant::now() + STOP_GRACE;
        while group.alive() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if group.alive() {
            group.kill();
        }
    }

    /// Stops a running agent; the run task maps the exit to `cancelled`.
    pub async fn stop(&self, task_id: &str) -> Result<(), String> {
        let live = crate::lock(&self.live).get(task_id).map(|l| {
            l.stopping.store(true, Ordering::SeqCst);
            l.group.clone()
        });
        match live {
            Some(group) => {
                self.halt(&group).await;
                Ok(())
            }
            None => Err("this task has no agent running".into()),
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = crate::lock(&self.live).keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }

    /// Records the outcome: exit 0 wants a human (`review`), anything else failed.
    async fn finish(
        &self,
        task_id: &str,
        code: Option<i32>,
        cancelled: bool,
        over_budget: bool,
        harvest: crate::stream::Harvest,
    ) {
        crate::lock(&self.live).remove(task_id);
        self.terminal(task_id).set_io(None);

        let Ok(mut task) = self.db.task(task_id) else { return };
        let mut run = match self.db.task_runs(task_id).map(|r| r.into_iter().next_back()) {
            Ok(Some(r)) => r,
            _ => return,
        };
        harvest.apply(&mut run);
        run.ended_at = Some(now_ms());
        run.exit_code = code;
        run.pid = None;
        run.state = if code == Some(0) { RunState::Stopped } else { RunState::Crashed };

        let elapsed = run
            .started_at
            .map(|s| crate::supervisor::fmt_ms((run.ended_at.unwrap_or(s) - s).max(0) as u64))
            .unwrap_or_default();
        let summary = run
            .turns
            .map(|t| format!(" · {t} turns"))
            .unwrap_or_default()
            + &run.cost_usd.map(|c| format!(" · ${c:.2}")).unwrap_or_default();

        task.status = if cancelled {
            self.terminal(task_id).write(&notice(&format!("stopped by you after {elapsed}{summary}")));
            TaskStatus::Cancelled
        } else if over_budget {
            TaskStatus::Failed
        } else if code == Some(0) {
            self.terminal(task_id)
                .write(&notice(&format!("agent exited 0 after {elapsed}{summary} · ready for review")));
            TaskStatus::Review
        } else {
            let what = code.map_or_else(|| "was killed".into(), |c| format!("exited {c}"));
            self.terminal(task_id).write(&notice(&format!("agent {what} after {elapsed}{summary}")));
            TaskStatus::Failed
        };
        self.save(&mut task, &run);
    }
}
