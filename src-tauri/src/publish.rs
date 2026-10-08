//! Turning a task's branch into a pull request (B2 in `docs/REVIEW-LOOP.md`).
//!
//! Three steps, each of which can fail on its own and says so: commit whatever
//! the agent left behind, push the branch, open a PR against the task's base.
//! None of them is fatal to the task — the agent's work is already done and
//! safe on a branch, so a publish failure leaves the task in review with a
//! notice rather than marking it failed.
//!
//! Opening the PR needs `gh`, authenticated. Without it the branch is still
//! pushed and the user is told to open the PR themselves: a degraded result
//! beats refusing to do the part that works.

use std::path::Path;
use std::process::Command;

use crate::worktree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pr {
    pub url: String,
    pub number: i64,
}

/// What a publish achieved, so the caller can describe it precisely.
#[derive(Debug, Default)]
pub struct Published {
    /// Set when Scriptr had to make the commit itself.
    pub committed: Option<String>,
    pub pushed: bool,
    pub pr: Option<Pr>,
    /// Why there is no PR, when there is none.
    pub pr_note: Option<String>,
}

fn gh(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .args(args)
        .current_dir(cwd)
        // gh opens a browser or a pager given the chance; neither belongs in an
        // unattended pipeline.
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| format!("gh is not installed or could not run: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("gh failed");
    Err(line.trim().to_string())
}

/// Whether `gh` is present and logged in. Checked before use so the reason for
/// skipping the PR is accurate rather than a guess.
pub fn gh_ready(cwd: &Path) -> Result<(), String> {
    if which("gh").is_none() {
        return Err("gh is not installed — install the GitHub CLI to have Scriptr open PRs".into());
    }
    gh(cwd, &["auth", "status"]).map(drop).map_err(|_| "gh is not logged in — run `gh auth login`".to_string())
}

/// A bare `which`, so a missing binary is reported as missing rather than as a
/// spawn error.
fn which(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join(bin)).find(|p| p.is_file())
    })
}

/// The PR already open for `branch`, if any. Re-publishing a task should update
/// the existing PR rather than fail trying to open a second one.
pub fn existing_pr(cwd: &Path, branch: &str) -> Option<Pr> {
    let raw = gh(cwd, &["pr", "view", branch, "--json", "url,number,state"]).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    // A closed or merged PR is not one to reuse.
    if v["state"].as_str().is_some_and(|s| s != "OPEN") {
        return None;
    }
    Some(Pr { url: v["url"].as_str()?.to_string(), number: v["number"].as_i64()? })
}

/// The PR body: what was asked for, and what the branch actually contains.
fn pr_body(goal: &str, commits: &[String], stat: Option<&str>) -> String {
    let mut body = String::from("### Goal\n\n");
    body += goal.trim();
    if !commits.is_empty() {
        body += "\n\n### Commits\n\n";
        for c in commits {
            body += &format!("- {c}\n");
        }
    }
    if let Some(stat) = stat {
        body += &format!("\n{}\n", stat.trim());
    }
    body += "\n---\nOpened by [Scriptr](https://github.com/shafayet035/scriptr). \
             The agent's work has not been reviewed by a human.\n";
    body
}

/// Commits, pushes and opens the PR. Each step's failure stops the rest and is
/// returned as an error; partial progress is reported through `Published` so
/// the caller can say "pushed, but no PR".
pub fn publish(
    worktree_path: &Path,
    branch: &str,
    base: &str,
    title: &str,
    goal: &str,
    draft: bool,
) -> Result<Published, String> {
    let mut done = Published {
        committed: worktree::commit_all(worktree_path, &format!("{title}\n\nFiled in Scriptr."))?,
        ..Default::default()
    };
    if worktree::commits_ahead(worktree_path, base) == 0 {
        return Err(format!("nothing to publish — the branch has no commits {base} does not already have"));
    }

    let remote = worktree::default_remote(worktree_path)
        .ok_or("this repository has no remote to push to — add one with `git remote add origin …`")?;
    worktree::push(worktree_path, branch, &remote)?;
    done.pushed = true;

    // From here on a failure is not fatal: the work is on the remote.
    if let Err(why) = gh_ready(worktree_path) {
        done.pr_note = Some(format!("{why}. The branch is pushed — open the PR yourself."));
        return Ok(done);
    }
    if let Some(pr) = existing_pr(worktree_path, branch) {
        done.pr = Some(pr);
        return Ok(done);
    }

    let commits = worktree::log_since(worktree_path, base, 20);
    let stat = worktree::diff_stat(worktree_path, base);
    let body = pr_body(goal, &commits, stat.as_deref());

    let mut args = vec!["pr", "create", "--base", base, "--head", branch, "--title", title, "--body", &body];
    if draft {
        args.push("--draft");
    }
    match gh(worktree_path, &args) {
        // `gh pr create` prints the URL it made.
        Ok(out) => {
            let url = out.lines().rev().find(|l| l.contains("/pull/")).unwrap_or("").trim().to_string();
            let number = url.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            done.pr = (!url.is_empty()).then_some(Pr { url, number });
            if done.pr.is_none() {
                done.pr_note = Some(format!("gh did not report a PR url: {out}"));
            }
        }
        Err(why) => done.pr_note = Some(format!("the branch is pushed, but gh could not open a PR: {why}")),
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_states_the_goal_and_what_the_branch_holds() {
        let body = pr_body(
            "Rate-limit the checkout endpoint",
            &["Add a token bucket".into(), "Cover it with tests".into()],
            Some(" 3 files changed, 92 insertions(+)"),
        );
        assert!(body.contains("### Goal"));
        assert!(body.contains("Rate-limit the checkout endpoint"));
        assert!(body.contains("- Add a token bucket"));
        assert!(body.contains("3 files changed"));
        // A human reading this must not mistake it for reviewed work.
        assert!(body.contains("not been reviewed by a human"));
    }

    #[test]
    fn a_body_with_no_commits_still_has_the_goal() {
        let body = pr_body("Do the thing", &[], None);
        assert!(body.contains("Do the thing"));
        assert!(!body.contains("### Commits"), "an empty section is noise: {body}");
    }

    #[test]
    fn publishing_without_a_remote_says_which_step_failed() {
        // A repo with a commit on a branch, and nowhere to push it.
        let dir = std::env::temp_dir().join(format!("scriptr-pub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git").args(args).current_dir(&dir).output().unwrap();
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "first"]);
        run(&["checkout", "-b", "scriptr/work"]);
        std::fs::write(dir.join("b.txt"), "2\n").unwrap();

        let err = publish(&dir, "scriptr/work", "main", "Work", "goal", false).unwrap_err();
        assert!(err.contains("no remote"), "{err}");
        // The commit still happened — that part succeeded before the failure.
        assert!(worktree::dirty(&dir).is_empty(), "the agent's work was committed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_branch_level_with_its_base_has_nothing_to_publish() {
        let dir = std::env::temp_dir().join(format!("scriptr-pub-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            Command::new("git").args(args).current_dir(&dir).output().unwrap();
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.txt"), "1\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "first"]);
        run(&["checkout", "-b", "scriptr/idle"]);

        let err = publish(&dir, "scriptr/idle", "main", "Nothing", "goal", false).unwrap_err();
        assert!(err.contains("nothing to publish"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
