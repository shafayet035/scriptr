import { Icon } from "./Icon";
import { setPalette, state, toggleSettings } from "../store/app";
import { isMac } from "../lib/ipc";

export function TitleBar() {
  return (
    <header class="titlebar" data-tauri-drag-region>
      {/* native traffic lights sit here (titleBarStyle: Overlay) */}
      <div class="traffic-space" data-tauri-drag-region />
      <div class="wordmark" data-tauri-drag-region>
        <span class="logo">
          <Icon name="logo" size={12} color="var(--text-inverse)" />
        </span>
        <span class="t-title-13">Scriptr</span>
      </div>
      <div class="grow" style={{ "align-self": "stretch" }} data-tauri-drag-region />
      <button class="cmdk" onClick={() => setPalette(true)} title="Command palette">
        <Icon name="search" size={13} />
        <span>Search or run…</span>
        <span>{isMac ? "⌘K" : "Ctrl K"}</span>
      </button>
      <button class="icon-btn" aria-pressed={state.ui.settingsOpen} onClick={toggleSettings} title="Settings">
        <Icon name="settings" size={16} />
      </button>
    </header>
  );
}
