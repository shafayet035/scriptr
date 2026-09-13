import { createSignal, For, Show } from "solid-js";
import { Icon } from "./Icon";
import { popupMenu } from "../lib/menu";
import { displayName, isLive, tone } from "../lib/format";
import type { Group, Project, Script } from "../lib/types";
import {
  activeScriptId,
  beginAddProject,
  countsOf,
  deleteScript,
  exportToml,
  groupsOf,
  openInspector,
  openScript,
  removeProject,
  restartScript,
  runOf,
  runScope,
  saveGroup,
  saveScript,
  scriptsOf,
  setSidebarWidth,
  SIDEBAR_DEFAULT,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  selectedGroup,
  selectGroup,
  selectProject,
  setFilter,
  setView,
  startScript,
  state,
  stopScope,
  stopScript,
  toggleCollapsed,
} from "../store/app";

export function Sidebar() {
  return (
    <aside class="sidebar" style={{ width: `${state.ui.sidebarWidth}px` }}>
      <SidebarResizer />
      <div class="sb-header">
        <span class="t-label-caps c-muted">Projects</span>
        <div class="grow" />
        <Show when={state.projects.length > 0}>
          <button class="sb-add" onClick={() => void beginAddProject()} title="Add project">
            <Icon name="plus" size={13} />
          </button>
        </Show>
      </div>

      <Show
        when={state.projects.length > 0}
        fallback={
          <Show when={state.loaded}>
            <div class="sb-empty">
              <p class="t-body-12">Nothing here yet.</p>
              <p class="t-caption-11">Projects you add will be listed here with their scripts and groups.</p>
            </div>
          </Show>
        }
      >
        <div class="sb-filter">
          <label class="sb-field">
            <Icon name="search" size={13} />
            <input
              placeholder="Filter scripts…"
              value={state.ui.filter}
              onInput={(e) => setFilter(e.currentTarget.value)}
              onKeyDown={(e) => e.key === "Escape" && setFilter("")}
              spellcheck={false}
            />
          </label>
        </div>
        <div class="sb-list scroll-y">
          <For each={state.projects}>{(p) => <ProjectBlock project={p} />}</For>
        </div>
        <div class="sb-footer">
          <button class="btn btn-secondary" onClick={() => void beginAddProject()}>
            <Icon name="plus" size={13} />
            Add project
          </button>
        </div>
      </Show>
    </aside>
  );
}

/** Drag the sidebar's right edge; double-click resets; arrow keys nudge when focused. */
function SidebarResizer() {
  const [dragging, setDragging] = createSignal(false);

  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = state.ui.sidebarWidth;
    setDragging(true);
    document.documentElement.classList.add("col-resizing");
    const move = (ev: PointerEvent) => setSidebarWidth(startW + ev.clientX - startX);
    const up = () => {
      setDragging(false);
      document.documentElement.classList.remove("col-resizing");
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  return (
    <div
      class="sb-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      aria-valuenow={state.ui.sidebarWidth}
      tabIndex={0}
      data-dragging={dragging()}
      title="Drag to resize · double-click to reset"
      onPointerDown={onPointerDown}
      onDblClick={() => setSidebarWidth(SIDEBAR_DEFAULT)}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") setSidebarWidth(state.ui.sidebarWidth - 16);
        if (e.key === "ArrowRight") setSidebarWidth(state.ui.sidebarWidth + 16);
      }}
    />
  );
}

function ProjectBlock(props: { project: Project }) {
  const p = () => props.project;
  const selected = () => state.ui.projectId === p().id;
  const expanded = () => {
    const c = state.ui.collapsed[p().id];
    return state.ui.filter ? true : c === undefined ? selected() : !c;
  };
  const running = () => countsOf(p().id).running;
  const filtered = () => {
    const q = state.ui.filter.trim().toLowerCase();
    const all = scriptsOf(p().id);
    if (!q) return all;
    return all.filter((s) => `${s.name} ${s.label ?? ""} ${s.cmd}`.toLowerCase().includes(q));
  };

  const onProjectClick = () => {
    if (selected()) toggleCollapsed(p().id);
    else {
      selectProject(p().id);
      if (state.ui.collapsed[p().id]) toggleCollapsed(p().id);
    }
  };

  const projectMenu = (e: MouseEvent) =>
    popupMenu(
      [
        { label: "Run all", action: () => void runScope(p().id) },
        { label: "Stop all", action: () => void stopScope(p().id) },
        { separator: true },
        { label: "Add script", action: () => void addScript(p().id) },
        { label: "New group from running scripts", action: () => void newGroup(p().id) },
        { separator: true },
        { label: "Rescan folder…", action: () => void beginAddProject(p().path) },
        { label: "Export scriptr.toml", action: () => void exportToml(p().id) },
        { separator: true },
        { label: "Remove project", action: () => void removeProject(p().id) },
      ],
      e,
    );

  return (
    <>
      <button class="sb-row project" aria-current={selected()} onClick={onProjectClick} onContextMenu={projectMenu}>
        <span
          class="chev"
          style={{ display: "inline-flex" }}
          onClick={(e) => {
            e.stopPropagation();
            toggleCollapsed(p().id);
          }}
        >
          <Icon name={expanded() ? "chevron-down" : "chevron-right"} size={12} />
        </span>
        <span class="ellipsis">{p().name}</span>
        <div class="grow" />
        <Show when={running() > 0}>
          <span class="pill" data-tone="running">
            {running()} running
          </span>
        </Show>
      </button>

      <Show when={expanded()}>
        <Show when={!state.ui.filter && groupsOf(p().id).length > 0}>
          <div class="sb-section t-label-caps">Groups</div>
          <For each={groupsOf(p().id)}>{(g) => <GroupRow group={g} />}</For>
        </Show>
        <Show when={filtered().length > 0}>
          <div class="sb-section t-label-caps">Scripts</div>
          <For each={filtered()}>{(s) => <ScriptRow script={s} />}</For>
        </Show>
      </Show>
    </>
  );
}

