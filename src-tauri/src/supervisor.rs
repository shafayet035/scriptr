//! Per-script supervisor. Each started script gets one tokio task that owns
//! its state machine:
//!
//! ```text
//! idle → queued → starting → running → stopping → stopped
//!                    ↑          │
//!                    │          └─ exit ≠ 0 → crashed → backoff ─┐
//!                    └───────────────────────────────────────────┘
//! ```
//!
//! plus dependency waiting, readiness gates and restart policies. Tasks talk
//! to the outside world through [`EventSink`] so the supervisor is testable
//! without a Tauri window.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::db::Db;
use crate::graph;
use crate::model::{now_ms, Gate, GroupProgress, RestartOn, RestartPolicy, RunInfo, RunState, Script};
use crate::pty::{self, DataSink, LogMatcher, ProcessGroup, ScriptOutput, SpawnSpec};

/// SIGTERM → SIGKILL grace period.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// A run that stays up this long resets the restart attempt counter.
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// How long to wait for trailing output after the process exits.
const DRAIN_WAIT: Duration = Duration::from_millis(250);
const GATE_POLL: Duration = Duration::from_millis(250);
const GROUP_POLL: Duration = Duration::from_millis(10);

/// Where state changes go (Tauri events in the app, a recorder in tests).
pub trait EventSink: Send + Sync + 'static {
    fn run_changed(&self, info: &RunInfo);
    fn group_progress(&self, progress: &GroupProgress);
}

/// Whether a script currently satisfies dependents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Not running and not a completed one-shot.
    Down,
    /// Queued, starting, waiting on its gate, or in backoff.
    Pending,
    /// Gate passed (or one-shot exited 0).
    Ready,
    /// Gate timed out but the process is alive.
    Degraded,
}

#[derive(Clone)]
struct Status {
    info: RunInfo,
    readiness: Readiness,
    /// A supervisor task exists for this script.
    active: bool,
}

enum Ctl {
    Stop,
    CancelRetry,
}

struct Slot {
    status: watch::Sender<Status>,
    ctl: Option<mpsc::UnboundedSender<Ctl>>,
    io: Option<Arc<pty::PtyIo>>,
    output: Arc<ScriptOutput>,
}

impl Slot {
    fn new(id: &str) -> Self {
        let (status, _) = watch::channel(Status {
            info: RunInfo::idle(id),
            readiness: Readiness::Down,
            active: false,
        });
        Self { status, ctl: None, io: None, output: Arc::default() }
    }
}

/// Everything needed to spawn one run, resolved fresh from the database so
/// edits apply on the next (re)start.
struct Launch {
    spec: SpawnSpec,
    cmd: String,
    ready: Gate,
    policy: RestartPolicy,
}

struct Exited {
    code: Option<i32>,
    gate_timeout: bool,
    uptime: Duration,
    ended_at: i64,
}

enum RunEnd {
    StoppedByUser,
    Exited(Exited),
}

enum DepsOutcome {
    Ready,
    StoppedByUser,
    Failed(String),
}

enum GateOutcome {
    Passed(String),
    TimedOut(String),
}

pub struct Supervisor {
    db: Arc<Db>,
    events: Arc<dyn EventSink>,
    slots: Mutex<HashMap<String, Slot>>,
}

