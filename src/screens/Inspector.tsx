import { createEffect, createResource, createSignal, For, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { createStore, reconcile, unwrap } from "solid-js/store";
import { Icon } from "../components/Icon";
import { popupMenu } from "../lib/menu";
import { backend, pickFolder } from "../lib/ipc";
import { wouldCycle } from "../lib/graph";
import { displayName, fmtDuration, fmtUptime, gateDescription, isLive, parseDuration } from "../lib/format";
import type { Gate, RestartOn, Script } from "../lib/types";
import { now, openInspector, project, runOf, saveScript, script, scriptsOf, state } from "../store/app";

type Tab = "command" | "env" | "health";

const GATE_OPTS: { kind: Gate["kind"]; label: string }[] = [
  { kind: "port", label: "Port open" },
  { kind: "log", label: "Log match" },
  { kind: "http", label: "HTTP 200" },
  { kind: "exit", label: "Exit 0" },
  { kind: "instant", label: "Instantly" },
];

const RESTART_OPTS: { on: RestartOn; label: string }[] = [
  { on: "never", label: "Never restart" },
  { on: "crash", label: "On crash" },
  { on: "always", label: "Always" },
];

const SECRETISH = /secret|token|key|password|passwd|credential|dsn/i;

/** Screen 04 — 340px panel: command, cwd, shell, Starts after, Ready when, On exit. */
export function Inspector(props: { scriptId: string }) {
  const original = () => script(props.scriptId);
  const [draft, setDraft] = createStore<Script>(structuredClone(unwrap(original()!)));
  const [tab, setTab] = createSignal<Tab>("command");

  createEffect(
    on(
      () => props.scriptId,
      () => {
        const o = original();
        if (o) setDraft(reconcile(structuredClone(unwrap(o))));
      },
    ),
  );

  // Stringify the proxies (not unwrap) so every field is a tracked read.
  const dirty = () => {
    const o = original();
    return !!o && JSON.stringify(draft) !== JSON.stringify(o);
  };
  const run = () => runOf(props.scriptId);
  const revert = () => setDraft(reconcile(structuredClone(unwrap(original()!))));
  const save = () => dirty() && void saveScript(structuredClone(unwrap(draft)));

  let panel!: HTMLElement;
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s" && panel.contains(document.activeElement)) {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <aside class="inspector" ref={panel}>
      <div class="insp-header">
        <div class="row" style={{ gap: "8px" }}>
          <span class="dot" data-state={run().state} style={{ width: "8px", height: "8px" }} />
          <span class="t-heading-15 c-primary ellipsis">{displayName(draft)}</span>
          <div class="grow" />
          <button class="icon-btn" style={{ width: "20px", height: "20px" }} onClick={() => openInspector(null)} title="Close inspector">
            <Icon name="x" size={14} />
          </button>
        </div>
        <div class="seg neutral insp-tabs" role="tablist">
          <button class="seg-opt" aria-pressed={tab() === "command"} onClick={() => setTab("command")}>
            Command
          </button>
          <button class="seg-opt" aria-pressed={tab() === "env"} onClick={() => setTab("env")}>
            Environment
          </button>
          <button class="seg-opt" aria-pressed={tab() === "health"} onClick={() => setTab("health")}>
            Health
          </button>
        </div>
      </div>

      <div class="insp-body scroll-y">
        <Switch>
          <Match when={tab() === "command"}>
            <CommandTab draft={draft} setDraft={setDraft} />
          </Match>
          <Match when={tab() === "env"}>
            <EnvTab draft={draft} setDraft={setDraft} />
          </Match>
          <Match when={tab() === "health"}>
            <HealthTab scriptId={props.scriptId} />
          </Match>
        </Switch>
      </div>

      <div class="insp-footer">
        <span class="t-caption-11 c-muted ellipsis">
          {dirty() ? (isLive(run().state) ? "Changes apply on next start" : "Unsaved changes") : "Changes apply on next start"}
        </span>
        <div class="grow" />
        <button class="btn btn-secondary btn-sm" disabled={!dirty()} onClick={revert}>
          Revert
        </button>
        <button class="btn btn-primary" style={{ padding: "7px 14px" }} disabled={!dirty()} onClick={save}>
          Save
        </button>
      </div>
    </aside>
  );
}

type DraftProps = { draft: Script; setDraft: (...args: any[]) => void };

function Info(props: { text: string }) {
  return (
    <span title={props.text} style={{ display: "inline-flex" }}>
      <Icon name="info" size={12} color="var(--border-strong)" />
    </span>
  );
}

