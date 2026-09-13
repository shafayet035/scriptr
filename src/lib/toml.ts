import { fmtDuration } from "./format";
import type { Group, Script } from "./types";

export interface TomlToken {
  kind: "comment" | "table" | "key" | "str" | "arr" | "plain";
  text: string;
}

const q = (s: string) => JSON.stringify(s);

/** Tokenised preview of the scriptr.toml the core exports (see src-tauri/src/config.rs). */
export function renderToml(scripts: Script[], groups: Group[]): TomlToken[][] {
  const lines: TomlToken[][] = [[{ kind: "comment", text: "# scriptr.toml — commit this at the project root" }]];
  const nameOf = (id: string) => scripts.find((s) => s.id === id)?.name ?? id;
  const kv = (key: string, value: TomlToken[]) => [{ kind: "key" as const, text: key.padEnd(8) + "= " }, ...value];
  const arr = (items: string[]): TomlToken => ({ kind: "arr", text: `[${items.map(q).join(", ")}]` });

  scripts.forEach((s, i) => {
    if (i > 0) lines.push([]);
    lines.push([{ kind: "table", text: "[[script]]" }]);
    lines.push(kv("name", [{ kind: "str", text: q(s.name) }]));
    if (s.label) lines.push(kv("label", [{ kind: "str", text: q(s.label) }]));
    lines.push(kv("cmd", [{ kind: "str", text: q(s.cmd) }]));
    if (s.cwd && s.cwd !== "./") lines.push(kv("cwd", [{ kind: "str", text: q(s.cwd) }]));
    if (s.after.length) lines.push(kv("after", [arr(s.after.map(nameOf))]));
    const g = s.ready;
    const timeout = "timeoutMs" in g ? `, timeout = ${q(fmtDuration(g.timeoutMs))} }` : " }";
    if (g.kind === "log") lines.push(kv("ready", [{ kind: "plain", text: "{ log = " }, { kind: "str", text: q(g.pattern) }, { kind: "plain", text: timeout }]));
    if (g.kind === "port") lines.push(kv("ready", [{ kind: "plain", text: `{ port = ${g.port}${timeout}` }]));
    if (g.kind === "http") lines.push(kv("ready", [{ kind: "plain", text: "{ http = " }, { kind: "str", text: q(g.url) }, { kind: "plain", text: timeout }]));
    if (g.kind === "exit") lines.push(kv("ready", [{ kind: "plain", text: `{ exit = 0${timeout}` }]));
    if (s.restart.on !== "never") {
      lines.push(
        kv("restart", [
          { kind: "plain", text: "{ on = " },
          { kind: "str", text: q(s.restart.on) },
          { kind: "plain", text: `, max = ${s.restart.max}, backoff = ${q(fmtDuration(s.restart.backoffMs))} }` },
        ]),
      );
    }
    if (s.envFile) lines.push(kv("env_file", [{ kind: "str", text: q(s.envFile) }]));
  });

  for (const g of groups) {
    lines.push([]);
    lines.push([{ kind: "table", text: "[[group]]" }]);
    lines.push(kv("name", [{ kind: "str", text: q(g.name) }]));
    lines.push(kv("scripts", [arr(g.scriptIds.map(nameOf))]));
  }
  return lines;
}