impl Supervisor {
    pub fn new(db: Arc<Db>, events: Arc<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self { db, events, slots: Mutex::default() })
    }

    fn slots(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        crate::lock(&self.slots)
    }

    fn with_slot<T>(&self, id: &str, f: impl FnOnce(&mut Slot) -> T) -> T {
        let mut slots = self.slots();
        f(slots.entry(id.to_string()).or_insert_with(|| Slot::new(id)))
    }

    /// Mutates a script's status and emits `RunInfo` if it changed. Emitting
    /// under the lock keeps events ordered.
    fn update(&self, id: &str, f: impl FnOnce(&mut Status)) {
        self.with_slot(id, |slot| {
            let mut changed = None;
            slot.status.send_modify(|s| {
                let before = s.info.clone();
                f(s);
                if s.info != before {
                    changed = Some(s.info.clone());
                }
            });
            if let Some(info) = changed {
                self.events.run_changed(&info);
            }
        });
    }

    // ---- queries ----------------------------------------------------------

    pub fn output(&self, id: &str) -> Arc<ScriptOutput> {
        self.with_slot(id, |s| s.output.clone())
    }

    pub fn run_info(&self, id: &str) -> RunInfo {
        self.slots().get(id).map_or_else(|| RunInfo::idle(id), |s| s.status.borrow().info.clone())
    }

    pub fn readiness(&self, id: &str) -> Readiness {
        self.slots().get(id).map_or(Readiness::Down, |s| s.status.borrow().readiness)
    }

    /// `(script_id, pid)` for every live process.
    pub fn live_pids(&self) -> Vec<(String, u32)> {
        self.slots()
            .iter()
            .filter_map(|(id, s)| s.status.borrow().info.pid.map(|pid| (id.clone(), pid)))
            .collect()
    }

    /// Resolves once the script's readiness is no longer `Pending`.
    pub fn wait_settled(&self, id: &str) -> impl Future<Output = Readiness> + Send + 'static {
        let mut rx = self.with_slot(id, |s| s.status.subscribe());
        async move {
            rx.wait_for(|s| s.readiness != Readiness::Pending)
                .await
                .map(|s| s.readiness)
                .unwrap_or(Readiness::Down)
        }
    }

    // ---- terminal I/O -----------------------------------------------------

    /// Replaces the script's output sink; the ring buffer snapshot goes first.
    pub fn attach(&self, id: &str, sink: DataSink) {
        self.output(id).attach(sink);
    }

    pub fn write_input(&self, id: &str, data: &[u8]) -> Result<(), String> {
        match self.with_slot(id, |s| s.io.clone()) {
            Some(io) => io.write(data),
            None => Err("script is not running".into()),
        }
    }

    /// Remembers the size for future spawns and applies it to a live PTY.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let (output, io) = self.with_slot(id, |s| (s.output.clone(), s.io.clone()));
        output.set_size(cols, rows);
        io.map_or(Ok(()), |io| io.resize(cols.max(1), rows.max(1)))
    }

    // ---- control ----------------------------------------------------------

    /// Starts a script (no-op when it already has a supervisor task). Missing
    /// dependencies are started and awaited by the task.
    pub fn start(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let script = self.db.script(id)?;
        self.check_cycles(&script)?;
        let rx = self.with_slot(id, |slot| {
            if slot.ctl.is_some() {
                return None;
            }
            let (tx, rx) = mpsc::unbounded_channel();
            slot.ctl = Some(tx);
            let state = if script.after.is_empty() { RunState::Starting } else { RunState::Queued };
            slot.status.send_modify(|s| {
                s.active = true;
                s.readiness = Readiness::Pending;
                s.info = RunInfo { state, max_attempts: script.restart.max, ..RunInfo::idle(id) };
                self.events.run_changed(&s.info);
            });
            Some(rx)
        });
        if let Some(rx) = rx {
            let this = Arc::clone(self);
            let id = id.to_string();
            tokio::spawn(async move { this.supervise(id, rx).await });
        }
        Ok(())
    }

    /// Stops a script's task (process, queue wait or backoff) and waits for it.
    pub async fn stop(&self, id: &str) {
        let target = self.slots().get(id).and_then(|s| Some((s.ctl.clone()?, s.status.subscribe())));
        if let Some((ctl, mut rx)) = target {
            let _ = ctl.send(Ctl::Stop);
            let _ = rx.wait_for(|s| !s.active).await;
        }
    }

    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.stop(id).await;
        self.start(id)
    }

    /// Abandons a pending automatic restart (→ stopped).
    pub fn cancel_retry(&self, id: &str) {
        if let Some(ctl) = self.slots().get(id).and_then(|s| s.ctl.clone()) {
            let _ = ctl.send(Ctl::CancelRetry);
        }
    }

    /// Stops the given scripts in parallel.
    pub async fn stop_many(self: &Arc<Self>, ids: &[String]) {
        let mut set = JoinSet::new();
        for id in ids {
            let this = Arc::clone(self);
            let id = id.clone();
            set.spawn(async move { this.stop(&id).await });
        }
        while set.join_next().await.is_some() {}
    }

    pub async fn stop_all(self: &Arc<Self>) {
        let ids: Vec<String> =
            self.slots().iter().filter(|(_, s)| s.ctl.is_some()).map(|(id, _)| id.clone()).collect();
        self.stop_many(&ids).await;
    }

    /// Drops all state for a deleted script. Stop it first.
    pub fn forget(&self, id: &str) {
        self.slots().remove(id);
    }

    fn check_cycles(&self, script: &Script) -> Result<(), String> {
        let all = self.db.project_scripts(&script.project_id)?;
        let by_id: HashMap<&str, &Script> = all.iter().map(|s| (s.id.as_str(), s)).collect();
        // Transitive dependency closure of `script`.
        let mut closure = vec![script.id.as_str()];
        let mut i = 0;
        while let Some(id) = closure.get(i).copied() {
            for dep in by_id.get(id).map(|s| s.after.as_slice()).unwrap_or_default() {
                if by_id.contains_key(dep.as_str()) && !closure.contains(&dep.as_str()) {
                    closure.push(dep);
                }
            }
            i += 1;
        }
        let nodes: Vec<graph::Node> = closure
            .iter()
            .filter_map(|id| by_id.get(id))
            .map(|s| graph::Node { id: &s.id, after: &s.after })
            .collect();
        graph::waves(&nodes).map(drop).map_err(|edge| cycle_error(&all, &edge))
    }

    // ---- the task ---------------------------------------------------------

    async fn supervise(self: Arc<Self>, id: String, mut ctl: mpsc::UnboundedReceiver<Ctl>) {
        self.lifecycle(&id, &mut ctl).await;
        self.with_slot(&id, |slot| {
            slot.ctl = None;
            slot.io = None;
            slot.status.send_modify(|s| {
                s.active = false;
                if s.readiness == Readiness::Pending {
                    s.readiness = Readiness::Down;
                }
            });
        });
    }

    async fn lifecycle(self: &Arc<Self>, id: &str, ctl: &mut mpsc::UnboundedReceiver<Ctl>) {
        let output = self.output(id);
        match self.await_dependencies(id, ctl).await {
            DepsOutcome::Ready => {}
            DepsOutcome::StoppedByUser => {
                self.update(id, |s| {
                    s.info.state = RunState::Stopped;
                    s.info.gate_note = None;
                    s.readiness = Readiness::Down;
                });
                return;
            }
            DepsOutcome::Failed(msg) => {
                output.write(&notice(&msg));
                self.update(id, |s| {
                    s.info.state = RunState::Stopped;
                    s.info.gate_note = Some(msg);
                    s.readiness = Readiness::Down;
                });
                return;
            }
        }

        let mut attempt = 0;
        loop {
            let launch = match self.prepare(id) {
                Ok(launch) => launch,
                Err(e) => {
                    output.write(&notice(&format!("cannot start: {e}")));
                    self.update(id, |s| {
                        s.info.state = RunState::Crashed;
                        s.info.gate_note = Some(e);
                        s.readiness = Readiness::Down;
                    });
                    return;
                }
            };
            let policy = launch.policy.clone();
            let one_shot = launch.ready.is_one_shot();
            let exited = match self.run_once(id, launch, attempt, ctl).await {
                RunEnd::StoppedByUser => return,
                RunEnd::Exited(e) => e,
            };
            let exit_fields = |s: &mut Status| {
                s.info.pid = None;
                s.info.exit_code = exited.code;
                s.info.ended_at = Some(exited.ended_at);
                s.info.next_retry_at = None;
                s.info.backoff_ms = None;
            };

            if one_shot && exited.code == Some(0) && !exited.gate_timeout {
                self.update(id, |s| {
                    exit_fields(s);
                    s.info.state = RunState::Stopped;
                    s.info.attempt = 0;
                    s.info.gate_note = Some("exit code 0".into());
                    s.readiness = Readiness::Ready;
                });
                return;
            }

            let crashed = exited.gate_timeout || exited.code != Some(0);
            let retry = match policy.on {
                RestartOn::Never => false,
                RestartOn::Crash => crashed,
                RestartOn::Always => true,
            };
            if exited.uptime >= STABLE_AFTER {
                attempt = 0;
            }
            if !retry || attempt >= policy.max {
                if retry {
                    output.write(&dim_notice(&format!(
                        "restart policy: giving up after {} attempts",
                        policy.max
                    )));
                }
                // Retries exhausted → stopped; no policy → crashed stays visible.
                let state = if crashed && !retry { RunState::Crashed } else { RunState::Stopped };
                self.update(id, |s| {
                    exit_fields(s);
                    s.info.state = state;
                    s.readiness = Readiness::Down;
                });
                return;
            }

            if crashed {
                self.update(id, |s| {
                    exit_fields(s);
                    s.info.state = RunState::Crashed;
                    s.readiness = Readiness::Pending;
                });
            }
            attempt += 1;
            let delay = policy.delay_ms(attempt);
            output.write(&dim_notice(&format!(
                "restart policy: {} · attempt {attempt}/{} · waiting {}…",
                policy_label(policy.on),
                policy.max,
                fmt_ms(delay)
            )));
            self.update(id, |s| {
                exit_fields(s);
                s.info.state = RunState::Backoff;
                s.info.attempt = attempt;
                s.info.max_attempts = policy.max;
                s.info.next_retry_at = Some(now_ms() + delay as i64);
                s.info.backoff_ms = Some(delay);
                s.readiness = Readiness::Pending;
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                msg = ctl.recv() => {
                    let what = if matches!(msg, Some(Ctl::CancelRetry)) { "retry cancelled" } else { "stopped" };
                    output.write(&notice(what));
                    self.update(id, |s| {
                        s.info.state = RunState::Stopped;
                        s.info.next_retry_at = None;
                        s.info.backoff_ms = None;
                        s.readiness = Readiness::Down;
                    });
                    return;
                }
            }
        }
    }

    /// Starts dependencies that aren't up and waits for all of them.
    async fn await_dependencies(
        self: &Arc<Self>,
        id: &str,
        ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    ) -> DepsOutcome {
        let script = match self.db.script(id) {
            Ok(s) => s,
            Err(e) => return DepsOutcome::Failed(e),
        };
        let mut waits = JoinSet::new();
        let mut names = Vec::new();
        for dep in &script.after {
            // Dangling ids (deleted scripts) are ignored.
            let Ok(dep_script) = self.db.script(dep) else { continue };
            if matches!(self.readiness(dep), Readiness::Ready | Readiness::Degraded) {
                continue;
            }
            if let Err(e) = self.start(dep) {
                return DepsOutcome::Failed(e);
            }
            let settled = self.wait_settled(dep);
            names.push(dep_script.name.clone());
            waits.spawn(async move { (dep_script.name, settled.await) });
        }
        if waits.is_empty() {
            return DepsOutcome::Ready;
        }
        self.update(id, |s| {
            s.info.state = RunState::Queued;
            s.info.gate_note = Some(format!("waiting for {}", names.join(", ")));
        });
        loop {
            tokio::select! {
                joined = waits.join_next() => match joined {
                    None => return DepsOutcome::Ready,
                    Some(Ok((_, Readiness::Ready | Readiness::Degraded))) => {}
                    Some(Ok((name, _))) => {
                        return DepsOutcome::Failed(format!("dependency \"{name}\" stopped before it was ready"));
                    }
                    Some(Err(e)) => return DepsOutcome::Failed(e.to_string()),
                },
                msg = ctl.recv() => {
                    if !matches!(msg, Some(Ctl::CancelRetry)) {
                        return DepsOutcome::StoppedByUser;
                    }
                }
            }
        }
    }

    fn prepare(&self, id: &str) -> Result<Launch, String> {
        let script = self.db.script(id)?;
        let project = self.db.project(&script.project_id)?;
        let settings = self.db.settings()?;
        let shell = script
            .shell
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&settings.default_shell);
        let (program, args) = pty::shell_command(shell, &script.cmd);
        let cwd = Path::new(&project.path).join(&script.cwd);
        if !cwd.is_dir() {
            return Err(format!("working directory {} does not exist", cwd.display()));
        }

        let output = self.output(id);
        let mut env = Vec::new();
        // env_file is resolved relative to the script's working directory.
        if let Some(file) = script.env_file.as_deref().filter(|f| !f.is_empty()) {
            let path = cwd.join(file);
            match std::fs::read_to_string(&path) {
                Ok(text) => env.extend(pty::parse_env_file(&text)),
                Err(e) => output.write(&notice(&format!("env file {}: {e}", path.display()))),
            }
        }
        env.extend(script.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        for (k, v) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("FORCE_COLOR", "1")] {
            env.push((k.into(), v.into()));
        }
        Ok(Launch {
            spec: SpawnSpec { program, args, cwd, env, size: output.size() },
            cmd: script.cmd,
            ready: script.ready,
            policy: script.restart,
        })
    }

    /// Spawns one run and drives it until exit or a user stop.
    async fn run_once(
        self: &Arc<Self>,
        id: &str,
        launch: Launch,
        attempt: u32,
        ctl: &mut mpsc::UnboundedReceiver<Ctl>,
    ) -> RunEnd {
        let Launch { spec, cmd, ready, policy } = launch;
        let output = self.output(id);
        self.update(id, |s| {
            s.info = RunInfo {
                state: RunState::Starting,
                attempt,
                max_attempts: policy.max,
                ..RunInfo::idle(id)
            };
            s.readiness = Readiness::Pending;
        });
        output.write(format!("\x1b[32m$\x1b[0m {cmd}\r\n").as_bytes());

        let (mut matcher, log_rx) = match &ready {
            Gate::Log { pattern, .. } => {
                let (m, rx) = LogMatcher::new(pattern);
                (Some(m), Some(rx))
            }
            _ => (None, None),
        };
        let sink = output.clone();
        let spawned = pty::spawn(spec, move |batch| {
            sink.write(batch);
            // The gate sees every batch whether or not a terminal is attached.
            if let Some(m) = matcher.as_mut() {
                m.feed(batch);
            }
        });
        let started = Instant::now();
        let mut spawned = match spawned {
            Ok(s) => s,
            Err(e) => {
                output.write(&notice(&format!("failed to start: {e}")));
                return RunEnd::Exited(Exited {
                    code: None,
                    gate_timeout: false,
                    uptime: Duration::ZERO,
                    ended_at: now_ms(),
                });
            }
        };

        let started_at = now_ms();
        let history = self
            .db
            .history_start(id, started_at)
            .map_err(|e| log::warn!("run history: {e}"))
            .ok();
        let pid = spawned.group.pid();
        let instant = matches!(ready, Gate::Instant);
        self.with_slot(id, |s| s.io = Some(spawned.io.clone()));
        self.update(id, |s| {
            s.info.state = RunState::Running;
            s.info.pid = Some(pid);
            s.info.started_at = Some(started_at);
            s.info.gate_note = waiting_note(&ready);
            s.readiness = if instant { Readiness::Ready } else { Readiness::Pending };
        });

        let gate = evaluate_gate(ready.clone(), log_rx);
        tokio::pin!(gate);
        let mut gate_done = instant;
        let mut gate_timeout = false;
        let code = loop {
            tokio::select! {
                code = &mut spawned.exited => break code.unwrap_or(None),
                outcome = &mut gate, if !gate_done => {
                    gate_done = true;
                    match outcome {
                        GateOutcome::Passed(note) => self.update(id, |s| {
                            s.info.gate_note = Some(note);
                            s.info.degraded = false;
                            s.readiness = Readiness::Ready;
                        }),
                        // A one-shot that overruns its timeout is killed and counts as a crash.
                        GateOutcome::TimedOut(note) if ready.is_one_shot() => {
                            output.write(&notice(&note));
                            gate_timeout = true;
                            self.update(id, |s| {
                                s.info.state = RunState::Stopping;
                                s.info.gate_note = Some(note);
                            });
                            break terminate(&spawned.group, &mut spawned.exited).await;
                        }
                        GateOutcome::TimedOut(note) => {
                            output.write(&notice(&note));
                            self.update(id, |s| {
                                s.info.gate_note = Some(note);
                                s.info.degraded = true;
                                s.readiness = Readiness::Degraded;
                            });
                        }
                    }
                }
                msg = ctl.recv() => {
                    if matches!(msg, Some(Ctl::CancelRetry)) {
                        continue;
                    }
                    self.update(id, |s| s.info.state = RunState::Stopping);
                    let code = terminate(&spawned.group, &mut spawned.exited).await;
                    let ended_at = now_ms();
                    self.finish_run(id, history, ended_at, code, spawned.drained).await;
                    output.write(&notice(&format!("stopped after {}", fmt_duration(started.elapsed()))));
                    self.update(id, |s| {
                        s.info.state = RunState::Stopped;
                        s.info.pid = None;
                        s.info.exit_code = code;
                        s.info.ended_at = Some(ended_at);
                        s.info.degraded = false;
                        s.readiness = Readiness::Down;
                    });
                    return RunEnd::StoppedByUser;
                }
            }
        };

        // The leader is gone; don't leave its process group behind.
        reap_stragglers(&spawned.group).await;
        let ended_at = now_ms();
        self.finish_run(id, history, ended_at, code, spawned.drained).await;
        let uptime = started.elapsed();
        let how = match code {
            Some(c) => format!("process exited with code {c}"),
            None => "process was terminated by a signal".into(),
        };
        output.write(&notice(&format!("{how} after {}", fmt_duration(uptime))));
        RunEnd::Exited(Exited { code, gate_timeout, uptime, ended_at })
    }

    async fn finish_run(
        &self,
        id: &str,
        history: Option<i64>,
        ended_at: i64,
        code: Option<i32>,
        drained: oneshot::Receiver<()>,
    ) {
        let _ = tokio::time::timeout(DRAIN_WAIT, drained).await;
        self.with_slot(id, |s| s.io = None);
        if let Some(row) = history {
            if let Err(e) = self.db.history_end(row, ended_at, code) {
                log::warn!("run history: {e}");
            }
        }
    }
}

