import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AddProjectInput,
  AgentAdapter,
  GroupProgress,
  Group,
  ImportReport,
  McpInfo,
  Plan,
  ProcStats,
  RunInfo,
  RunRecord,
  ScanResult,
  Script,
  WorkspaceInfo,
  Settings,
  Snapshot,
  Task,
  TaskRun,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const isMac = /Mac/.test(navigator.platform);

export interface EventMap {
  "script:state": RunInfo;
  "group:progress": GroupProgress;
  "project:changed": { projectId: string };
  "task:state": { task: Task; run: TaskRun | null };
  stats: ProcStats[];
  menu: string;
}

export interface Backend {
  appState(): Promise<Snapshot>;
  projectScan(path: string): Promise<ScanResult>;
  projectAdd(input: AddProjectInput): Promise<Snapshot>;
  projectRemove(projectId: string): Promise<void>;
  scriptSave(script: Script): Promise<Script>;
  scriptDelete(scriptId: string): Promise<void>;
  groupSave(group: Group): Promise<Group>;
  groupDelete(groupId: string): Promise<void>;
  scriptStart(scriptId: string): Promise<void>;
  scriptStop(scriptId: string): Promise<void>;
  scriptRestart(scriptId: string): Promise<void>;
  retryCancel(scriptId: string): Promise<void>;
  groupPlan(groupId: string): Promise<Plan>;
  groupRun(groupId: string): Promise<Plan>;
  groupStop(groupId: string): Promise<void>;
  groupContinue(groupId: string): Promise<void>;
  stopEverything(): Promise<void>;
  /** First chunk is the scrollback snapshot, then live output. Re-attaching replaces the previous sink. */
  ptyAttach(scriptId: string, onData: (bytes: Uint8Array) => void): Promise<void>;
  ptyWrite(scriptId: string, data: string): Promise<void>;
  ptyResize(scriptId: string, cols: number, rows: number): Promise<void>;
  configExport(projectId: string): Promise<string>;
  configImport(projectId: string, path: string, mode: Settings["importMode"]): Promise<ImportReport>;
  settingsSet(settings: Settings): Promise<Settings>;
  dbBackup(dest: string): Promise<void>;
  runHistory(scriptId: string): Promise<RunRecord[]>;
  /** loopback MCP API status + the command that registers the bridge */
  mcpInfo(): Promise<McpInfo>;
  // --- AI tasks (docs/AI-PM.md) ---
  agentList(): Promise<AgentAdapter[]>;
  taskList(projectId: string): Promise<Task[]>;
  taskSave(task: Task): Promise<Task>;
  taskDelete(taskId: string): Promise<void>;
  taskStart(taskId: string): Promise<void>;
  taskStop(taskId: string): Promise<void>;
  taskRuns(taskId: string): Promise<TaskRun[]>;
  /** branches in the project's repository, for a base-branch picker */
  projectBranches(projectId: string): Promise<string[]>;
  /** a task's isolated checkout, or null when it has none (B1) */
  taskWorkspace(taskId: string): Promise<WorkspaceInfo | null>;
  taskWorkspaceDiscard(taskId: string, force: boolean): Promise<void>;
  /** commit, push and open the PR by hand; resolves with what it did */
  taskPublish(taskId: string): Promise<string>;
  taskAttach(taskId: string, onData: (bytes: Uint8Array) => void): Promise<void>;
  taskWrite(taskId: string, data: string): Promise<void>;
  taskResize(taskId: string, cols: number, rows: number): Promise<void>;
  on<K extends keyof EventMap>(event: K, handler: (payload: EventMap[K]) => void): Promise<() => void>;
}

function toBytes(msg: unknown): Uint8Array {
  if (msg instanceof ArrayBuffer) return new Uint8Array(msg);
  if (msg instanceof Uint8Array) return msg;
  if (Array.isArray(msg)) return Uint8Array.from(msg as number[]);
  if (typeof msg === "string") return new TextEncoder().encode(msg);
  return new Uint8Array();
}

