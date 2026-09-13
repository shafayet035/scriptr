import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { Icon } from "../components/Icon";
import { tildify } from "../lib/format";
import type { DetectedScript, ScanResult } from "../lib/types";
import { beginAddProject, cancelAddProject, confirmAddProject } from "../store/app";

/** Screen 02 — detected commands grouped by source file. Nothing is saved until Add. */
export function AddProjectModal(props: { scan: ScanResult }) {
  const all = () => props.scan.sources.flatMap((s) => s.scripts);
  const [selected, setSelected] = createSignal(new Set(all().filter((s) => s.suggested).map((s) => s.key)));
  const [importToml, setImportToml] = createSignal(props.scan.hasToml);
  const [busy, setBusy] = createSignal(false);

  const toggle = (key: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const toggleSource = (scripts: DetectedScript[]) =>
    setSelected((prev) => {
      const next = new Set(prev);
      const allOn = scripts.every((s) => next.has(s.key));
      for (const s of scripts) allOn ? next.delete(s.key) : next.add(s.key);
      return next;
    });

  const count = () => selected().size;
  const canAdd = () => !busy() && (importToml() || count() > 0);

  const add = async () => {
    if (!canAdd()) return;
    setBusy(true);
    await confirmAddProject({
      path: props.scan.path,
      name: props.scan.name,
      scripts: all().filter((s) => selected().has(s.key)),
      importToml: importToml(),
    });
    setBusy(false);
  };

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancelAddProject();
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void add();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <>
      <div class="scrim" onClick={cancelAddProject} />
      <div class="dialog add-project" role="dialog" aria-label="Add project">
        <div class="modal-header">
          <p class="t-display-20 c-primary">Add project</p>
          <div class="path-field">
            <Icon name="folder" size={14} color="var(--text-muted)" />
            <span class="t-mono-11 c-primary ellipsis selectable">{tildify(props.scan.path)}</span>
            <div class="grow" />
            <button class="change-btn" onClick={() => void beginAddProject()}>
              Change…
            </button>
          </div>
          <Show when={props.scan.hasToml}>
            <button class="toml-offer" onClick={() => setImportToml(!importToml())}>
              <span class="toggle" aria-checked={importToml()} />
              <span class="col" style={{ gap: "2px", "text-align": "left" }}>
                <span class="t-medium-12 c-primary">Import scriptr.toml instead</span>
                <span class="t-caption-11 c-muted">This folder already has a committed scriptr.toml with scripts, groups and dependencies.</span>
              </span>
            </button>
          </Show>
          <p class="t-body-12 c-secondary">
            <Show
              when={all().length > 0}
              fallback="Scriptr didn't find any runnable scripts here. Add the project anyway and create scripts by hand."
            >
              Scriptr found {all().length} runnable {all().length === 1 ? "script" : "scripts"} across {props.scan.sources.length}{" "}
              {props.scan.sources.length === 1 ? "source" : "sources"}. Pick what you want to manage — you can add, edit or remove
              scripts at any time.
            </Show>
          </p>
        </div>

        <div class="detected scroll-y" data-disabled={importToml()}>
          <For each={props.scan.sources}>
            {(src) => (
              <>
                <div class="source-row">
                  <Icon name="file" size={13} color="var(--text-muted)" />
                  <span class="t-mono-11 c-secondary">{src.file}</span>
                  <span class="dir-chip">{src.dir}</span>
                  <div class="grow" />
                  <button class="select-all" onClick={() => toggleSource(src.scripts)}>
                    {src.scripts.every((s) => selected().has(s.key)) ? "Select none" : "Select all"}
                  </button>
                </div>
                <For each={src.scripts}>
                  {(d) => (
                    <button class="detected-row" aria-pressed={selected().has(d.key)} onClick={() => toggle(d.key)}>
                      <span class="checkbox" role="checkbox" aria-checked={selected().has(d.key)}>
                        <Show when={selected().has(d.key)}>
                          <Icon name="check" size={11} color="var(--text-inverse)" />
                        </Show>
                      </span>
                      <span class="col" style={{ gap: "3px", "min-width": "0" }}>
                        <span class="t-medium-12 name">{d.name}</span>
                        <span class="t-mono-11 c-muted ellipsis">{d.cmd}</span>
                      </span>
                      <div class="grow" />
                      <Show when={d.port} fallback={<Show when={d.oneShot}><span class="pill">one-shot</span></Show>}>
                        <span class="pill">detects :{d.port}</span>
                      </Show>
                    </button>
                  )}
                </For>
              </>
            )}
          </For>
        </div>

        <div class="modal-footer">
          <span class="t-caption-11 c-muted ellipsis" style={{ "white-space": "pre" }}>
            {importToml()
              ? "scriptr.toml will be imported  ·  you can rescan manifests later"
              : `${count()} of ${all().length} selected  ·  commands, env and dependencies are editable after import`}
          </span>
          <div class="grow" />
          <button class="btn btn-secondary" style={{ padding: "9px 14px", "border-radius": "8px" }} onClick={cancelAddProject}>
            Cancel
          </button>
          <button class="btn btn-primary" style={{ padding: "9px 16px", "border-radius": "8px" }} disabled={!canAdd()} onClick={() => void add()}>
            {importToml() ? "Import scriptr.toml" : count() === 0 ? "Add project" : `Add ${count()} ${count() === 1 ? "script" : "scripts"}`}
          </button>
        </div>
      </div>
    </>
  );
}