// ---- process termination ------------------------------------------------------

/// SIGTERM the group, wait up to the grace period for the leader and every
/// other member, then SIGKILL whatever is left.
async fn terminate(group: &ProcessGroup, exited: &mut oneshot::Receiver<Option<i32>>) -> Option<i32> {
    let deadline = tokio::time::Instant::now() + STOP_GRACE;
    group.terminate();
    let code = match tokio::time::timeout_at(deadline, &mut *exited).await {
        Ok(code) => code.unwrap_or(None),
        Err(_) => {
            group.kill();
            exited.await.unwrap_or(None)
        }
    };
    wait_group_gone(group, deadline).await;
    code
}

async fn reap_stragglers(group: &ProcessGroup) {
    if group.alive() {
        group.terminate();
        wait_group_gone(group, tokio::time::Instant::now() + STOP_GRACE).await;
    }
}

async fn wait_group_gone(group: &ProcessGroup, deadline: tokio::time::Instant) {
    while group.alive() {
        if tokio::time::Instant::now() >= deadline {
            group.kill();
            return;
        }
        tokio::time::sleep(GROUP_POLL).await;
    }
}

// ---- gates ----------------------------------------------------------------------

fn waiting_note(gate: &Gate) -> Option<String> {
    Some(match gate {
        Gate::Instant => return None,
        Gate::Port { port, .. } => format!("waiting for port {port}"),
        Gate::Log { .. } => "waiting for log match".into(),
        Gate::Http { url, .. } => format!("waiting for {url}"),
        Gate::Exit { .. } => "waiting for exit".into(),
    })
}

