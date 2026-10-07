import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { Icon } from "../../components/Icon";
import { fitTerminal, mountTerminal, taskKey, terminalFor } from "../../lib/terminals";
import { fmtUptime, taskTone } from "../../lib/format";
import {
  DECK_RAIL,
  deckHeight,
  now,
  pinTaskToTab,
  setDeckHeight,
  setDeckState,
  showInDeck,
  state,
  task,
  taskRunOf,
  tasksOf,
} from "../../store/app";
import { isWorking } from "./taskMenu";
import { TaskBar } from "./TaskBar";

/**
 * The live agent, docked under the board. A 36px rail while nothing needs you,
 * 320px once an agent is working or you pick a card, full height on Enter —
 * so a card and its PTY are the same object, not two places.
 */
export function AgentDeck() {
  let host!: HTMLDivElement;
  const pid = () => state.ui.projectId!;
  const live = () => tasksOf(pid()).filter(isWorking);
  const shown = () => {
    const explicit = task(state.ui.deckTaskId);
    if (explicit && explicit.projectId === pid()) return explicit;
    return live()[0] ?? null;
  };
  /** Tasks worth a chip: everything live, plus whatever you're looking at. */
  const chips = () => {
    const out = [...live()];
    const s = shown();
    if (s && !out.some((t) => t.id === s.id)) out.unshift(s);
    return out;
  };
  const open = () => state.ui.deckState !== "rail";

  // The deck owns one terminal host at a time; the rest stay alive, hidden.
  createEffect(() => {
    const t = shown();
    if (!t || !open()) return;
    const key = taskKey(t.id);
    mountTerminal(key, host).host.hidden = false;
    for (const el of Array.from(host.querySelectorAll<HTMLElement>(".term-host"))) {
      el.hidden = el.dataset.scriptId !== key;
    }
    requestAnimationFrame(() => fitTerminal(key));
  });

  createEffect(
    on(
      () => [state.ui.deckState, state.ui.deckHeight] as const,
      () => {
        const t = shown();
        if (t && open()) requestAnimationFrame(() => fitTerminal(taskKey(t.id)));
      },
    ),
  );

  onMount(() => {
    const ro = new ResizeObserver(() => {
      const t = shown();
      if (t && open()) fitTerminal(taskKey(t.id));
    });
    ro.observe(host);
    onCleanup(() => ro.disconnect());
  });

  const focusTerminal = () => {
    const t = shown();
    if (t) terminalFor(taskKey(t.id)).term.focus();
  };

  return (
    <section
      class="deck"
      data-state={state.ui.deckState}
      style={{ height: open() ? `${Math.min(deckHeight(), window.innerHeight * 0.7)}px` : `${DECK_RAIL}px` }}
    >
      <DeckResizer />

      <Show when={open()} fallback={<DeckRail live={live()} />}>
        <header class="deck-tabs">
          <For each={chips()}>
            {(t) => (
              <button
                class="deck-chip"
                aria-selected={shown()?.id === t.id}
                data-tone={taskTone(t.status)}
                onClick={() => showInDeck(t.id, "load")}
              >
                <span class="dot" data-task={taskTone(t.status)} style={{ width: "6px", height: "6px" }} />
                <span class="ellipsis">{t.title}</span>
              </button>
            )}
          </For>
          <div class="grow" />
          <Show when={shown()}>
            <button class="icon-btn" title="Open in Terminals as a tab" onClick={() => pinTaskToTab(shown()!.id)}>
              <Icon name="split" size={13} />
            </button>
          </Show>
          <button
            class="icon-btn"
            title={state.ui.deckState === "max" ? "Restore" : "Maximize"}
            onClick={() => setDeckState(state.ui.deckState === "max" ? "open" : "max")}
          >
            <Icon name={state.ui.deckState === "max" ? "arrow-down" : "arrow-up"} size={13} />
          </button>
          <button class="icon-btn" title="Collapse to rail (⌘J)" onClick={() => setDeckState("rail")}>
            <Icon name="x" size={12} />
          </button>
        </header>

        <Show when={shown()}>
          <TaskBar taskId={shown()!.id} />
        </Show>
      </Show>

      <div class="deck-body" ref={host} hidden={!open()} onClick={focusTerminal}>
        <Show when={open() && shown() && !taskRunOf(shown()!.id)}>
          <div class="term-empty">
            <p class="t-caption-11">This task hasn't run yet — press Run to start the agent.</p>
          </div>
        </Show>
      </div>
    </section>
  );
}

/** Collapsed: one line per working agent, so the board still tells you what's alive. */
function DeckRail(props: { live: ReturnType<typeof tasksOf> }) {
  return (
    <div class="deck-rail" onClick={() => setDeckState("open")} title="Show the agent terminal (⌘J)">
      <Show
        when={props.live.length > 0}
        fallback={<span class="t-caption-11 c-muted">No agent running — pick a card to see its terminal</span>}
      >
        <For each={props.live}>
          {(t) => {
            const run = () => taskRunOf(t.id);
            return (
              <span class="deck-rail-item">
                <span class="dot" data-task={taskTone(t.status)} style={{ width: "6px", height: "6px" }} />
                <span class="ellipsis" style={{ "max-width": "180px" }}>
                  {t.title}
                </span>
                <Show when={run()?.startedAt}>
                  <span class="t-caption-11 c-muted">{fmtUptime(now() - run()!.startedAt!)}</span>
                </Show>
              </span>
            );
          }}
        </For>
      </Show>
      <div class="grow" />
      <Icon name="chevron-down" size={12} color="var(--text-muted)" style={{ transform: "rotate(180deg)" }} />
    </div>
  );
}

/** Drag the deck's top edge; a row-axis twin of the sidebar resizer. */
function DeckResizer() {
  const [dragging, setDragging] = createSignal(false);
  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const startY = e.clientY;
    const startH = deckHeight();
    setDragging(true);
    if (state.ui.deckState === "rail") setDeckState("open");
    document.documentElement.classList.add("row-resizing");
    const move = (ev: PointerEvent) => setDeckHeight(startH + (startY - ev.clientY));
    const up = () => {
      setDragging(false);
      document.documentElement.classList.remove("row-resizing");
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  return (
    <div
      class="deck-resizer"
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize the agent deck"
      data-dragging={dragging()}
      title="Drag to resize · double-click to collapse"
      onPointerDown={onPointerDown}
      onDblClick={() => setDeckState(state.ui.deckState === "rail" ? "open" : "rail")}
    />
  );
}
