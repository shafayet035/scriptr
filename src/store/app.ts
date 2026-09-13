import { createEffect, createRoot, createSignal } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { computePlan } from "../lib/graph";
import { backend, initBackend, pickFolder, pickSavePath, pickTomlFile } from "../lib/ipc";
import { isLive } from "../lib/format";
import type {
  AddProjectInput,
  Group,
  GroupProgress,
  Plan,
  ProcStats,
  Project,
  RunInfo,
  ScanResult,
  Script,
  Settings,
} from "../lib/types";

export type View = "terminals" | "deps" | "log";
export type SettingsSection =
  | "general"
  | "terminal"
  | "shell"
  | "defaults"
  | "import-export"
  | "shortcuts"
  | "updates"
  | "about";

interface Ui {
  projectId: string | null;
  collapsed: Record<string, boolean>;
  /** selected group per project; null = all scripts */
  groupByProject: Record<string, string | null>;
  tabsByProject: Record<string, string[]>;
  activeByProject: Record<string, string | null>;
  view: View;
  inspectorScriptId: string | null;
  paletteOpen: boolean;
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  scan: ScanResult | null;
  filter: string;
  searchOpen: boolean;
  recent: string[];
  sidebarWidth: number;
}

export const SIDEBAR_DEFAULT = 256;
export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 560;

interface AppState {
  loaded: boolean;
  projects: Project[];
  scripts: Script[];
  groups: Group[];
  settings: Settings;
  dbPath: string;
  runs: Record<string, RunInfo>;
  stats: Record<string, ProcStats>;
  progress: Record<string, GroupProgress>;
  /** last observed starting → ready duration per script, for the cold-start estimate */
  readyMs: Record<string, number>;
  ui: Ui;
}

const UI_KEY = "scriptr.ui.v1";

export const [state, setState] = createStore<AppState>({
  loaded: false,
  projects: [],
  scripts: [],
  groups: [],
  settings: { onQuit: "stop", keepTomlInSync: false, importMode: "merge", defaultShell: "/bin/zsh -lc" },
  dbPath: "",
  runs: {},
  stats: {},
  progress: {},
  readyMs: {},
  ui: {
    projectId: null,
    collapsed: {},
    groupByProject: {},
    tabsByProject: {},
    activeByProject: {},
    view: "terminals",
    inspectorScriptId: null,
    paletteOpen: false,
    settingsOpen: false,
    settingsSection: "import-export",
    scan: null,
    filter: "",
    searchOpen: false,
    recent: [],
    sidebarWidth: SIDEBAR_DEFAULT,
  },
});

/** 1 Hz clock for uptime badges and countdowns. */
export const [now, setNow] = createSignal(Date.now());
setInterval(() => setNow(Date.now()), 1000);

export const [toastMsg, setToastMsg] = createSignal<{ text: string; tone: "info" | "error" } | null>(null);
let toastTimer: number | undefined;
export function toast(text: string, tone: "info" | "error" = "info") {
  setToastMsg({ text, tone });
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => setToastMsg(null), tone === "error" ? 6000 : 3500);
}

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));
async function attempt<T>(p: Promise<T>): Promise<T | undefined> {
  try {
    return await p;
  } catch (e) {
    toast(errText(e), "error");
    return undefined;
  }
}

// ---------------------------------------------------------------- selectors

const IDLE: Omit<RunInfo, "scriptId"> = {
  state: "idle",
  pid: null,
  startedAt: null,
  endedAt: null,
  exitCode: null,
  attempt: 0,
  maxAttempts: 0,
  nextRetryAt: null,
  backoffMs: null,
  degraded: false,
  gateNote: null,
};

export const project = (id: string | null) => state.projects.find((p) => p.id === id);
export const currentProject = () => project(state.ui.projectId);
export const script = (id: string | null) => state.scripts.find((s) => s.id === id);
export const runOf = (scriptId: string): RunInfo => state.runs[scriptId] ?? { scriptId, ...IDLE };

export const scriptsOf = (projectId: string) =>
  state.scripts.filter((s) => s.projectId === projectId).sort((a, b) => a.sortOrder - b.sortOrder);
export const groupsOf = (projectId: string) => state.groups.filter((g) => g.projectId === projectId);

