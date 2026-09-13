import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { gateDescription, gateShort, shortCmd, tone } from "../lib/format";
import { wouldCycle } from "../lib/graph";
import type { Plan, RunInfo, Script } from "../lib/types";
import {
  openInspector,
  openScript,
  planFor,
  runOf,
  saveScript,
  scopeScripts,
  script,
  scriptsOf,
  state,
  toast,
} from "../store/app";

const NODE_W = 156;
const NODE_H = 83;
const COL = 176;
const X0 = 24;
const TOP = 80;

interface Layout {
  pos: Record<string, { x: number; y: number }>;
  columns: number;
  isolatedTop: number | null;
  width: number;
  height: number;
}

/** Waves become columns; nodes with no edges in this scope sit in a row underneath. */
function layout(scripts: Script[], plan: Plan): Layout {
  const inScope = new Set(scripts.map((s) => s.id));
  const connected = (id: string) =>
    script(id)!.after.some((d) => inScope.has(d)) || scripts.some((o) => o.after.includes(id));

  const placedByPlan = new Set(plan.waves.flat());
  const columns = [...plan.waves];
  const unresolved = scripts.map((s) => s.id).filter((id) => !placedByPlan.has(id));
  if (unresolved.length) columns.push(unresolved);

  const pos: Layout["pos"] = {};
  const isolated: string[] = [];
  columns.forEach((col, ci) => {
    const conn = col.filter((id) => connected(id) || unresolved.includes(id));
    isolated.push(...col.filter((id) => !conn.includes(id)));
    conn.forEach((id, i) => {
      pos[id] = { x: X0 + ci * COL, y: 120 - 40 * (conn.length - 1) + 120 * i };
    });
  });

  const tops = Object.values(pos).map((p) => p.y);
  const shift = tops.length ? Math.max(0, TOP - Math.min(...tops)) : 0;
  for (const p of Object.values(pos)) p.y += shift;
  const bottom = tops.length ? Math.max(...Object.values(pos).map((p) => p.y)) + NODE_H : 40;

  let isolatedTop: number | null = null;
  if (isolated.length) {
    isolatedTop = Math.max(bottom + 47, TOP);
    isolated.forEach((id, i) => (pos[id] = { x: X0 + i * COL, y: isolatedTop! }));
  }
  const lastRight = X0 + (Math.max(columns.length, isolated.length, 1) - 1) * COL + NODE_W;
  const maxY = isolatedTop !== null ? isolatedTop + NODE_H : bottom;
  return { pos, columns: columns.length, isolatedTop, width: lastRight + 16, height: maxY + 80 };
}

function isReady(s: Script, r: RunInfo) {
  return (r.state === "running" && !r.degraded) || (r.state === "stopped" && r.exitCode === 0 && s.ready.kind === "exit" && !!r.startedAt);
}

function gateLine(s: Script, r: RunInfo): { text: string; tone: string } {
  if (isReady(s, r)) return { text: `ready · ${r.gateNote ?? gateShort(s.ready)}`, tone: "running" };
  switch (r.state) {
    case "running":
      return { text: `degraded · ${gateShort(s.ready)} timed out`, tone: "starting" };
    case "starting":
      return { text: `starting · ${r.gateNote ?? `waiting for ${gateShort(s.ready)}`}`, tone: "starting" };
    case "queued":
      return { text: "queued · waiting on deps", tone: "queued" };
    case "backoff":
    case "crashed":
      return { text: `crashed · exit ${r.exitCode ?? "?"}`, tone: "crashed" };
    case "stopping":
      return { text: "stopping…", tone: "starting" };
    default:
      return { text: `gate · ${gateShort(s.ready)}`, tone: "stopped" };
  }
}

