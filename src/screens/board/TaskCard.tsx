import { Show } from "solid-js";
import { openUrl } from "../../lib/ipc";
import { Icon } from "../../components/Icon";
import { popupMenu } from "../../lib/menu";
import { fmtCost, fmtUptime, taskTone } from "../../lib/format";
import type { Task } from "../../lib/types";
import {
  agent,
  now,
  openComposer,
  openTask,
  selectCard,
  showInDeck,
  startTask,
  state,
  stopTask,
  taskRunOf,
} from "../../store/app";
import { isWorking, taskMenu } from "./taskMenu";

/**
 * One task. The live line is the point: agent, model, how long, how many turns,
 * what it cost — the things you need to decide whether to intervene.
 */
export function TaskCard(props: { task: Task }) {
  const t = () => props.task;
  const run = () => taskRunOf(t().id);
  const a = () => agent(t().agentId);
  const selected = () => state.ui.boardSelection === t().id;

  const elapsed = () => {
    const r = run();
    if (!r?.startedAt) return null;
    return fmtUptime((r.endedAt ?? now()) - r.startedAt);
  };

  const live = () => {
    const parts = [a()?.name ?? t().agentId];
    if (t().model) parts.push(t().model!);
    if (t().effort) parts.push(`${t().effort} effort`);
    const e = elapsed();
    if (e) parts.push(e);
    if (run()?.turns) parts.push(`${run()!.turns}t`);
    const cost = fmtCost(run()?.costUsd ?? null);
    if (cost) parts.push(cost);
    return parts.join(" · ");
  };

  return (
    <article
      class="task-card"
      data-tone={taskTone(t().status)}
      data-selected={selected()}
      tabIndex={0}
      onClick={() => {
        selectCard(t().id);
        // Loading the terminal must not yank the board out from under the click.
        showInDeck(t().id, "load");
      }}
      onDblClick={() => openTask(t().id)}
      onContextMenu={(e) => popupMenu(taskMenu(t()), e)}
      onKeyDown={(e) => {
        if (e.key === " ") {
          e.preventDefault();
          selectCard(t().id);
          showInDeck(t().id, "raise");
        }
        if (e.key === "Enter") {
          selectCard(t().id);
          showInDeck(t().id, "max");
        }
        if (e.key === "e") openComposer(t().id);
      }}
    >
      <header class="tc-head">
        <span class="dot" data-task={taskTone(t().status)} style={{ width: "7px", height: "7px" }} />
        <h3 class="tc-title">{t().title || "Untitled task"}</h3>
        <Show when={t().priority >= 3}>
          <span class="tc-prio" title="High priority">
            !
          </span>
        </Show>
      </header>

      <p class="tc-live">{live()}</p>

      <Show when={t().status === "failed" && run()?.exitCode != null}>
        <p class="tc-fail">exit {run()!.exitCode}</p>
      </Show>

      <Show when={t().labels.length > 0 || t().branch || t().prUrl}>
        <div class="tc-chips">
          <Show when={t().prUrl}>
            {/* The one chip that is a destination: this is what you merge. */}
            <button
              class="tc-chip tc-chip-pr"
              title={t().prUrl!}
              onClick={(e) => (e.stopPropagation(), void openUrl(t().prUrl!))}
            >
              <Icon name="branch" size={10} />
              PR #{t().prNumber ?? "?"}
            </button>
          </Show>
          <Show when={t().branch}>
            {/* The `scriptr/` prefix is on every one of them; the title is the news. */}
            <span class="tc-chip" title={t().branch!}>
              <Icon name="branch" size={10} />
              {t().branch!.replace(/^scriptr\//, "")}
            </span>
          </Show>
          {t().labels.map((l) => (
            <span class="tc-chip">{l}</span>
          ))}
        </div>
      </Show>

      <div class="tc-actions">
        <Show
          when={isWorking(t())}
          fallback={
            <button class="icon-btn" title="Run agent" onClick={(e) => (e.stopPropagation(), void startTask(t().id))}>
              <Icon name="play" size={12} color="var(--status-running)" />
            </button>
          }
        >
          <button class="icon-btn" title="Stop agent" onClick={(e) => (e.stopPropagation(), void stopTask(t().id))}>
            <Icon name="stop" size={12} color="var(--status-crashed)" />
          </button>
        </Show>
        <button class="icon-btn" title="Show terminal" onClick={(e) => (e.stopPropagation(), openTask(t().id))}>
          <Icon name="terminal" size={12} />
        </button>
        <button class="icon-btn" title="Edit task" onClick={(e) => (e.stopPropagation(), openComposer(t().id))}>
          <Icon name="settings" size={12} />
        </button>
      </div>
    </article>
  );
}
