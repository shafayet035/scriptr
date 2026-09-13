import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { Icon, type IconName } from "../components/Icon";
import { computePlan } from "../lib/graph";
import { isLive, tildify } from "../lib/format";
import { isMac } from "../lib/ipc";
import {
  beginAddProject,
  exportToml,
  openInspector,
  openScript,
  openSettings,
  restartScript,
  runOf,
  runScope,
  script,
  selectGroup,
  selectProject,
  setPalette,
  setView,
  startScript,
  state,
  stopEverything,
  stopScript,
  terminalFocus,
} from "../store/app";

type Section = "Scripts" | "Groups" | "Projects & actions";

interface Item {
  key: string;
  section: Section;
  icon: IconName;
  iconColor: string;
  verb: string;
  name: string;
  nameHits: number[];
  rest: string;
  meta: string;
  kbd?: string;
  score: number;
  run: (mods: { meta: boolean; alt: boolean }) => void;
}

const VERBS = ["run", "start", "restart", "stop", "open", "edit"];

/** Subsequence match with bonuses for word starts and runs. Returns hit indices into `text`. */
function fuzzy(query: string, text: string): { score: number; hits: number[] } | null {
  if (!query) return { score: 1, hits: [] };
  const q = query.toLowerCase().replace(/\s+/g, "");
  const t = text.toLowerCase();
  const isWordStart = (i: number) => i === 0 || /[\s\-_·/.:]/.test(t[i - 1]);

  // Contiguous substring wins outright.
  const sub = t.indexOf(q);
  if (sub >= 0) {
    return { score: 10 + q.length * 3 + (isWordStart(sub) ? 6 : 0) - t.length * 0.02, hits: [...q].map((_, k) => sub + k) };
  }

  // Otherwise every jump must land on a word start ("fs" → "Full stack"); scattered letters don't count.
  const hits: number[] = [];
  let ti = 0;
  let score = 0;
  for (const ch of q) {
    let i = t.indexOf(ch, ti);
    const prev = hits[hits.length - 1];
    while (i >= 0 && prev !== undefined && i !== prev + 1 && !isWordStart(i)) i = t.indexOf(ch, i + 1);
    if (i < 0 || (prev === undefined && !isWordStart(i))) return null;
    score += 1 + (prev !== undefined && i === prev + 1 ? 2 : 3);
    hits.push(i);
    ti = i + 1;
  }
  return { score: score - t.length * 0.02, hits };
}

const CMD = isMac ? "⌘" : "Ctrl ";

