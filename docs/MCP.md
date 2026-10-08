# Driving Scriptr from an AI client

Scriptr is an MCP server. Claude Desktop — or Claude Code, or any MCP client —
can read a project's board, add and move cards, see what the project is
running, bring the stack up in dependency order, and read the logs of whatever
broke.

The desktop app cannot be a stdio MCP server itself: it is a GUI process behind
the single-instance plugin. So the MCP speaker is a small `scriptr-mcp` bridge
shipped inside the app bundle, which talks to a loopback HTTP API the app serves
on `127.0.0.1:7378` behind a bearer token. The token lives in
`~/Library/Application Support/scriptr/mcp.json`, mode 0600.

## Claude Desktop

Claude Desktop reads its own config file, separate from Claude Code's:

`~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "scriptr": {
      "command": "/Applications/Scriptr.app/Contents/MacOS/scriptr-mcp"
    }
  }
}
```

Quit and reopen Claude Desktop afterwards — it only reads that file at launch.

## Claude Code

```bash
claude mcp add --scope user scriptr -- /Applications/Scriptr.app/Contents/MacOS/scriptr-mcp
```

Settings → Integrations shows the exact line, with the port it is actually on.

## Tools

| tool | what it is for |
|---|---|
| `scriptr_status` | is Scriptr up, how many scripts are running, how many cards are in progress |
| `list_projects` | the projects and their paths — call it first if unsure what to pass as `project` |
| `list_tasks` | the board, by column |
| `get_task` | one card in full, including what blocks it |
| `file_task` | add a card, optionally straight into a column |
| `update_task` | retitle, re-prioritise, relabel, assign, or record what blocks it |
| `move_task` | put a card in a column at a position, the way dragging it would |
| `list_scripts` | a project's processes with state, pid, port and command |
| `list_groups` | named groups that can be brought up together |
| `control_script` | start, stop or restart one process |
| `run_group` | bring a whole stack up in dependency order, waiting on each wave's gates |
| `get_logs` | recent output for `script:<name>` or `task:<id>`, ANSI stripped |

`file_task` works while Scriptr is closed: the bridge spools to an inbox the app
drains at launch, so a card is never lost to a quit app.

## Where the line is drawn

Reading is free. Running the user's own configured scripts is allowed — those
are commands they wrote themselves, and the value of this integration is a
client that can bring the stack up and read the failure.

Nothing over this socket can delete a project, script, group or card, edit a
script's command, or change Scriptr's own settings. A card filed from outside
carries a label recording its origin, so the board shows where it came from.

## What it is good at

The combination worth knowing about is **`run_group` → `get_logs`**. A client
can bring up a stack whose parts have real readiness gates — database before
migrations before server — and then read the actual output of the thing that
failed, rather than guessing from an exit code.
