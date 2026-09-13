//! Watches each project's root and first-level subdirectories for changes to
//! `scriptr.toml` and manifests, reporting project ids after a 300 ms quiet
//! period. Never imports anything by itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::detect::{MANIFESTS, SKIP_DIRS};
use crate::lock;
use crate::model::Project;

const DEBOUNCE: Duration = Duration::from_millis(300);
/// Events for files we wrote ourselves within this window are ignored.
const SELF_WRITE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Shared {
    /// Canonical project root → project id.
    roots: Vec<(PathBuf, String)>,
    suppressed: HashMap<PathBuf, Instant>,
}

impl Shared {
    fn project_for(&mut self, path: &Path) -> Option<String> {
        let file = path.file_name()?.to_str()?;
        if !MANIFESTS.contains(&file) {
            return None;
        }
        self.suppressed.retain(|_, at| at.elapsed() < SELF_WRITE_WINDOW);
        if self.suppressed.contains_key(path) {
            return None;
        }
        self.roots
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.as_os_str().len())
            .map(|(_, id)| id.clone())
    }
}

pub struct ProjectWatcher {
    watcher: RecommendedWatcher,
    shared: Arc<Mutex<Shared>>,
    watched: HashSet<PathBuf>,
}

impl ProjectWatcher {
    pub fn new(on_change: impl Fn(String) + Send + 'static) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel::<String>();
        let handler_shared = shared.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let mut shared = lock(&handler_shared);
            for id in event.paths.iter().filter_map(|p| shared.project_for(p)) {
                let _ = tx.send(id);
            }
        })
        .map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("scriptr-watch".into())
            .spawn(move || debounce(rx, on_change))
            .map_err(|e| e.to_string())?;
        Ok(Self { watcher, shared, watched: HashSet::new() })
    }

    /// Updates the watch set to match `projects`.
    pub fn sync(&mut self, projects: &[Project]) {
        let mut roots = Vec::new();
        let mut wanted = HashSet::new();
        for project in projects {
            let Ok(root) = Path::new(&project.path).canonicalize() else { continue };
            for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                if is_dir && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                    wanted.insert(entry.path());
                }
            }
            wanted.insert(root.clone());
            roots.push((root, project.id.clone()));
        }
        let gone: Vec<PathBuf> = self.watched.difference(&wanted).cloned().collect();
        for dir in gone {
            let _ = self.watcher.unwatch(&dir);
            self.watched.remove(&dir);
        }
        for dir in wanted {
            if self.watched.contains(&dir) {
                continue;
            }
            match self.watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(dir);
                }
                Err(e) => log::warn!("watch {}: {e}", dir.display()),
            }
        }
        lock(&self.shared).roots = roots;
    }

    /// Ignore the change we're about to cause by writing `path`.
    pub fn suppress(&self, path: &Path) {
        let canonical = match (path.parent().and_then(|p| p.canonicalize().ok()), path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        };
        lock(&self.shared).suppressed.insert(canonical, Instant::now());
    }
}

/// Trailing debounce: report every touched project once events stop for 300 ms.
fn debounce(rx: mpsc::Receiver<String>, on_change: impl Fn(String)) {
    while let Ok(first) = rx.recv() {
        let mut pending = HashSet::from([first]);
        loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(id) => {
                    pending.insert(id);
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    pending.into_iter().for_each(&on_change);
                    return;
                }
            }
        }
        pending.into_iter().for_each(&on_change);
    }
}
