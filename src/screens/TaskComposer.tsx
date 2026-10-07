import { createMemo, For, onCleanup, onMount, Show } from "solid-js";
import { createStore, unwrap } from "solid-js/store";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { AUTONOMY_LABEL, PRIORITY_LABEL, tildify } from "../lib/format";
import type { Autonomy, Task } from "../lib/types";
import { agent, closeComposer, currentProject, newTaskDraft, openComposer, saveTask, startTask, state, task } from "../store/app";

const AUTONOMY: Autonomy[] = ["ask", "auto-edit", "full"];

/** Compose or edit an agent task (docs/AI-PM.md slice A). */
export function TaskComposer(props: { taskId: string }) {
  const editing = () => (props.taskId ? task(props.taskId) : undefined);
  const project = () => currentProject()!;
  const [draft, setDraft] = createStore<Task>(
    editing() ? structuredClone(unwrap(editing()!)) : newTaskDraft(project().id),
  );

  const chosen = () => agent(draft.agentId);
  const valid = () => draft.title.trim().length > 0 && draft.goal.trim().length > 0 && !!chosen()?.available;

  const agentMenu = (e: MouseEvent) =>
    popupMenu(
      state.agents.map((a) => ({
        label: a.available ? `${a.name}${a.version ? `  ${a.version}` : ""}` : `${a.name} — not installed`,
        checked: a.id === draft.agentId,
        enabled: a.available,
        action: () => {
          setDraft("agentId", a.id);
          setDraft("model", null);
          setDraft("assignee", `agent:${a.id}`);
        },
      })),
      e.currentTarget as HTMLElement,
    );

  const modelMenu = (e: MouseEvent) =>
    popupMenu(
      [
        { label: "Agent default", checked: !draft.model, action: () => setDraft("model", null) },
        ...(chosen()?.models ?? []).map((m) => ({ label: m, checked: draft.model === m, action: () => setDraft("model", m) })),
      ],
      e.currentTarget as HTMLElement,
    );

  const priorityMenu = (e: MouseEvent) =>
    popupMenu(
      PRIORITY_LABEL.map((label, i) => ({ label, checked: draft.priority === i, action: () => setDraft("priority", i) })),
      e.currentTarget as HTMLElement,
    );

  const save = async (andRun: boolean) => {
    if (!valid()) return;
    const saved = await saveTask({ ...structuredClone(unwrap(draft)), title: draft.title.trim(), goal: draft.goal.trim() });
    closeComposer();
    if (saved && andRun) void startTask(saved.id);
  };

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeComposer();
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void save(true);
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const missing = createMemo(() => state.agents.filter((a) => !a.available).map((a) => a.name));

  return (
    <>
      <div class="scrim light" onClick={closeComposer} />
      <div class="dialog composer" role="dialog" aria-label={editing() ? "Edit task" : "New task"}>
        <div class="modal-header" style={{ gap: "12px" }}>
          <p class="t-display-20 c-primary">{editing() ? "Edit task" : "New task"}</p>
          <p class="t-caption-11 c-muted">
            The agent runs in <span class="t-mono-11">{tildify(project().path)}</span> and edits files directly — worktree
            isolation lands in the next slice.
          </p>
        </div>

        <div class="composer-body scroll-y">
          <div class="field">
            <span class="field-label">Title</span>
            <label class="input">
              <input
                ref={(el) => queueMicrotask(() => el.focus())}
                placeholder="Rate-limit the checkout endpoint"
                value={draft.title}
                onInput={(e) => setDraft("title", e.currentTarget.value)}
              />
            </label>
          </div>

          <div class="field">
            <span class="field-label">Goal</span>
            <label class="input">
              <textarea
                rows={6}
                placeholder="What should the agent do? Be specific about the acceptance criteria — this is the prompt."
                value={draft.goal}
                onInput={(e) => setDraft("goal", e.currentTarget.value)}
              />
            </label>
          </div>

          <div class="composer-row">
            <div class="field grow">
              <span class="field-label">Agent</span>
              <button class="select" onClick={agentMenu}>
                <Icon name="terminal" size={13} color="var(--text-secondary)" />
                {chosen()?.name ?? "Pick an agent"}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
            <div class="field grow">
              <span class="field-label">Model</span>
              <button class="select" onClick={modelMenu} disabled={!chosen()}>
                {draft.model ?? "Agent default"}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
            <div class="field" style={{ width: "120px" }}>
              <span class="field-label">Priority</span>
              <button class="select" onClick={priorityMenu}>
                {PRIORITY_LABEL[draft.priority] ?? "None"}
                <div class="grow" />
                <Icon name="chevron-down" size={12} color="var(--text-muted)" />
              </button>
            </div>
          </div>

          <div class="field">
            <span class="field-label">Autonomy</span>
            <div class="seg bordered">
              <For each={AUTONOMY}>
                {(a) => (
                  <button class="seg-opt" aria-pressed={draft.autonomy === a} onClick={() => setDraft("autonomy", a)}>
                    {AUTONOMY_LABEL[a]}
                  </button>
                )}
              </For>
            </div>
            <p class="t-caption-11 c-muted">
              {draft.autonomy === "ask"
                ? "You answer the agent's permission prompts in the terminal."
                : draft.autonomy === "auto-edit"
                  ? "File edits are accepted automatically; shell commands still ask."
                  : "The agent runs without asking. Use it only on work you can throw away."}
            </p>
          </div>

          <Show when={missing().length > 0}>
            <p class="t-caption-11 c-muted">Not installed: {missing().join(", ")}.</p>
          </Show>
        </div>

        <div class="modal-footer">
          <Show when={editing()} fallback={<span class="t-caption-11 c-muted">Saved to the backlog until you run it.</span>}>
            <button class="btn btn-secondary btn-sm" onClick={() => openComposer("")}>
              New instead
            </button>
          </Show>
          <div class="grow" />
          <button class="btn btn-secondary" style={{ padding: "9px 14px", "border-radius": "8px" }} onClick={closeComposer}>
            Cancel
          </button>
          <button class="btn btn-secondary" style={{ padding: "9px 14px", "border-radius": "8px" }} disabled={!valid()} onClick={() => void save(false)}>
            Save
          </button>
          <button class="btn btn-primary" style={{ padding: "9px 16px", "border-radius": "8px" }} disabled={!valid()} onClick={() => void save(true)}>
            <Icon name="play" size={12} />
            Save and run
          </button>
        </div>
      </div>
    </>
  );
}
