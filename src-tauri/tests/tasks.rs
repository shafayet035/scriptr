//! End-to-end task runner tests: real agent processes in real PTYs, no Tauri
//! window. The "agent" is `sh`, declared as a project-level adapter.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use scriptr_lib::db::Db;
use scriptr_lib::model::{
    now_ms, Autonomy, GroupProgress, Project, RunInfo, Task, TaskRun, TaskStatus, WorkspaceMode,
};
use scriptr_lib::supervisor::EventSink;
use scriptr_lib::tasks::TaskRunner;

#[derive(Default)]
struct Recorder {
    statuses: Mutex<Vec<TaskStatus>>,
}

impl EventSink for Recorder {
    fn run_changed(&self, _info: &RunInfo) {}
    fn group_progress(&self, _progress: &GroupProgress) {}
    fn task_changed(&self, task: &Task, _run: Option<&TaskRun>) {
        self.statuses.lock().unwrap().push(task.status);
    }
}

/// A project folder whose `scriptr.toml` declares `sh` as an agent, so the
/// runner exercises the real adapter path.
fn project_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("scriptr-task-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("scriptr.toml"),
        r#"
[[agent]]
id = "fake"
name = "Fake agent"
bin = "sh"
interactive_args = ["-c", "{prompt}"]
"#,
    )
    .unwrap();
    dir
}

/// Turns a project folder into a git repository with one commit on `main`,
/// so worktree tasks have something to branch from.
fn git_init(dir: &PathBuf) {
    let run = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["add", "-A"]);
    run(&["commit", "-m", "first"]);
}

fn task(goal: &str) -> Task {
    Task {
        id: "t1".into(),
        project_id: "p".into(),
        title: "test task".into(),
        goal: goal.into(),
        agent_id: "fake".into(),
        model: None,
        autonomy: Autonomy::Ask,
        effort: None,
        workspace: WorkspaceMode::InPlace,
        branch: None,
        base_branch: None,
        after: vec![],
        verify: vec![],
        status: TaskStatus::Backlog,
        priority: 1,
        assignee: None,
        labels: vec![],
        issue_url: None,
        budget_tokens: None,
        budget_seconds: None,
        created_at: now_ms(),
        updated_at: now_ms(),
        sort_order: 0,
    }
}

struct Harness {
    db: Arc<Db>,
    runner: Arc<TaskRunner>,
    events: Arc<Recorder>,
    output: Arc<Mutex<Vec<u8>>>,
    _dir: PathBuf,
}

fn setup(name: &str, task: Task) -> Harness {
    let dir = project_dir(name);
    let db = Arc::new(Db::open_in_memory().unwrap());
    db.insert_project(&Project {
        id: "p".into(),
        name: "p".into(),
        path: dir.to_string_lossy().into_owned(),
        branch: None,
        sort_order: 0,
    })
    .unwrap();
    db.upsert_task(&task).unwrap();

    let events = Arc::new(Recorder::default());
    let runner = TaskRunner::new(db.clone(), events.clone(), Arc::default());
    let output = Arc::new(Mutex::new(Vec::new()));
    let sink = output.clone();
    runner.attach(&task.id, Box::new(move |bytes| {
        sink.lock().unwrap().extend_from_slice(&bytes);
        true
    }));
    Harness { db, runner, events, output, _dir: dir }
}

impl Harness {
    fn status(&self) -> TaskStatus {
        self.db.task("t1").unwrap().status
    }

