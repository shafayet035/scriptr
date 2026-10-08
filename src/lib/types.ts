// IPC contract. Mirrors src-tauri/src/model.rs (serde camelCase).
// Any change here must be made on the Rust side too.

export type RunState =
  | "idle"
  | "queued"
  | "starting"
  | "running"
  | "stopping"
  | "stopped"
  | "crashed"
  | "backoff";

/** What makes a script count as "up". Timeouts in milliseconds. */
export type Gate =
  | { kind: "instant" }
  | { kind: "port"; port: number; timeoutMs: number }
  | { kind: "log"; pattern: string; timeoutMs: number }
  | { kind: "http"; url: string; timeoutMs: number }
  | { kind: "exit"; timeoutMs: number };

export type RestartOn = "never" | "crash" | "always";

export interface RestartPolicy {
  on: RestartOn;
  max: number;
  /** First backoff delay; doubles each attempt up to backoffMaxMs. */
  backoffMs: number;
  backoffMaxMs: number;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  branch: string | null;
  sortOrder: number;
}

export interface Script {
  id: string;
  projectId: string;
  /** Identifier used in dependency lists and scriptr.toml, e.g. "api". */
  name: string;
  /** Optional qualifier shown after a dot, e.g. "runserver" → "api · runserver". */
  label: string | null;
  cmd: string;
  /** Relative to the project root, e.g. "./api". */
  cwd: string;
  /** e.g. "/bin/zsh -lc". null → settings.defaultShell. */
  shell: string | null;
  env: Record<string, string>;
  envFile: string | null;
  /** Script ids this script starts after. */
  after: string[];
  ready: Gate;
  restart: RestartPolicy;
  port: number | null;
  source: string | null;
  sortOrder: number;
}

export interface Group {
  id: string;
  projectId: string;
  name: string;
  scriptIds: string[];
}

export interface RunInfo {
  scriptId: string;
  state: RunState;
  pid: number | null;
  /** epoch ms */
  startedAt: number | null;
  endedAt: number | null;
  exitCode: number | null;
  /** 1-based restart attempt currently in progress / scheduled. 0 when not retrying. */
  attempt: number;
  maxAttempts: number;
  /** epoch ms of next automatic restart when state == backoff */
  nextRetryAt: number | null;
  backoffMs: number | null;
  /** gate timed out but process alive */
  degraded: boolean;
  /** human readable gate status, e.g. "port 5432 open", "exit code 0" */
  gateNote: string | null;
}

export interface Settings {
  onQuit: "stop" | "leave";
  keepTomlInSync: boolean;
  importMode: "merge" | "replace" | "preview";
  defaultShell: string;
}

export interface Snapshot {
  projects: Project[];
  scripts: Script[];
  groups: Group[];
  runs: RunInfo[];
  settings: Settings;
  dbPath: string;
}

export interface DetectedScript {
  /** stable key within a scan */
  key: string;
  name: string;
  label: string | null;
  cmd: string;
  cwd: string;
  port: number | null;
  oneShot: boolean;
  ready: Gate;
  /** pre-checked in the import checklist */
  suggested: boolean;
}

export interface DetectedSource {
  /** "package.json" */
  file: string;
  /** "web/" */
  dir: string;
  scripts: DetectedScript[];
}

export interface ScanResult {
  path: string;
  name: string;
  branch: string | null;
  sources: DetectedSource[];
  /** a scriptr.toml exists at the root → offer import instead of scan */
  hasToml: boolean;
}

export interface AddProjectInput {
  path: string;
  name: string;
  scripts: DetectedScript[];
  importToml: boolean;
}

export interface Plan {
  groupId: string;
  /** script ids per wave, in start order */
  waves: string[][];
  /** offending edge [from, to] (script ids) when the graph has a cycle */
  cycle: [string, string] | null;
}

export interface GroupProgress {
  groupId: string;
  state: "idle" | "running" | "waiting" | "degraded" | "done" | "failed" | "stopping" | "stopped";
  /** 0-based index of the wave currently starting */
  wave: number;
  totalWaves: number;
  degradedScriptId: string | null;
  message: string | null;
}

export interface ProcStats {
  scriptId: string;
  pid: number;
  cpu: number;
  memBytes: number;
}

// ---------------------------------------------------------------------------
// AI tasks (docs/AI-PM.md). Slice A: an agent run is a run like any other.
// Fields marked "later" are stored and exported now so the model doesn't have to
// change when worktrees (B), verification (C), the board (D) and the backlog (G) land.
// ---------------------------------------------------------------------------

