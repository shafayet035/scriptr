import { createSignal } from "solid-js";
import { isTauri } from "./ipc";

export interface MenuEntry {
  label?: string;
  checked?: boolean;
  enabled?: boolean;
  separator?: boolean;
  action?: () => void;
}

export interface HtmlMenu {
  x: number;
  y: number;
  entries: MenuEntry[];
}

/** Browser fallback state, rendered by <PopupMenuHost/>. */
export const [htmlMenu, setHtmlMenu] = createSignal<HtmlMenu | null>(null);

/** Native context/popup menu in Tauri; an HTML stand-in in the browser. */
export async function popupMenu(entries: MenuEntry[], anchor?: HTMLElement | MouseEvent): Promise<void> {
  let x = 0;
  let y = 0;
  if (anchor instanceof MouseEvent) {
    x = anchor.clientX;
    y = anchor.clientY;
  } else if (anchor) {
    const r = anchor.getBoundingClientRect();
    x = r.left;
    y = r.bottom + 4;
  }

  if (!isTauri) {
    setHtmlMenu({ x, y, entries });
    return;
  }

  const { Menu, MenuItem, CheckMenuItem, PredefinedMenuItem } = await import("@tauri-apps/api/menu");
  const { LogicalPosition } = await import("@tauri-apps/api/dpi");
  const items = await Promise.all(
    entries.map((e) => {
      if (e.separator) return PredefinedMenuItem.new({ item: "Separator" });
      const opts = { text: e.label ?? "", enabled: e.enabled ?? true, action: () => e.action?.() };
      return e.checked === undefined ? MenuItem.new(opts) : CheckMenuItem.new({ ...opts, checked: e.checked });
    }),
  );
  const menu = await Menu.new({ items });
  await menu.popup(anchor ? new LogicalPosition(x, y) : undefined);
}