/** Screen 05 — DAG canvas with waves + startup order panel. */
export function DependencyView() {
  const pid = () => state.ui.projectId!;
  const scripts = () => scopeScripts(pid());
  const plan = () => planFor(pid());
  const geo = createMemo(() => layout(scripts(), plan()));

  const [drag, setDrag] = createSignal<{ id: string; dx: number; dy: number; over: string | null } | null>(null);
  const [flash, setFlash] = createSignal<[string, string] | null>(null);

  const edgePath = (from: string, to: string) => {
    const p = geo().pos;
    const x1 = p[from].x + NODE_W;
    const y1 = p[from].y + NODE_H / 2;
    const x2 = p[to].x;
    const y2 = p[to].y + NODE_H / 2;
    const end = x2 - 7;
    const d = x2 > x1 && y1 === y2 ? `M${x1} ${y1}H${end}` : `M${x1} ${y1}C${x1 + 16} ${y1} ${end - 16} ${y2} ${end} ${y2}`;
    const arrow = `M${x2 - 8} ${y2 - 4.5}L${x2 - 1} ${y2}L${x2 - 8} ${y2 + 4.5}Z`;
    return { from, to, d, arrow };
  };

  const edges = () => {
    const p = geo().pos;
    const out = scripts().flatMap((s) => s.after.filter((dep) => p[dep] && p[s.id]).map((dep) => edgePath(dep, s.id)));
    // A rejected link is drawn briefly so the offending edge is visible, not just described.
    const f = flash();
    if (f && p[f[0]] && p[f[1]] && !out.some((e) => e.from === f[0] && e.to === f[1])) out.push(edgePath(f[0], f[1]));
    return out;
  };

  const isCycleEdge = (from: string, to: string) => {
    const c = plan().cycle ?? flash();
    return !!c && c[0] === from && c[1] === to;
  };

  const removeEdge = (from: string, to: string, e: MouseEvent) => {
    const dependent = script(to)!;
    const dependency = script(from)!;
    return popupMenu(
      [
        {
          label: `${dependent.name} no longer waits for ${dependency.name}`,
          action: () => void saveScript({ ...dependent, after: dependent.after.filter((d) => d !== from) }),
        },
      ],
      e,
    );
  };

  const onPointerDown = (id: string, e: PointerEvent) => {
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const sx = e.clientX;
    const sy = e.clientY;
    const move = (ev: PointerEvent) => {
      const hit = document
        .elementsFromPoint(ev.clientX, ev.clientY)
        .map((n) => (n as HTMLElement).closest?.<HTMLElement>(".dag-node"))
        .find((n) => n && n.dataset.id !== id);
      setDrag({ id, dx: ev.clientX - sx, dy: ev.clientY - sy, over: hit?.dataset.id ?? null });
    };
    const up = (ev: PointerEvent) => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      const d = drag();
      setDrag(null);
      const moved = Math.hypot(ev.clientX - sx, ev.clientY - sy) > 4;
      if (!moved) return openInspector(id);
      if (!d?.over) return;
      const dependent = script(id)!;
      if (dependent.after.includes(d.over)) return;
      if (wouldCycle(scriptsOf(pid()), id, d.over)) {
        setFlash([d.over, id]);
        setTimeout(() => setFlash(null), 1600);
        toast(`Rejected: ${script(d.over)!.name} already waits for ${dependent.name} — that would be a cycle`, "error");
        return;
      }
      void saveScript({ ...dependent, after: [...dependent.after, d.over] });
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  return (
    <>
      <div class="dag-canvas scroll-y">
        <div class="dag-inner" style={{ width: `${geo().width}px`, height: `${geo().height}px` }}>
          <For each={Array.from({ length: plan().waves.length + (geo().columns > plan().waves.length ? 1 : 0) })}>
            {(_, i) => (
              <p class="dag-wave t-label-caps" style={{ left: `${X0 + i() * COL}px` }} data-unresolved={i() >= plan().waves.length}>
                {i() >= plan().waves.length ? "In a cycle" : `Wave ${i() + 1}`}
              </p>
            )}
          </For>

          <svg class="dag-edges" width={geo().width} height={geo().height}>
            <For each={edges()}>
              {(edge) => (
                <g class="dag-edge" data-cycle={isCycleEdge(edge.from, edge.to)} onContextMenu={(e) => removeEdge(edge.from, edge.to, e)} onClick={(e) => removeEdge(edge.from, edge.to, e)}>
                  <path d={edge.d} class="hit" />
                  <path d={edge.d} class="line" />
                  <path d={edge.arrow} class="head" />
                </g>
              )}
            </For>
          </svg>

          <For each={scripts()}>
            {(s) => {
              const r = () => runOf(s.id);
              const g = () => gateLine(s, r());
              const p = () => geo().pos[s.id];
              const dragging = () => drag()?.id === s.id;
              return (
                <Show when={p()}>
                  <div
                    class="dag-node"
                    data-id={s.id}
                    data-tone={isReady(s, r()) ? "running" : tone(r().state)}
                    data-dragging={dragging()}
                    data-drop={drag()?.over === s.id}
                    style={{
                      left: `${p()!.x}px`,
                      top: `${p()!.y}px`,
                      transform: dragging() ? `translate(${drag()!.dx}px, ${drag()!.dy}px)` : undefined,
                    }}
                    onPointerDown={(e) => onPointerDown(s.id, e)}
                    onDblClick={() => openScript(s.id)}
                    title="Drag onto another node to make this wait for it · double-click for terminal"
                  >
                    <div class="r">
                      <span class="dot" data-state={isReady(s, r()) ? "running" : r().state} style={{ width: "7px", height: "7px" }} />
                      <span class="t-title-13 c-primary ellipsis">{s.name}</span>
                    </div>
                    <p class="t-mono-11 c-muted ellipsis">{shortCmd(s.cmd)}</p>
                    <p class="t-caption-11 ellipsis gate" data-tone={g().tone}>
                      {g().text}
                    </p>
                  </div>
                </Show>
              );
            }}
          </For>

          <p class="dag-hint t-caption-11 c-muted" style={{ top: `${geo().height - 56}px` }}>
            Drag a node onto another to make it wait for it. Scriptr recomputes the waves and rejects cycles.
          </p>
        </div>
        <Show when={plan().cycle}>
          {(c) => (
            <div class="dag-alert">
              <Icon name="alert" size={15} color="var(--status-crashed)" />
              <span>
                {script(c()[1])?.name} and {script(c()[0])?.name} wait for each other. Remove the red edge to start this group.
              </span>
            </div>
          )}
        </Show>
      </div>
      <StartupOrder />
    </>
  );
}

function StartupOrder() {
  const pid = () => state.ui.projectId!;
  const plan = () => planFor(pid());
  const scripts = () => scopeScripts(pid());
  const g = () => {
    const id = state.ui.groupByProject[pid()];
    return id ? state.progress[id] : undefined;
  };

  const gateText = (wave: string[]) => {
    const gates = wave.map((id) => script(id)!.ready).filter((gate) => gate.kind !== "instant");
    if (gates.length === 0) return gateDescription({ kind: "instant" });
    return [...new Set(gates.map(gateDescription))].join(" · ");
  };

  const coldStart = () => {
    let total = 0;
    for (const wave of plan().waves) {
      const times = wave.map((id) => state.readyMs[id]);
      if (times.some((t) => t === undefined)) return "—";
      total += Math.max(...times);
    }
    return plan().waves.length ? `~${Math.max(1, Math.round(total / 1000))}s` : "—";
  };

  return (
    <aside class="startup">
      <div class="startup-head">
        <p class="t-heading-15 c-primary">Startup order</p>
        <p class="t-caption-11 c-muted">Computed from the dependency graph. Shutdown runs in reverse.</p>
      </div>
      <div class="startup-waves scroll-y">
        <For each={plan().waves}>
          {(wave, i) => (
            <div class="wave-card" data-current={g()?.state === "running" && g()?.wave === i()}>
              <div class="hr">
                <span class="wave-num">{i() + 1}</span>
                <span class="t-title-13 c-primary">Wave {i() + 1}</span>
                <div class="grow" />
                <span class="t-caption-11 c-muted">
                  {wave.length} {wave.length === 1 ? "script" : "scripts"}
                </span>
              </div>
              <div class="items">
                <For each={wave}>
                  {(id) => (
                    <button class="wave-chip" onClick={() => openInspector(id)}>
                      <span class="dot" data-state={runOf(id).state} style={{ width: "6px", height: "6px" }} />
                      {script(id)?.name}
                    </button>
                  )}
                </For>
              </div>
              <p class="t-caption-11 c-muted">Gate: {gateText(wave)}</p>
            </div>
          )}
        </For>
        <Show when={scripts().length === 0}>
          <p class="t-caption-11 c-muted" style={{ padding: "8px 2px" }}>
            This group has no scripts yet.
          </p>
        </Show>
      </div>
      <div class="startup-foot">
        <span class="t-caption-11 c-muted">Typical cold start</span>
        <div class="grow" />
        <span class="t-mono-11 c-secondary">{coldStart()}</span>
      </div>
    </aside>
  );
}
