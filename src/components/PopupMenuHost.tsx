import { For, onCleanup, onMount, Show } from "solid-js";
import { htmlMenu, setHtmlMenu } from "../lib/menu";

/** Browser-only stand-in for native popup menus. */
export function PopupMenuHost() {
  onMount(() => {
    const close = () => setHtmlMenu(null);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", close);
    onCleanup(() => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", close);
    });
  });

  return (
    <Show when={htmlMenu()}>
      {(m) => (
        <div
          class="popup-menu"
          style={{ left: `${Math.min(m().x, window.innerWidth - 200)}px`, top: `${m().y}px` }}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <For each={m().entries}>
            {(entry) =>
              entry.separator ? (
                <hr />
              ) : (
                <button
                  disabled={entry.enabled === false}
                  onClick={() => {
                    setHtmlMenu(null);
                    entry.action?.();
                  }}
                >
                  <span style={{ width: "10px" }}>{entry.checked ? "✓" : ""}</span>
                  {entry.label}
                </button>
              )
            }
          </For>
        </div>
      )}
    </Show>
  );
}