function CommandTab(props: DraftProps) {
  const d = () => props.draft;
  const others = () => scriptsOf(d().projectId).filter((s) => s.id !== d().id);

  const setGateKind = (kind: Gate["kind"]) => {
    const prev = d().ready;
    const timeoutMs = "timeoutMs" in prev ? prev.timeoutMs : 60000;
    const next: Gate =
      kind === "port"
        ? { kind, port: d().port ?? 3000, timeoutMs }
        : kind === "log"
          ? { kind, pattern: "", timeoutMs }
          : kind === "http"
            ? { kind, url: `http://localhost:${d().port ?? 3000}/`, timeoutMs }
            : kind === "exit"
              ? { kind, timeoutMs }
              : { kind: "instant" };
    props.setDraft("ready", reconcile(next));
  };

  const addDep = (e: MouseEvent) => {
    const all = scriptsOf(d().projectId).map((s) => (s.id === d().id ? { ...s, after: [...d().after] } : s));
    const candidates = others().filter((s) => !d().after.includes(s.id));
    return popupMenu(
      candidates.length
        ? candidates.map((s) => {
            const cyc = wouldCycle(all, d().id, s.id);
            return {
              label: cyc ? `${displayName(s)} — would create a cycle` : displayName(s),
              enabled: !cyc,
              action: () => props.setDraft("after", [...d().after, s.id]),
            };
          })
        : [{ label: "No other scripts in this project", enabled: false }],
      e.currentTarget as HTMLElement,
    );
  };

  const shellMenu = (e: MouseEvent) =>
    popupMenu(
      [
        { label: `Default (${state.settings.defaultShell})`, checked: d().shell === null, action: () => props.setDraft("shell", null) },
        ...["/bin/zsh -lc", "/bin/bash -lc", "/bin/sh -c", "/usr/bin/env fish -c"].map((sh) => ({
          label: sh,
          checked: d().shell === sh,
          action: () => props.setDraft("shell", sh),
        })),
      ],
      e.currentTarget as HTMLElement,
    );

  const pickCwd = async () => {
    const root = project(d().projectId)?.path;
    const folder = await pickFolder();
    if (!folder || !root) return;
    const rel = folder.startsWith(root) ? `.${folder.slice(root.length) || "/"}` : folder;
    props.setDraft("cwd", rel);
  };

  const [backoffText, setBackoffText] = createSignal("");
  createEffect(() => setBackoffText(`${fmtDuration(d().restart.backoffMs)} → ${fmtDuration(d().restart.backoffMaxMs)}`));
  const commitBackoff = () => {
    const [a, b] = backoffText().split(/→|->|,/).map((t) => parseDuration(t));
    if (a) props.setDraft("restart", "backoffMs", a);
    if (b) props.setDraft("restart", "backoffMaxMs", Math.max(b, a ?? d().restart.backoffMs));
    setBackoffText(`${fmtDuration(d().restart.backoffMs)} → ${fmtDuration(d().restart.backoffMaxMs)}`);
  };

  const timeout = () => ("timeoutMs" in d().ready ? (d().ready as { timeoutMs: number }).timeoutMs : 0);

  return (
    <>
      <div class="insp-row">
        <div class="field grow">
          <span class="field-label">Name</span>
          <label class="input">
            <input class="t-mono-11" value={d().name} spellcheck={false} onInput={(e) => props.setDraft("name", e.currentTarget.value.trim())} />
          </label>
        </div>
        <div class="field grow">
          <span class="field-label">Label</span>
          <label class="input">
            <input
              class="t-mono-11"
              value={d().label ?? ""}
              placeholder="optional"
              spellcheck={false}
              onInput={(e) => props.setDraft("label", e.currentTarget.value.trim() || null)}
            />
          </label>
        </div>
      </div>

      <div class="field">
        <span class="field-label">Command</span>
        <label class="input">
          <textarea
            class="t-mono-11"
            rows={Math.min(6, Math.max(1, Math.ceil(d().cmd.length / 34)))}
            value={d().cmd}
            spellcheck={false}
            onInput={(e) => props.setDraft("cmd", e.currentTarget.value)}
          />
        </label>
      </div>

      <div class="field">
        <span class="field-label">Working directory</span>
        <label class="input">
          <input class="t-mono-11" value={d().cwd} spellcheck={false} onInput={(e) => props.setDraft("cwd", e.currentTarget.value)} />
          <button class="icon-btn" style={{ width: "18px", height: "18px" }} onClick={() => void pickCwd()} title="Choose folder">
            <Icon name="folder" size={13} />
          </button>
        </label>
      </div>

      <div class="field">
        <span class="field-label">Shell</span>
        <label class="input">
          <input
            class="t-mono-11"
            value={d().shell ?? ""}
            placeholder={state.settings.defaultShell}
            spellcheck={false}
            onInput={(e) => props.setDraft("shell", e.currentTarget.value.trim() || null)}
          />
          <button class="icon-btn" style={{ width: "18px", height: "18px" }} onClick={shellMenu} title="Pick a shell">
            <Icon name="chevron-down" size={13} />
          </button>
        </label>
      </div>

      <div class="insp-section">
        <div class="row" style={{ gap: "6px" }}>
          <span class="t-label-caps c-muted">Starts after</span>
          <Info text="Scripts that must be ready before this one launches." />
        </div>
        <div class="dep-chips">
          <For each={d().after}>
            {(id) => (
              <span class="dep-chip">
                <span class="dot" data-state={runOf(id).state} style={{ width: "6px", height: "6px" }} />
                {script(id)?.name ?? "missing"}
                <button onClick={() => props.setDraft("after", d().after.filter((x) => x !== id))} title="Remove dependency">
                  <Icon name="x" size={10} />
                </button>
              </span>
            )}
          </For>
          <button class="dep-add" onClick={addDep}>
            <Icon name="plus" size={10} />
            Add
          </button>
        </div>
        <p class="t-caption-11 c-muted">Scriptr waits until each dependency reports ready before launching this script.</p>
      </div>

      <div class="insp-section">
        <div class="row" style={{ gap: "6px" }}>
          <span class="t-label-caps c-muted">Ready when</span>
          <Info text={gateDescription(d().ready)} />
        </div>
        <div class="seg bordered">
          <For each={GATE_OPTS}>
            {(o) => (
              <button class="seg-opt" aria-pressed={d().ready.kind === o.kind} onClick={() => setGateKind(o.kind)}>
                {o.label}
              </button>
            )}
          </For>
        </div>
        <Show when={d().ready.kind !== "instant"}>
          <div class="insp-row">
            <Switch>
              <Match when={d().ready.kind === "port"}>
                <div class="field grow">
                  <span class="field-label">Port</span>
                  <label class="input">
                    <input
                      class="t-mono-11"
                      inputmode="numeric"
                      value={(d().ready as { port: number }).port}
                      onInput={(e) => {
                        const n = parseInt(e.currentTarget.value, 10);
                        if (n > 0 && n < 65536) props.setDraft("ready", "port", n);
                      }}
                    />
                  </label>
                </div>
              </Match>
              <Match when={d().ready.kind === "log"}>
                <div class="field grow">
                  <span class="field-label">Log pattern</span>
                  <label class="input">
                    <input
                      class="t-mono-11"
                      value={(d().ready as { pattern: string }).pattern}
                      placeholder="regex or plain text"
                      spellcheck={false}
                      onInput={(e) => props.setDraft("ready", "pattern", e.currentTarget.value)}
                    />
                  </label>
                </div>
              </Match>
              <Match when={d().ready.kind === "http"}>
                <div class="field grow">
                  <span class="field-label">URL</span>
                  <label class="input">
                    <input
                      class="t-mono-11"
                      value={(d().ready as { url: string }).url}
                      spellcheck={false}
                      onInput={(e) => props.setDraft("ready", "url", e.currentTarget.value)}
                    />
                  </label>
                </div>
              </Match>
            </Switch>
            <div class="field" style={{ width: d().ready.kind === "exit" ? "100%" : "84px" }}>
              <span class="field-label">Timeout</span>
              <label class="input">
                <input
                  class="t-mono-11"
                  value={fmtDuration(timeout())}
                  onChange={(e) => {
                    const ms = parseDuration(e.currentTarget.value);
                    if (ms) props.setDraft("ready", "timeoutMs", ms);
                    e.currentTarget.value = fmtDuration(timeout());
                  }}
                />
              </label>
            </div>
          </div>
        </Show>
      </div>

      <div class="insp-section">
        <span class="t-label-caps c-muted">On exit</span>
        <div class="seg bordered">
          <For each={RESTART_OPTS}>
            {(o) => (
              <button class="seg-opt" aria-pressed={d().restart.on === o.on} onClick={() => props.setDraft("restart", "on", o.on)}>
                {o.label}
              </button>
            )}
          </For>
        </div>
        <div class="insp-row" style={{ opacity: d().restart.on === "never" ? 0.5 : 1 }}>
          <div class="field grow">
            <span class="field-label">Max retries</span>
            <label class="input">
              <input
                class="t-body-12"
                inputmode="numeric"
                disabled={d().restart.on === "never"}
                value={d().restart.max}
                onInput={(e) => {
                  const n = parseInt(e.currentTarget.value, 10);
                  if (n >= 0 && n < 1000) props.setDraft("restart", "max", n);
                }}
              />
            </label>
          </div>
          <div class="field grow">
            <span class="field-label">Backoff</span>
            <label class="input">
              <input
                class="t-body-12"
                disabled={d().restart.on === "never"}
                value={backoffText()}
                onInput={(e) => setBackoffText(e.currentTarget.value)}
                onBlur={commitBackoff}
                onKeyDown={(e) => e.key === "Enter" && commitBackoff()}
              />
            </label>
          </div>
        </div>
      </div>
    </>
  );
}

