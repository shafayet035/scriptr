# Scriptr as an AI project manager — concept

> Status: concept agreed. **Scope: the full project manager** — backlog, priorities, assignment and reporting,
> not just agent runs. Building starts with slice A below; later slices are sketches.
> See `docs/PLAN.md` for what exists today.

## The thesis

Coding agents (Claude Code, opencode, Codex, aider, Gemini CLI) are terminal programs. Scriptr already runs
terminal programs well: real PTYs, lifecycle, dependencies, readiness gates, restart policy, scrollback, history.

So an agent is **just another kind of run** — but one that produces a *change* instead of a service.

The interesting part isn't running them. It's that Scriptr is the only tool in this space that already knows
**how to bring the project up and how to tell when it's actually working**. That turns an agent's claim ("done!")
into something a machine can check:

> *db is listening on 5432 → migrations exited 0 → api matched "Starting development server" → web's port opened →
> tests exited 0 → therefore the task is done.*

Everyone else in this market verifies with a test command. Scriptr can verify against a **running stack**.

## Prior art, and where the wedge is

| Tool | What it does | Gap we exploit |
|---|---|---|
| Conductor (Melty Labs, macOS, $22M A) | Parallel Claude Code / Codex in worktrees, polished native UI | No service orchestration; "done" is a diff a human reads |
| Vibe Kanban (Bloop) | Kanban board over agent CLIs; hosted side shut down Apr 2026, code community-run | Board without a runtime; no readiness gates |
| Crystal (Stravu) | Parallel sessions in worktrees; deprecated Feb 2026 | — |
| Claude Squad / emdash / ccswarm | tmux-style multiplexing of agents | No project model, no gates, no verification |
| Cloud agents (Codex cloud, Copilot agents) | Async backlog draining in a remote sandbox | Can't run *your* local stack with your db and env |

**Positioning:** *the agent manager that can check the work, because it already runs the app.*

A second, quieter wedge: Scriptr is **MIT, local-first, and BYO-subscription**. Two of the four leaders above
died or pivoted inside a year; an open, provider-neutral tool that treats agent CLIs as pluggable adapters is a
safer bet for teams, and a natural open-source contribution surface (one TOML file per agent).

## Model: a Task is a Script with a goal

Reuse the whole engine — supervisor, PTY, DAG, gates, history — instead of building a parallel one.

```
Script  = long-running process  → ready when a gate passes
Task    = agent run             → done when verification passes and a human approves
```

New records (alongside `Project`, `Script`, `Group`, `Run`):

- **Agent** — a provider adapter, data not code:
  `id`, `name`, `cmd` template (`{prompt}`, `{model}`, `{resume}`), `interactive` vs `headless` form,
  `env` keys required, optional `stream_json` parser for cost/turns, permission flags.
  Ships with adapters for Claude Code, opencode, Codex, aider, Gemini CLI; users add their own in
  `~/.config/scriptr/agents/*.toml` or a repo's `scriptr.toml`.
- **Task** — `title`, `goal` (the prompt), `agent`, `model`, `workspace` (worktree branch | in-place | sandbox),
  `after[]` (task dependencies — the *same* Kahn wave scheduler), `verify[]` (script ids + ad-hoc commands with
  gates), `autonomy` (ask / auto-edit / full), `budget` (tokens, wall-clock, turns), `status`,
  `cost`, `diff` summary, `pr_url`.
- **TaskRun** — one attempt, with phases.

### Task lifecycle (phases, not a new state machine)

```
Queued → Preparing (git worktree + branch + env)
       → Working   (agent in a real PTY; you can watch and type)
       → Verifying (project scripts run in the worktree, gates decide)
       → Repairing (failed gate output fed back to the agent, bounded retries)
       → Review     (diff + verification report, human gate)
       → Integrating (commit / push / PR / merge)
       → Done | Failed | Abandoned (worktree discarded)
```

`Repairing` is the restart-with-backoff machinery pointed at a different goal: instead of restarting a crashed
service, it hands the failure back to the agent — *"`pnpm test` exited 1: <tail>. Fix it."* — up to `max` attempts.
That loop is the product.

## What this unlocks that is genuinely ours

