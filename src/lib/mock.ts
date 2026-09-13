// In-memory backend for `pnpm dev` in a plain browser. It mirrors the Figma sample data so
// every screen can be checked against the design without a Tauri window. Not shipped logic.
import type { Backend, EventMap } from "./ipc";
import { computePlan } from "./graph";
import type {
  DetectedScript,
  Group,
  GroupProgress,
  Project,
  RunInfo,
  ScanResult,
  Script,
  Settings,
  Snapshot,
} from "./types";

const ESC = "\x1b[";
const dim = (s: string) => `${ESC}2m${s}${ESC}22m`;
const col = (c: number, s: string) => `${ESC}22;${c}m${s}${ESC}39m`;
const line = (s = "") => `${s}\r\n`;
const scriptrLine = (msg: string, dimRest = false) =>
  line(`${ESC}35m[scriptr]${ESC}39m${dimRest ? dim(msg) : msg}`);

let seq = 100;
const uid = (p: string) => `${p}${++seq}`;

export function createMockBackend(): Backend {
  const t0 = Date.now();
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = <K extends keyof EventMap>(ev: K, payload: EventMap[K]) =>
    listeners.get(ev)?.forEach((h) => h(structuredClone(payload)));

  const noRestart = { on: "never" as const, max: 5, backoffMs: 2000, backoffMaxMs: 32000 };
  const mk = (p: Partial<Script> & Pick<Script, "id" | "projectId" | "name" | "cmd">): Script => ({
    label: null,
    cwd: "./",
    shell: null,
    env: {},
    envFile: null,
    after: [],
    ready: { kind: "instant" },
    restart: noRestart,
    port: null,
    source: null,
    sortOrder: 0,
    ...p,
  });

  const projects: Project[] = [
    { id: "p1", name: "acme-platform", path: "/Users/you/dev/acme-platform", branch: "main", sortOrder: 0 },
    { id: "p2", name: "docs-site", path: "/Users/you/dev/docs-site", branch: "main", sortOrder: 1 },
    { id: "p3", name: "ml-pipeline", path: "/Users/you/dev/ml-pipeline", branch: "exp/lora", sortOrder: 2 },
  ];

  const scripts: Script[] = [
    mk({ id: "api", projectId: "p1", name: "api", label: "runserver", cwd: "./api", sortOrder: 0, port: 8000,
      cmd: "poetry run python manage.py runserver 0.0.0.0:8000", after: ["db", "migrate"], shell: "/bin/zsh -lc",
      ready: { kind: "log", pattern: "Starting development server at", timeoutMs: 60000 },
      restart: { on: "crash", max: 5, backoffMs: 2000, backoffMaxMs: 32000 } }),
    mk({ id: "worker", projectId: "p1", name: "worker", label: "celery", cwd: "./api", sortOrder: 1,
      cmd: "poetry run celery -A acme worker -l info", after: ["migrate"],
      ready: { kind: "log", pattern: "celery@\\S+ ready", timeoutMs: 60000 } }),
    mk({ id: "web", projectId: "p1", name: "web", label: "vite dev", cwd: "./web", sortOrder: 2, port: 5173,
      cmd: "npm run dev", after: ["api"], ready: { kind: "port", port: 5173, timeoutMs: 60000 } }),
    mk({ id: "stripe", projectId: "p1", name: "stripe-mock", sortOrder: 3, port: 12111,
      cmd: "stripe listen --forward-to localhost:8000/webhooks",
      restart: { on: "crash", max: 5, backoffMs: 4000, backoffMaxMs: 16000 } }),
    mk({ id: "migrate", projectId: "p1", name: "migrate", cwd: "./api", sortOrder: 4,
      cmd: "poetry run python manage.py migrate", after: ["db"], ready: { kind: "exit", timeoutMs: 120000 } }),
    mk({ id: "db", projectId: "p1", name: "db", label: "docker up", sortOrder: 5, port: 5432,
      cmd: "docker compose up db", ready: { kind: "port", port: 5432, timeoutMs: 60000 } }),
    mk({ id: "docs", projectId: "p2", name: "dev", label: "astro", sortOrder: 0, port: 4321, cmd: "pnpm dev",
      ready: { kind: "port", port: 4321, timeoutMs: 60000 } }),
    mk({ id: "nb", projectId: "p3", name: "notebook", sortOrder: 0, port: 8888, cmd: "uv run jupyter lab --port 8888",
      ready: { kind: "port", port: 8888, timeoutMs: 60000 } }),
    mk({ id: "train", projectId: "p3", name: "train", sortOrder: 1, cmd: "uv run python train.py --epochs 3",
      ready: { kind: "exit", timeoutMs: 3600000 } }),
  ];

  const groups: Group[] = [
    { id: "g1", projectId: "p1", name: "Full stack", scriptIds: ["db", "migrate", "api", "worker", "web", "stripe"] },
    { id: "g2", projectId: "p1", name: "Backend only", scriptIds: ["api", "worker"] },
  ];

  let settings: Settings = { onQuit: "stop", keepTomlInSync: true, importMode: "merge", defaultShell: "/bin/zsh -lc" };

  const idle = (scriptId: string): RunInfo => ({
    scriptId, state: "idle", pid: null, startedAt: null, endedAt: null, exitCode: null, attempt: 0, maxAttempts: 0,
    nextRetryAt: null, backoffMs: null, degraded: false, gateNote: null,
  });
  const runs = new Map<string, RunInfo>(scripts.map((s) => [s.id, idle(s.id)]));
  const put = (r: RunInfo) => {
    runs.set(r.scriptId, r);
    emit("script:state", r);
  };

  runs.set("api", { ...idle("api"), state: "running", pid: 48213, startedAt: t0 - 842_000, gateNote: "log match" });
  runs.set("worker", { ...idle("worker"), state: "running", pid: 48220, startedAt: t0 - 838_000, gateNote: "log match" });
  runs.set("web", { ...idle("web"), state: "running", pid: 48231, startedAt: t0 - 835_000, gateNote: "port 5173 open" });
  runs.set("stripe", { ...idle("stripe"), state: "backoff", exitCode: 1, startedAt: t0 - 260_000, endedAt: t0 - 8_000,
    attempt: 2, maxAttempts: 5, backoffMs: 4000, nextRetryAt: t0 + 45_000 });
  runs.set("migrate", { ...idle("migrate"), state: "stopped", exitCode: 0, startedAt: t0 - 850_000, endedAt: t0 - 846_000, gateNote: "exit code 0" });
  runs.set("db", { ...idle("db"), state: "stopped", startedAt: t0 - 7_200_000, endedAt: t0 - 900_000 });
  runs.set("nb", { ...idle("nb"), state: "running", pid: 51002, startedAt: t0 - 3_600_000, gateNote: "port 8888 open" });

  // ---- fake terminal output ----
  const buffers = new Map<string, string>();
  const sinks = new Map<string, (b: Uint8Array) => void>();
  const enc = new TextEncoder();
  const write = (id: string, text: string) => {
    buffers.set(id, (buffers.get(id) ?? "") + text);
    sinks.get(id)?.(enc.encode(text));
  };

  const req = (time: string, verb: string, verbCol: number, path: string, status: number, size: number) =>
    line(dim(`[${time}] "${col(verbCol, verb)}${ESC}2m ${path} HTTP/1.1" ${col(status < 400 ? 32 : 31, String(status))}${ESC}2m ${size}`));

  const boot: Record<string, string[]> = {
    api: [
      line(dim("Watching for file changes with StatReloader")),
      line(dim("Performing system checks...")),
      line(),
      line("System check identified no issues (0 silenced)."),
      line(dim("September 13, 2026 - 09:41:22")),
      line(`Django version 5.1.2, using settings ${col(33, "'acme.settings.dev'")}`),
      line(`Starting development server at ${col(34, "http://0.0.0.0:8000/")}`),
      line(dim("Quit the server with CONTROL-C.")),
      line(),
    ],
    worker: [
      line(`${col(34, " -------------- ")}celery@acme-mbp v5.4.0 (opalescent)`),
      line(dim("--- ***** ----- .> transport:   redis://localhost:6379/0")),
      line(dim("-- ******* ---- .> concurrency: 8 (prefork)")),
      line(),
      line(`[tasks]`),
      line(dim("  . acme.orders.tasks.sync_inventory")),
      line(dim("  . acme.billing.tasks.send_invoice")),
      line(),
      line(`${col(32, "celery@acme-mbp ready.")}`),
    ],
    web: [
      line(),
      line(`  ${col(32, "VITE v7.1.3")}  ready in ${col(37, "412")} ms`),
      line(),
      line(`  ${col(32, "➜")}  Local:   ${col(36, "http://localhost:5173/")}`),
      line(dim(`  ➜  Network: use --host to expose`)),
    ],
    stripe: [
      line("Ready! You are using Stripe API Version [2026-04-10]"),
      line(dim("Getting ready…")),
      line(),
    ],
    migrate: [
      line(`${col(36, "Operations to perform:")}`),
      line("  Apply all migrations: admin, auth, billing, orders, sessions"),
      line(`${col(36, "Running migrations:")}`),
      line(`  Applying billing.0042_invoice_due_at... ${col(32, "OK")}`),
    ],
    db: [
      line(dim("[+] Running 1/1")),
      line(` ${col(32, "✔")} Container acme-db-1  ${col(32, "Started")}`),
      line(dim("db-1  | database system is ready to accept connections")),
    ],
    docs: [line(`${col(35, " astro ")} v5.3.0 ready in 318 ms`), line(`┃ Local    ${col(36, "http://localhost:4321/")}`)],
    nb: [line(dim("[I 2026-09-13 08:52:11 ServerApp] Jupyter Server 2.14 is running at:")), line(col(36, "http://localhost:8888/lab"))],
    train: [line("epoch 1/3  loss=0.412"), line("epoch 2/3  loss=0.301"), line("epoch 3/3  loss=0.254")],
  };

  const byId = (id: string) => scripts.find((s) => s.id === id)!;
  const prompt = (id: string) => line(`${col(32, "$")} ${byId(id).cmd}`);

  for (const id of ["api", "worker", "web", "nb"]) write(id, prompt(id) + boot[id].join(""));
  write("api",
    req("13/Sep/2026 09:41:48", "GET", 36, "/api/v1/orders?limit=50", 200, 14821) +
    req("13/Sep/2026 09:41:49", "GET", 36, "/api/v1/me", 200, 512) +
    req("13/Sep/2026 09:41:52", "POST", 35, "/api/v1/checkout", 402, 118) +
    req("13/Sep/2026 09:41:55", "GET", 36, "/api/v1/health", 200, 2));
  write("migrate", prompt("migrate") + boot.migrate.join("") + scriptrLine(" process exited with code 0 after 4s"));
  write("db", prompt("db") + boot.db.join("") + scriptrLine(" stopped"));
  write("stripe",
    prompt("stripe") + boot.stripe.join("") +
    line(dim(`2026-09-13 09:52:04  ${col(36, "-->")}${ESC}2m payment_intent.succeeded [evt_3Qa1]`)) +
    line(dim(`2026-09-13 09:52:06  <--  ${col(32, "[200]")}${ESC}2m POST http://localhost:8000/webhooks`)) +
    line(col(33, "2026-09-13 09:52:31  connection lost: websocket: close 1006")) +
    line() +
    line(col(31, "Error: failed to reconnect to Stripe after 3 attempts")) +
    line(col(31, "       dial tcp 34.194.0.12:443: i/o timeout")) +
    line() +
    scriptrLine(" process exited with code 1 after 4m 12s") +
    scriptrLine(" restart policy: on-crash · attempt 2/5 · waiting 45s…", true));

  // ---- lifecycle simulation ----
  const timers = new Map<string, number[]>();
  const later = (id: string, ms: number, fn: () => void) => {
    const t = window.setTimeout(fn, ms);
    timers.set(id, [...(timers.get(id) ?? []), t]);
  };
  const clearTimers = (id: string) => {
    timers.get(id)?.forEach((t) => clearTimeout(t));
    timers.delete(id);
  };
  const isReady = (id: string) => {
    const r = runs.get(id)!;
    return r.state === "running" || (r.state === "stopped" && r.exitCode === 0 && byId(id).ready.kind === "exit");
  };
  const waitReady = (id: string) =>
    new Promise<void>((resolve) => {
      const tick = () => (isReady(id) ? resolve() : window.setTimeout(tick, 150));
      tick();
    });

  let pid = 60000;
  async function start(id: string, attempt = 0): Promise<void> {
    const s = byId(id);
    const cur = runs.get(id)!;
    if (["running", "starting", "queued"].includes(cur.state)) return;
    clearTimers(id);
    const deps = s.after.filter((d) => !isReady(d));
    put({ ...idle(id), state: "queued", attempt, maxAttempts: attempt ? s.restart.max : 0 });
    if (deps.length) {
      deps.forEach((d) => void start(d));
      await Promise.all(deps.map(waitReady));
    }
    const startedAt = Date.now();
    write(id, line() + prompt(id));
    put({ ...runs.get(id)!, state: "starting", pid: ++pid, startedAt, gateNote: s.ready.kind === "port" ? `waiting for port ${s.ready.port}` : null });
    const lines = boot[id] ?? [line("started")];
    lines.forEach((l, i) => later(id, 120 + i * 90, () => write(id, l)));
    const readyAt = 200 + lines.length * 90 + 300;
    if (s.ready.kind === "exit") {
      later(id, readyAt, () => {
        write(id, scriptrLine(` process exited with code 0 after ${Math.round((Date.now() - startedAt) / 1000)}s`));
        put({ ...runs.get(id)!, state: "stopped", pid: null, exitCode: 0, endedAt: Date.now(), gateNote: "exit code 0" });
      });
    } else {
      const note = s.ready.kind === "port" ? `port ${s.ready.port} open` : s.ready.kind === "log" ? "log match" : s.ready.kind === "http" ? "HTTP 200" : null;
      later(id, readyAt, () => put({ ...runs.get(id)!, state: "running", gateNote: note }));
    }
  }

  async function stop(id: string): Promise<void> {
    const r = runs.get(id)!;
    clearTimers(id);
    if (r.state === "backoff" || r.state === "crashed") {
      put({ ...r, state: "stopped", nextRetryAt: null, attempt: 0 });
      return;
    }
    if (!["running", "starting", "queued"].includes(r.state)) return;
    put({ ...r, state: "stopping" });
    await new Promise((res) => setTimeout(res, 260));
    write(id, line(`^C`) + scriptrLine(" stopped (SIGTERM → process group)"));
    put({ ...runs.get(id)!, state: "stopped", pid: null, endedAt: Date.now(), degraded: false });
  }

  // live chatter + backoff countdowns
  const verbs: [string, number, string, number][] = [
    ["GET", 36, "/api/v1/orders?limit=50", 200], ["GET", 36, "/api/v1/me", 200],
    ["POST", 35, "/api/v1/cart/items", 201], ["GET", 36, "/api/v1/health", 200],
  ];
  window.setInterval(() => {
    const now = new Date();
    const stamp = `${now.getDate()}/Sep/2026 ${now.toTimeString().slice(0, 8)}`;
    if (runs.get("api")?.state === "running" && Math.random() < 0.6) {
      const [v, c, p, st] = verbs[Math.floor(Math.random() * verbs.length)];
      write("api", req(stamp, v, c, p, st, Math.floor(Math.random() * 16000)));
    }
    if (runs.get("worker")?.state === "running" && Math.random() < 0.25) {
      write("worker", line(dim(`[${now.toISOString().slice(0, 19)}: INFO/ForkPoolWorker-3] Task acme.orders.tasks.sync_inventory succeeded in 0.21s`)));
    }
    for (const r of runs.values()) {
      if (r.state === "backoff" && r.nextRetryAt && Date.now() >= r.nextRetryAt) void start(r.scriptId, r.attempt);
    }
  }, 1800);

  window.setInterval(() => {
    const stats = [...runs.values()]
      .filter((r) => r.pid && (r.state === "running" || r.state === "starting"))
      .map((r) => ({ scriptId: r.scriptId, pid: r.pid!, cpu: 0.4 + Math.random() * 3, memBytes: (120 + Math.random() * 90) * 1024 * 1024 }));
    emit("stats", stats);
  }, 1000);

  // `?empty` starts with no projects (screen 01).
  if (new URLSearchParams(location.search).has("empty")) {
    projects.length = 0;
    scripts.length = 0;
    groups.length = 0;
    runs.clear();
  }

  const snapshot = (): Snapshot =>
    structuredClone({ projects, scripts, groups, runs: [...runs.values()], settings, dbPath: "/Users/you/Library/Application Support/scriptr/scriptr.db" });

  const progress = (p: GroupProgress) => emit("group:progress", p);

  const scan = (path: string): ScanResult => {
    const d = (key: string, name: string, cmd: string, cwd: string, extra: Partial<DetectedScript> = {}): DetectedScript => ({
      key, name, label: null, cmd, cwd, port: null, oneShot: false, ready: { kind: "instant" }, suggested: false, ...extra,
    });
    return {
      path,
      name: path.split("/").filter(Boolean).pop() ?? "project",
      branch: "main",
      hasToml: false,
      sources: [
        { file: "package.json", dir: "web/", scripts: [
          d("web:dev", "dev", "npm run dev", "./web", { port: 5173, suggested: true, ready: { kind: "port", port: 5173, timeoutMs: 60000 } }),
          d("web:build", "build", "npm run build", "./web", { oneShot: true, ready: { kind: "exit", timeoutMs: 600000 } }),
          d("web:test", "test", "npm run test -- --watch", "./web"),
        ] },
        { file: "pyproject.toml", dir: "api/", scripts: [
          d("api:runserver", "runserver", "poetry run python manage.py runserver 0.0.0.0:8000", "./api", { port: 8000, suggested: true, ready: { kind: "port", port: 8000, timeoutMs: 60000 } }),
          d("api:migrate", "migrate", "poetry run python manage.py migrate", "./api", { oneShot: true, suggested: true, ready: { kind: "exit", timeoutMs: 600000 } }),
          d("api:celery", "celery", "poetry run celery -A acme worker -l info", "./api", { suggested: true }),
        ] },
        { file: "docker-compose.yml", dir: "./", scripts: [
          d("compose:db", "db", "docker compose up db", "./", { port: 5432, suggested: true, ready: { kind: "port", port: 5432, timeoutMs: 60000 } }),
        ] },
        { file: "Makefile", dir: "./", scripts: [
          d("make:lint", "lint", "make lint", "./", { oneShot: true, ready: { kind: "exit", timeoutMs: 600000 } }),
          d("make:seed", "seed", "make seed", "./", { oneShot: true, ready: { kind: "exit", timeoutMs: 600000 } }),
        ] },
      ],
    };
  };

  const backend: Backend = {
    appState: async () => snapshot(),
    projectScan: async (path) => scan(path),
    projectAdd: async (input) => {
      const pId = uid("p");
      projects.push({ id: pId, name: input.name, path: input.path, branch: "main", sortOrder: projects.length });
      input.scripts.forEach((d, i) => {
        const s = mk({ id: uid("s"), projectId: pId, name: d.name, label: d.label, cmd: d.cmd, cwd: d.cwd, port: d.port, ready: d.ready, sortOrder: i });
        scripts.push(s);
        runs.set(s.id, idle(s.id));
      });
      return snapshot();
    },
    projectRemove: async (projectId) => {
      for (const s of scripts.filter((x) => x.projectId === projectId)) await stop(s.id);
      projects.splice(projects.findIndex((p) => p.id === projectId), 1);
    },
    scriptSave: async (script) => {
      const s = { ...script, id: script.id || uid("s") };
      const i = scripts.findIndex((x) => x.id === s.id);
      if (i >= 0) scripts[i] = s;
      else {
        scripts.push(s);
        runs.set(s.id, idle(s.id));
      }
      return structuredClone(s);
    },
    scriptDelete: async (scriptId) => {
      await stop(scriptId);
      scripts.splice(scripts.findIndex((s) => s.id === scriptId), 1);
    },
    groupSave: async (group) => {
      const g = { ...group, id: group.id || uid("g") };
      const i = groups.findIndex((x) => x.id === g.id);
      if (i >= 0) groups[i] = g;
      else groups.push(g);
      return structuredClone(g);
    },
    groupDelete: async (groupId) => {
      groups.splice(groups.findIndex((g) => g.id === groupId), 1);
    },
    scriptStart: (id) => start(id),
    scriptStop: (id) => stop(id),
    scriptRestart: async (id) => {
      await stop(id);
      await start(id);
    },
    retryCancel: async (id) => {
      const r = runs.get(id)!;
      write(id, scriptrLine(" retries cancelled"));
      put({ ...r, state: "stopped", nextRetryAt: null });
    },
    groupPlan: async (groupId) => {
      const g = groups.find((x) => x.id === groupId)!;
      return computePlan(groupId, g.scriptIds.map(byId));
    },
    groupRun: async (groupId) => {
      const g = groups.find((x) => x.id === groupId)!;
      const plan = computePlan(groupId, g.scriptIds.map(byId));
      if (plan.cycle) {
        const [a, b] = plan.cycle.map((i) => byId(i).name);
        progress({ groupId, state: "failed", wave: 0, totalWaves: 0, degradedScriptId: null, message: `cycle: ${a} → ${b}` });
        throw new Error(`Refusing to start “${g.name}”: ${b} and ${a} wait for each other.`);
      }
      void (async () => {
        for (const [i, wave] of plan.waves.entries()) {
          progress({ groupId, state: "running", wave: i, totalWaves: plan.waves.length, degradedScriptId: null, message: null });
          wave.forEach((id) => void start(id));
          await Promise.all(wave.map(waitReady));
        }
        progress({ groupId, state: "done", wave: plan.waves.length, totalWaves: plan.waves.length, degradedScriptId: null, message: null });
      })();
      return plan;
    },
    groupStop: async (groupId) => {
      const g = groups.find((x) => x.id === groupId)!;
      const plan = computePlan(groupId, g.scriptIds.map(byId));
      progress({ groupId, state: "stopping", wave: 0, totalWaves: plan.waves.length, degradedScriptId: null, message: null });
      for (const wave of [...plan.waves].reverse()) await Promise.all(wave.map(stop));
      progress({ groupId, state: "stopped", wave: 0, totalWaves: plan.waves.length, degradedScriptId: null, message: null });
    },
    groupContinue: async () => {},
    stopEverything: async () => {
      await Promise.all([...runs.keys()].map(stop));
    },
    ptyAttach: async (scriptId, onData) => {
      sinks.set(scriptId, onData);
      onData(enc.encode(buffers.get(scriptId) ?? ""));
    },
    ptyWrite: async (scriptId, data) => {
      if (runs.get(scriptId)?.state !== "running") return;
      write(scriptId, data === "\r" ? "\r\n" : data === "\x03" ? "^C\r\n" : data);
    },
    ptyResize: async () => {},
    configExport: async (projectId) => `${projects.find((p) => p.id === projectId)!.path}/scriptr.toml`,
    configImport: async (_projectId, _path, mode) => ({
      added: 1, updated: 2, removed: mode === "replace" ? 1 : 0,
      preview: mode === "preview" ? "+ script web\n~ script api   ready: port → log\n~ group Full stack   +web" : null,
    }),
    settingsSet: async (s) => {
      settings = s;
      return structuredClone(s);
    },
    dbBackup: async () => {},
    runHistory: async (scriptId) => {
      const r = runs.get(scriptId);
      if (!r?.startedAt) return [];
      const past = [1, 2, 3].map((i) => {
        const startedAt = r.startedAt! - i * 3_600_000;
        return { startedAt, endedAt: startedAt + 1_200_000 + i * 60_000, exitCode: i === 2 ? 1 : 0 };
      });
      return r.endedAt ? [{ startedAt: r.startedAt, endedAt: r.endedAt, exitCode: r.exitCode }, ...past] : past;
    },
    on: async (event, handler) => {
      const set = listeners.get(event) ?? new Set();
      set.add(handler as (p: unknown) => void);
      listeners.set(event, set);
      return () => set.delete(handler as (p: unknown) => void);
    },
  };
  return backend;
}
