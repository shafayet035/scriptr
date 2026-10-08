import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "../../components/Icon";
import { TASK_STATUS_LABEL } from "../../lib/format";
import type { Task, TaskStatus } from "../../lib/types";
import { moveTask, openComposer, state, tasksOf } from "../../store/app";
import { TaskCard } from "./TaskCard";

export const COLUMNS: TaskStatus[] = ["backlog", "todo", "doing", "review", "done"];

/** The card being dragged, so a drop knows what to move. */
const [dragging, setDragging] = createSignal<string | null>(null);

export function BoardCanvas() {
  const pid = () => state.ui.projectId!;
  const all = createMemo(() => tasksOf(pid()));
  // The card a drop would land above; null means the end of the column.
  const [overColumn, setOverColumn] = createSignal<TaskStatus | null>(null);
  const [overCard, setOverCard] = createSignal<string | null>(null);

  const drop = (status: TaskStatus) => {
    const id = dragging();
    setDragging(null);
    setOverColumn(null);
    const before = overCard();
    setOverCard(null);
    if (id) void moveTask(id, status, before);
  };

  return (
    <div class="board scroll-y">
      <Show when={all().length > 0} fallback={<BoardEmpty />}>
        <For each={COLUMNS}>
          {(status) => {
            const tasks = createMemo(() => all().filter((t) => t.status === status));
            return (
              <section
                class="board-col"
                data-col={status}
                data-over={overColumn() === status}
                onDragOver={(e) => {
                  // Without preventDefault the browser refuses the drop.
                  e.preventDefault();
                  setOverColumn(status);
                }}
                onDragLeave={() => overColumn() === status && setOverColumn(null)}
                onDrop={(e) => (e.preventDefault(), drop(status))}
              >
                <header class="board-col-head">
                  <span class="t-label-caps c-muted">{TASK_STATUS_LABEL[status]}</span>
                  <span class="board-count">{tasks().length}</span>
                  <div class="grow" />
                  <Show when={status === "backlog"}>
                    <button class="icon-btn" title="New card" onClick={() => openComposer("")}>
                      <Icon name="plus" size={12} />
                    </button>
                  </Show>
                </header>
                <div class="board-col-body scroll-y">
                  <For each={tasks()}>
                    {(t: Task) => (
                      <div
                        class="board-slot"
                        data-before={overCard() === t.id}
                        onDragOver={(e) => (e.stopPropagation(), setOverCard(t.id))}
                      >
                        <TaskCard
                          task={t}
                          onDragStart={(e) => {
                            setDragging(t.id);
                            if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                          }}
                        />
                      </div>
                    )}
                  </For>
                  {/* A tail target, so a column's end is droppable even when full. */}
                  <div class="board-tail" onDragOver={(e) => (e.stopPropagation(), setOverCard(null))} />
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
      <p class="t-title-13 c-secondary">No cards yet</p>
      <p class="t-caption-11 c-muted" style={{ "max-width": "380px", "text-align": "center" }}>
        Add one here, or let an AI client file it over MCP — Scriptr's board is readable and writable from Claude.
      </p>
      <button class="btn btn-primary" onClick={() => openComposer("")}>
        <Icon name="plus" size={12} />
        New card
      </button>
    </div>
  );
}
