//! Group runs: start waves in parallel, await every gate, then the next wave.
//! Shutdown goes in reverse wave order.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};

use crate::db::Db;
use crate::graph;
use crate::model::{GroupProgress, GroupState, Plan};
use crate::supervisor::{cycle_error, EventSink, Readiness, Supervisor};

struct GroupRun {
    task: AbortHandle,
    resume: mpsc::UnboundedSender<()>,
    paused: Arc<AtomicBool>,
}

pub struct Scheduler {
    db: Arc<Db>,
    sup: Arc<Supervisor>,
    events: Arc<dyn EventSink>,
    runs: Mutex<HashMap<String, GroupRun>>,
}

impl Scheduler {
    pub fn new(db: Arc<Db>, sup: Arc<Supervisor>, events: Arc<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self { db, sup, events, runs: Mutex::default() })
    }

    /// Waves for the group's scripts. Dependencies outside the group don't
    /// affect ordering (the supervisor still starts and awaits them).
    pub fn plan(&self, group_id: &str) -> Result<Plan, String> {
        let group = self.db.group(group_id)?;
        let scripts = self.db.project_scripts(&group.project_id)?;
        let nodes: Vec<graph::Node> = group
            .script_ids
            .iter()
            .filter_map(|id| scripts.iter().find(|s| &s.id == id))
            .map(|s| graph::Node { id: &s.id, after: &s.after })
            .collect();
        let (waves, cycle) = match graph::waves(&nodes) {
            Ok(waves) => (waves, None),
            Err(edge) => (Vec::new(), Some(edge)),
        };
        Ok(Plan { group_id: group_id.to_string(), waves, cycle })
    }

    pub fn run(self: &Arc<Self>, group_id: &str) -> Result<Plan, String> {
        let plan = self.plan(group_id)?;
        if let Some(edge) = &plan.cycle {
            let group = self.db.group(group_id)?;
            let msg = cycle_error(&self.db.project_scripts(&group.project_id)?, edge);
            self.emit(group_id, GroupState::Failed, 0, 0, None, Some(msg.clone()));
            return Err(msg);
        }

        let (resume, rx) = mpsc::unbounded_channel();
        let paused = Arc::new(AtomicBool::new(false));
        let this = Arc::clone(self);
        let waves = plan.waves.clone();
        let gid = group_id.to_string();
        let flag = paused.clone();
        let task = tokio::spawn(async move { this.drive(&gid, waves, rx, flag).await }).abort_handle();
        if let Some(old) = crate::lock(&self.runs).insert(group_id.to_string(), GroupRun { task, resume, paused }) {
            old.task.abort();
        }
        Ok(plan)
    }

    /// Resumes a run paused on a degraded script.
    pub fn resume(&self, group_id: &str) -> Result<(), String> {
        match crate::lock(&self.runs).get(group_id) {
            Some(run) if run.paused.load(Ordering::SeqCst) => {
                run.resume.send(()).map_err(|_| "group run has already finished".to_string())
            }
            _ => Err("group is not waiting on a degraded script".into()),
        }
    }

    /// Cancels a pending run and stops the group's scripts, last wave first.
    pub async fn stop(&self, group_id: &str) -> Result<(), String> {
        if let Some(run) = crate::lock(&self.runs).remove(group_id) {
            run.task.abort();
        }
        let plan = self.plan(group_id)?;
        let waves = match plan.cycle {
            Some(_) => vec![self.db.group(group_id)?.script_ids],
            None => plan.waves,
        };
        let total = waves.len();
        for (i, wave) in waves.iter().enumerate().rev() {
            self.emit(group_id, GroupState::Stopping, i, total, None, None);
            self.sup.stop_many(wave).await;
        }
        self.emit(group_id, GroupState::Stopped, 0, total, None, None);
        Ok(())
    }

    async fn drive(
        &self,
        group_id: &str,
        waves: Vec<Vec<String>>,
        mut resume: mpsc::UnboundedReceiver<()>,
        paused: Arc<AtomicBool>,
    ) {
        let total = waves.len();
        for (i, wave) in waves.iter().enumerate() {
            self.emit(group_id, GroupState::Running, i, total, None, None);
            for id in wave {
                if let Err(e) = self.sup.start(id) {
                    self.emit(group_id, GroupState::Failed, i, total, None, Some(e));
                    return;
                }
            }
            self.emit(group_id, GroupState::Waiting, i, total, None, None);

            let mut set = JoinSet::new();
            for id in wave {
                let settled = self.sup.wait_settled(id);
                let id = id.clone();
                set.spawn(async move { (id, settled.await) });
            }
            while let Some(joined) = set.join_next().await {
                let Ok((id, readiness)) = joined else { continue };
                let name = self.db.script(&id).map_or_else(|_| id.clone(), |s| s.name);
                match readiness {
                    Readiness::Ready => {}
                    Readiness::Degraded => {
                        paused.store(true, Ordering::SeqCst);
                        let msg = format!("\"{name}\" is running but its readiness gate timed out");
                        self.emit(group_id, GroupState::Degraded, i, total, Some(id), Some(msg));
                        if resume.recv().await.is_none() {
                            return;
                        }
                        paused.store(false, Ordering::SeqCst);
                        self.emit(group_id, GroupState::Waiting, i, total, None, None);
                    }
                    Readiness::Down | Readiness::Pending => {
                        let msg = format!("\"{name}\" stopped before it was ready");
                        self.emit(group_id, GroupState::Failed, i, total, None, Some(msg));
                        return;
                    }
                }
            }
        }
        self.emit(group_id, GroupState::Done, total.saturating_sub(1), total, None, None);
    }

    fn emit(
        &self,
        group_id: &str,
        state: GroupState,
        wave: usize,
        total_waves: usize,
        degraded_script_id: Option<String>,
        message: Option<String>,
    ) {
        self.events.group_progress(&GroupProgress {
            group_id: group_id.to_string(),
            state,
            wave,
            total_waves,
            degraded_script_id,
            message,
        });
    }
}
