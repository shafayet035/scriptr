import { isTaskBusy } from "../../lib/format";
import { openUrl } from "../../lib/ipc";
import type { MenuEntry } from "../../lib/menu";
import type { Task, TaskStatus } from "../../lib/types";
import { deleteTask, moveTask, openComposer } from "../../store/app";

export const isWorking = (t: Task) => isTaskBusy(t.status);

const COLUMNS: { status: TaskStatus; label: string }[] = [
  { status: "backlog", label: "Backlog" },
  { status: "todo", label: "To do" },
  { status: "doing", label: "In progress" },
  { status: "review", label: "Review" },
  { status: "done", label: "Done" },
];

/** One menu for a card, wherever it is shown: board, sidebar row, palette. */
export function taskMenu(t: Task): MenuEntry[] {
  return [
    { label: "Edit…", action: () => openComposer(t.id) },
    ...(t.issueUrl ? [{ label: "Open linked issue", action: () => void openUrl(t.issueUrl!) }] : []),
    { separator: true },
    // Moving to the end of a column is the common case; dragging handles the rest.
    ...COLUMNS.map((c) => ({
      label: `Move to ${c.label}`,
      enabled: t.status !== c.status,
      action: () => void moveTask(t.id, c.status),
    })),
    { separator: true },
    { label: "Delete", action: () => void deleteTask(t.id) },
  ];
}
