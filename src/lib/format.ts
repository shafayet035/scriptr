import type { Gate, RunInfo, RunState, Script } from "./types";

export function displayName(s: Pick<Script, "name" | "label">): string {
  return s.label ? `${s.name} · ${s.label}` : s.name;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 8s · 14m 02s · 3h 04m */
export function fmtUptime(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${pad(m)}m`;
  if (m > 0) return `${m}m ${pad(s)}s`;
  return `${s}s`;
}

/** 500ms · 2s · 1m 30s — the scriptr.toml duration style */
export function fmtDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms % 60000 === 0) return `${ms / 60000}m`;
  if (ms >= 60000) return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`;
  return Number.isInteger(ms / 1000) ? `${ms / 1000}s` : `${(ms / 1000).toFixed(1)}s`;
}

/** "2s" | "500ms" | "1m" | "90" (seconds) → ms; null when unparseable */
export function parseDuration(text: string): number | null {
  const m = text.trim().match(/^(\d+(?:\.\d+)?)\s*(ms|s|m|h)?$/i);
  if (!m) return null;
  const n = parseFloat(m[1]);
  const unit = (m[2] ?? "s").toLowerCase();
  const mult = unit === "ms" ? 1 : unit === "s" ? 1000 : unit === "m" ? 60000 : 3600000;
  return Math.round(n * mult);
}

export function fmtBytes(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

export function gateDescription(g: Gate): string {
  switch (g.kind) {
    case "port":
      return `port ${g.port} accepts a TCP connection`;
    case "log":
      return `stdout matches /${g.pattern}/`;
    case "http":
      return `${g.url} returns HTTP 200`;
    case "exit":
      return "process exits with code 0";
    case "instant":
      return "ready as soon as it starts";
  }
}

export function gateShort(g: Gate): string {
  switch (g.kind) {
    case "port":
      return `port ${g.port}`;
    case "log":
      return "log match";
    case "http":
      return "HTTP 200";
    case "exit":
      return "exit code 0";
    case "instant":
      return "instantly";
  }
}

/** Collapse the lifecycle into the five colours the design system uses. */
export function tone(state: RunState): "running" | "starting" | "queued" | "crashed" | "stopped" {
  switch (state) {
    case "running":
      return "running";
    case "starting":
    case "stopping":
      return "starting";
    case "queued":
      return "queued";
    // Figma paints a crash that is waiting to retry red, not queued-purple.
    case "backoff":
    case "crashed":
      return "crashed";
    default:
      return "stopped";
  }
}

export function isLive(state: RunState): boolean {
  return state === "running" || state === "starting" || state === "queued" || state === "backoff" || state === "stopping";
}

/** Badge text next to the command in the command bar. */
export function runBadge(run: RunInfo, now: number): { text: string; tone: string } | null {
  switch (run.state) {
    case "running":
      return {
        text: `up ${fmtUptime(now - (run.startedAt ?? now))}${run.degraded ? " · degraded" : ""}`,
        tone: run.degraded ? "starting" : "running",
      };
    case "starting":
      return { text: run.gateNote ? `starting · ${run.gateNote}` : "starting", tone: "starting" };
    case "queued":
      return { text: "queued · waiting for dependencies", tone: "queued" };
    case "backoff":
      return {
        text: `exited ${run.exitCode ?? "?"} · retrying in ${fmtUptime(Math.max(0, (run.nextRetryAt ?? now) - now))}`,
        tone: "crashed",
      };
    case "crashed":
      return { text: `exited ${run.exitCode ?? "?"} · ${fmtUptime(now - (run.endedAt ?? now))} ago`, tone: "crashed" };
    case "stopping":
      return { text: "stopping…", tone: "starting" };
    case "stopped":
      if (run.exitCode === 0 && run.gateNote) return { text: run.gateNote, tone: "stopped" };
      return run.endedAt ? { text: `stopped · ${fmtUptime(now - run.endedAt)} ago`, tone: "stopped" } : null;
    default:
      return null;
  }
}

/** "poetry run celery -A acme worker -l info" → "celery worker" (graph node subtitle). */
export function shortCmd(cmd: string): string {
  const wrappers = new Set(["poetry", "run", "uv", "python", "python3", "docker", "pnpm", "npx", "bunx", "exec", "env"]);
  const tokens = cmd.trim().split(/\s+/);
  const out: string[] = [];
  let skipValue = false;
  for (const t of tokens) {
    if (skipValue) {
      skipValue = false;
      continue;
    }
    if (t.startsWith("-")) {
      skipValue = !t.includes("=") && t.length === 2;
      continue;
    }
    if (out.length === 0 && wrappers.has(t)) continue;
    if (/[:/=]/.test(t) && out.length > 0) continue;
    out.push(t);
    if (out.length === 3) break;
  }
  return out.join(" ") || cmd;
}

export function tildify(path: string): string {
  const m = path.match(/^\/Users\/[^/]+(\/.*)?$/) ?? path.match(/^\/home\/[^/]+(\/.*)?$/);
  return m ? `~${m[1] ?? ""}` : path;
}
