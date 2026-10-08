# The review loop — from goal to a PR you only have to merge

Extends [AI-PM.md](AI-PM.md), slices B and C. The target, in the user's words:
a task reaches Review, a **Reviewer Agent** spawns by itself, checks the code
and the PR, and the only thing left for a human is pressing Merge — into a base
branch chosen per task, settable from the UI or over MCP.

## The pipeline

```
Backlog ──you or MCP file it
   │
Ready ─────────────────────────────── scheduler: deps met, slot free
   │
Working ───────────────────────────── author agent, in its own worktree
   │                                  commits as it goes
Verifying ─────────────────────────── the task's verify scripts, through the
   │                                  existing gate machinery
   │         gates red ──> repair turn to the author (capped) ──┐
   │                                                            │
   │<───────────────────────────────────────────────────────────┘
   │   gates green
Publishing ────────────────────────── push branch, open PR against its base
   │
Reviewing ─────────────────────────── reviewer agent: diff + goal + gate output
   │
   │         changes requested ──> repair turn to the author (capped) ──┐
   │                                                                     │
   │<──────────────────────────────────────────────────── re-verify, re-review
   │   approved
Ready to merge ────────────────────── YOU. one button.
   │
Done ──────────────────────────────── the PR merged (observed, not asserted)
```

Everything between Ready and Ready-to-merge runs unattended. The human appears
exactly twice: writing the goal, and merging. A third appearance is possible but
is a failure mode, not a step — when repair exhausts its attempts the task stops
in Ready-to-merge carrying the reviewer's findings, and asks for a decision.

### Why verify runs before review

Gates are deterministic, cheap and fast; a reviewer agent is slow, non-committal
and costs tokens. Never pay a model to review code that does not compile. The
gate output is also the most useful thing the reviewer can be handed — it turns
"does this look right?" into "the suite says X, does the diff explain X?".

## The Reviewer Agent

**A role, not a binary.** Same adapters as any other agent (`claude`,
`opencode`, …) with a different prompt, a different session and read-only
permissions. Configured per project:

```toml
[review]
agent   = "claude"        # default: the project's agent
effort  = "high"          # worth more than the author's, it is cheaper than a bug
checks  = ["correctness", "security", "tests", "migrations", "conventions"]
attempts = 2              # repair rounds before it wants a human
```

Four properties are not negotiable:

**It cannot write.** A reviewer that edits is an author, and then nothing
independent has looked at the result. It runs with the adapter's read-only
autonomy and no edit tools. If a finding needs fixing, it says so and the author
agent fixes it.

**It gets a fresh session.** Handing the author's own context back to it
produces a rubber stamp — it already believes the work is correct; that is why
it exited. The reviewer starts cold, with the diff and the goal and nothing
else, so it can disagree.

**A different agent is allowed, and better.** `author = claude`,
`reviewer = opencode` means two different models have to agree. Same-model review
catches typos and misses blind spots, because the blind spots correlate.

**Its verdict is structured.** Prose cannot drive a board. The reviewer is asked
for JSON and its reply is parsed:

```json
{
  "verdict": "approve" | "changes_requested",
  "summary": "one line for the card",
  "findings": [
    {"severity": "blocker"|"major"|"minor"|"nit",
     "file": "src/api/checkout.ts", "line": 142,
     "summary": "...", "why_it_matters": "..."}
  ]
}
```

Only `blocker` and `major` block a merge; `minor` and `nit` ride along as PR
comments. Unparseable output is a failed review, not an approval — the one
direction this must never fail is open.

### What it is handed

- the task's goal, verbatim — the definition of done it is judging against
- `git diff <base>...<branch>` and the commit messages
- the verify scripts' output, pass or fail
- the project's conventions file (`CLAUDE.md`, `CONTRIBUTING.md`) when present

Not the author's transcript. See above.

## Target branch

Per task, with a sane default. Not an enum of `main | staging` — a branch name,
because the next project will say `develop` or `release/24.3`.

- `projects.default_base` — set once per project, defaults to the repo's HEAD.
- `tasks.base_branch` — `Option<String>`, overrides it for one task.
- The UI offers a dropdown populated from `git branch --format=%(refname:short)`,
  with the project default preselected.
- MCP `file_task` grows a `base` string: *"branch the PR should target, e.g.
  main or staging. Omit for the project default."*

A base that does not exist is refused when the task is started, not when it is
filed — filing happens while the app may be closed, and branches come and go.

## The merge boundary

**The agent can approve. Only a human merges.** This is the one hard line in the
design, and it is why the whole loop is safe to leave unattended: the worst an
agent can do is leave an unmerged branch and a wrong opinion.

