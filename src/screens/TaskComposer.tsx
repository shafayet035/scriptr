import { createStore, unwrap } from "solid-js/store";
import { onCleanup, onMount, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { PRIORITY_LABEL, TASK_STATUS_LABEL } from "../lib/format";
import type { Task, TaskStatus } from "../lib/types";
import { closeComposer, currentProject, newTaskDraft, openComposer, saveTask, task, tasksOf } from "../store/app";

const COLUMNS: TaskStatus[] = ["backlog", "todo", "doing", "review", "done"];

/** Compose or edit a board card. */
export function TaskComposer(props: { taskId: string }) {
  const editing = () => (props.taskId ? task(props.taskId) : undefined);
  const project = () => currentProject()!;
  const [draft, setDraft] = createStore<Task>(
    editing() ? structuredClone(unwrap(editing()!)) : newTaskDraft(project().id),
  );

  const valid = () => draft.title.trim().length > 0;

  const statusMenu = (e: MouseEvent) =>
    popupMenu(
      COLUMNS.map((s) => ({
        label: TASK_STATUS_LABEL[s],
        checked: draft.status === s,
        action: () => setDraft("status", s),
      })),
      e.currentTarget as HTMLElement,
    );

  const priorityMenu = (e: MouseEvent) =>
    popupMenu(
      PRIORITY_LABEL.map((label, i) => ({ label, checked: draft.priority === i, action: () => setDraft("priority", i) })),
      e.currentTarget as HTMLElement,
    );

  /** Any other card in this project can block this one. */
  const blockerMenu = (e: MouseEvent) =>
    popupMenu(
      tasksOf(project().id)
        .filter((t) => t.id !== draft.id)
        .map((t) => ({
          label: t.title || "Untitled",
          checked: draft.after.includes(t.id),
          action: () =>
            setDraft("after", (list) => (list.includes(t.id) ? list.filter((x) => x !== t.id) : [...list, t.id])),
        })),
      e.currentTarget as HTMLElement,
    );

  const save = async () => {
    if (!valid()) return;
    await saveTask({ ...structuredClone(unwrap(draft)), title: draft.title.trim(), goal: draft.goal.trim() });
    closeComposer();
  };

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeComposer();
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void save();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const blockerNames = () => draft.after.map((id) => task(id)?.title ?? id);

  return (
    <>
      <div class="scrim light" onClick={closeComposer} />
      <div class="dialog composer" role="dialog" aria-label={editing() ? "Edit card" : "New card"}>
        <div class="modal-header" style={{ gap: "12px" }}>
          <p class="t-display-20 c-primary">{editing() ? "Edit card" : "New card"}</p>
        </div>

        <div class="composer-body scroll-y">
          <div class="field">
            <span class="field-label">Title</span>
            <label class="input">
              <input
                type="text"
                value={draft.title}
                placeholder="Rate-limit the checkout endpoint"
                autofocus
                onInput={(e) => setDraft("title", e.currentTarget.value)}
              />
            </label>
          </div>

          <div class="field">
            <span class="field-label">Detail</span>
            <label class="input">
              <textarea
                rows={5}
                value={draft.goal}
                placeholder="Why this card exists, and what finishing it looks like."
                onInput={(e) => setDraft("goal", e.currentTarget.value)}
              />
            </label>
          </div>

          <div class="composer-row">
            <div class="field grow">
              <span class="field-label">Column</span>
              <button class="select" onClick={statusMenu}>
                {TASK_STATUS_LABEL[draft.status]}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
            <div class="field" style={{ width: "140px" }}>
              <span class="field-label">Priority</span>
              <button class="select" onClick={priorityMenu}>
                {PRIORITY_LABEL[draft.priority] ?? "None"}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
          </div>

          <div class="composer-row">
            <div class="field grow">
              <span class="field-label">Assignee</span>
              <label class="input">
                <input
                  type="text"
                  value={draft.assignee ?? ""}
                  placeholder="me"
                  onInput={(e) => setDraft("assignee", e.currentTarget.value.trim() || null)}
                />
              </label>
            </div>
            <div class="field grow">
              <span class="field-label">Linked issue</span>
              <label class="input">
                <input
                  type="text"
                  value={draft.issueUrl ?? ""}
                  placeholder="https://github.com/…/issues/1"
                  onInput={(e) => setDraft("issueUrl", e.currentTarget.value.trim() || null)}
                />
              </label>
            </div>
          </div>

          <div class="field">
            <span class="field-label">Labels</span>
            <label class="input">
              <input
                type="text"
                value={draft.labels.join(", ")}
                placeholder="bug, ci"
                onInput={(e) =>
                  setDraft(
                    "labels",
                    e.currentTarget.value
                      .split(",")
                      .map((l) => l.trim())
                      .filter(Boolean),
                  )
                }
              />
            </label>
          </div>

          <Show when={tasksOf(project().id).some((t) => t.id !== draft.id)}>
            <div class="field">
              <span class="field-label">Blocked by</span>
              <button class="select" onClick={blockerMenu}>
                {blockerNames().length > 0 ? blockerNames().join(", ") : "Nothing"}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
          </Show>
        </div>

        <div class="modal-footer">
          <Show when={editing()}>
            <button class="btn btn-secondary btn-sm" onClick={() => openComposer("")}>
              New instead
            </button>
          </Show>
          <div class="grow" />
          <button class="btn btn-secondary" style={{ padding: "9px 14px", "border-radius": "8px" }} onClick={closeComposer}>
            Cancel
          </button>
          <button
            class="btn btn-primary"
            style={{ padding: "9px 16px", "border-radius": "8px" }}
            disabled={!valid()}
            onClick={() => void save()}
          >
            Save
          </button>
        </div>
      </div>
    </>
  );
}