function EnvTab(props: DraftProps) {
  const d = () => props.draft;
  const entries = () => Object.entries(d().env);
  const [revealed, setRevealed] = createSignal<string | null>(null);

  const setEntries = (list: [string, string][]) => props.setDraft("env", reconcile(Object.fromEntries(list)));

  return (
    <>
      <div class="field">
        <span class="field-label">Env file</span>
        <label class="input">
          <input
            class="t-mono-11"
            value={d().envFile ?? ""}
            placeholder=".env.local"
            spellcheck={false}
            onInput={(e) => props.setDraft("envFile", e.currentTarget.value.trim() || null)}
          />
          <Icon name="file" size={13} color="var(--text-muted)" />
        </label>
        <p class="t-caption-11 c-muted">Loaded before the variables below. Relative to the working directory.</p>
      </div>

      <div class="insp-section">
        <span class="t-label-caps c-muted">Variables</span>
        <For each={entries()}>
          {([key, value], i) => (
            <div class="env-row">
              <label class="input">
                <input
                  class="t-mono-11"
                  value={key}
                  spellcheck={false}
                  onChange={(e) => setEntries(entries().map((kv, j) => (j === i() ? [e.currentTarget.value.trim(), kv[1]] : kv)))}
                />
              </label>
              <label class="input">
                <input
                  class="t-mono-11"
                  type={SECRETISH.test(key) && revealed() !== key ? "password" : "text"}
                  value={value}
                  spellcheck={false}
                  onFocus={() => setRevealed(key)}
                  onBlur={() => setRevealed(null)}
                  onInput={(e) => props.setDraft("env", key, e.currentTarget.value)}
                />
              </label>
              <button class="icon-btn" style={{ width: "20px", height: "20px" }} onClick={() => setEntries(entries().filter((_, j) => j !== i()))} title="Remove">
                <Icon name="x" size={11} />
              </button>
            </div>
          )}
        </For>
        <button class="dep-add" style={{ "align-self": "flex-start" }} onClick={() => setEntries([...entries(), [`VAR_${entries().length + 1}`, ""]])}>
          <Icon name="plus" size={10} />
          Add variable
        </button>
        <p class="t-caption-11 c-muted">Values stay in the local database. scriptr.toml only records the env file path — never values.</p>
      </div>
    </>
  );
}

