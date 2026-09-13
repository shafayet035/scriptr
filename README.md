# Scriptr

**The pile of terminal tabs your project needs — server, worker, frontend, db, migrations — as one list you can start, stop and watch, with the right things starting in the right order.**

Scriptr is a small, native-feeling desktop app built with [Tauri](https://tauri.app) and Rust. Point it at a project folder, and it finds the commands you already have (`package.json`, `pyproject.toml`, `Makefile`, `docker-compose.yml`, `Procfile`, `*.sh`), runs each one in a real terminal, and knows that `api` shouldn't start until `db` is accepting connections and `migrate` has exited cleanly.

> **Status: early development.** macOS is the primary target today; Windows and Linux builds are planned. Expect rough edges and breaking changes to `scriptr.toml` until 1.0.

## Features

- **Real terminals** — every script runs in a PTY rendered with xterm.js: colours, progress bars, prompts, keyboard input and Ctrl+C all work.
- **Dependencies and readiness gates** — a script can start after others, and counts as *up* when a port opens, a log line matches, an HTTP endpoint returns 200, or (for one-shots like migrations) it exits with code 0.
- **Groups that start in waves** — "Full stack", "Backend only": Scriptr builds a dependency graph, rejects cycles, starts each wave in parallel and waits for every gate before the next. Shutdown runs in reverse.
- **Crash handling** — per-script restart policy (never / on crash / always) with exponential backoff, attempt counters and a clear crash banner.
- **Stops the whole process tree** — `npm run dev` and `poetry run …` fork children; Scriptr signals the process group (SIGTERM, grace period, SIGKILL) so nothing is left holding your ports.
- **Command palette** — ⌘K to fuzzy-find and run, restart or stop anything across projects.
- **Local first, team friendly** — everything lives in a local SQLite database; export a `scriptr.toml` to commit and share the same setup with your team.
- **Native feel** — system font, native menus and dialogs, sidebar vibrancy, window state restore.

## Install

There are no signed releases yet, so build from source:

```bash
git clone https://github.com/shafayet035/scriptr.git
cd scriptr
pnpm install
pnpm tauri build
```

The app bundle lands in `src-tauri/target/release/bundle/` (`macos/Scriptr.app` and a `.dmg` on macOS). Drag it into `/Applications`.

Prerequisites: Rust (stable), Node.js 20+, pnpm 10+, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

## `scriptr.toml`

Commit this at the project root. It holds commands and wiring only — never secrets (reference an env file instead).

```toml
[[script]]
name    = "db"
cmd     = "docker compose up db"
ready   = { port = 5432, timeout = "60s" }

[[script]]
name    = "migrate"
cmd     = "poetry run python manage.py migrate"
cwd     = "./api"
after   = ["db"]
ready   = { exit = 0, timeout = "2m" }

[[script]]
name    = "api"
cmd     = "poetry run python manage.py runserver 0.0.0.0:8000"
cwd     = "./api"
after   = ["db", "migrate"]
ready   = { log = "Starting development server at", timeout = "60s" }
restart = { on = "crash", max = 5, backoff = "2s" }
env_file = ".env.local"

[[group]]
name    = "Full stack"
scripts = ["db", "migrate", "api"]
```

When you add a folder that already contains a `scriptr.toml`, Scriptr offers to import it instead of scanning.

## How it works

```
┌──────────── SolidJS UI (webview) ────────────┐
│ sidebar · tabs · xterm.js · graph · palette  │
└──────▲──────────────────────────┬────────────┘
       │ state events (low rate)  │ commands
       │ PTY bytes via ipc::Channel (batched 16 ms / 8 KB)
┌──────┴──────────────────────────▼────────────┐
│ Rust core (tokio)                            │
│ supervisor per run · scheduler per group     │
│ portable-pty · gates · ring buffer (~5k ln)  │
│ rusqlite · notify · sysinfo · toml           │
└──────────────────────────────────────────────┘
```

- `src/` — SolidJS frontend. `src/lib/types.ts` is the IPC contract; `src/lib/mock.ts` is an in-memory backend for browser development.
- `src-tauri/src/` — Rust core: `supervisor.rs` (per-run state machine, gates, restart), `scheduler.rs` (waves), `graph.rs` (Kahn's algorithm + cycle edge), `pty.rs` (spawn, batching, ring buffer), `detect.rs` (manifest scanning), `config.rs` (`scriptr.toml`), `db.rs`.
- `docs/PLAN.md` — milestones, design decisions and known gaps.

Script lifecycle: `idle → queued → starting → running → stopping → stopped`, with `crashed → backoff → starting` when a restart policy applies.

## Development

```bash
pnpm tauri dev   # desktop app with hot reload
pnpm dev         # UI only in the browser, mock backend (add ?empty for a fresh state)
pnpm typecheck && pnpm build
cd src-tauri && cargo test
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## Roadmap

- Windows process hygiene (Job Objects, `CTRL_BREAK_EVENT`) and Linux builds
- HTTPS health checks
- Keep processes running in the tray after the window closes
- Signed releases and auto-update
- Combined, time-interleaved log view across a group

## License

[MIT](LICENSE)