1. **Definition of done, picked from the project's own scripts.** The composer shows this project's scripts as a
   checklist (the same UI as import detection): ☑ migrate ☑ test ☑ api up. That *is* the acceptance criteria.
2. **A preview environment per task.** Each worktree gets the dev group with remapped ports, so "Open preview"
   shows the feature running at :5201 while main still runs at :5173. No one else can do this without
   reimplementing Scriptr.
3. **Crash → task, in one click.** The crash banner gains *"Hand to agent"*: last N lines of the PTY, the command,
   the exit code and the gate become a prepared task. The existing product feeds the new one.
4. **Scriptr as an MCP server to its own agents.** Expose `list_scripts`, `start_script`, `wait_until_ready`,
   `tail_logs`, `run_verification` over MCP. The agent inside Scriptr can then bring up the stack, read real logs
   and iterate — instead of blindly running `npm run dev` and guessing. This is the strongest differentiator and
   is cheap: the commands already exist.
5. **Fan-out with the DAG you have.** "Implement endpoint" → three independent tasks in parallel, then a
   dependent "wire the UI" task, with a concurrency cap so you don't melt the machine or the budget.

## Surfaces (fits the existing design language)

- **Sidebar**: a `TASKS` section per project under `GROUPS` / `SCRIPTS`, with the same status dots.
- **Board view**: fourth view mode next to Terminals / Dependencies / Combined log — Queued · Working · Needs
  review · Done. Cards carry agent badge, branch, elapsed, cost, verify dot.
- **Task detail**: agent PTY on the left (interactive — answer its permission prompts); right-hand tabs for
  Diff, Verification (which gates passed), Files, Cost.
- **Composer**: goal, agent, model, branch name, definition-of-done checklist, autonomy, budget.
- **Command palette**: "new task", "retry verification", "hand to agent" alongside today's run/stop verbs.
- **Status bar**: agents working · tasks awaiting review · spend today.

## Safety and trust (non-negotiable for a tool that runs agents)

- Worktree isolation by default; `main` is never the agent's workspace.
- Never auto-push or auto-merge unless explicitly enabled per project.
- Scriptr does not bypass the agent's own permission prompts — it hosts them. Autonomy flags are opt-in.
- Secrets: env files are referenced, never pasted into prompts; a redaction pass on prompt context.
- Budget caps (tokens / wall-clock / turns) with hard stop; "Stop everything" already reaps process trees.
- An audit trail per task: prompt, diff, commands run, exit codes, cost.

## The review loop

Slices B and C, specified in detail: [REVIEW-LOOP.md](REVIEW-LOOP.md) — worktree
per task, PR against a per-task base branch, verify gates, a read-only Reviewer
Agent with a structured verdict, and a merge only a human performs.

## Roadmap sketch

| Slice | Content | Why this order |
|---|---|---|
| **A — Agents as runs** | Agent adapters (TOML), task record, composer, interactive PTY, cost/turn parsing | Smallest thing that is already useful; proves adapters |
| **B — Isolation & review** | Worktree per task, diff view, commit/push/PR, discard | Makes parallel safe |
| **C — Verification & repair** | Definition-of-done from project scripts, verify phase, bounded repair loop | **The differentiator** |
| **D — Board & fan-out** | Board view, queue, concurrency cap, task deps via the DAG | Scales to a backlog |
| **E — Preview envs** | Per-worktree dev stack with port remapping, "Open preview" | The demo that sells it |
| **F — Scriptr MCP server** | Agents get hands on the environment | Compounds C and E |
| **G — Backlog in** | GitHub issues / Linear import, markdown backlog | Completes "project manager" |

## Slice A — agents as runs (built)

Shipped: adapters (`src-tauri/agents/*.toml` + `agents.rs`), stream harvesting (`stream.rs`), the task runner
(`tasks.rs`), storage, the ten IPC commands and the `task:state` event, plus the UI — sidebar section, composer,
task tabs with a header strip, and palette entries. Covered by `src-tauri/tests/tasks.rs`: output streams,
exit 0 → review, non-zero → failed, stop kills the whole process group, a budget stops a runaway agent, and an
unknown agent is refused before anything spawns.