const tauriBackend: Backend = {
  appState: () => invoke("app_state"),
  projectScan: (path) => invoke("project_scan", { path }),
  projectAdd: (input) => invoke("project_add", { input }),
  projectRemove: (projectId) => invoke("project_remove", { projectId }),
  scriptSave: (script) => invoke("script_save", { script }),
  scriptDelete: (scriptId) => invoke("script_delete", { scriptId }),
  groupSave: (group) => invoke("group_save", { group }),
  groupDelete: (groupId) => invoke("group_delete", { groupId }),
  scriptStart: (scriptId) => invoke("script_start", { scriptId }),
  scriptStop: (scriptId) => invoke("script_stop", { scriptId }),
  scriptRestart: (scriptId) => invoke("script_restart", { scriptId }),
  retryCancel: (scriptId) => invoke("retry_cancel", { scriptId }),
  groupPlan: (groupId) => invoke("group_plan", { groupId }),
  groupRun: (groupId) => invoke("group_run", { groupId }),
  groupStop: (groupId) => invoke("group_stop", { groupId }),
  groupContinue: (groupId) => invoke("group_continue", { groupId }),
  stopEverything: () => invoke("stop_everything"),
  ptyAttach: (scriptId, onData) => {
    const onDataChannel = new Channel<unknown>();
    onDataChannel.onmessage = (msg) => onData(toBytes(msg));
    return invoke("pty_attach", { scriptId, onData: onDataChannel });
  },
  ptyWrite: (scriptId, data) => invoke("pty_write", { scriptId, data }),
  ptyResize: (scriptId, cols, rows) => invoke("pty_resize", { scriptId, cols, rows }),
  configExport: (projectId) => invoke("config_export", { projectId }),
  configImport: (projectId, path, mode) => invoke("config_import", { projectId, path, mode }),
  settingsSet: (settings) => invoke("settings_set", { settings }),
  dbBackup: (dest) => invoke("db_backup", { dest }),
  runHistory: (scriptId) => invoke("run_history", { scriptId }),
  mcpInfo: () => invoke("mcp_info"),
  agentList: () => invoke("agent_list"),
  taskList: (projectId) => invoke("task_list", { projectId }),
  taskSave: (task) => invoke("task_save", { task }),
  taskDelete: (taskId) => invoke("task_delete", { taskId }),
  taskStart: (taskId) => invoke("task_start", { taskId }),
  taskStop: (taskId) => invoke("task_stop", { taskId }),
  taskRuns: (taskId) => invoke("task_runs", { taskId }),
  projectBranches: (projectId) => invoke("project_branches", { projectId }),
  taskWorkspace: (taskId) => invoke("task_workspace", { taskId }),
  taskWorkspaceDiscard: (taskId, force) => invoke("task_workspace_discard", { taskId, force }),
  taskPublish: (taskId) => invoke("task_publish", { taskId }),
  taskAttach: (taskId, onData) => {
    const onDataChannel = new Channel<unknown>();
    onDataChannel.onmessage = (msg) => onData(toBytes(msg));
    return invoke("task_attach", { taskId, onData: onDataChannel });
  },
  taskWrite: (taskId, data) => invoke("task_write", { taskId, data }),
  taskResize: (taskId, cols, rows) => invoke("task_resize", { taskId, cols, rows }),
  on: (event, handler) => listen(event, (e) => handler(e.payload as never)),
};

export let backend: Backend = tauriBackend;

/** Browser dev mode (vite without Tauri): swap in the in-memory mock. */
export async function initBackend(): Promise<void> {
  if (!isTauri) {
    const { createMockBackend } = await import("./mock");
    backend = createMockBackend();
  }
}

// ---- native shell helpers (graceful no-ops in the browser) ----

export async function pickFolder(): Promise<string | null> {
  if (!isTauri) return "/Users/you/dev/acme-platform";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ directory: true, multiple: false, title: "Add project folder" });
  return typeof res === "string" ? res : null;
}

export async function pickTomlFile(): Promise<string | null> {
  if (!isTauri) return "/Users/you/dev/acme-platform/scriptr.toml";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ multiple: false, filters: [{ name: "scriptr.toml", extensions: ["toml"] }] });
  return typeof res === "string" ? res : null;
}

export async function pickSavePath(defaultPath: string): Promise<string | null> {
  if (!isTauri) return defaultPath;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return save({ defaultPath });
}

/** Native yes/no. `kind: "warning"` for anything that destroys work. */
export async function confirmAction(message: string, confirmLabel: string, title = "Scriptr"): Promise<boolean> {
  const { confirm } = await import("@tauri-apps/plugin-dialog");
  return confirm(message, { title, kind: "warning", okLabel: confirmLabel, cancelLabel: "Cancel" });
}

/** Opens a url in the user's own browser, not in the app's webview. */
export async function openUrl(url: string): Promise<void> {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

export async function revealInFinder(path: string): Promise<void> {
  if (!isTauri) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

export async function onFolderDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    if (e.payload.type === "drop") handler(e.payload.paths);
  });
}