export const selectedGroup = (projectId: string): Group | undefined => {
  const id = state.ui.groupByProject[projectId];
  if (id === null) return undefined;
  return state.groups.find((g) => g.id === id) ?? groupsOf(projectId)[0];
};

/** Scripts the toolbar's Run all / graph act on: the selected group, or every script in the project. */
export const scopeScripts = (projectId: string): Script[] => {
  const g = selectedGroup(projectId);
  if (!g) return scriptsOf(projectId);
  return g.scriptIds.map((id) => script(id)).filter((s): s is Script => !!s);
};

export const planFor = (projectId: string): Plan => {
  const g = selectedGroup(projectId);
  return computePlan(g?.id ?? "", scopeScripts(projectId));
};

export const tabsOf = (projectId: string) => state.ui.tabsByProject[projectId] ?? [];
export const activeScriptId = () => {
  const pid = state.ui.projectId;
  return pid ? (state.ui.activeByProject[pid] ?? null) : null;
};

export function countsOf(projectId: string) {
  let running = 0;
  let crashed = 0;
  let stopped = 0;
  for (const s of scriptsOf(projectId)) {
    const st = runOf(s.id).state;
    if (st === "running" || st === "starting" || st === "queued" || st === "stopping") running++;
    else if (st === "crashed" || st === "backoff") crashed++;
    else stopped++;
  }
  return { running, crashed, stopped };
}

// ---------------------------------------------------------------- lifecycle

const startedAtLocal = new Map<string, number>();

function onRunInfo(r: RunInfo) {
  // Read before writing: the store proxy reflects the new value once reconciled.
  const prevState = state.runs[r.scriptId]?.state;
  if (r.state === "starting" && prevState !== "starting") startedAtLocal.set(r.scriptId, performance.now());
  if (r.state === "running" && prevState === "starting" && startedAtLocal.has(r.scriptId)) {
    setState("readyMs", r.scriptId, Math.round(performance.now() - startedAtLocal.get(r.scriptId)!));
  }
  setState("runs", r.scriptId, reconcile(r));
  if (isLive(r.state) && (!prevState || !isLive(prevState))) ensureTab(r.scriptId, false);
}

function ensureTab(scriptId: string, focus: boolean) {
  const s = script(scriptId);
  if (!s) return;
  setState(
    "ui",
    produce((ui) => {
      const tabs = ui.tabsByProject[s.projectId] ?? [];
      if (!tabs.includes(scriptId)) ui.tabsByProject[s.projectId] = [...tabs, scriptId];
      if (focus || !ui.activeByProject[s.projectId]) ui.activeByProject[s.projectId] = scriptId;
    }),
  );
}