/** Screen 07 — ⌘K. Fuzzy across scripts, groups, projects, actions. */
export function CommandPalette() {
  const [query, setQuery] = createSignal("");
  const [index, setIndex] = createSignal(0);

  const items = createMemo<Item[]>(() => {
    const raw = query().trim();
    const [first, ...restTokens] = raw.split(/\s+/);
    const verb = VERBS.includes(first?.toLowerCase()) ? first.toLowerCase() : null;
    const q = verb ? restTokens.join(" ") : raw;
    const out: Item[] = [];
    const projectName = (pid: string) => state.projects.find((p) => p.id === pid)?.name ?? "";
    const current = state.ui.projectId;

    const scriptMatches = new Map<string, number>();
    const lifecycleVerb = verb === "restart" || verb === "stop" || verb === "edit";

    // scripts — current project first
    for (const s of [...state.scripts].sort((a, b) => Number(b.projectId === current) - Number(a.projectId === current))) {
      const m = fuzzy(q, s.label ? `${s.name} · ${s.label}` : s.name);
      if (!m) continue;
      scriptMatches.set(s.id, m.score);
      const nameHits = m.hits.filter((h) => h < s.name.length);
      const live = isLive(runOf(s.id).state);
      const rest = s.label ? ` · ${s.label}` : "";
      const meta = `${projectName(s.projectId)}${live ? " · running" : ""}`;
      const base = m.score + (s.projectId === current ? 2 : 0);
      const add = (v: string, icon: IconName, color: string, bonus: number, run: Item["run"]) =>
        out.push({ key: `${v}:${s.id}`, section: "Scripts", icon, iconColor: color, verb: v, name: s.name, nameHits, rest, meta, score: base + bonus, run });

      // ⏎ run (a running script just comes to the front) · ⌘⏎ run + focus terminal · ⌥⏎ edit
      const runIt: Item["run"] = ({ meta: focus, alt }) => {
        if (alt) return (openScript(s.id), openInspector(s.id));
        if (live) openScript(s.id);
        else void startScript(s.id, focus);
        if (focus) terminalFocus(s.id);
      };
      const editIt = () => (openScript(s.id), openInspector(s.id));
      if (!lifecycleVerb) add("Run", "play", "var(--status-running)", verb ? 3 : live ? 0.5 : 1, runIt);
      if (live && verb !== "edit") add("Restart", "restart", "var(--text-secondary)", verb === "restart" ? 3 : 0, ({ alt }) => (alt ? editIt() : void restartScript(s.id)));
      if (live && verb !== "edit") add("Stop", "stop", "var(--status-crashed)", verb === "stop" ? 3 : -0.5, ({ alt }) => (alt ? editIt() : void stopScript(s.id)));
      if (verb === "edit") add("Edit", "settings", "var(--text-secondary)", 3, editIt);
    }

    // groups — by name, or because they contain a matching script
    if (!lifecycleVerb) {
      for (const g of state.groups) {
        const byMember = q ? Math.max(0, ...g.scriptIds.map((id) => scriptMatches.get(id) ?? 0)) : 0;
        const m = fuzzy(q, g.name) ?? (byMember > 0 ? { score: byMember / 4, hits: [] } : null);
        if (!m) continue;
        const scripts = g.scriptIds.map(script).filter((x) => !!x);
        const waves = computePlan(g.id, scripts as NonNullable<(typeof scripts)[number]>[]).waves.length;
        out.push({
          key: `group:${g.id}`, section: "Groups", icon: "layers", iconColor: "var(--text-secondary)", verb: "Run group",
          name: g.name, nameHits: m.hits, rest: "", meta: `${g.scriptIds.length} scripts · ${waves} waves`,
          score: m.score + (g.projectId === current ? 2 : 0),
          run: () => {
            selectProject(g.projectId);
            selectGroup(g.projectId, g.id);
            void runScope(g.projectId);
          },
        });
      }
    }

    // projects & actions
    if (!lifecycleVerb) {
      for (const p of state.projects) {
        const byScript = q ? state.scripts.some((s) => s.projectId === p.id && scriptMatches.has(s.id)) : false;
        const m = fuzzy(q, p.name) ?? (byScript ? { score: 0.5, hits: [] } : null);
        if (!m) continue;
        out.push({
          key: `project:${p.id}`, section: "Projects & actions", icon: "folder", iconColor: "var(--text-secondary)", verb: "Open",
          name: p.name, nameHits: m.hits, rest: "", meta: tildify(p.path), score: m.score, run: () => selectProject(p.id),
        });
      }
    }
    const anyLive = state.scripts.some((s) => isLive(runOf(s.id).state));
    if (anyLive && (verb === null || verb === "run" || verb === "stop") && !out.some((i) => i.key === "action:Stop everything")) {
      if (q && !fuzzy(q, "Stop everything")) {
        out.push({
          key: "action:Stop everything", section: "Projects & actions", icon: "power", iconColor: "var(--status-crashed)", verb: "",
          name: "Stop everything", nameHits: [], rest: "", meta: "all projects", kbd: `${CMD}⇧.`, score: -2, run: () => void stopEverything(),
        });
      }
    }
    if (!verb) {
      const actions: [string, IconName, string, string, string | undefined, () => void][] = [
        ["Stop everything", "power", "var(--status-crashed)", "all projects", `${CMD}⇧.`, () => void stopEverything()],
        ["Add project…", "folder-plus", "var(--text-secondary)", "scan a folder", `${CMD}O`, () => void beginAddProject()],
        ["Show dependencies", "graph", "var(--text-secondary)", "current group", `${CMD}2`, () => setView("deps")],
        ["Export scriptr.toml", "arrow-up", "var(--text-secondary)", "current project", undefined, () => current && void exportToml(current)],
        ["Settings", "settings", "var(--text-secondary)", "import, export, shell", `${CMD},`, () => openSettings()],
      ];
      for (const [name, icon, color, meta, kbd, run] of actions) {
        const m = fuzzy(q, name);
        if (!m) continue;
        out.push({ key: `action:${name}`, section: "Projects & actions", icon, iconColor: color, verb: "", name, nameHits: m.hits, rest: "", meta, kbd, score: m.score - 1, run });
      }
    }

    const limits: Record<Section, number> = { Scripts: 6, Groups: 3, "Projects & actions": 5 };
    const sections: Section[] = ["Scripts", "Groups", "Projects & actions"];
    return sections.flatMap((sec) =>
      out
        .filter((i) => i.section === sec)
        .sort((a, b) => b.score - a.score)
        .slice(0, limits[sec]),
    );
  });

  createEffect(on(query, () => setIndex(0)));

  const choose = (item: Item | undefined, mods: { meta: boolean; alt: boolean }) => {
    if (!item) return;
    setPalette(false);
    item.run(mods);
  };

  const onKey = (e: KeyboardEvent) => {
    const n = items().length;
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      setIndex((i) => (i + 1) % Math.max(1, n));
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      setIndex((i) => (i - 1 + n) % Math.max(1, n));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(items()[index()], { meta: e.metaKey || e.ctrlKey, alt: e.altKey });
    } else if (e.key === "Escape") {
      e.preventDefault();
      setPalette(false);
    }
  };

  const highlighted = (item: Item) => {
    const hits = new Set(item.nameHits);
    return item.name.split("").map((ch, i) => (hits.has(i) ? <span class="c-accent">{ch}</span> : ch));
  };

  return (
    <>
      <div class="scrim light" onClick={() => setPalette(false)} />
      <div class="dialog palette" role="dialog" aria-label="Command palette">
        <div class="palette-input">
          <Icon name="search" size={16} color="var(--text-muted)" />
          <input
            ref={(el) => queueMicrotask(() => el.focus())}
            class="t-heading-15"
            placeholder="Run, stop or open anything…"
            value={query()}
            spellcheck={false}
            autocomplete="off"
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={onKey}
          />
          <span class="t-caption-11 c-muted">esc</span>
        </div>
        <div class="palette-results scroll-y">
          <For each={items()}>
            {(item, i) => (
              <>
                <Show when={i() === 0 || items()[i() - 1].section !== item.section}>
                  <p class="palette-section t-label-caps">{item.section}</p>
                </Show>
                <button
                  class="palette-item"
                  aria-selected={index() === i()}
                  onMouseMove={() => setIndex(i())}
                  onClick={(e) => choose(item, { meta: e.metaKey || e.ctrlKey, alt: e.altKey })}
                  ref={(el) => createEffect(() => index() === i() && el.scrollIntoView({ block: "nearest" }))}
                >
                  <Icon name={item.icon} size={14} color={item.iconColor} />
                  <span class="label ellipsis">
                    <Show when={item.verb}>{item.verb}&nbsp;&nbsp;</Show>
                    {highlighted(item)}
                    {item.rest}
                  </span>
                  <div class="grow" />
                  <span class="t-caption-11 c-muted ellipsis">{item.meta}</span>
                  <Show when={index() === i()} fallback={<Show when={item.kbd}><span class="kbd">{item.kbd}</span></Show>}>
                    <span class="kbd">⏎</span>
                  </Show>
                </button>
              </>
            )}
          </For>
          <Show when={items().length === 0}>
            <p class="t-body-12 c-muted" style={{ padding: "18px 12px" }}>
              Nothing matches “{query()}”.
            </p>
          </Show>
        </div>
        <div class="palette-footer">
          <span class="hint"><span class="kbd">↑↓</span>navigate</span>
          <span class="hint"><span class="kbd">⏎</span>run</span>
          <span class="hint"><span class="kbd">{CMD}⏎</span>run and focus terminal</span>
          <span class="hint"><span class="kbd">{isMac ? "⌥⏎" : "Alt ⏎"}</span>edit script</span>
        </div>
      </div>
    </>
  );
}
