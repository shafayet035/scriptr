import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AddProjectInput,
  GroupProgress,
  Group,
  ImportReport,
  Plan,
  ProcStats,
  RunInfo,
  RunRecord,
  ScanResult,
  Script,
  Settings,
  Snapshot,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const isMac = /Mac/.test(navigator.platform);

export interface EventMap {
  "script:state": RunInfo;
  "group:progress": GroupProgress;
  "project:changed": { projectId: string };
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
