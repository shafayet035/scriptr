import { createMemo, For, Show } from "solid-js";
import { Icon } from "../../components/Icon";
import type { Task, TaskStatus } from "../../lib/types";
import { openComposer, state, tasksOf } from "../../store/app";
import { TaskCard } from "./TaskCard";

/** Five columns, because the lifecycle has five moments a human cares about. */
export const COLUMNS: { id: string; title: string; statuses: TaskStatus[] }[] = [
  { id: "backlog", title: "Backlog", statuses: ["backlog", "cancelled"] },
  { id: "ready", title: "Ready", statuses: ["queued"] },
  { id: "working", title: "Working", statuses: ["working", "verifying", "publishing"] },
  { id: "review", title: "Review", statuses: ["review", "failed"] },
  { id: "done", title: "Done", statuses: ["done"] },
];

export function BoardCanvas() {
  const pid = () => state.ui.projectId!;
  const all = createMemo(() => tasksOf(pid()));

  const inColumn = (statuses: TaskStatus[]) =>
    all()
      .filter((t) => statuses.includes(t.status))
      // Failures first — they are the ones asking for a human.
      .sort((a, b) => Number(b.status === "failed") - Number(a.status === "failed"));

  return (
    <div class="board scroll-y">
      <Show when={all().length > 0} fallback={<BoardEmpty />}>
        <For each={COLUMNS}>
          {(col) => {
            const tasks = createMemo(() => inColumn(col.statuses));
            const failed = createMemo(() => tasks().filter((t) => t.status === "failed").length);
            return (
              <section class="board-col" data-col={col.id}>
                <header class="board-col-head">
                  <span class="t-label-caps c-muted">{col.title}</span>
                  <span class="board-count">{tasks().length}</span>
                  <Show when={failed() > 0}>
                    <span class="board-count failed">{failed()} failed</span>
                  </Show>
                  <div class="grow" />
                  <Show when={col.id === "backlog"}>
                    <button class="icon-btn" title="New task" onClick={() => openComposer("")}>
                      <Icon name="plus" size={12} />
                    </button>
                  </Show>
                </header>
                <div class="board-col-body scroll-y">
                  <For each={tasks()}>{(t: Task) => <TaskCard task={t} />}</For>
                </div>
              </section>
            );
          }}
        </For>
      </Show>
    </div>
  );
}

function BoardEmpty() {
  return (
    <div class="board-empty">
      <Icon name="layers" size={26} color="var(--border-strong)" />
      <p class="t-title-13 c-secondary">No tasks yet</p>
      <p class="t-caption-11 c-muted" style={{ "max-width": "380px", "text-align": "center" }}>
        A task is a goal you hand to a coding agent. It runs here in a real terminal, and lands in Review when it's
        done so you can read the diff before anything is committed.
      </p>
      <button class="btn btn-primary" onClick={() => openComposer("")}>
        <Icon name="plus" size={12} />
        New task
      </button>
    </div>
  );
}
