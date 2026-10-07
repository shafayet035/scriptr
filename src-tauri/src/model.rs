//! IPC data model. Mirrors `src/lib/types.ts` exactly (serde camelCase).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default gate timeout used by detection and TOML import.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Idle,
    Queued,
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
    Backoff,
}

/// What makes a script count as "up". Timeouts in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Gate {
    Instant,
    Port { port: u16, timeout_ms: u64 },
    Log { pattern: String, timeout_ms: u64 },
    Http { url: String, timeout_ms: u64 },
    Exit { timeout_ms: u64 },
}

impl Gate {
    pub fn timeout_ms(&self) -> Option<u64> {
        match self {
            Gate::Instant => None,
            Gate::Port { timeout_ms, .. }
            | Gate::Log { timeout_ms, .. }
            | Gate::Http { timeout_ms, .. }
            | Gate::Exit { timeout_ms } => Some(*timeout_ms),
        }
    }

    /// One-shot scripts (migrations, builds) are "ready" once they exit 0.
    pub fn is_one_shot(&self) -> bool {
        matches!(self, Gate::Exit { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartOn {
    Never,
    Crash,
    Always,
}

impl RestartOn {
    pub fn as_str(self) -> &'static str {
        match self {
            RestartOn::Never => "never",
            RestartOn::Crash => "crash",
            RestartOn::Always => "always",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartPolicy {
    pub on: RestartOn,
    pub max: u32,
    /// First backoff delay; doubles each attempt up to backoff_max_ms.
    pub backoff_ms: u64,
    pub backoff_max_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self { on: RestartOn::Never, max: 5, backoff_ms: 2_000, backoff_max_ms: 32_000 }
    }
}

impl RestartPolicy {
    /// Delay before restart `attempt` (1-based): `backoff_ms * 2^(attempt-1)`,
    /// capped at `backoff_max_ms`.
    pub fn delay_ms(&self, attempt: u32) -> u64 {
        let exp = attempt.saturating_sub(1).min(63);
        self.backoff_ms.saturating_mul(1u64 << exp).min(self.backoff_max_ms.max(self.backoff_ms))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub label: Option<String>,
    pub cmd: String,
    pub cwd: String,
    pub shell: Option<String>,
    pub env: BTreeMap<String, String>,
    pub env_file: Option<String>,
    /// Script ids this script starts after.
    pub after: Vec<String>,
    pub ready: Gate,
    pub restart: RestartPolicy,
    pub port: Option<u16>,
    pub source: Option<String>,
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub script_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub script_id: String,
    pub state: RunState,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub attempt: u32,
    pub max_attempts: u32,
    pub next_retry_at: Option<i64>,
    pub backoff_ms: Option<u64>,
    pub degraded: bool,
    pub gate_note: Option<String>,
}

impl RunInfo {
    pub fn idle(script_id: &str) -> Self {
        Self {
            script_id: script_id.to_string(),
            state: RunState::Idle,
            pid: None,
            started_at: None,
            ended_at: None,
            exit_code: None,
            attempt: 0,
            max_attempts: 0,
            next_retry_at: None,
            backoff_ms: None,
            degraded: false,
            gate_note: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnQuit {
    Stop,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportMode {
    Merge,
    Replace,
    Preview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub on_quit: OnQuit,
    pub keep_toml_in_sync: bool,
    pub import_mode: ImportMode,
    pub default_shell: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            on_quit: OnQuit::Stop,
            keep_toml_in_sync: false,
            import_mode: ImportMode::Merge,
            default_shell: default_shell(),
        }
    }
}

/// Platform default for `settings.defaultShell`.
pub fn default_shell() -> String {
    if cfg!(windows) {
        "cmd.exe /C".into()
    } else if cfg!(target_os = "macos") {
        "/bin/zsh -lc".into()
    } else {
        format!("{} -lc", std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub scripts: Vec<Script>,
    pub groups: Vec<Group>,
    pub runs: Vec<RunInfo>,
    pub settings: Settings,
    pub db_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedScript {
    pub key: String,
    pub name: String,
    pub label: Option<String>,
    pub cmd: String,
    pub cwd: String,
    pub port: Option<u16>,
    pub one_shot: bool,
    pub ready: Gate,
    pub suggested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedSource {
    pub file: String,
    pub dir: String,
    pub scripts: Vec<DetectedScript>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub path: String,
    pub name: String,
    pub branch: Option<String>,
    pub sources: Vec<DetectedSource>,
    pub has_toml: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddProjectInput {
    pub path: String,
    pub name: String,
    pub scripts: Vec<DetectedScript>,
    pub import_toml: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub group_id: String,
    pub waves: Vec<Vec<String>>,
    pub cycle: Option<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupState {
    Idle,
    Running,
    Waiting,
    Degraded,
    Done,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupProgress {
    pub group_id: String,
    pub state: GroupState,
    pub wave: usize,
    pub total_waves: usize,
    pub degraded_script_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcStats {
    pub script_id: String,
    pub pid: u32,
    pub cpu: f32,
    pub mem_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
}

// ---------------------------------------------------------------------------
// AI tasks (docs/AI-PM.md). Slice A: an agent run is a run like any other.
// Mirrors the `AgentAdapter` / `Task` / `TaskRun` block of `src/lib/types.ts`.
// String unions with dashes ("auto-edit", "in-place", "claude-json") are
// kebab-case, the rest camelCase.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Backlog,
    Queued,
    Working,
    Verifying,
    Review,
    Done,
    Failed,
    Cancelled,
}

/// How much the agent may do without asking. Maps to per-adapter flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Autonomy {
    Ask,
    AutoEdit,
    Full,
}

impl Autonomy {
    pub const ALL: [Autonomy; 3] = [Autonomy::Ask, Autonomy::AutoEdit, Autonomy::Full];

    pub fn as_str(self) -> &'static str {
        match self {
            Autonomy::Ask => "ask",
            Autonomy::AutoEdit => "auto-edit",
            Autonomy::Full => "full",
        }
    }
}

/// How hard the model should think. Mapped to per-adapter flags; `None` means
/// "whatever the agent does by default".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Extra,
    Max,
    Ultracode,
}

impl Effort {
    pub const ALL: [Effort; 6] =
        [Effort::Low, Effort::Medium, Effort::High, Effort::Extra, Effort::Max, Effort::Ultracode];

    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Extra => "extra",
            Effort::Max => "max",
            Effort::Ultracode => "ultracode",
        }
    }
}

/// Where the agent works. `Worktree` lands in slice B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceMode {
    InPlace,
    Worktree,
}

/// Machine-readable progress format, when the CLI offers one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStream {
    #[default]
    None,
    ClaudeJson,
    OpencodeJson,
    CursorJson,
}

/// Where an adapter came from; later sources win on id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSource {
    Builtin,
    User,
    Project,
}

/// A provider adapter: data, not code, so new CLIs need no Rust change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapter {
    pub id: String,
    pub name: String,
    /// Executable looked up on PATH.
    pub bin: String,
    /// argv templates; placeholders: {prompt} {model} {session}
    pub interactive_args: Vec<String>,
    pub headless_args: Vec<String>,
    pub resume_args: Vec<String>,
    pub model_args: Vec<String>,
    /// Extra argv per autonomy level; every level is present.
    pub autonomy_args: BTreeMap<Autonomy, Vec<String>>,
    /// Extra argv per effort level. Every level is present; an empty list means
    /// this agent cannot express that level, and the picker greys it out.
    pub effort_args: BTreeMap<Effort, Vec<String>>,
    /// Text appended to the prompt for a level, for CLIs whose mode is
    /// keyword-triggered rather than a flag (Claude Code's ultracode).
    pub effort_prompt: BTreeMap<Effort, String>,
    pub stream: AgentStream,
    pub models: Vec<String>,
    pub docs_url: Option<String>,
    pub source: AgentSource,
    /// Resolved at load time.
    pub available: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// The prompt handed to the agent.
    pub goal: String,
    pub agent_id: String,
    pub model: Option<String>,
    pub autonomy: Autonomy,
    /// `None` = the agent's own default.
    #[serde(default)]
    pub effort: Option<Effort>,
    pub workspace: WorkspaceMode,
    /// later (B): branch backing the worktree
    pub branch: Option<String>,
    /// later (D): task ids this task starts after
    pub after: Vec<String>,
    /// later (C): script ids that must pass for the task to count as done
    pub verify: Vec<String>,
    pub status: TaskStatus,
    /// 0 none · 1 low · 2 medium · 3 high
    pub priority: i64,
    /// "me" or "agent:<adapterId>"
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub issue_url: Option<String>,
    pub budget_tokens: Option<i64>,
    pub budget_seconds: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub sort_order: i64,
}

/// One attempt at a task. Mirrors `RunInfo`, plus what agents report about
/// themselves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    pub id: String,
    pub task_id: String,
    pub state: RunState,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    /// Agent session id, for resume and repair loops.
    pub session_id: Option<String>,
    pub turns: Option<i64>,
    pub cost_usd: Option<f64>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    /// Final assistant message / result text, when the stream format gives one.
    pub summary: Option<String>,
}

impl TaskRun {
    pub fn queued(id: String, task_id: &str) -> Self {
        Self {
            id,
            task_id: task_id.to_string(),
            state: RunState::Queued,
            pid: None,
            started_at: None,
            ended_at: None,
            exit_code: None,
            session_id: None,
            turns: None,
            cost_usd: None,
            tokens_in: None,
            tokens_out: None,
            summary: None,
        }
    }
}

/// Payload of the `task:state` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub task: Task,
    pub run: Option<TaskRun>,
}

/// Epoch milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let p = RestartPolicy { on: RestartOn::Crash, max: 10, backoff_ms: 2_000, backoff_max_ms: 32_000 };
        let delays: Vec<u64> = (1..=7).map(|a| p.delay_ms(a)).collect();
        assert_eq!(delays, vec![2_000, 4_000, 8_000, 16_000, 32_000, 32_000, 32_000]);
        // Huge attempt counts must not overflow.
        assert_eq!(p.delay_ms(u32::MAX), 32_000);
    }

    #[test]
    fn task_enum_serde_shapes() {
        // Dashed string unions in types.ts must not become camelCase.
        assert_eq!(serde_json::to_string(&Autonomy::AutoEdit).unwrap(), r#""auto-edit""#);
        assert_eq!(serde_json::to_string(&WorkspaceMode::InPlace).unwrap(), r#""in-place""#);
        assert_eq!(serde_json::to_string(&AgentStream::ClaudeJson).unwrap(), r#""claude-json""#);
        assert_eq!(serde_json::to_string(&TaskStatus::Review).unwrap(), r#""review""#);
        assert_eq!(serde_json::to_string(&AgentSource::Builtin).unwrap(), r#""builtin""#);
        // autonomyArgs is a Record<Autonomy, string[]> keyed by those strings.
        let args: BTreeMap<Autonomy, Vec<String>> =
            Autonomy::ALL.into_iter().map(|a| (a, vec![a.as_str().to_string()])).collect();
        assert_eq!(
            serde_json::to_string(&args).unwrap(),
            r#"{"ask":["ask"],"auto-edit":["auto-edit"],"full":["full"]}"#
        );
    }

    #[test]
    fn gate_serde_shape() {
        let g = Gate::Port { port: 5432, timeout_ms: 60_000 };
        assert_eq!(
            serde_json::to_string(&g).unwrap(),
            r#"{"kind":"port","port":5432,"timeoutMs":60000}"#
        );
        let i: Gate = serde_json::from_str(r#"{"kind":"instant"}"#).unwrap();
        assert_eq!(i, Gate::Instant);
    }
}
