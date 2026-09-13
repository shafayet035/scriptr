import { createSignal } from "solid-js";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { tildify } from "../lib/format";
import { beginAddProject, state } from "../store/app";

const MANIFESTS = ["package.json", "pyproject.toml", "Makefile", "docker-compose.yml", "Procfile", "*.sh"];

/** Screen 01 — no projects yet. Dropping a folder is handled app-wide (native drag-drop event). */
export function Welcome() {
  const [over, setOver] = createSignal(false);

  const openRecent = (e: MouseEvent) =>
    popupMenu(
      state.ui.recent.length
        ? state.ui.recent.map((path) => ({ label: tildify(path), action: () => void beginAddProject(path) }))
        : [{ label: "No recent folders", enabled: false }],
      e.currentTarget as HTMLElement,
    );

  return (
    <main
      class="welcome"
      onDragOver={(e) => (e.preventDefault(), setOver(true))}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => (e.preventDefault(), setOver(false))}
    >
      <div class="dropzone" data-over={over()}>
        <div class="icon-badge">
          <Icon name="folder-plus" size={26} color="var(--accent)" />
        </div>
        <div class="copy">
          <p class="t-display-20 c-primary">Add your first project</p>
          <p class="t-body-13 c-secondary">
            Point Scriptr at a project folder. It scans package.json, Makefile, pyproject.toml, docker-compose.yml and
            more, then suggests the scripts you can run.
          </p>
        </div>
        <div class="row" style={{ gap: "10px" }}>
          <button class="btn btn-primary" onClick={() => void beginAddProject()}>
            <Icon name="plus" size={14} />
            Add project folder
          </button>
          <button class="btn btn-secondary" onClick={openRecent}>
            <Icon name="clock" size={14} />
            Open recent
            <Icon name="chevron-down" size={12} color="var(--text-muted)" />
          </button>
        </div>
        <p class="t-caption-11 c-muted">…or drop a folder anywhere in this window</p>
      </div>
      <div class="detects">
        {MANIFESTS.map((m) => (
          <span>{m}</span>
        ))}
      </div>
    </main>
  );
}
