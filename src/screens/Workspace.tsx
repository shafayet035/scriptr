import { createEffect, createSignal, For, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { Icon } from "../components/Icon";
import { DependencyView } from "./DependencyView";
import { popupMenu } from "../lib/menu";
import {
  displayName,
  fmtBytes,
  fmtDuration,
  fmtUptime,
  gateDescription,
  isLive,
  runBadge,
  tildify,
  tone,
} from "../lib/format";
import {
  COMBINED_ID,
  clearTerminal,
  fitTerminal,
  mountTerminal,
  seedCombined,
  setCombinedNames,
  terminalFor,
  terminalText,
} from "../lib/terminals";
import {
  activeScriptId,
  cancelRetry,
  closeTab,
  continueGroup,
  countsOf,
  currentProject,
  groupsOf,
  now,
  openInspector,
  openScript,
  planFor,
  restartScript,
  runOf,
  runScope,
  scopeScripts,
  script,
  scriptsOf,
  selectedGroup,
  selectGroup,
  setSearchOpen,
  setView,
  startScript,
  state,
  stopScope,
  stopScript,
  tabsOf,
  toast,
  toggleInspector,
} from "../store/app";

/** Screen 03 (terminals) and 05 (dependencies / combined log) share this column. */
export function Workspace() {
  return (
    <main class="main">
      <Toolbar />
      <Switch>
        <Match when={state.ui.view === "terminals"}>
          <TabBar />
          <TerminalPane />
        </Match>
        <Match when={true}>
          <ViewSwitcher />
          <div class="group-view">
            <Show when={state.ui.view === "deps"} fallback={<CombinedLog />}>
              <DependencyView />
            </Show>
          </div>
        </Match>
      </Switch>
      <StatusBar />
    </main>
  );
}

// ---------------------------------------------------------------- toolbar

function Toolbar() {
  const p = () => currentProject()!;
  const group = () => selectedGroup(p().id);
  const progress = () => (group() ? state.progress[group()!.id] : undefined);
  const anyLive = () => scopeScripts(p().id).some((s) => isLive(runOf(s.id).state));
  const starting = () => progress()?.state === "running" || progress()?.state === "waiting";

  const groupMenu = (e: MouseEvent) =>
    popupMenu(
      [
        ...groupsOf(p().id).map((g) => ({
          label: `${g.name}  ·  ${g.scriptIds.length}`,
          checked: group()?.id === g.id,
          action: () => selectGroup(p().id, g.id),
        })),
        { label: `All scripts  ·  ${scriptsOf(p().id).length}`, checked: !group(), action: () => selectGroup(p().id, null) },
        { separator: true },
        { label: "Show dependencies", action: () => setView("deps") },
      ],
      e.currentTarget as HTMLElement,
    );

  return (
    <div class="toolbar">
      <div class="project-id">
        <div class="title-row">
          <span class="t-heading-15 c-primary ellipsis">{p().name}</span>
          <Show when={p().branch}>
            <span class="branch">
              <Icon name="branch" size={11} />
              {p().branch}
            </span>
          </Show>
        </div>
        <span class="t-mono-11 c-muted ellipsis selectable">{tildify(p().path)}</span>
      </div>
      <div class="grow" />
      <button class="group-select" onClick={groupMenu}>
        <Icon name="layers" size={13} color="var(--text-secondary)" />
        {group()?.name ?? "All scripts"}
        <Icon name="chevron-down" size={12} color="var(--text-muted)" />
      </button>
      <Show
        when={progress()?.state === "degraded"}
        fallback={
          <button class="btn btn-primary" onClick={() => void runScope(p().id)} disabled={starting()}>
            <Icon name="play" size={12} />
            {starting() ? `Wave ${progress()!.wave + 1} of ${progress()!.totalWaves}` : "Run all"}
          </button>
        }
      >
        <button class="btn btn-primary" onClick={() => void continueGroup(group()!.id)} title="A gate timed out — continue with the next wave">
          <Icon name="play" size={12} />
          Continue
        </button>
      </Show>
      <button class="btn btn-secondary" disabled={!anyLive()} onClick={() => void stopScope(p().id)}>
        <Icon name="stop" size={12} color="var(--status-crashed)" />
        Stop all
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- tabs

function TabBar() {
  const pid = () => state.ui.projectId!;

  const addMenu = (e: MouseEvent) => {
    const open = new Set(tabsOf(pid()));
    const rest = scriptsOf(pid()).filter((s) => !open.has(s.id));
    return popupMenu(
      rest.length
        ? rest.map((s) => ({ label: displayName(s), action: () => openScript(s.id) }))
        : [{ label: "Every script already has a tab", enabled: false }],
      e.currentTarget as HTMLElement,
    );
  };

  return (
    <div class="tabbar">
      <div class="tabs" role="tablist">
        <For each={tabsOf(pid())}>
          {(id) => (
            <Show when={script(id)}>
              {(s) => (
                <button
                  class="tab"
                  role="tab"
                  aria-selected={activeScriptId() === id}
                  data-tone={tone(runOf(id).state)}
                  onClick={() => openScript(id)}
                  onAuxClick={(e) => e.button === 1 && closeTab(id)}
                  onContextMenu={(e) =>
                    popupMenu(
                      [
                        { label: "Close tab", action: () => closeTab(id) },
                        { label: "Close other tabs", action: () => tabsOf(pid()).filter((t) => t !== id).forEach(closeTab) },
                        { separator: true },
                        { label: "Edit script…", action: () => openInspector(id) },
                      ],
                      e,
                    )
                  }
                >
                  <span class="dot" data-state={runOf(id).state} style={{ width: "7px", height: "7px" }} />
                  {displayName(s())}
                  <span
                    class="tab-close"
                    role="button"
                    title="Close tab"
                    onClick={(e) => {
                      e.stopPropagation();
                      closeTab(id);
                    }}
                  >
                    <Icon name="x" size={11} />
                  </span>
                </button>
              )}
            </Show>
          )}
        </For>
        <button class="icon-btn" style={{ width: "25px", height: "25px" }} onClick={addMenu} title="Open a script">
          <Icon name="plus" size={13} />
        </button>
      </div>
      <div class="grow" />
      <button class="icon-btn" onClick={() => setView("deps")} title="Dependencies">
        <Icon name="graph" size={13} />
      </button>
      <button class="icon-btn" aria-pressed={state.ui.searchOpen} onClick={() => setSearchOpen(!state.ui.searchOpen)} title="Find in terminal">
        <Icon name="search" size={14} />
      </button>
      <button class="icon-btn" aria-pressed={!!state.ui.inspectorScriptId} onClick={toggleInspector} title="Inspector">
        <Icon name="split" size={14} />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- terminal pane

function TerminalPane() {
  let stack!: HTMLDivElement;
  const pid = () => state.ui.projectId!;

  // Every open tab keeps a live xterm host in the stack; only the active one is visible.
  createEffect(() => {
    const tabs = tabsOf(pid());
    const active = activeScriptId();
    for (const id of tabs) mountTerminal(id, stack).host.hidden = id !== active;
    for (const el of Array.from(stack.querySelectorAll<HTMLElement>(".term-host"))) {
      if (!tabs.includes(el.dataset.scriptId!)) el.remove();
    }
  });

  createEffect(
    on(activeScriptId, (active) => {
      if (!active) return;
      requestAnimationFrame(() => {
        fitTerminal(active);
        terminalFor(active).term.focus();
      });
    }),
  );

  onMount(() => {
    const ro = new ResizeObserver(() => {
      const active = activeScriptId();
      if (active) fitTerminal(active);
    });
    ro.observe(stack);
    onCleanup(() => ro.disconnect());
  });

  const active = () => activeScriptId();
  const idle = () => {
    const r = active() ? runOf(active()!) : undefined;
    return !!r && (r.state === "idle" || (r.state === "stopped" && !r.startedAt));
  };

  return (
    <section class="terminal-pane">
      <Show when={active() && script(active()!)}>
        <CommandBar scriptId={active()!} />
        <CrashBanner scriptId={active()!} />
      </Show>
      <div class="term-stack" ref={stack}>
        <Show when={state.ui.searchOpen && active()}>
          <SearchBar scriptId={active()!} />
        </Show>
        <Show when={!active()}>
          <div class="term-empty">
            <Icon name="terminal" size={26} color="var(--border-strong)" />
            <p class="t-title-13 c-secondary">No terminals open</p>
            <p class="t-caption-11">Start a script from the sidebar, or press ⌘K to search.</p>
          </div>
        </Show>
        <Show when={idle()}>
          <div class="term-empty">
            <p class="t-title-13 c-secondary">{displayName(script(active()!)!)} isn't running</p>
            <button class="btn btn-primary" onClick={() => void startScript(active()!)}>
              <Icon name="play" size={12} />
              Start
            </button>
          </div>
        </Show>
      </div>
    </section>
  );
}

function CommandBar(props: { scriptId: string }) {
  const s = () => script(props.scriptId)!;
  const r = () => runOf(props.scriptId);
  const badge = () => runBadge(r(), now());

  const copy = async () => {
    const text = terminalText(props.scriptId);
    try {
      await navigator.clipboard.writeText(text);
      toast(`Copied ${text.split("\n").length} lines of output`);
    } catch {
      toast("Clipboard is not available", "error");
    }
  };

  return (
    <div class="commandbar">
      <span class="cmd ellipsis selectable" title={s().cmd}>
        {s().cmd}
      </span>
      <Show when={badge()}>
        {(b) => (
          <span class="pill" data-tone={b().tone}>
            {b().text}
          </span>
        )}
      </Show>
      <div class="grow" />
      <button class="icon-btn" title="Restart" onClick={() => void restartScript(props.scriptId)}>
        <Icon name="restart" size={14} color="var(--text-secondary)" />
      </button>
      <Show
        when={isLive(r().state)}
        fallback={
          <button class="icon-btn" title="Start" onClick={() => void startScript(props.scriptId)}>
            <Icon name="play" size={14} color="var(--status-running)" />
          </button>
        }
      >
        <button class="icon-btn" title="Stop" onClick={() => void stopScript(props.scriptId)}>
          <Icon name="stop" size={14} color="var(--status-crashed)" />
        </button>
      </Show>
      <button class="icon-btn" title="Clear terminal" onClick={() => clearTerminal(props.scriptId)}>
        <Icon name="trash" size={14} />
      </button>
      <button class="icon-btn" title="Copy output" onClick={() => void copy()}>
        <Icon name="copy" size={14} />
      </button>
    </div>
  );
}

/** Screen 06 — exit code, attempt n/max, backoff, Restart now / Stop retrying. */
function CrashBanner(props: { scriptId: string }) {
  const s = () => script(props.scriptId)!;
  const r = () => runOf(props.scriptId);
  const crashed = () => r().state === "backoff" || r().state === "crashed";
  const degraded = () => r().state === "running" && r().degraded;

  const backoffSeq = () => {
    const base = r().backoffMs ?? s().restart.backoffMs;
    const cap = s().restart.backoffMaxMs;
    return [...new Set([base, base * 2, base * 4].map((ms) => Math.min(ms, cap)))].map(fmtDuration).join(" → ");
  };

  const detail = () => {
    const run = r();
    if (run.state === "backoff") {
      const wait = fmtUptime(Math.max(0, (run.nextRetryAt ?? now()) - now()));
      return `Auto-restart is on · retrying in ${wait} · attempt ${run.attempt} of ${run.maxAttempts} · backoff ${backoffSeq()}`;
    }
    if (s().restart.on === "never") return "Auto-restart is off · set a policy under On exit in the inspector";
    return `Gave up after ${run.maxAttempts || s().restart.max} attempts`;
  };

  return (
    <>
      <Show when={crashed()}>
        <div class="crash-banner" role="alert">
          <Icon name="alert" size={17} color="var(--status-crashed)" />
          <div class="copy">
            <p class="t-title-13 c-primary ellipsis">
              {s().name} exited with code {r().exitCode ?? "?"}
            </p>
            <p class="t-caption-11 c-secondary ellipsis">{detail()}</p>
          </div>
          <div class="grow" />
          <button class="btn btn-secondary" onClick={() => void restartScript(props.scriptId)}>
            <Icon name="restart" size={12} color="var(--text-secondary)" />
            Restart now
          </button>
          <Show
            when={r().state === "backoff"}
            fallback={
              <button class="btn btn-secondary quiet" onClick={() => openInspector(props.scriptId)}>
                Edit policy
              </button>
            }
          >
            <button class="btn btn-secondary quiet" onClick={() => void cancelRetry(props.scriptId)}>
              Stop retrying
            </button>
          </Show>
        </div>
      </Show>
      <Show when={degraded()}>
        <div class="crash-banner" data-tone="degraded" role="status">
          <Icon name="alert" size={17} color="var(--status-starting)" />
          <div class="copy">
            <p class="t-title-13 c-primary ellipsis">{s().name} is running but never reported ready</p>
            <p class="t-caption-11 c-secondary ellipsis">Gate: {gateDescription(s().ready)} · dependents are waiting</p>
          </div>
          <div class="grow" />
          <button class="btn btn-secondary" onClick={() => void restartScript(props.scriptId)}>
            <Icon name="restart" size={12} color="var(--text-secondary)" />
            Restart now
          </button>
          <button class="btn btn-secondary quiet" onClick={() => openInspector(props.scriptId)}>
            Edit gate
          </button>
        </div>
      </Show>
    </>
  );
}

function SearchBar(props: { scriptId: string }) {
  const [query, setQuery] = createSignal("");
  const decorations = {
    matchBackground: "#3a3212",
    matchBorder: "#3a3212",
    matchOverviewRuler: "#e3b341",
    activeMatchBackground: "#7a5f14",
    activeMatchBorder: "#e3b341",
    activeMatchColorOverviewRuler: "#e3b341",
  };
  const find = (backwards = false) => {
    const { search } = terminalFor(props.scriptId);
    if (!query()) return search.clearDecorations();
    if (backwards) search.findPrevious(query(), { decorations });
    else search.findNext(query(), { decorations, incremental: true });
  };
  const close = () => {
    terminalFor(props.scriptId).search.clearDecorations();
    setSearchOpen(false);
  };

  return (
    <div class="term-search">
      <Icon name="search" size={13} />
      <input
        ref={(el) => queueMicrotask(() => el.focus())}
        placeholder="Find in output"
        value={query()}
        spellcheck={false}
        onInput={(e) => {
          setQuery(e.currentTarget.value);
          find();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") find(e.shiftKey);
          if (e.key === "Escape") close();
        }}
      />
      <button class="icon-btn" title="Previous match" onClick={() => find(true)}>
        <Icon name="arrow-up" size={13} />
      </button>
      <button class="icon-btn" title="Next match" onClick={() => find()}>
        <Icon name="arrow-down" size={13} />
      </button>
      <button class="icon-btn" title="Close" onClick={close}>
        <Icon name="x" size={11} />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- view switcher + combined log

function ViewSwitcher() {
  const pid = () => state.ui.projectId!;
  const plan = () => planFor(pid());
  const g = () => selectedGroup(pid());
  const n = () => scopeScripts(pid()).length;
  const opt = (view: typeof state.ui.view, icon: "terminal" | "graph" | "list", label: string) => (
    <button class="seg-opt" aria-pressed={state.ui.view === view} onClick={() => setView(view)}>
      <Icon name={icon} size={13} />
      {label}
    </button>
  );

  return (
    <div class="viewswitcher">
      <div class="seg neutral">
        {opt("terminals", "terminal", "Terminals")}
        {opt("deps", "graph", "Dependencies")}
        {opt("log", "list", "Combined log")}
      </div>
      <div class="grow" />
      <Show when={plan().cycle}>
        <span class="t-caption-11 c-crashed">cycle · won't start</span>
        <span class="t-caption-11 c-muted">·</span>
      </Show>
      <span class="t-caption-11 c-muted nowrap">
        {g() ? `Group “${g()!.name}”` : "All scripts"} · {n()} {n() === 1 ? "script" : "scripts"} · {plan().waves.length}{" "}
        {plan().waves.length === 1 ? "wave" : "waves"}
      </span>
    </div>
  );
}

function CombinedLog() {
  let stack!: HTMLDivElement;
  const pid = () => state.ui.projectId!;

  onMount(() => {
    const scripts = scopeScripts(pid());
    setCombinedNames(scripts.map((s) => ({ id: s.id, name: s.name })));
    mountTerminal(COMBINED_ID, stack, true);
    seedCombined(scripts.map((s) => s.id));
    const ro = new ResizeObserver(() => fitTerminal(COMBINED_ID));
    ro.observe(stack);
    onCleanup(() => ro.disconnect());
  });

  return (
    <section class="terminal-pane">
      <div class="term-stack" ref={stack} />
    </section>
  );
}

// ---------------------------------------------------------------- status bar

function StatusBar() {
  const pid = () => state.ui.projectId!;
  const counts = () => countsOf(pid());
  const active = () => activeScriptId();
  const run = () => (active() ? runOf(active()!) : undefined);
  const stats = () => (active() ? state.stats[active()!] : undefined);
  const shell = () => {
    const s = active() ? script(active()!) : undefined;
    const cmd = (s?.shell ?? state.settings.defaultShell).split(/\s+/)[0];
    return cmd.split(/[\\/]/).pop();
  };

  return (
    <footer class="statusbar">
      <span class="chip">
        <span class="dot" data-state="running" style={{ width: "6px", height: "6px", animation: "none" }} />
        {counts().running} running
      </span>
      <span class="chip">
        <span class="dot" data-state="crashed" style={{ width: "6px", height: "6px" }} />
        {counts().crashed} crashed
      </span>
      <span class="chip">
        <span class="dot" style={{ width: "6px", height: "6px" }} />
        {counts().stopped} stopped
      </span>
      <div class="grow" />
      <Show when={run()?.pid}>
        <span class="meta">PID {run()!.pid}</span>
        <span class="sep">·</span>
        <span class="meta">CPU {stats() ? `${stats()!.cpu.toFixed(1)}%` : "—"}</span>
        <span class="sep">·</span>
        <span class="meta">MEM {stats() ? fmtBytes(stats()!.memBytes) : "—"}</span>
        <span class="sep">·</span>
      </Show>
      <span class="meta">{shell()}</span>
    </footer>
  );
}
