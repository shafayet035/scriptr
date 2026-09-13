import { createMemo, createSignal, For, type JSX, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { revealInFinder } from "../lib/ipc";
import { tildify } from "../lib/format";
import { renderToml, type TomlToken } from "../lib/toml";
import type { ImportReport, Settings } from "../lib/types";
import {
  backupDb,
  closeSettings,
  exportToml,
  groupsOf,
  importToml,
  saveSettings,
  scriptsOf,
  type SettingsSection,
  state,
  setState,
} from "../store/app";

const NAV: { id: SettingsSection; label: string }[] = [
  { id: "general", label: "General" },
  { id: "terminal", label: "Terminal" },
  { id: "shell", label: "Shell & environment" },
  { id: "defaults", label: "Project defaults" },
  { id: "import-export", label: "Import & export" },
  { id: "shortcuts", label: "Keyboard shortcuts" },
  { id: "updates", label: "Updates" },
  { id: "about", label: "About" },
];

/** Screen 08 — settings; Import & export is the designed section. */
export function SettingsScreen() {
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !(document.activeElement instanceof HTMLInputElement)) closeSettings();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <>
      <nav class="settings-nav">
        <p class="t-label-caps c-muted" style={{ padding: "0 0 8px 8px" }}>
          Settings
        </p>
        <For each={NAV}>
          {(n) => (
            <button class="settings-nav-item" aria-current={state.ui.settingsSection === n.id} onClick={() => setState("ui", "settingsSection", n.id)}>
              {n.label}
            </button>
          )}
        </For>
      </nav>
      <main class="settings-content scroll-y">
        <div class="settings-inner">
          <Switch>
            <Match when={state.ui.settingsSection === "import-export"}>
              <ImportExport />
            </Match>
            <Match when={state.ui.settingsSection === "general"}>
              <General />
            </Match>
            <Match when={state.ui.settingsSection === "shell"}>
              <Shell />
            </Match>
            <Match when={state.ui.settingsSection === "shortcuts"}>
              <Shortcuts />
            </Match>
            <Match when={state.ui.settingsSection === "terminal"}>
              <Head title="Terminal" body="Output is rendered with xterm.js on the GPU when the webview supports WebGL, falling back to the DOM renderer." />
              <Card title="Scrollback" body="Scriptr keeps the last ~5,000 lines per script in the core, so output survives tab switches and restarts.">
                <p class="t-mono-11 c-secondary">font JetBrains Mono 12 · scrollback 5000 · TERM=xterm-256color</p>
              </Card>
            </Match>
            <Match when={state.ui.settingsSection === "defaults"}>
              <Head title="Project defaults" body="What new scripts get when a folder is scanned." />
              <Card title="Ready gate" body="A command that mentions a port waits for the port to open. Known one-shots (migrate, build, test) wait for exit code 0. Everything else counts as ready immediately." />
              <Card title="Restart policy" body="New scripts never restart automatically. Change it per script under On exit in the inspector." />
            </Match>
            <Match when={state.ui.settingsSection === "updates"}>
              <Head title="Updates" body="This is a development build. Signed releases with auto-update come with packaging (M7)." />
            </Match>
            <Match when={state.ui.settingsSection === "about"}>
              <Head title="Scriptr" body="Version 0.1.0 · Tauri + Rust core · SolidJS UI. Turns the pile of terminal tabs a project needs into one window." />
            </Match>
          </Switch>
        </div>
      </main>
    </>
  );
}

function Head(props: { title: string; body: string }) {
  return (
    <div class="col" style={{ gap: "6px" }}>
      <p class="t-display-20 c-primary">{props.title}</p>
      <p class="t-body-13 c-secondary">{props.body}</p>
    </div>
  );
}

