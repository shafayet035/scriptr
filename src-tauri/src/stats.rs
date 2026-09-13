//! 1 Hz CPU / memory sampler. Each run reports the sum over its process tree
//! (the PTY leader plus all descendants).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::model::ProcStats;
use crate::supervisor::Supervisor;

const INTERVAL: Duration = Duration::from_secs(1);

/// Runs the sampler on its own thread, calling `emit` every second while
/// anything is running (and once with an empty list when the last run ends).
pub fn spawn_sampler(
    sup: Arc<Supervisor>,
    emit: impl Fn(Vec<ProcStats>) + Send + 'static,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("scriptr-stats".into())
        .spawn(move || {
            let mut sys = System::new();
            let mut was_empty = true;
            loop {
                std::thread::sleep(INTERVAL);
                let live = sup.live_pids();
                if live.is_empty() {
                    if !was_empty {
                        emit(Vec::new());
                        was_empty = true;
                    }
                    continue;
                }
                was_empty = false;
                emit(sample(&mut sys, &live));
            }
        })
        .map(drop)
}

fn sample(sys: &mut System, live: &[(String, u32)]) -> Vec<ProcStats> {
    // Cheap pass (pids + parents only) to discover descendants...
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, process) in sys.processes() {
        if let Some(parent) = process.parent() {
            children.entry(parent).or_default().push(*pid);
        }
    }
    let trees: Vec<(&str, u32, Vec<Pid>)> = live
        .iter()
        .map(|(id, root)| {
            let mut tree = vec![Pid::from_u32(*root)];
            let mut i = 0;
            while let Some(pid) = tree.get(i).copied() {
                tree.extend(children.get(&pid).into_iter().flatten());
                i += 1;
            }
            (id.as_str(), *root, tree)
        })
        .collect();

    // ...then CPU and memory only for the processes we report on.
    let wanted: Vec<Pid> = trees.iter().flat_map(|(_, _, tree)| tree.iter().copied()).collect();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&wanted),
        false,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    trees
        .into_iter()
        .map(|(id, root, tree)| {
            let (cpu, mem_bytes) = tree
                .iter()
                .filter_map(|pid| sys.process(*pid))
                .fold((0.0, 0), |(cpu, mem), p| (cpu + p.cpu_usage(), mem + p.memory()));
            ProcStats { script_id: id.to_string(), pid: root, cpu, mem_bytes }
        })
        .collect()
}