function GroupRow(props: { group: Group }) {
  const g = () => props.group;
  const live = () => g().scriptIds.some((id) => isLive(runOf(id).state));
  const isSelected = () => state.ui.projectId === g().projectId && selectedGroup(g().projectId)?.id === g().id;
  const [renaming, setRenaming] = createSignal(false);

  const run = () => {
    selectGroup(g().projectId, g().id);
    void (live() ? stopScope(g().projectId) : runScope(g().projectId));
  };

  const menu = (e: MouseEvent) =>
    popupMenu(
      [
        { label: "Run group", action: () => (selectGroup(g().projectId, g().id), void runScope(g().projectId)) },
        { label: "Stop group", enabled: live(), action: () => (selectGroup(g().projectId, g().id), void stopScope(g().projectId)) },
        { label: "Show dependencies", action: () => (selectProject(g().projectId), selectGroup(g().projectId, g().id), setView("deps")) },
        { separator: true },
        { label: "Rename", action: () => setRenaming(true) },
      ],
      e,
    );

  return (
    <div
      class="sb-row group"
      role="button"
      aria-current={isSelected()}
      onClick={() => (selectProject(g().projectId), selectGroup(g().projectId, g().id))}
      onDblClick={() => setRenaming(true)}
      onContextMenu={menu}
    >
      <Icon name="layers" size={13} color={isSelected() && live() ? "var(--status-running)" : "var(--text-muted)"} />
      <Show when={renaming()} fallback={<span class="ellipsis">{g().name}</span>}>
        <input
          class="sb-rename"
          value={g().name}
          ref={(el) => queueMicrotask(() => (el.focus(), el.select()))}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") setRenaming(false);
          }}
          onBlur={(e) => {
            const name = e.currentTarget.value.trim();
            setRenaming(false);
            if (name && name !== g().name) void saveGroup({ ...g(), name });
          }}
        />
      </Show>
      <div class="grow" />
      <span class="count">
        {g().scriptIds.length} {g().scriptIds.length === 1 ? "script" : "scripts"}
      </span>
      <RowAction live={live()} onClick={run} />
    </div>
  );
}

function ScriptRow(props: { script: Script }) {
  const s = () => props.script;
  const run = () => runOf(s().id);
  const active = () => state.ui.projectId === s().projectId && activeScriptId() === s().id;

  const menu = (e: MouseEvent) =>
    popupMenu(
      [
        isLive(run().state)
          ? { label: "Stop", action: () => void stopScript(s().id) }
          : { label: "Start", action: () => void startScript(s().id) },
        { label: "Restart", enabled: isLive(run().state), action: () => void restartScript(s().id) },
        { separator: true },
        { label: "Show terminal", action: () => openScript(s().id) },
        { label: "Edit…", action: () => (openScript(s().id), openInspector(s().id)) },
        { label: "Duplicate", action: () => void saveScript({ ...s(), id: "", name: `${s().name}-copy`, sortOrder: s().sortOrder + 1 }) },
        { separator: true },
        { label: "Delete", action: () => void deleteScript(s().id) },
      ],
      e,
    );

  return (
    <div
      class="sb-row script"
      role="button"
      aria-current={active()}
      data-tone={tone(run().state)}
      onClick={() => openScript(s().id)}
      onDblClick={() => openInspector(s().id)}
      onContextMenu={menu}
      title={`${displayName(s())}\n${s().cmd}`}
    >
      <span class="dot" data-state={run().state} style={{ width: "7px", height: "7px" }} />
      <span class="ellipsis">{displayName(s())}</span>
      <div class="grow" />
      <Show when={s().port}>
        <span class="port">:{s().port}</span>
      </Show>
      <RowAction live={isLive(run().state)} onClick={() => void (isLive(run().state) ? stopScript(s().id) : startScript(s().id, false))} />
    </div>
  );
}

function RowAction(props: { live: boolean; onClick: () => void }) {
  return (
    <button
      class="row-action"
      data-kind={props.live ? "stop" : "play"}
      title={props.live ? "Stop" : "Start"}
      onClick={(e) => {
        e.stopPropagation();
        props.onClick();
      }}
      onDblClick={(e) => e.stopPropagation()}
    >
      <Icon name={props.live ? "stop" : "play"} size={12} />
    </button>
  );
}

async function addScript(projectId: string) {
  const saved = await saveScript({
    id: "",
    projectId,
    name: "new-script",
    label: null,
    cmd: "echo hello from scriptr",
    cwd: "./",
    shell: null,
    env: {},
    envFile: null,
    after: [],
    ready: { kind: "instant" },
    restart: { on: "never", max: 5, backoffMs: 2000, backoffMaxMs: 32000 },
    port: null,
    source: null,
    sortOrder: scriptsOf(projectId).length,
  });
  if (saved) {
    openScript(saved.id);
    openInspector(saved.id);
  }
}

async function newGroup(projectId: string) {
  const live = scriptsOf(projectId).filter((s) => isLive(runOf(s.id).state));
  const ids = (live.length ? live : scriptsOf(projectId)).map((s) => s.id);
  const saved = await saveGroup({ id: "", projectId, name: `Group ${groupsOf(projectId).length + 1}`, scriptIds: ids });
  if (saved) selectGroup(projectId, saved.id);
}
