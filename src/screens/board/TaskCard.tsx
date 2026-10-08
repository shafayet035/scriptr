import { Show } from "solid-js";
import { Icon } from "../../components/Icon";
import { openUrl } from "../../lib/ipc";
import { popupMenu } from "../../lib/menu";
import { taskTone } from "../../lib/format";
import type { Task } from "../../lib/types";
import { openComposer, selectCard, state, task } from "../../store/app";
import { taskMenu } from "./taskMenu";

/** One card. Title first, then only what helps you choose what to pick up. */
export function TaskCard(props: { task: Task; onDragStart?: (e: DragEvent) => void }) {
  const t = () => props.task;
  const selected = () => state.ui.boardSelection === t().id;

  /** Blockers are shown by title: an id tells you nothing at a glance. */
  const blockers = () => t().after.map((id) => task(id)?.title ?? id);

  return (
    <article
      class="task-card"
      data-tone={taskTone(t().status)}
      data-selected={selected()}
      tabIndex={0}
      draggable
      onDragStart={(e) => props.onDragStart?.(e)}
      onClick={() => selectCard(t().id)}
      onDblClick={() => openComposer(t().id)}
      onContextMenu={(e) => popupMenu(taskMenu(t()), e)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === "e") {
          e.preventDefault();
          openComposer(t().id);
        }
      }}
    >
      <header class="tc-head">
        <span class="dot" data-task={taskTone(t().status)} style={{ width: "7px", height: "7px" }} />
        <h3 class="tc-title">{t().title || "Untitled card"}</h3>
        <Show when={t().priority >= 3}>
          <span class="tc-prio" title="High priority">
            !
          </span>
        </Show>
      </header>

      <Show when={t().goal.trim()}>
        <p class="tc-goal">{t().goal}</p>
      </Show>

      <Show when={blockers().length > 0}>
        <p class="tc-blocked" title={blockers().join(", ")}>
          Blocked by {blockers().join(", ")}
        </p>
      </Show>

      <Show when={t().labels.length > 0 || t().assignee || t().issueUrl}>
        <div class="tc-chips">
          <Show when={t().assignee}>
            <span class="tc-chip">{t().assignee}</span>
          </Show>
          <Show when={t().issueUrl}>
            <button
              class="tc-chip tc-chip-link"
              title={t().issueUrl!}
              onClick={(e) => (e.stopPropagation(), void openUrl(t().issueUrl!))}
            >
              <Icon name="branch" size={10} />
              Issue
            </button>
          </Show>
          {t().labels.map((l) => (
            <span class="tc-chip">{l}</span>
          ))}
        </div>
      </Show>

      <div class="tc-actions">
        <button class="icon-btn" title="Edit card" onClick={(e) => (e.stopPropagation(), openComposer(t().id))}>
          <Icon name="settings" size={12} />
        </button>
      </div>
    </article>
  );
}