function Card(props: { title: string; body: string; children?: JSX.Element }) {
  return (
    <section class="settings-card">
      <div class="col" style={{ gap: "5px" }}>
        <p class="t-title-13 c-primary">{props.title}</p>
        <p class="t-caption-11 c-muted">{props.body}</p>
      </div>
      {props.children}
    </section>
  );
}

function ImportExport() {
  const [projectId, setProjectId] = createSignal(state.ui.projectId ?? state.projects[0]?.id ?? null);
  const [report, setReport] = createSignal<ImportReport | null>(null);
  const proj = () => state.projects.find((p) => p.id === projectId());
  const mode = () => state.settings.importMode;
  const set = (patch: Partial<Settings>) => void saveSettings({ ...state.settings, ...patch });

  const projectMenu = (e: MouseEvent) =>
    popupMenu(
      state.projects.map((p) => ({ label: p.name, checked: p.id === projectId(), action: () => setProjectId(p.id) })),
      e.currentTarget as HTMLElement,
    );

  const tokens = createMemo<TomlToken[][]>(() => {
    const p = proj();
    if (!p) return [];
    return renderToml(scriptsOf(p.id), groupsOf(p.id));
  });

  const radio = (value: Settings["importMode"], label: string) => (
    <button class="radio" aria-checked={mode() === value} onClick={() => set({ importMode: value })}>
      <span class="radio-dot" />
      {label}
    </button>
  );

  return (
    <>
      <Head title="Import & export" body="Scriptr stores everything locally. Exporting writes a single file you can commit so your team shares the same scripts." />

      <Card title="Local database" body="Projects, scripts, groups, dependencies and health checks live here. Nothing is written into your repositories until you export.">
        <div class="settings-row">
          <Icon name="database" size={15} color="var(--text-muted)" />
          <span class="path-chip selectable ellipsis">{tildify(state.dbPath)}</span>
          <div class="grow" />
          <button class="btn btn-secondary btn-sm" onClick={() => void revealInFinder(state.dbPath)}>
            Reveal
          </button>
          <button class="btn btn-secondary btn-sm" onClick={() => void backupDb()}>
            Back up…
          </button>
        </div>
      </Card>

      <Card title="Export to the project" body="Writes a scriptr.toml at the project root. Safe to commit — it contains no secrets, only commands and wiring.">
        <div class="settings-row">
          <button class="select" onClick={projectMenu} disabled={state.projects.length === 0}>
            {proj()?.name ?? "No projects"}
            <Icon name="chevron-down" size={12} color="var(--text-muted)" />
          </button>
          <span class="path-chip">./scriptr.toml</span>
          <div class="grow" />
          <button class="btn btn-primary" style={{ font: "500 12px/18px var(--font-ui)", padding: "7px 13px 7px 12px", gap: "6px" }} disabled={!proj()} onClick={() => proj() && void exportToml(proj()!.id)}>
            <Icon name="arrow-up" size={13} />
            Export
          </button>
        </div>
        <button class="settings-row toggle-row" onClick={() => set({ keepTomlInSync: !state.settings.keepTomlInSync })}>
          <span class="toggle" role="switch" aria-checked={state.settings.keepTomlInSync} />
          <span class="col" style={{ gap: "2px", "text-align": "left" }}>
            <span class="t-medium-12 c-primary">Keep the file in sync</span>
            <span class="t-caption-11 c-muted">Rewrite scriptr.toml whenever a script, group or dependency changes.</span>
          </span>
        </button>
      </Card>

      <Card title="Import from a project" body="When a scriptr.toml is found in a folder you add, Scriptr offers to import it. You can also import one manually.">
        <div class="settings-row" style={{ gap: "8px" }}>
          {radio("merge", "Merge — keep my local edits")}
          {radio("replace", "Replace local scripts")}
          {radio("preview", "Preview the diff first")}
          <div class="grow" />
          <button
            class="btn btn-secondary btn-sm"
            style={{ gap: "6px" }}
            disabled={!proj()}
            onClick={async () => setReport((await importToml(proj()!.id, mode())) ?? null)}
          >
            <Icon name="arrow-down" size={13} />
            Import file…
          </button>
        </div>
        <Show when={report()?.preview}>
          <pre class="code selectable">{report()!.preview}</pre>
        </Show>
      </Card>

      <section class="settings-card">
        <div class="col" style={{ gap: "5px" }}>
          <p class="t-title-13 c-primary">What a scriptr.toml looks like</p>
          <p class="t-caption-11 c-muted">
            The exported schema mirrors what you configure in the inspector{proj() ? ` — this is ${proj()!.name} right now.` : "."}
          </p>
        </div>
        <div class="code selectable">
          <For each={tokens()}>
            {(line) => (
              <p>
                <For each={line}>{(t) => <span class={`tok-${t.kind}`}>{t.text}</span>}</For>
                {line.length === 0 ? " " : ""}
              </p>
            )}
          </For>
        </div>
      </section>
    </>
  );
}