async fn evaluate_gate(gate: Gate, log: Option<watch::Receiver<bool>>) -> GateOutcome {
    let limit = gate.timeout_ms().filter(|t| *t > 0).map(Duration::from_millis);
    let probe = async {
        match &gate {
            Gate::Instant => "ready".to_string(),
            Gate::Port { port, .. } => {
                while !port_open(*port).await {
                    tokio::time::sleep(GATE_POLL).await;
                }
                format!("port {port} open")
            }
            Gate::Log { .. } => {
                if let Some(mut rx) = log {
                    if rx.wait_for(|matched| *matched).await.is_ok() {
                        return "log match".to_string();
                    }
                }
                // Output ended without a match: only the timeout can resolve this.
                std::future::pending().await
            }
            Gate::Http { url, .. } => loop {
                if let Ok(code) = http_status(url).await {
                    if (200..300).contains(&code) {
                        break format!("HTTP {code}");
                    }
                }
                tokio::time::sleep(GATE_POLL).await;
            },
            Gate::Exit { .. } => std::future::pending().await,
        }
    };
    let Some(limit) = limit else { return GateOutcome::Passed(probe.await) };
    match tokio::time::timeout(limit, probe).await {
        Ok(note) => GateOutcome::Passed(note),
        Err(_) => {
            let after = fmt_ms(limit.as_millis() as u64);
            GateOutcome::TimedOut(match &gate {
                Gate::Port { port, .. } => format!("port {port} not open after {after}"),
                Gate::Log { .. } => format!("no log match after {after}"),
                Gate::Http { url, .. } => format!("{url} not healthy after {after}"),
                Gate::Exit { .. } | Gate::Instant => format!("did not exit within {after}"),
            })
        }
    }
}