function HealthTab(props: { scriptId: string }) {
  const s = () => script(props.scriptId)!;
  const r = () => runOf(props.scriptId);
  const [history] = createResource(
    () => [props.scriptId, r().state] as const,
    ([id]) => backend.runHistory(id).catch(() => []),
  );

  return (
    <>
      <div class="insp-section">
        <span class="t-label-caps c-muted">Gate</span>
        <div class="health-card">
          <p class="t-body-12 c-primary">Ready when {gateDescription(s().ready)}</p>
          <p class="t-caption-11 c-muted">
            Now: {r().gateNote ?? r().state}
            {r().degraded ? " · timed out (degraded)" : ""}
          </p>
        </div>
      </div>
      <div class="insp-section">
        <span class="t-label-caps c-muted">Recent runs</span>
        <Show when={(history() ?? []).length > 0} fallback={<p class="t-caption-11 c-muted">No finished runs yet.</p>}>
          <div class="history">
            <For each={history()}>
              {(h) => (
                <div class="history-row">
                  <span class="pill" data-tone={h.exitCode === 0 ? "running" : h.exitCode === null ? undefined : "crashed"}>
                    {h.exitCode === null ? "signal" : `exit ${h.exitCode}`}
                  </span>
                  <span class="t-caption-11 c-secondary">{h.endedAt ? fmtUptime(h.endedAt - h.startedAt) : "running"}</span>
                  <div class="grow" />
                  <span class="t-caption-11 c-muted">{fmtUptime(now() - (h.endedAt ?? h.startedAt))} ago</span>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>
    </>
  );
}