export async function init() {
  await initBackend();
  const snap = await backend.appState();

  let saved: Partial<Ui> = {};
  try {
    saved = JSON.parse(localStorage.getItem(UI_KEY) ?? "{}");
  } catch {
    /* first run or blocked storage */
  }
  const scriptIds = new Set(snap.scripts.map((s) => s.id));
  const projectIds = new Set(snap.projects.map((p) => p.id));
  const keepProjects = <T>(rec: Record<string, T> | undefined) =>
    Object.fromEntries(Object.entries(rec ?? {}).filter(([k]) => projectIds.has(k)));

  setState({
    projects: snap.projects,
    scripts: snap.scripts,
    groups: snap.groups,
    settings: snap.settings,
    dbPath: snap.dbPath,
    runs: Object.fromEntries(snap.runs.map((r) => [r.scriptId, r])),
    loaded: true,
  });
  setState(
    "ui",
    produce((ui) => {
      ui.projectId = saved.projectId && projectIds.has(saved.projectId) ? saved.projectId : (snap.projects[0]?.id ?? null);
      ui.collapsed = saved.collapsed ?? {};
      ui.groupByProject = keepProjects(saved.groupByProject);
      ui.recent = saved.recent ?? [];
      if (typeof saved.sidebarWidth === "number") ui.sidebarWidth = clampSidebar(saved.sidebarWidth);
      const tabs = keepProjects(saved.tabsByProject);
      for (const k of Object.keys(tabs)) tabs[k] = tabs[k].filter((id) => scriptIds.has(id));
      // every live script gets a tab, in sidebar order
      for (const s of [...snap.scripts].sort((a, b) => a.sortOrder - b.sortOrder)) {
        const r = snap.runs.find((x) => x.scriptId === s.id);
        if (r && (isLive(r.state) || r.state === "crashed")) {
          const t = tabs[s.projectId] ?? [];
          if (!t.includes(s.id)) tabs[s.projectId] = [...t, s.id];
        }
      }
      ui.tabsByProject = tabs;
      const active = keepProjects(saved.activeByProject);
      for (const [pid, t] of Object.entries(tabs)) {
        if (!active[pid] || !t.includes(active[pid]!)) active[pid] = t[0] ?? null;
      }
      ui.activeByProject = active;
    }),
  );

  createRoot(() =>
    createEffect(() => {
      const { projectId, collapsed, groupByProject, tabsByProject, activeByProject, recent, sidebarWidth } = state.ui;
      try {
        localStorage.setItem(
          UI_KEY,
          JSON.stringify({ projectId, collapsed, groupByProject, tabsByProject, activeByProject, recent, sidebarWidth }),
        );
      } catch {
        /* storage unavailable */
      }
    }),
  );

  void backend.on("script:state", onRunInfo);
  void backend.on("stats", (list) => setState("stats", reconcile(Object.fromEntries(list.map((s) => [s.scriptId, s])))));
  void backend.on("group:progress", (p) => {
    setState("progress", p.groupId, p);
    if (p.state === "degraded" && p.degradedScriptId) {
      toast(`${script(p.degradedScriptId)?.name ?? "A script"} didn't report ready in time — group paused`, "error");
    }
  });
  void backend.on("project:changed", () => void refresh());
  void backend.on("menu", onMenu);
}

export async function refresh() {
  const snap = await backend.appState();
  setState({ projects: snap.projects, scripts: snap.scripts, groups: snap.groups, settings: snap.settings });
  setState("runs", reconcile(Object.fromEntries(snap.runs.map((r) => [r.scriptId, r]))));
}

// ---------------------------------------------------------------- navigation

export function selectProject(projectId: string) {
  setState("ui", { projectId, settingsOpen: false });
  if (state.ui.inspectorScriptId && script(state.ui.inspectorScriptId)?.projectId !== projectId) {
    setState("ui", "inspectorScriptId", null);
  }
}

/** Keep the main column usable: never wider than the window minus ~560px of content. */
export function clampSidebar(px: number) {
  const roomy = Math.max(SIDEBAR_MIN, window.innerWidth - 560);
  return Math.round(Math.min(Math.max(px, SIDEBAR_MIN), SIDEBAR_MAX, roomy));
}

export const setSidebarWidth = (px: number) => setState("ui", "sidebarWidth", clampSidebar(px));

export function toggleCollapsed(projectId: string) {
  setState("ui", "collapsed", projectId, (c) => !c);
}

export function openScript(scriptId: string) {
  const s = script(scriptId);
  if (!s) return;
  selectProject(s.projectId);
  ensureTab(scriptId, true);
  setState("ui", "view", "terminals");
  if (state.ui.inspectorScriptId) setState("ui", "inspectorScriptId", scriptId);
}

export function closeTab(scriptId: string) {
  const s = script(scriptId);
  if (!s) return;
  setState(
    "ui",
    produce((ui) => {
      const tabs = ui.tabsByProject[s.projectId] ?? [];
      const i = tabs.indexOf(scriptId);
      const next = tabs.filter((id) => id !== scriptId);
      ui.tabsByProject[s.projectId] = next;
      if (ui.activeByProject[s.projectId] === scriptId) {
        ui.activeByProject[s.projectId] = next[Math.min(i, next.length - 1)] ?? null;
      }
    }),
  );
}

export function cycleTab(delta: number) {
  const pid = state.ui.projectId;
  if (!pid) return;
  const tabs = tabsOf(pid);
  if (tabs.length === 0) return;
  const i = tabs.indexOf(activeScriptId() ?? "");
  setState("ui", "activeByProject", pid, tabs[(i + delta + tabs.length) % tabs.length]);
}

