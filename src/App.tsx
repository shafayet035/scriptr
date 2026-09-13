import { Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { Workspace } from "./screens/Workspace";
import { Welcome } from "./screens/Welcome";
import { Inspector } from "./screens/Inspector";
import { AddProjectModal } from "./screens/AddProjectModal";
import { CommandPalette } from "./screens/CommandPalette";
import { SettingsScreen } from "./screens/Settings";
import { PopupMenuHost } from "./components/PopupMenuHost";
import { Toast } from "./components/Toast";
import { beginAddProject, handleShortcut, setPalette, state } from "./store/app";
import { isTauri, onFolderDrop } from "./lib/ipc";

export function App() {
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && state.ui.paletteOpen) return setPalette(false);
      // In Tauri the native menu owns accelerators and emits `menu` events.
      if (!isTauri) handleShortcut(e);
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));

    // Dropping a folder anywhere starts flow A.
    let unlisten: (() => void) | undefined;
    void onFolderDrop((paths) => paths[0] && void beginAddProject(paths[0])).then((u) => (unlisten = u));
    onCleanup(() => unlisten?.());

    const noContext = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest("input, textarea, .xterm")) e.preventDefault();
    };
    window.addEventListener("contextmenu", noContext);
    onCleanup(() => window.removeEventListener("contextmenu", noContext));
  });

  return (
    <div class="app">
      <TitleBar />
      <div class="body">
        <Switch>
          <Match when={state.ui.settingsOpen}>
            <SettingsScreen />
          </Match>
          <Match when={true}>
            <Sidebar />
            <Show when={state.loaded}>
              <Show when={state.projects.length > 0 && state.ui.projectId} fallback={<Welcome />}>
                <Workspace />
                <Show when={state.ui.inspectorScriptId}>
                  <Inspector scriptId={state.ui.inspectorScriptId!} />
                </Show>
              </Show>
            </Show>
          </Match>
        </Switch>
      </div>
      <Show when={state.ui.scan}>
        <AddProjectModal scan={state.ui.scan!} />
      </Show>
      <Show when={state.ui.paletteOpen}>
        <CommandPalette />
      </Show>
      <Toast />
      <PopupMenuHost />
    </div>
  );
}