Known gaps in A: `agent_list` only reports built-in and user adapters (project-level `[[agent]]` tables are
honoured when a task starts, but don't appear in the picker); runs are interactive, so cost and session
harvesting only kicks in for adapters whose interactive output is machine-readable; `task_start` resumes
nothing yet (`resume_args` are wired but unused until the repair loop in C).


The smallest useful thing: pick an agent, write a goal, watch it work in a real terminal, keep the result.
Everything else is deferred, but the **data model is PM-shaped from day one** (`priority`, `assignee`, `labels`,
`issueUrl`, `after`, `verify`, `workspace`, `branch` all exist and round-trip), so later slices add behaviour,
not migrations.

### Agent adapters

Data, not code — a TOML file per CLI. Built-ins ship embedded; users drop files in
`~/.config/scriptr/agents/*.toml`; a project can add its own in `scriptr.toml`. Placeholders: `{prompt}`,
`{model}`, `{session}`.

```toml
id   = "claude-code"
name = "Claude Code"
bin  = "claude"
interactive_args = ["{prompt}"]
headless_args    = ["-p", "{prompt}", "--output-format", "stream-json", "--verbose"]
resume_args      = ["--resume", "{session}"]
model_args       = ["--model", "{model}"]
stream           = "claude-json"
models           = ["opus", "sonnet", "haiku"]

[autonomy]
ask       = ["--permission-mode", "default"]
auto-edit = ["--permission-mode", "acceptEdits"]
full      = ["--dangerously-skip-permissions"]
```

Verified against the CLIs installed on this machine (Oct 2026): `claude` 2.1.289, `opencode` 1.18.34,
`cursor-agent`, `gemini`. opencode uses `run --format json`, `-m provider/model`, `--agent`, `-s {session}`,
`--auto`; cursor-agent uses `-p`, `--output-format`, `--resume`, `-f`; gemini uses `-p`, `-m`, `--yolo`.
Adapters whose `bin` isn't on PATH are listed but disabled, with an install hint.

### Behaviour in slice A

- A task runs **in the project directory** (`workspace: "in-place"`). Worktrees are slice B — until then the
  composer warns that the agent edits the working tree directly.
- **Interactive by default**: the agent gets a real PTY, so its own permission prompts work and you can type.
  Headless (`stream`) is used only to harvest session id, turns and cost when the adapter supports it.
- Task status follows the run: `queued → working → review` on exit 0, `failed` on non-zero. `review` means
  "a human should look"; marking done is manual in A, automatic in C once verification exists.
- No auto-commit, no auto-push, ever. The agent's own permission model is hosted, never bypassed, unless the
  user picks `full` autonomy explicitly.

### IPC additions

Commands: `agent_list() -> AgentAdapter[]` · `task_list(projectId) -> Task[]` · `task_save(task) -> Task` ·
`task_delete(taskId)` · `task_start(taskId)` · `task_stop(taskId)` · `task_runs(taskId) -> TaskRun[]` ·
`task_attach(taskId, onData)` / `task_write` / `task_resize` (same channel contract as scripts).
Events: `task:state -> { task, run }`.

### Surfaces in slice A

- `TASKS` section in the sidebar under `SCRIPTS`, same status dots.
- Composer: title, goal, agent, model, autonomy, priority. (Definition-of-done checklist appears in C.)
- Task tab in the terminal area: agent PTY plus a header strip with agent, model, elapsed, turns, cost.
- Palette: "new task", "start task", "stop task".

## Open questions

- **Shared vs isolated services.** Five worktrees × one Postgres: shared instance with a schema per task, or a
  container per task? Shared is cheap but tasks can corrupt each other's data; isolated is correct but heavy.
- **Port allocation.** Needs a deterministic offset scheme and env injection (`PORT`, `DATABASE_URL`) per task —
  and the project's scripts must honour it.
- **Headless vs watch-me.** `claude -p --output-format stream-json` and `opencode run --format json` give
  machine-readable progress; the interactive TUI gives control. Support both; default to interactive for the
  first run of a task and headless for repairs.
- **Adapter drift.** Agent CLIs change monthly. Adapters must be data, versioned, and testable with a smoke run.
- **Merge conflicts** between parallel tasks touching the same files — detect early (overlapping path warning at
  composition time) rather than at merge.
- **Does the PM framing earn its name?** v1 is really "verified agent runs". Calling it a project manager means
  owning a backlog, priorities and reporting — worth deciding before the UI hardens.
