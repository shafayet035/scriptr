//! End-to-end supervisor tests: real PTYs and processes, no Tauri window.
#![cfg(unix)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use scriptr_lib::db::Db;
use scriptr_lib::model::{
    Gate, Group, GroupProgress, GroupState, Project, RestartOn, RestartPolicy, RunInfo, RunState, Script,
};
use scriptr_lib::scheduler::Scheduler;
use scriptr_lib::supervisor::{EventSink, Supervisor};

#[derive(Default)]
struct Recorder {
    runs: Mutex<Vec<RunInfo>>,
    groups: Mutex<Vec<GroupProgress>>,
}

impl EventSink for Recorder {
    fn run_changed(&self, info: &RunInfo) {
        self.runs.lock().unwrap().push(info.clone());
    }
    fn group_progress(&self, progress: &GroupProgress) {
        self.groups.lock().unwrap().push(progress.clone());
    }
}

fn script(id: &str, cmd: &str, ready: Gate, restart: RestartPolicy, after: &[&str]) -> Script {
    Script {
        id: id.into(),
        project_id: "p".into(),
        name: id.into(),
        label: None,
        cmd: cmd.into(),
        cwd: ".".into(),
        shell: Some("/bin/sh -c".into()),
        env: Default::default(),
        env_file: None,
        after: after.iter().map(|s| s.to_string()).collect(),
        ready,
        restart,
        port: None,
        source: None,
        sort_order: 0,
    }
}

fn setup(scripts: &[Script]) -> (Arc<Db>, Arc<Recorder>, Arc<Supervisor>) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    db.insert_project(&Project {
        id: "p".into(),
        name: "p".into(),
        path: std::env::temp_dir().to_string_lossy().into_owned(),
        branch: None,
        sort_order: 0,
    })
    .unwrap();
    for s in scripts {
        db.upsert_script(s).unwrap();
    }
    let rec = Arc::new(Recorder::default());
    let sup = Supervisor::new(db.clone(), rec.clone());
    (db, rec, sup)
}

async fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `(pid, command)` of every process in process group `pgid`.
fn group_members(pgid: u32) -> Vec<(i32, String)> {
    let out = std::process::Command::new("ps").args(["-A", "-o", "pid=,pgid=,comm="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let pg: u32 = it.next()?.parse().ok()?;
            (pg == pgid).then(|| (pid, it.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pty_output_log_gate_and_process_group_stop() {
    let s = script(
        "hello",
        "echo hello; sleep 30",
        Gate::Log { pattern: "hel+o".into(), timeout_ms: 5_000 },
        RestartPolicy::default(),
        &[],
    );
    let (_db, _rec, sup) = setup(&[s]);

    let received = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = received.clone();
    sup.attach(
        "hello",
        Box::new(move |bytes| {
            sink.lock().unwrap().extend(bytes);
            true
        }),
    );
    sup.start("hello").unwrap();

    wait_for("log gate", Duration::from_secs(5), || {
        sup.run_info("hello").gate_note.as_deref() == Some("log match")
    })
    .await;
    let output = String::from_utf8_lossy(&received.lock().unwrap()).into_owned();
    assert!(output.contains("hello"), "output: {output:?}");
    assert!(output.contains("\x1b[32m$\x1b[0m echo hello; sleep 30"));

    let info = sup.run_info("hello");
    assert_eq!(info.state, RunState::Running);
    let pid = info.pid.expect("running pid");
    wait_for("sleep child", Duration::from_secs(2), || {
        group_members(pid).iter().any(|(_, cmd)| cmd.contains("sleep"))
    })
    .await;
    let members = group_members(pid);

    let started = Instant::now();
    sup.stop("hello").await;
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_millis(500), "stop took {elapsed:?}");

    for (member, cmd) in members {
        assert_eq!(kill(Pid::from_raw(member), None), Err(Errno::ESRCH), "{cmd} ({member}) survived");
    }
    let info = sup.run_info("hello");
    assert_eq!(info.state, RunState::Stopped);
    assert_eq!(info.pid, None);

    // A re-attach gets the ring buffer (which survives the run) as its first message.
    let first = Arc::new(Mutex::new(None::<Vec<u8>>));
    let slot = first.clone();
    sup.attach(
        "hello",
        Box::new(move |bytes| {
            slot.lock().unwrap().get_or_insert(bytes);
            true
        }),
    );
    let snapshot = first.lock().unwrap().clone().unwrap();
    assert!(String::from_utf8_lossy(&snapshot).contains("[scriptr]\x1b[0m stopped after"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_backoff_then_give_up() {
    let policy = RestartPolicy { on: RestartOn::Crash, max: 2, backoff_ms: 50, backoff_max_ms: 1_000 };
    let (db, rec, sup) = setup(&[script("fail", "exit 3", Gate::Instant, policy, &[])]);
    sup.start("fail").unwrap();

    wait_for("retries exhausted", Duration::from_secs(10), || sup.run_info("fail").state == RunState::Stopped).await;
    let info = sup.run_info("fail");
    assert_eq!((info.exit_code, info.attempt, info.max_attempts), (Some(3), 2, 2));

    let mut transitions: Vec<(RunState, u32)> =
        rec.runs.lock().unwrap().iter().map(|i| (i.state, i.attempt)).collect();
    transitions.dedup();
    use RunState::*;
    assert_eq!(
        transitions,
        vec![
            (Starting, 0),
            (Running, 0),
            (Crashed, 0),
            (Backoff, 1),
            (Starting, 1),
            (Running, 1),
            (Crashed, 1),
            (Backoff, 2),
            (Starting, 2),
            (Running, 2),
            (Stopped, 2),
        ]
    );
    let backoffs: Vec<Option<u64>> =
        rec.runs.lock().unwrap().iter().filter(|i| i.state == Backoff).map(|i| i.backoff_ms).collect();
    assert_eq!(backoffs.first(), Some(&Some(50)));
    assert_eq!(backoffs.last(), Some(&Some(100)));
    assert_eq!(db.history("fail").unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_runs_in_waves_and_stops_in_reverse() {
    let never = RestartPolicy::default();
    let scripts = [
        script("db", "echo db ready; sleep 30", Gate::Log { pattern: "ready".into(), timeout_ms: 5_000 }, never.clone(), &[]),
        script("migrate", "sleep 0.2", Gate::Exit { timeout_ms: 5_000 }, never.clone(), &["db"]),
        script("api", "sleep 30", Gate::Instant, never.clone(), &["db", "migrate"]),
    ];
    let (db, rec, sup) = setup(&scripts);
    db.upsert_group(&Group {
        id: "g".into(),
        project_id: "p".into(),
        name: "all".into(),
        script_ids: vec!["api".into(), "migrate".into(), "db".into()],
    })
    .unwrap();
    let sched = Scheduler::new(db.clone(), sup.clone(), rec.clone());

    let plan = sched.run("g").unwrap();
    assert_eq!(plan.waves, vec![vec!["db"], vec!["migrate"], vec!["api"]]);
    wait_for("group done", Duration::from_secs(10), || {
        rec.groups.lock().unwrap().iter().any(|p| p.state == GroupState::Done)
    })
    .await;
    let migrate = sup.run_info("migrate");
    assert_eq!((migrate.state, migrate.exit_code, migrate.gate_note.as_deref()), (RunState::Stopped, Some(0), Some("exit code 0")));
    assert_eq!(sup.run_info("api").state, RunState::Running);

    let started = Instant::now();
    sched.stop("g").await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    for id in ["db", "api"] {
        assert_eq!(sup.run_info(id).state, RunState::Stopped, "{id}");
    }
    let stopping: Vec<usize> = rec
        .groups
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.state == GroupState::Stopping)
        .map(|p| p.wave)
        .collect();
    assert_eq!(stopping, vec![2, 1, 0]);

    // Starting a dependent alone brings its dependency up first.
    sup.start("api").unwrap();
    wait_for("api via deps", Duration::from_secs(10), || sup.run_info("api").state == RunState::Running).await;
    assert_eq!(sup.run_info("db").state, RunState::Running);
    sup.stop_all().await;
}
