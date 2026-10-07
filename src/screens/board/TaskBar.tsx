import { Show } from "solid-js";
import { Icon } from "../../components/Icon";
import { popupMenu } from "../../lib/menu";
import { fmtCost, fmtUptime, taskTone, TASK_STATUS_LABEL } from "../../lib/format";
import {
  agent,
  now,
  openComposer,
  setTaskStatus,
  startTask,
  stopTask,
  task,
  taskRunOf,
} from "../../store/app";
import { isWorking, taskMenu } from "./taskMenu";

/**
 * One header for a task's terminal, used by the deck and by a pinned tab — so a
 * card and its terminal can never disagree about status, turns or cost.
 */
export function TaskBar(props: { taskId: string }) {
  const t = () => task(props.taskId)!;
  const run = () => taskRunOf(props.taskId);
  const a = () => agent(t().agentId);
  const elapsed = () => {
    const r = run();
    if (!r?.startedAt) return null;
    return fmtUptime((r.endedAt ?? now()) - r.startedAt);
  };

  return (
    <div class="commandbar" onContextMenu={(e) => popupMenu(taskMenu(t()), e)}>
      <span class="t-medium-12 c-primary ellipsis" title={t().goal}>
        {t().title}
      </span>
      <span class="pill" data-tone={taskTone(t().status) === "review" ? undefined : taskTone(t().status)}>
        {TASK_STATUS_LABEL[t().status]}
      </span>
      <span class="t-caption-11 c-muted nowrap">
        {a()?.name ?? t().agentId}
        {t().model ? ` · ${t().model}` : ""}
        {t().effort ? ` · ${t().effort}` : ""}
        {elapsed() ? ` · ${elapsed()}` : ""}
        {run()?.turns ? ` · ${run()!.turns} turns` : ""}
        {fmtCost(run()?.costUsd ?? null) ? ` · ${fmtCost(run()!.costUsd)}` : ""}
      </span>
      <div class="grow" />
      <Show
        when={isWorking(t())}
        fallback={
          <button class="icon-btn" title={run() ? "Run again" : "Run"} onClick={() => void startTask(props.taskId)}>
            <Icon name="play" size={14} color="var(--status-running)" />
          </button>
        }
      >
        <button class="icon-btn" title="Stop agent" onClick={() => void stopTask(props.taskId)}>
          <Icon name="stop" size={14} color="var(--status-crashed)" />
        </button>
      </Show>
      <button class="icon-btn" title="Edit task" onClick={() => openComposer(props.taskId)}>
        <Icon name="settings" size={14} />
      </button>
      <Show when={t().status === "review"}>
        <button class="btn btn-secondary btn-sm" style={{ "margin-left": "4px" }} onClick={() => void setTaskStatus(t(), "done")}>
          <Icon name="check" size={11} color="var(--status-running)" />
          Mark done
        </button>
      </Show>
    </div>
  );
}
