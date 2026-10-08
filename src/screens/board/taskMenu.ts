import type { MenuEntry } from "../../lib/menu";
import type { Task } from "../../lib/types";
import {
  deleteTask,
  discardWorkspace,
  openComposer,
  openTask,
  setTaskStatus,
  startTask,
  stopTask,
  taskRunOf,
} from "../../store/app";

export const isWorking = (t: Task) =>
  t.status === "working" || t.status === "queued" || t.status === "verifying";

/** One menu for a task, wherever it is shown: card, deck header, sidebar row. */
export function taskMenu(t: Task): MenuEntry[] {
  const ran = !!taskRunOf(t.id);
  return [
    isWorking(t)
      ? { label: "Stop agent", action: () => void stopTask(t.id) }
      : { label: ran ? "Run again" : "Run", action: () => void startTask(t.id) },
    { label: "Show terminal", action: () => openTask(t.id) },
    { label: "Edit…", action: () => openComposer(t.id) },
    { separator: true },
    { label: "Mark as done", enabled: t.status !== "done", action: () => void setTaskStatus(t, "done") },
    { label: "Move to backlog", enabled: t.status !== "backlog", action: () => void setTaskStatus(t, "backlog") },
    { separator: true },
    {
      label: "Discard workspace…",
      enabled: t.workspace === "worktree" && !!t.branch && !isWorking(t),
      action: () => void discardWorkspace(t.id),
    },
    { label: "Delete", action: () => void deleteTask(t.id) },
  ];
}