/** Focus a script's terminal once its tab has rendered. */
export function terminalFocus(scriptId: string) {
  requestAnimationFrame(() => requestAnimationFrame(() => document.querySelector<HTMLElement>(`.term-host[data-script-id="${scriptId}"] textarea`)?.focus()));
}

export const setView = (view: View) => setState("ui", { view, settingsOpen: false });
export const selectGroup = (projectId: string, groupId: string | null) => setState("ui", "groupByProject", projectId, groupId);
export const openInspector = (scriptId: string | null) => setState("ui", "inspectorScriptId", scriptId);
export const toggleInspector = () => openInspector(state.ui.inspectorScriptId ? null : activeScriptId());
export const setPalette = (open: boolean) => setState("ui", "paletteOpen", open);
export const setFilter = (filter: string) => setState("ui", "filter", filter);
export const setSearchOpen = (open: boolean) => setState("ui", "searchOpen", open);
export function openSettings(section?: SettingsSection) {
  setState("ui", { settingsOpen: true, paletteOpen: false, ...(section ? { settingsSection: section } : {}) });
}
export const closeSettings = () => setState("ui", "settingsOpen", false);
export const toggleSettings = () => (state.ui.settingsOpen ? closeSettings() : openSettings());

// ---------------------------------------------------------------- run control

export async function startScript(scriptId: string, focus = true) {
  if (focus) openScript(scriptId);
  await attempt(backend.scriptStart(scriptId));
}
export const stopScript = (scriptId: string) => attempt(backend.scriptStop(scriptId));
export const restartScript = (scriptId: string) => attempt(backend.scriptRestart(scriptId));
export const cancelRetry = (scriptId: string) => attempt(backend.retryCancel(scriptId));

export async function runScope(projectId: string) {
  const g = selectedGroup(projectId);
  if (g) {
    const plan = await attempt(backend.groupRun(g.id));
    if (plan) plan.waves.flat().forEach((id) => ensureTab(id, false));
    return;
  }
  const plan = planFor(projectId);
  if (plan.cycle) {
    toast(`Refusing to start: ${script(plan.cycle[1])?.name} and ${script(plan.cycle[0])?.name} wait for each other`, "error");
    return;
  }
  for (const s of scriptsOf(projectId)) void attempt(backend.scriptStart(s.id));
}

export async function stopScope(projectId: string) {
  const g = selectedGroup(projectId);
  if (g) return attempt(backend.groupStop(g.id));
  // reverse waves so dependents go down before what they depend on
  for (const wave of [...planFor(projectId).waves].reverse()) {
    await Promise.all(wave.map((id) => attempt(backend.scriptStop(id))));
  }
}

export const continueGroup = (groupId: string) => attempt(backend.groupContinue(groupId));
export const stopEverything = () => attempt(backend.stopEverything());

// ---------------------------------------------------------------- editing

/** Store values are proxies; hand the backend plain data. */
const plain = <T>(value: T): T => JSON.parse(JSON.stringify(value));

export async function saveScript(next: Script) {
  const saved = await attempt(backend.scriptSave(plain(next)));
  if (!saved) return undefined;
  setState("scripts", (list) => {
    const i = list.findIndex((s) => s.id === saved.id);
    return i >= 0 ? list.map((s) => (s.id === saved.id ? saved : s)) : [...list, saved];
  });
  return saved;
}

export async function deleteScript(scriptId: string) {
  closeTab(scriptId);
  if (state.ui.inspectorScriptId === scriptId) openInspector(null);
  await attempt(backend.scriptDelete(scriptId));
  setState("scripts", (list) => list.filter((s) => s.id !== scriptId));
}

export async function saveGroup(next: Group) {
  const saved = await attempt(backend.groupSave(plain(next)));
  if (!saved) return undefined;
  setState("groups", (list) => {
    const i = list.findIndex((g) => g.id === saved.id);
    return i >= 0 ? list.map((g) => (g.id === saved.id ? saved : g)) : [...list, saved];
  });
  return saved;
}

export async function removeProject(projectId: string) {
  await attempt(backend.projectRemove(projectId));
  await refresh();
  if (state.ui.projectId === projectId) setState("ui", "projectId", state.projects[0]?.id ?? null);
}

