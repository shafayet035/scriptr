//! Git worktrees, one per task (B1 in `docs/REVIEW-LOOP.md`).
//!
//! An agent gets its own checkout on its own branch, so two can work at once
//! and neither touches the tree you have open in your editor. The worktree
//! lives outside the repository — under Scriptr's data directory — because a
//! checkout nested inside the repo confuses watchers, globs and test runners.
//!
//! Removal is never automatic. An agent's uncommitted work *is* the task, and
//! nothing in B1 has pushed it anywhere yet, so tearing a worktree down is an
//! explicit act with a dirty check in front of it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Branch names Scriptr creates are all under this prefix, so a stray one is
/// recognisable and `git branch --list "scriptr/*"` finds the lot.
pub const BRANCH_PREFIX: &str = "scriptr";

#[derive(Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    /// False when the branch and checkout were already there and got reused.
    pub created: bool,
}

/// Runs git in `cwd` and returns stdout, or stderr's last meaningful line.
fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("git failed");
    Err(format!("git {}: {}", args.first().copied().unwrap_or(""), line.trim()))
}

/// The root of the repository containing `dir`, or an error naming what is wrong.
pub fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} no longer exists", dir.display()));
    }
    let root = git(dir, &["rev-parse", "--show-toplevel"])
        .map_err(|_| format!("{} is not a git repository — a worktree needs one", dir.display()))?;
    Ok(PathBuf::from(root))
}

/// Where this task's checkout lives. Deterministic, so it can be found again
/// without storing it.
pub fn path_for(task_id: &str) -> Result<PathBuf, String> {
    let dir = dirs::data_dir().ok_or("no data directory on this system")?.join("scriptr").join("worktrees");
    Ok(dir.join(task_id))
}

/// `scriptr/fix-the-flaky-test-a1b2c3d4`: readable in `git branch`, unique per
/// task, and safe for a ref name.
pub fn branch_name(task_id: &str, title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).take(6).collect::<Vec<_>>().join("-");
    let short: String = task_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    if slug.is_empty() {
        format!("{BRANCH_PREFIX}/{short}")
    } else {
        format!("{BRANCH_PREFIX}/{slug}-{short}")
    }
}

/// Branches in the repository, local first, for a base-branch picker.
pub fn branches(project_dir: &Path) -> Result<Vec<String>, String> {
    let root = repo_root(project_dir)?;
    let out = git(&root, &["branch", "--format=%(refname:short)", "--sort=-committerdate"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|b| !b.is_empty() && !b.starts_with(BRANCH_PREFIX))
        .map(str::to_string)
        .collect())
}

/// The branch the repository is on now, used when a task names no base.
pub fn current_branch(project_dir: &Path) -> Result<String, String> {
    let root = repo_root(project_dir)?;
    let head = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| "the repository has a detached HEAD — pick a base branch for the task".to_string())?;
    Ok(head)
}

fn ref_exists(root: &Path, r: &str) -> bool {
    git(root, &["rev-parse", "--verify", "--quiet", r]).is_ok()
}