/// Dev servers on macOS often bind `localhost` → ::1 only, so try both.
async fn port_open(port: u16) -> bool {
    for host in ["127.0.0.1", "::1"] {
        let connect = TcpStream::connect((host, port));
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(200), connect).await {
            return true;
        }
    }
    false
}

/// Minimal HTTP/1.1 GET returning the status code.
async fn http_status(url: &str) -> Result<u16, String> {
    let rest = url.strip_prefix("http://").ok_or("only http:// URLs are supported")?;
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let has_port = authority.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok());
    let addr = if has_port { authority.to_string() } else { format!("{authority}:80") };
    let request = async {
        let mut stream = TcpStream::connect(&addr).await.map_err(|e| e.to_string())?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: scriptr\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut buf = [0u8; 512];
        while !head.windows(2).any(|w| w == b"\r\n") && head.len() < 4096 {
            let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        parse_status_line(&head)
    };
    tokio::time::timeout(Duration::from_secs(2), request).await.map_err(|_| "timed out".to_string())?
}

fn parse_status_line(head: &[u8]) -> Result<u16, String> {
    let line = String::from_utf8_lossy(head);
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(version), Some(code)) if version.starts_with("HTTP/") => {
            code.parse().map_err(|_| format!("bad status code {code}"))
        }
        _ => Err("not an HTTP response".into()),
    }
}

