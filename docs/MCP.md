# Driving Scriptr from an AI agent

Scriptr speaks MCP, so Claude Code — or any MCP client — can see what a project
is running, read why it broke, bring the stack up, and file work onto the board.

The desktop app cannot be a stdio MCP server itself: it is a GUI process behind
the single-instance plugin. So the MCP speaker is a small `scriptr-mcp` bridge
shipped inside the app bundle, which talks to a loopback HTTP API the app
serves on `127.0.0.1:7378` behind a bearer token. The token lives in
`~/Library/Application Support/scriptr/mcp.json`, mode 0600.

```bash
claude mcp add --scope user scriptr -- /Applications/Scriptr.app/Contents/MacOS/scriptr-mcp
```

Settings → Integrations shows the exact line, with the port it is actually on.

## Tools

| tool | what it is for |
|---|---|
| `scriptr_status` | is Scriptr up, how many scripts are running, how many agents are working |
| `list_projects` | the projects and their paths — call it first if unsure what to pass as `project` |
| `list_scripts` | a project's processes with state, pid, port and command |
| `list_groups` | named groups that can be brought up together |
| `list_tasks` | tasks and their board column |
| `get_task` | one task in full: goal, branch, workspace, and every run with exit code, turns and cost |
| `get_logs` | recent output for `script:<name>` or `task:<id>`, ANSI stripped |
| `control_script` | start, stop or restart one process |
| `run_group` | bring a whole stack up in dependency order, waiting on each wave's gates |
| `file_task` | add work to the backlog, with agent, effort, workspace and base branch |
| `update_task` | move, re-prioritise, retarget or rewrite a filed task |
| `start_task` | run a task's agent now — **gated**, see below |
| `stop_task` | stop a running agent, signalling its process group |

`file_task` works while Scriptr is closed: the bridge spools to an inbox the app
drains at launch, so a filed task is never lost to a quit app.

## Where the line is drawn

Reading is free. Running the user's own configured scripts is allowed — those
are commands they wrote themselves, and the value of this integration is an
agent that can bring the stack up and read the failure.

**Starting an agent run is gated.** `start_task` spends tokens and edits code,
and the point of filing to a board is that a human sees the work before it
happens. It is refused unless *Settings → Integrations → Let agents start runs*
is on, which it is not by default. The refusal says so, in words meant to be
relayed to the user.

Nothing over this socket can delete a project, script, group or task, edit a
script's command, or change Scriptr's own settings. A task filed from outside
lands in **Backlog** with ask-first autonomy and a label recording its origin.

## What it is good at

The combination worth knowing about is **`run_group` → `get_logs`**. An agent
can bring up a stack whose parts have real readiness gates — database before
migrations before server — and then read the actual output of the thing that
failed, rather than guessing from an exit code. That loop is the reason this
API exists, and it is what makes verification (slice C of
[REVIEW-LOOP.md](REVIEW-LOOP.md)) possible at all.
