import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { SerializeAddon } from "@xterm/addon-serialize";
import { backend } from "./ipc";

// One xterm instance per script, created on first view and kept for the session:
// switching tabs hides the host element instead of destroying the terminal.

const css = getComputedStyle(document.documentElement);
const token = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;

export const terminalTheme: ITheme = {
  background: token("--bg-terminal", "#07090b"),
  foreground: token("--term-fg", "#c9d1d9"),
  cursor: token("--term-green", "#7ee787"),
  cursorAccent: token("--bg-terminal", "#07090b"),
  selectionBackground: "#264f78",
  black: "#484f58",
  red: token("--term-red", "#ff7b72"),
  green: token("--term-green", "#7ee787"),
  yellow: token("--term-yellow", "#e3b341"),
  blue: token("--term-blue", "#79c0ff"),
  magenta: token("--term-magenta", "#d2a8ff"),
  cyan: token("--term-cyan", "#56d4dd"),
  white: token("--term-fg", "#c9d1d9"),
  brightBlack: token("--term-dim", "#6e7681"),
  brightRed: "#ffa198",
  brightGreen: "#aff5b4",
  brightYellow: "#f2cc60",
  brightBlue: "#a5d6ff",
  brightMagenta: "#e2c5ff",
  brightCyan: "#76e3ea",
  brightWhite: "#f0f6fc",
};

const baseOptions = {
  fontFamily: '"JetBrains Mono", ui-monospace, Menlo, monospace',
  fontSize: 12,
  lineHeight: 1.45,
  letterSpacing: 0,
  theme: terminalTheme,
  cursorStyle: "block" as const,
  cursorBlink: false,
  scrollback: 5000,
  allowProposedApi: true,
  macOptionIsMeta: true,
  drawBoldTextInBrightColors: false,
  minimumContrastRatio: 1,
};

export interface TermEntry {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  serialize: SerializeAddon;
  host: HTMLDivElement;
  opened: boolean;
}

const entries = new Map<string, TermEntry>();

let webglOk: boolean | null = null;
function canUseWebgl(): boolean {
  if (webglOk === null) {
    try {
      webglOk = !!document.createElement("canvas").getContext("webgl2");
    } catch {
      webglOk = false;
    }
  }
  return webglOk;
}

/** WebGL renderer when the webview supports it; xterm's DOM renderer otherwise or on context loss. */
async function loadRenderer(term: Terminal) {
  if (!canUseWebgl()) return;
  try {
    const { WebglAddon } = await import("@xterm/addon-webgl");
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    webglOk = false;
  }
}

function create(scriptId: string, readOnly = false): TermEntry {
  const term = new Terminal({ ...baseOptions, disableStdin: readOnly, cursorInactiveStyle: readOnly ? "none" : "outline" });
  const fit = new FitAddon();
  const search = new SearchAddon();
  const serialize = new SerializeAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(serialize);
  const host = document.createElement("div");
  host.className = "term-host";
  host.dataset.scriptId = scriptId;
  const entry: TermEntry = { term, fit, search, serialize, host, opened: false };
  entries.set(scriptId, entry);

  if (!readOnly) {
    term.onData((d) => void backend.ptyWrite(scriptId, d));
    term.onResize(({ cols, rows }) => void backend.ptyResize(scriptId, cols, rows));
    void backend.ptyAttach(scriptId, (bytes) => {
      term.write(bytes);
      feedCombined(scriptId, bytes);
    });
  }
  return entry;
}

export function terminalFor(scriptId: string): TermEntry {
  return entries.get(scriptId) ?? create(scriptId);
}

/** Put the terminal's host into `parent` (once) and make sure xterm is opened and sized. */
export function mountTerminal(scriptId: string, parent: HTMLElement, readOnly = false): TermEntry {
  const e = entries.get(scriptId) ?? create(scriptId, readOnly);
  if (e.host.parentElement !== parent) parent.appendChild(e.host);
  if (!e.opened) {
    e.term.open(e.host);
    e.opened = true;
    void loadRenderer(e.term);
  }
  return e;
}

export function fitTerminal(scriptId: string) {
  const e = entries.get(scriptId);
  if (!e?.opened || e.host.offsetParent === null) return;
  try {
    e.fit.fit();
  } catch {
    /* host not laid out yet */
  }
}

export function clearTerminal(scriptId: string) {
  entries.get(scriptId)?.term.clear();
}

export function terminalText(scriptId: string): string {
  const e = entries.get(scriptId);
  if (!e) return "";
  const buf = e.term.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buf.length; i++) lines.push(buf.getLine(i)?.translateToString(true) ?? "");
  return lines.join("\n").trimEnd();
}

/** Attach output streams for scripts not yet viewed (needed by the combined log). */
export function ensureAttached(scriptIds: string[]) {
  for (const id of scriptIds) terminalFor(id);
}

// ---------------------------------------------------------------- combined log

export const COMBINED_ID = "__combined__";
const nameColors = [36, 35, 33, 34, 32, 31];
const names = new Map<string, { label: string; color: number }>();
const decoders = new Map<string, { dec: TextDecoder; carry: string }>();

export function setCombinedNames(list: { id: string; name: string }[]) {
  const width = Math.max(6, ...list.map((l) => l.name.length));
  list.forEach((l, i) => names.set(l.id, { label: l.name.padEnd(width), color: nameColors[i % nameColors.length] }));
}

const seeded = new Set<string>();

/**
 * First open of the combined log: replay what already-attached terminals hold, then attach
 * the rest (their scrollback snapshot arrives through feedCombined). Live lines interleave from here on.
 */
export function seedCombined(scriptIds: string[]) {
  const combined = entries.get(COMBINED_ID);
  if (!combined) return;
  for (const id of scriptIds) {
    if (seeded.has(id)) continue;
    seeded.add(id);
    const meta = names.get(id);
    if (!meta || !entries.has(id)) continue;
    const text = terminalText(id);
    if (!text) continue;
    const prefix = `\x1b[${meta.color}m${meta.label}\x1b[0m \x1b[2m│\x1b[0m `;
    combined.term.write(text.split("\n").slice(-200).map((l) => prefix + l).join("\r\n") + "\r\n");
  }
  ensureAttached(scriptIds);
}

function feedCombined(scriptId: string, bytes: Uint8Array) {
  const combined = entries.get(COMBINED_ID);
  const meta = names.get(scriptId);
  if (!combined || !meta) return;
  const d = decoders.get(scriptId) ?? { dec: new TextDecoder(), carry: "" };
  decoders.set(scriptId, d);
  const text = d.carry + d.dec.decode(bytes, { stream: true });
  const parts = text.split(/\r?\n/);
  d.carry = parts.pop() ?? "";
  if (parts.length === 0) return;
  const prefix = `\x1b[${meta.color}m${meta.label}\x1b[0m \x1b[2m│\x1b[0m `;
  combined.term.write(parts.map((p) => prefix + p.replace(/\r/g, "") + "\x1b[0m").join("\r\n") + "\r\n");
}