export type TaskStatus =
  | "backlog"
  | "queued"
  | "working"
  | "verifying"
  | "review"
  | "done"
  | "failed"
  | "cancelled";

/** How much the agent may do without asking. Maps to per-adapter flags. */
export type Autonomy = "ask" | "auto-edit" | "full";

/**
 * How hard the model should think. Claude Code's own ladder, where "extra" is
 * its `xhigh` and "ultracode" is its multi-agent mode (a prompt keyword, not a
 * flag). Other agents map these onto their own switches, and can't express all
 * of them — see `AgentAdapter.effortArgs`.
 */
export type Effort = "low" | "medium" | "high" | "extra" | "max" | "ultracode";

export const EFFORTS: Effort[] = ["low", "medium", "high", "extra", "max", "ultracode"];

/** Where the agent works. "worktree" lands in slice B. */
export type WorkspaceMode = "in-place" | "worktree";

/** Machine-readable progress format, when the CLI offers one. */
export type AgentStream = "none" | "claude-json" | "opencode-json" | "cursor-json";

/** A provider adapter: data, not code, so new CLIs need no Rust change. */
export interface AgentAdapter {
  id: string;
  name: string;
  /** executable looked up on PATH */
  bin: string;
  /** argv templates; placeholders: {prompt} {model} {session} */
  interactiveArgs: string[];
  headlessArgs: string[];
  resumeArgs: string[];
  modelArgs: string[];
  /** extra argv per autonomy level */
  autonomyArgs: Record<Autonomy, string[]>;
  /** extra argv per effort level; empty array = this agent can't express it */
  effortArgs: Record<Effort, string[]>;
  /** levels opted into by prompt keyword instead of a flag */
  effortPrompt: Partial<Record<Effort, string>>;
  stream: AgentStream;
  /** suggested models for the picker; free text is allowed too */
  models: string[];
  docsUrl: string | null;
  source: "builtin" | "user" | "project";
  /** resolved at load time */
  available: boolean;
  version: string | null;
}

export interface Task {
  id: string;
  projectId: string;
  title: string;
  /** the prompt handed to the agent */
  goal: string;
  agentId: string;
  model: string | null;
  autonomy: Autonomy;
  /** null = the agent's own default */
  effort: Effort | null;
  workspace: WorkspaceMode;
  /** the worktree's branch, once one has been cut */
  branch: string | null;
  /** branch the work is cut from, and later targeted by its PR */
  baseBranch: string | null;
  /** later (D): task ids this task starts after — same scheduler as scripts */
  after: string[];
  /** later (C): script ids that must pass for the task to count as done */
  verify: string[];
  status: TaskStatus;
  /** PM: 0 none · 1 low · 2 medium · 3 high */
  priority: number;
  /** PM: "me" or "agent:<adapterId>" */
  assignee: string | null;
  labels: string[];
  /** PM: linked GitHub/Linear issue */
  issueUrl: string | null;
  budgetTokens: number | null;
  budgetSeconds: number | null;
  createdAt: number;
  updatedAt: number;
  sortOrder: number;
}

/** One attempt at a task. Mirrors RunInfo, plus what agents report about themselves. */
export interface TaskRun {
  id: string;
  taskId: string;
  state: RunState;
  pid: number | null;
  startedAt: number | null;
  endedAt: number | null;
  exitCode: number | null;
  /** agent session id, for resume and repair loops */
  sessionId: string | null;
  turns: number | null;
  costUsd: number | null;
  tokensIn: number | null;
  tokensOut: number | null;
  /** final assistant message / result text, when the stream format gives one */
  summary: string | null;
}

/** Status of the loopback API that the scriptr-mcp bridge talks to. */
export interface McpInfo {
  running: boolean;
  port: number | null;
  /** the literal `claude mcp add …` line */
  command: string;
  configPath: string;
  bin: string;
}

export interface RunRecord {
  startedAt: number;
  endedAt: number | null;
  exitCode: number | null;
}

export interface ImportReport {
  added: number;
  updated: number;
  removed: number;
  /** unified-ish diff text when mode == preview */
  preview: string | null;
}

/** A task's isolated checkout, from the `task_workspace` command. */
export interface WorkspaceInfo {
  path: string;
  branch: string;
  base: string;
  /** commits on the task's branch the base does not have */
  ahead: number;
  /** files changed but not committed */
  dirty: number;
}