So: no `gh pr merge --auto`, ever, and no scheduled merge. The Merge button in
Scriptr calls `gh pr merge` at the moment you click it, with the method the
project configures (`squash` by default). `done` is then set by *observing* the
PR merged — if you merge on GitHub in your browser instead, Scriptr notices and
the card still moves. A status that reports reality beats a status someone
remembered to click.

## What has to be built

In dependency order. Each slice is useful alone, which is the point.

**B1 — worktree per task. Built.** `WorkspaceMode::Worktree` stops being a refusal.
`git worktree add` under a Scriptr-owned directory, branch named from the task
(`scriptr/<short-id>-<slug>`), and removal is always
deliberate — see below. Unlocks concurrency: two agents stop fighting over one
checkout.

Removal is never automatic, which is a change from the first sketch of this
plan. Nothing has pushed the agent's work anywhere yet, so an automatic
teardown on cancel or on `done` would be silent data loss. Discarding is a menu
item that refuses while the checkout is dirty, and says how many files it would
destroy before asking again. Deleting the task tries the same gentle removal
and, when it is refused, keeps the checkout and logs where it is.

What B1 does not do: **dependencies are not carried into the worktree.** A fresh
checkout has no `node_modules`, no `.venv`, no untracked `.env`, so an agent
whose task needs them has to install them — the composer says so, and the
terminal repeats it on the first run. Linking or copying a configured list of
ignored paths is the obvious follow-up.

**B2 — publish. Built.** On a successful run of an isolated task: commit
anything the agent left uncommitted, push the branch, open a PR against
`base_branch` with a body built from the goal, the commit subjects and the
diffstat. The task learns its `pr_url` and `pr_number`, and the card grows a
chip that opens it.

Three rules this slice settled:

**Only a worktree task is ever published.** An in-place task's diff is mixed in
with whatever the developer has open, so committing it would commit their work
too. Scriptr never commits in the checkout you have open — the setting does not
even apply there.

**A publish failure is not a task failure.** The agent's work is already
committed on a branch. A missing remote, a rejected push or an unauthenticated
`gh` leaves the task in review with a notice naming the step that did not
happen, and a *Publish* action to retry. Partial progress is reported
truthfully: "pushed, but gh could not open a PR" is a real outcome.

**Publishing twice reuses the PR.** `gh pr view` is consulted before
`gh pr create`, so a retry after a fix updates the existing PR rather than
failing on a duplicate — and a closed or merged PR is not reused.

PRs open as ready rather than draft, because until C3 lands there is no
reviewer to flip a draft to ready, and a permanently-draft PR is just friction.
That flips when the reviewer arrives.

**C1 — verify.** `verifying` finally gets a writer: run `task.verify` through
the existing scheduler and gates, pass/fail into the task. This is the slice
that makes `done` mean something, and it is already half-built — the storage,
the cascade and the gate machinery all exist.

**C2 — repair.** A capped loop feeding failures back to the author's session.
Shared by gates and reviews, so write it once.

**C3 — the Reviewer Agent.** The role, the prompt, the JSON contract, the
findings panel on the card. Smallest slice of the five, and worth nothing
without B2.

**C4 — merge.** The button, the method config, and merge observation driving
`done`.

### Board changes

Columns stop describing the agent and start describing who is waiting:

| column | statuses | who is waiting |
|---|---|---|
| Backlog | backlog, cancelled | nobody |
| Ready | queued | a slot |
| Working | working, verifying, publishing, reviewing | the machine |
| **Ready to merge** | review | **you** |
| Done | done | nobody |

Renaming Review to **Ready to merge** is not cosmetic: it is the promise of this
design, written where you look. A card there is either approved and waiting for
one click, or flagged with the findings that beat the repair loop.

## Costs this introduces

- **Token spend roughly doubles per task.** A reviewer on every task is a second
  full agent run. Hence the per-project toggle and the effort dial — and the
  ordering that refuses to review code the gates already rejected.
- **A wrong approval is worse than no review**, because it launders a bad diff
  as checked. Mitigated by the structured verdict (an unparseable or empty
  review fails), by read-only reviewers, and by never auto-merging.
- **Repair loops can burn budget quietly.** The existing per-task token and
  wall-clock budgets cover the whole pipeline, not each turn; exhausting them
  stops everything and asks for a human.

## Open decisions

1. **One reviewer or a panel?** Separate correctness / security / conventions
   passes catch more and cost more, and they can run in parallel. The config
   models it as a list from the start; the default is one.
2. **Draft or ready PR?** Opening as a draft until the reviewer approves keeps
   your PR list honest and keeps CI quiet. Leaning draft, flipped to ready on
   approval.
3. **Does the reviewer post to GitHub?** Writing findings as PR review comments
   makes them visible to teammates and reviewable in the place you already read
   code; keeping them in Scriptr keeps a wrong opinion out of a shared record.
   Leaning: in Scriptr always, on the PR behind a per-project opt-in.