function General() {
  const set = (patch: Partial<Settings>) => void saveSettings({ ...state.settings, ...patch });
  return (
    <>
      <Head title="General" body="How Scriptr behaves around the processes it owns." />
      <Card title="When the window closes" body="Stopping sends SIGTERM to each process group, waits for a grace period, then SIGKILL.">
        <div class="settings-row" style={{ gap: "8px" }}>
          <button class="radio" aria-checked={state.settings.onQuit === "stop"} onClick={() => set({ onQuit: "stop" })}>
            <span class="radio-dot" />
            Stop everything
          </button>
          <button class="radio" aria-checked={state.settings.onQuit === "leave"} onClick={() => set({ onQuit: "leave" })}>
            <span class="radio-dot" />
            Leave processes running
          </button>
        </div>
      </Card>
    </>
  );
}

function Shell() {
  const [shell, setShell] = createSignal(state.settings.defaultShell);
  return (
    <>
      <Head title="Shell & environment" body="Scripts run through a login shell so your PATH, nvm, pyenv and friends behave like they do in Terminal." />
      <Card title="Default shell" body="Used by every script without its own shell. The command is appended as the last argument.">
        <div class="settings-row">
          <label class="input" style={{ width: "320px" }}>
            <input class="t-mono-11" value={shell()} spellcheck={false} onInput={(e) => setShell(e.currentTarget.value)} />
          </label>
          <button
            class="btn btn-secondary btn-sm"
            disabled={shell().trim() === state.settings.defaultShell || !shell().trim()}
            onClick={() => void saveSettings({ ...state.settings, defaultShell: shell().trim() })}
          >
            Save
          </button>
        </div>
      </Card>
    </>
  );
}

function Shortcuts() {
  const m = /Mac/.test(navigator.platform) ? "⌘" : "Ctrl ";
  const rows: [string, string][] = [
    ["Command palette", `${m}K`],
    ["Add project", `${m}O`],
    ["Run group", `${m}R`],
    ["Stop group", `${m}.`],
    ["Restart script", `${m}⇧R`],
    ["Stop everything", `${m}⇧.`],
    ["Terminals / Dependencies / Combined log", `${m}1 · ${m}2 · ${m}3`],
    ["Find in terminal", `${m}F`],
    ["Toggle inspector", `${m}I`],
    ["Close tab", `${m}W`],
    ["Next / previous tab", `${m}⇧] · ${m}⇧[`],
    ["Settings", `${m},`],
  ];
  return (
    <>
      <Head title="Keyboard shortcuts" body="Shortcuts live in the native menu bar, so they work from anywhere in the window." />
      <section class="settings-card" style={{ gap: "0" }}>
        <For each={rows}>
          {([label, keys]) => (
            <div class="shortcut-row">
              <span class="t-body-12 c-secondary">{label}</span>
              <div class="grow" />
              <span class="kbd">{keys}</span>
            </div>
          )}
        </For>
      </section>
    </>
  );
}