    fn run(&self) -> TaskRun {
        self.db.task_runs("t1").unwrap().pop().expect("a run was recorded")
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    async fn wait_for(&self, want: TaskStatus, within: Duration) -> TaskStatus {
        let deadline = Instant::now() + within;
        loop {
            let got = self.status();
            if got == want || Instant::now() > deadline {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn alive(pid: u32) -> bool {
    kill(Pid::from_raw(pid as i32), None) != Err(Errno::ESRCH)
}

#[tokio::test]
async fn agent_output_streams_and_exit_zero_asks_for_review() {
    let h = setup("review", task("printf 'rate limiter added\\n'"));
    h.runner.start("t1").await.unwrap();

    assert_eq!(h.wait_for(TaskStatus::Review, Duration::from_secs(5)).await, TaskStatus::Review);
    let text = h.text();
    assert!(text.contains("rate limiter added"), "agent output missing: {text}");
    assert!(text.contains("[scriptr]"), "scriptr notices missing: {text}");
    assert!(text.contains("ready for review"), "completion notice missing: {text}");

    let run = h.run();
    assert_eq!(run.exit_code, Some(0));
    assert!(run.started_at.is_some() && run.ended_at.is_some());
    assert!(run.pid.is_none(), "pid is cleared once the agent exits");

    let seen = h.events.statuses.lock().unwrap().clone();
    assert_eq!(seen.first(), Some(&TaskStatus::Queued));
    assert!(seen.contains(&TaskStatus::Working));
    assert_eq!(seen.last(), Some(&TaskStatus::Review));
}

#[tokio::test]
async fn stopping_kills_the_whole_process_group() {
    // The agent backgrounds a child, like a real one spawning a test run.
    let h = setup("stop", task("sleep 30 & echo child $!; wait"));
    h.runner.start("t1").await.unwrap();
    assert_eq!(h.wait_for(TaskStatus::Working, Duration::from_secs(5)).await, TaskStatus::Working);

    // Wait for the child's pid to appear in the output.
    let deadline = Instant::now() + Duration::from_secs(5);
    let child = loop {
        if let Some(pid) = h
            .text()
            .lines()
            .find_map(|l| l.strip_prefix("child ").and_then(|p| p.trim().parse::<u32>().ok()))
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the agent never reported its child pid");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let leader = h.run().pid.expect("a running agent has a pid");
    assert!(alive(child), "the child should be running before the stop");

    let stopped_in = Instant::now();
    h.runner.stop("t1").await.unwrap();
    assert_eq!(h.wait_for(TaskStatus::Cancelled, Duration::from_secs(5)).await, TaskStatus::Cancelled);
    assert!(stopped_in.elapsed() < Duration::from_millis(1500), "stop took {:?}", stopped_in.elapsed());

    // Both the agent and what it spawned are gone.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!alive(leader), "the agent survived the stop");
    assert!(!alive(child), "the agent's child survived the stop");
    assert!(h.text().contains("stopped by you"), "missing stop notice: {}", h.text());
}

#[tokio::test]
async fn a_non_zero_exit_fails_the_task() {
    let h = setup("fail", task("echo 'could not apply the patch' >&2; exit 3"));
    h.runner.start("t1").await.unwrap();

    assert_eq!(h.wait_for(TaskStatus::Failed, Duration::from_secs(5)).await, TaskStatus::Failed);
    assert_eq!(h.run().exit_code, Some(3));
    assert!(h.text().contains("agent exited 3"), "missing exit notice: {}", h.text());
}

#[tokio::test]
async fn the_budget_stops_a_runaway_agent() {
    let mut t = task("sleep 30");
    t.budget_seconds = Some(1);
    let h = setup("budget", t);
    h.runner.start("t1").await.unwrap();

    assert_eq!(h.wait_for(TaskStatus::Failed, Duration::from_secs(8)).await, TaskStatus::Failed);
    assert!(h.text().contains("budget of 1s reached"), "missing budget notice: {}", h.text());
    assert!(!alive(h.run().pid.unwrap_or(1)) || h.run().pid.is_none());
}

#[tokio::test]
async fn an_unknown_agent_is_refused_before_anything_spawns() {
    let mut t = task("echo nope");
    t.agent_id = "not-installed".into();
    let h = setup("unknown", t);

    let err = h.runner.start("t1").await.unwrap_err();
    assert!(err.contains("unknown agent"), "unhelpful error: {err}");
    assert_eq!(h.status(), TaskStatus::Backlog, "a refused start must not move the task");
}

/// B1: an isolated task runs in its own checkout on its own branch, and the
/// project's own tree is left exactly as it was.
#[tokio::test]
async fn a_worktree_task_runs_on_its_own_branch() {
    let mut t = task("pwd > where.txt; git rev-parse --abbrev-ref HEAD >> where.txt");
    t.workspace = WorkspaceMode::Worktree;
    let h = setup("worktree", t);
    let project = PathBuf::from(h.db.project("p").unwrap().path);
    git_init(&project);

    h.runner.start("t1").await.unwrap();
    assert_eq!(h.wait_for(TaskStatus::Review, Duration::from_secs(10)).await, TaskStatus::Review);

    // The branch is on the record, so the work is findable after a crash.
    let branch = h.db.task("t1").unwrap().branch.expect("the branch is recorded");
    assert!(branch.starts_with("scriptr/"), "{branch}");

    // The agent's own view of where it was: not the project directory.
    let wt = scriptr_lib::worktree::path_for("t1").unwrap();
    let saw = std::fs::read_to_string(wt.join("where.txt")).expect("the agent wrote inside the worktree");
    let mut lines = saw.lines();
    let cwd = std::fs::canonicalize(lines.next().unwrap()).unwrap();
    assert_eq!(cwd, std::fs::canonicalize(&wt).unwrap(), "the agent ran in the worktree");
    assert_eq!(lines.next().unwrap(), branch, "…and on the task's branch");

    // The checkout you have open is untouched, still on main.
    assert!(!project.join("where.txt").exists(), "the project tree was written to");
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
    assert!(h.text().contains("worktree"), "the notice should name the workspace: {}", h.text());

    scriptr_lib::worktree::remove(&project, "t1", true).unwrap();
}

/// A task asking for isolation in a folder that is not a repository must say so
/// rather than quietly running in place.
#[tokio::test]
async fn a_worktree_task_without_a_repository_is_refused() {
    let mut t = task("echo nope");
    t.workspace = WorkspaceMode::Worktree;
    let h = setup("norepo", t);

    let err = h.runner.start("t1").await.unwrap_err();
    assert!(err.contains("not a git repository"), "{err}");
    assert_eq!(h.status(), TaskStatus::Backlog, "a refused start must not move the task");
}