export async function saveSettings(next: Settings) {
  const saved = await attempt(backend.settingsSet(plain(next)));
  if (saved) setState("settings", saved);
}

// ---------------------------------------------------------------- add project (flow A)

export async function beginAddProject(path?: string) {
  const folder = path ?? (await pickFolder());
  if (!folder) return;
  const scan = await attempt(backend.projectScan(folder));
  if (!scan) return;
  setState("ui", { scan, settingsOpen: false, paletteOpen: false });
}

export const cancelAddProject = () => setState("ui", "scan", null);

export async function confirmAddProject(input: AddProjectInput) {
  const snap = await attempt(backend.projectAdd(input));
  if (!snap) return;
  setState({ projects: snap.projects, scripts: snap.scripts, groups: snap.groups });
  setState("runs", reconcile(Object.fromEntries(snap.runs.map((r) => [r.scriptId, r]))));
  const added = snap.projects.find((p) => p.path === input.path);
  setState(
    "ui",
    produce((ui) => {
      ui.scan = null;
      ui.recent = [input.path, ...ui.recent.filter((r) => r !== input.path)].slice(0, 8);
      if (added) ui.projectId = added.id;
    }),
  );
  toast(`Added ${input.name} · ${input.importToml ? "imported scriptr.toml" : `${input.scripts.length} scripts`}`);
}

// ---------------------------------------------------------------- import / export

export async function exportToml(projectId: string) {
  const path = await attempt(backend.configExport(projectId));
  if (path) toast(`Wrote ${path}`);
}

export async function importToml(projectId: string, mode: Settings["importMode"]) {
  const file = await pickTomlFile();
  if (!file) return;
  const report = await attempt(backend.configImport(projectId, file, mode));
  if (!report) return;
  if (report.preview !== null) return report;
  await refresh();
  toast(`Imported · ${report.added} added · ${report.updated} updated · ${report.removed} removed`);
  return report;
}

export async function backupDb() {
  const dest = await pickSavePath("scriptr-backup.db");
  if (!dest) return;
  try {
    await backend.dbBackup(dest);
    toast(`Backed up to ${dest}`);
  } catch (e) {
    toast(errText(e), "error");
  }
}

// ---------------------------------------------------------------- native menu

function onMenu(id: string) {
  const pid = state.ui.projectId;
  const active = activeScriptId();
  switch (id) {
    case "settings":
      return toggleSettings();
    case "add-project":
      return void beginAddProject();
    case "import-toml":
      return pid && void importToml(pid, state.settings.importMode);
    case "export-toml":
      return pid && void exportToml(pid);
    case "close-tab":
      if (state.ui.paletteOpen) return setPalette(false);
      if (state.ui.settingsOpen) return closeSettings();
      return active && closeTab(active);
    case "palette":
      return setPalette(!state.ui.paletteOpen);
    case "view-terminals":
      return setView("terminals");
    case "view-deps":
      return setView("deps");
    case "view-log":
      return setView("log");
    case "find":
      return setSearchOpen(!state.ui.searchOpen);
    case "toggle-inspector":
      return toggleInspector();
    case "run-group":
      return pid && void runScope(pid);
    case "stop-group":
      return pid && void stopScope(pid);
    case "restart-script":
      return active && void restartScript(active);
    case "stop-everything":
      return void stopEverything();
    case "next-tab":
      return cycleTab(1);
    case "prev-tab":
      return cycleTab(-1);
  }
}

/** Keyboard fallback for the browser build, where there is no native menu bar. */
export function handleShortcut(e: KeyboardEvent): boolean {
  if (!(e.metaKey || e.ctrlKey)) return false;
  const k = e.key.toLowerCase();
  const map: Record<string, string> = e.shiftKey
    ? { ".": "stop-everything", ">": "stop-everything", r: "restart-script", "]": "next-tab", "[": "prev-tab", "}": "next-tab", "{": "prev-tab" }
    : {
        k: "palette", ",": "settings", o: "add-project", w: "close-tab", "1": "view-terminals", "2": "view-deps",
        "3": "view-log", f: "find", i: "toggle-inspector", r: "run-group", ".": "stop-group",
      };
  const id = map[k];
  if (!id) return false;
  e.preventDefault();
  onMenu(id);
  return true;
}