// ---- formatting -------------------------------------------------------------------

/// `\r\n[scriptr] msg\r\n` in magenta.
fn notice(msg: &str) -> Vec<u8> {
    format!("\r\n\x1b[35m[scriptr]\x1b[0m {msg}\r\n").into_bytes()
}

/// `[scriptr] msg` with a dim message.
fn dim_notice(msg: &str) -> Vec<u8> {
    format!("\x1b[35m[scriptr]\x1b[0m\x1b[2m {msg}\x1b[0m\r\n").into_bytes()
}

fn policy_label(on: RestartOn) -> &'static str {
    match on {
        RestartOn::Never => "never",
        RestartOn::Crash => "on-crash",
        RestartOn::Always => "always",
    }
}

/// `850ms`, `3s`, `2.5s`.
pub fn fmt_ms(ms: u64) -> String {
    match ms {
        0..=999 => format!("{ms}ms"),
        _ if ms.is_multiple_of(1000) => format!("{}s", ms / 1000),
        _ => format!("{:.1}s", ms as f64 / 1000.0),
    }
}

/// `850ms`, `12s`, `4m 12s`, `1h 3m`.
pub fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => format!("{}ms", d.as_millis()),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

/// Human-readable dependency cycle error naming the offending edge.
pub fn cycle_error(scripts: &[Script], (from, to): &(String, String)) -> String {
    let name = |id: &str| {
        scripts.iter().find(|s| s.id == id).map_or_else(|| id.to_string(), |s| s.name.clone())
    };
    format!("dependency cycle: \"{}\" runs after \"{}\", which depends on it", name(to), name(from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_ms(50), "50ms");
        assert_eq!(fmt_ms(3_000), "3s");
        assert_eq!(fmt_ms(2_500), "2.5s");
        assert_eq!(fmt_duration(Duration::from_secs(252)), "4m 12s");
        assert_eq!(fmt_duration(Duration::from_millis(420)), "420ms");
        assert_eq!(fmt_duration(Duration::from_secs(3_780)), "1h 3m");
    }

    #[test]
    fn status_line() {
        assert_eq!(parse_status_line(b"HTTP/1.1 204 No Content\r\n"), Ok(204));
        assert!(parse_status_line(b"SSH-2.0\r\n").is_err());
    }

    #[tokio::test]
    async fn http_and_port_probes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
            }
        });
        assert!(port_open(port).await);
        assert_eq!(http_status(&format!("http://127.0.0.1:{port}/health")).await, Ok(200));
        let gate = Gate::Http { url: format!("http://127.0.0.1:{port}/"), timeout_ms: 2_000 };
        assert!(matches!(evaluate_gate(gate, None).await, GateOutcome::Passed(n) if n == "HTTP 200"));
    }
}