/// Gives the task a checkout on its own branch, cut from `base`.
///
/// Idempotent: a task restarted after a crash finds its worktree and keeps the
/// work in it rather than starting over.
pub fn ensure(project_dir: &Path, task_id: &str, title: &str, base: Option<&str>) -> Result<Worktree, String> {
    let root = repo_root(project_dir)?;
    let path = path_for(task_id)?;
    let branch = branch_name(task_id, title);

    // Already set up: reuse it, uncommitted work and all.
    if path.join(".git").exists() {
        // `git worktree list` is the authority — a leftover directory whose
        // registration git has pruned is not a worktree.
        let listed = git(&root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
        if listed.lines().any(|l| l.strip_prefix("worktree ").is_some_and(|p| Path::new(p) == path)) {
            let on = git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_else(|_| branch.clone());
            return Ok(Worktree { path, branch: on, created: false });
        }
    }

    let base = match base.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => {
            if !ref_exists(&root, b) {
                return Err(format!("base branch \"{b}\" does not exist in this repository"));
            }
            b.to_string()
        }
        None => current_branch(project_dir)?,
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    // A stale directory with no registration would make `worktree add` refuse.
    if path.exists() {
        let _ = std::fs::remove_dir_all(&path);
    }
    let _ = git(&root, &["worktree", "prune"]);

    let target = path.to_string_lossy().to_string();
    // The branch may survive a removed worktree; check out rather than recreate.
    let args: Vec<&str> = if ref_exists(&root, &format!("refs/heads/{branch}")) {
        vec!["worktree", "add", &target, &branch]
    } else {
        vec!["worktree", "add", "-b", &branch, &target, &base]
    };
    git(&root, &args)?;
    Ok(Worktree { path, branch, created: true })
}

/// Files the agent changed but did not commit, as porcelain status lines.
pub fn dirty(path: &Path) -> Vec<String> {
    git(path, &["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Commits reachable from the worktree's branch but not from `base`.
pub fn commits_ahead(path: &Path, base: &str) -> usize {
    git(path, &["rev-list", "--count", &format!("{base}..HEAD")])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Tears the checkout down. Refuses while work is uncommitted unless `force`,
/// since nothing has pushed it anywhere yet.
pub fn remove(project_dir: &Path, task_id: &str, force: bool) -> Result<(), String> {
    let root = repo_root(project_dir)?;
    let path = path_for(task_id)?;
    if !path.exists() {
        let _ = git(&root, &["worktree", "prune"]);
        return Ok(());
    }
    let changed = dirty(&path);
    if !changed.is_empty() && !force {
        return Err(format!(
            "{} file{} in this workspace {} uncommitted changes — discarding loses the agent's work",
            changed.len(),
            if changed.len() == 1 { "" } else { "s" },
            if changed.len() == 1 { "has" } else { "have" },
        ));
    }
    let target = path.to_string_lossy().to_string();
    git(&root, &["worktree", "remove", "--force", &target])?;
    let _ = git(&root, &["worktree", "prune"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway repository with one commit on `main`.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("scriptr-wt-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let run = |args: &[&str]| {
                Command::new("git").args(args).current_dir(&dir).output().unwrap();
            };
            run(&["init", "--initial-branch=main"]);
            run(&["config", "user.email", "t@example.com"]);
            run(&["config", "user.name", "Test"]);
            std::fs::write(dir.join("README.md"), "hello\n").unwrap();
            run(&["add", "-A"]);
            run(&["commit", "-m", "first"]);
            Repo(dir)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            // Worktrees of this repo live elsewhere; take them with it.
            if let Ok(list) = git(&self.0, &["worktree", "list", "--porcelain"]) {
                for p in list.lines().filter_map(|l| l.strip_prefix("worktree ")) {
                    if p != self.0.to_string_lossy() {
                        let _ = std::fs::remove_dir_all(p);
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn branch_names_are_readable_and_safe() {
        let b = branch_name("a1b2c3d4-dead-beef", "Fix the flaky webhook test!");
        assert_eq!(b, "scriptr/fix-the-flaky-webhook-test-a1b2c3d4");
        // Long titles are clipped, punctuation never reaches the ref name.
        let long = branch_name("1234abcd", "One two three four five six seven eight");
        assert_eq!(long, "scriptr/one-two-three-four-five-six-1234abcd");
        // A title git could not take still yields a usable ref.
        assert_eq!(branch_name("ffff0000", "???"), "scriptr/ffff0000");
    }

    #[test]
    fn ensure_creates_a_checkout_on_its_own_branch() {
        let repo = Repo::new("create");
        let wt = ensure(&repo.0, "task-create-1", "Add rate limiting", None).unwrap();
        assert!(wt.created);
        assert_eq!(wt.branch, "scriptr/add-rate-limiting-taskcrea");
        assert!(wt.path.join("README.md").is_file(), "the base commit is checked out");
        assert_eq!(git(&wt.path, &["symbolic-ref", "--short", "HEAD"]).unwrap(), wt.branch);
        // The repository itself is untouched: still on main, no extra files.
        assert_eq!(git(&repo.0, &["symbolic-ref", "--short", "HEAD"]).unwrap(), "main");
        assert!(dirty(&repo.0).is_empty());
        remove(&repo.0, "task-create-1", true).unwrap();
    }

    #[test]
    fn ensure_is_idempotent_and_keeps_uncommitted_work() {
        let repo = Repo::new("reuse");
        let first = ensure(&repo.0, "task-reuse-1", "Touch things", None).unwrap();
        std::fs::write(first.path.join("new.txt"), "agent work\n").unwrap();

        let again = ensure(&repo.0, "task-reuse-1", "Touch things", None).unwrap();
        assert!(!again.created, "a restart must not start over");
        assert_eq!(again.path, first.path);
        assert!(again.path.join("new.txt").is_file(), "the agent's work survived");
        remove(&repo.0, "task-reuse-1", true).unwrap();
    }

    #[test]
    fn a_base_branch_that_does_not_exist_is_refused() {
        let repo = Repo::new("base");
        let err = ensure(&repo.0, "task-base-1", "Ship it", Some("staging")).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        assert!(!path_for("task-base-1").unwrap().exists(), "nothing half-created");

        // …and an existing one is cut from exactly there.
        Command::new("git").args(["branch", "staging"]).current_dir(&repo.0).output().unwrap();
        let wt = ensure(&repo.0, "task-base-1", "Ship it", Some("staging")).unwrap();
        let merge_base = git(&wt.path, &["merge-base", "HEAD", "staging"]).unwrap();
        let staging = git(&repo.0, &["rev-parse", "staging"]).unwrap();
        assert_eq!(merge_base, staging);
        remove(&repo.0, "task-base-1", true).unwrap();
    }

    #[test]
    fn removal_refuses_to_throw_away_uncommitted_work() {
        let repo = Repo::new("dirty");
        let wt = ensure(&repo.0, "task-dirty-1", "Write code", None).unwrap();
        std::fs::write(wt.path.join("wip.txt"), "half a feature\n").unwrap();

        let err = remove(&repo.0, "task-dirty-1", false).unwrap_err();
        assert!(err.contains("uncommitted"), "{err}");
        assert!(wt.path.is_dir(), "the refusal must not have deleted anything");

        remove(&repo.0, "task-dirty-1", true).unwrap();
        assert!(!wt.path.exists());
        // Removing again is not an error, and the repo is left clean.
        remove(&repo.0, "task-dirty-1", false).unwrap();
        assert!(!git(&repo.0, &["worktree", "list"]).unwrap().contains("task-dirty-1"));
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = std::env::temp_dir().join(format!("scriptr-wt-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = ensure(&dir, "task-plain-1", "Anything", None).unwrap_err();
        assert!(err.contains("not a git repository"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn branches_lists_bases_without_scriptrs_own() {
        let repo = Repo::new("branches");
        Command::new("git").args(["branch", "staging"]).current_dir(&repo.0).output().unwrap();
        let wt = ensure(&repo.0, "task-br-1", "Noise", None).unwrap();

        let list = branches(&repo.0).unwrap();
        assert!(list.contains(&"main".to_string()) && list.contains(&"staging".to_string()), "{list:?}");
        assert!(!list.iter().any(|b| b.starts_with("scriptr/")), "task branches are not bases: {list:?}");
        assert_eq!(commits_ahead(&wt.path, "main"), 0, "a fresh worktree is level with its base");
        remove(&repo.0, "task-br-1", true).unwrap();
    }
}
