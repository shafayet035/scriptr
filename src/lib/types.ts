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
