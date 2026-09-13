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
