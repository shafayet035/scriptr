//! Project scanning: finds runnable scripts in the root and first-level
//! subdirectories (package.json, pyproject.toml/manage.py, Makefile,
//! docker compose, Procfile, executable *.sh).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::model::{DetectedScript, DetectedSource, Gate, ScanResult, DEFAULT_TIMEOUT_MS};

pub const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "dist", ".venv", "venv", "build"];
/// File names whose changes are reported as `project:changed`.
pub const MANIFESTS: &[&str] = &[
    "scriptr.toml",
    "package.json",
    "pyproject.toml",
    "manage.py",
    "Makefile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "Procfile",
];
const COMPOSE_FILES: &[&str] = &["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"];
const ONE_SHOT_WORDS: &[&str] = &["migrate", "build", "test", "lint", "typecheck", "format", "install", "seed"];
/// Words that mark a long-running variant even when a one-shot word appears.
const LONG_RUNNING_WORDS: &[&str] = &["watch", "dev", "serve"];
const SUGGEST_WORDS: &[&str] = &["dev", "start", "serve", "runserver", "worker", "celery", "migrate"];
const NO_SUGGEST_WORDS: &[&str] = &["build", "test", "lint"];
/// Script names that stand for "the" server of a subdirectory.
const PRIMARY_NAMES: &[&str] = &["dev", "start", "serve", "runserver"];

static PORT_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"--port[= ](\d{2,5})", r"(?:^|\s)-p\s+(\d{2,5})", r"\bPORT=(\d{2,5})", r":(\d{2,5})\b"]
        .iter()
        .map(|p| Regex::new(p).expect("static regex"))
        .collect()
});

/// Scans `root` (and its first-level subdirectories) for scripts.
pub fn scan(root: &Path) -> Result<ScanResult, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    let mut subdirs: Vec<(PathBuf, String)> = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str())).then(|| (e.path(), name))
        })
        .collect();
    subdirs.sort_by(|a, b| a.1.cmp(&b.1));
    dirs.extend(subdirs);

    let root_pm = package_manager(root);
    let mut sources = Vec::new();
    for (dir, rel) in &dirs {
        let ctx = Ctx { dir, rel, root_pm };
        sources.extend(
            [
                ctx.package_json(),
                // Before pyproject so `runserver` claims the bare directory name.
                ctx.manage_py(),
                ctx.pyproject(),
                ctx.makefile(),
                ctx.compose(),
                ctx.procfile(),
            ]
            .into_iter()
            .flatten()
            .filter(|s| !s.scripts.is_empty()),
        );
        if rel.is_empty() {
            sources.extend(ctx.shell_scripts().filter(|s| !s.scripts.is_empty()));
        }
    }
    dedupe_names(&mut sources);

    Ok(ScanResult {
        path: root.to_string_lossy().into_owned(),
        name: root
            .file_name()
            .map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned()),
        branch: git_branch(root),
        sources,
        has_toml: root.join("scriptr.toml").is_file(),
    })
}

/// Current branch from `.git/HEAD` (short hash when detached). Handles
/// worktrees where `.git` is a `gitdir:` file.
pub fn git_branch(root: &Path) -> Option<String> {
    let git = root.join(".git");
    let git_dir = if git.is_file() {
        let text = fs::read_to_string(&git).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() { target } else { root.join(target) }
    } else {
        git
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_string()),
        None => head.get(..7).map(str::to_string),
    }
}

struct Ctx<'a> {
    dir: &'a Path,
    /// Subdirectory name, empty for the root.
    rel: &'a str,
    root_pm: Option<&'static str>,
}

impl Ctx<'_> {
    fn read(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.dir.join(file)).ok()
    }

    fn source(&self, file: &str, scripts: Vec<DetectedScript>) -> DetectedSource {
        let dir = if self.rel.is_empty() { "./".to_string() } else { format!("{}/", self.rel) };
        DetectedSource { file: file.to_string(), dir, scripts }
    }

    fn cwd(&self) -> String {
        if self.rel.is_empty() { ".".into() } else { format!("./{}", self.rel) }
    }

    /// Builds a script. `task` is the manifest-level name (npm script, make
    /// target); `own_name` marks names that are already meaningful on their
    /// own (compose services, Procfile entries).
    fn script(&self, file: &str, task: &str, cmd: String, hint: &str, own_name: bool) -> DetectedScript {
        let (name, label) = if self.rel.is_empty() || own_name {
            (task.to_string(), None)
        } else if PRIMARY_NAMES.contains(&task) {
            (self.rel.to_string(), Some(task.to_string()))
        } else {
            (format!("{}-{task}", self.rel), Some(task.to_string()))
        };
        let words = words(&format!("{task} {cmd}"));
        let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
        let one_shot = has(ONE_SHOT_WORDS) && !has(LONG_RUNNING_WORDS);
        let port = guess_port(&format!("{hint} {cmd}"));
        let ready = match port {
            _ if one_shot => Gate::Exit { timeout_ms: DEFAULT_TIMEOUT_MS },
            Some(port) => Gate::Port { port, timeout_ms: DEFAULT_TIMEOUT_MS },
            None => Gate::Instant,
        };
        DetectedScript {
            key: format!("{}{file}:{task}", self.source(file, vec![]).dir),
            name,
            label,
            cmd,
            cwd: self.cwd(),
            port,
            one_shot,
            ready,
            suggested: own_name || (has(SUGGEST_WORDS) && !has(NO_SUGGEST_WORDS)),
        }
    }

    fn package_json(&self) -> Option<DetectedSource> {
        let json: serde_json::Value = serde_json::from_str(&self.read("package.json")?).ok()?;
        let pm = package_manager(self.dir).or(self.root_pm).unwrap_or("npm");
        let has_vite = ["dependencies", "devDependencies"]
            .iter()
            .any(|k| json.get(k).and_then(|d| d.get("vite")).is_some());
        let scripts = json
            .get("scripts")?
            .as_object()?
            .iter()
            .filter_map(|(name, body)| {
                let body = body.as_str()?;
                let mut hint = body.to_string();
                if has_vite && name == "dev" && !body.contains("vite") {
                    hint.push_str(" vite");
                }
                Some(self.script("package.json", name, format!("{pm} run {name}"), &hint, false))
            })
            .collect();
        Some(self.source("package.json", scripts))
    }

    fn pyproject(&self) -> Option<DetectedSource> {
        let table: toml::Table = self.read("pyproject.toml")?.parse().ok()?;
        let uv = self.dir.join("uv.lock").is_file();
        let names = |v: Option<&toml::Value>| -> Vec<String> {
            v.and_then(|v| v.as_table()).map(|t| t.keys().cloned().collect()).unwrap_or_default()
        };
        let tool = table.get("tool");
        let poetry_scripts = names(tool.and_then(|t| t.get("poetry")).and_then(|p| p.get("scripts")));
        let project_scripts = names(table.get("project").and_then(|p| p.get("scripts")));
        let mut scripts: Vec<DetectedScript> = poetry_scripts
            .iter()
            .map(|n| self.script("pyproject.toml", n, format!("poetry run {n}"), "", false))
            .collect();
        for n in project_scripts.iter().filter(|n| !poetry_scripts.contains(n)) {
            let cmd = if uv { format!("uv run {n}") } else { n.clone() };
            scripts.push(self.script("pyproject.toml", n, cmd, "", false));
        }
        Some(self.source("pyproject.toml", scripts))
    }

    fn python_prefix(&self) -> &'static str {
        let is_poetry = self
            .read("pyproject.toml")
            .and_then(|t| t.parse::<toml::Table>().ok())
            .is_some_and(|t| t.get("tool").and_then(|t| t.get("poetry")).is_some());
        if is_poetry {
            "poetry run "
        } else if self.dir.join("uv.lock").is_file() {
            "uv run "
        } else {
            ""
        }
    }

    /// Django suggestions: runserver, migrate, and a celery worker when a
    /// `<module>/celery.py` exists.
    fn manage_py(&self) -> Option<DetectedSource> {
        if !self.dir.join("manage.py").is_file() {
            return None;
        }
        let pre = self.python_prefix();
        let mut scripts = vec![
            self.script("manage.py", "runserver", format!("{pre}python manage.py runserver 0.0.0.0:8000"), "", false),
            self.script("manage.py", "migrate", format!("{pre}python manage.py migrate"), "", false),
        ];
        let celery_module = fs::read_dir(self.dir).ok()?.filter_map(Result::ok).find_map(|e| {
            e.path().join("celery.py").is_file().then(|| e.file_name().to_string_lossy().into_owned())
        });
        if let Some(module) = celery_module {
            let cmd = format!("{pre}celery -A {module} worker -l info");
            scripts.push(self.script("manage.py", "worker", cmd, "", false));
        }
        Some(self.source("manage.py", scripts))
    }

    fn makefile(&self) -> Option<DetectedSource> {
        let text = self.read("Makefile")?;
        let scripts = make_targets(&text)
            .into_iter()
            .map(|t| self.script("Makefile", &t, format!("make {t}"), "", false))
            .collect();
        Some(self.source("Makefile", scripts))
    }

    fn compose(&self) -> Option<DetectedSource> {
        let file = COMPOSE_FILES.iter().find(|f| self.dir.join(f).is_file())?;
        let scripts = compose_services(&self.read(file)?)
            .into_iter()
            .map(|(svc, port)| {
                let mut s = self.script(file, &svc, format!("docker compose up {svc}"), "", true);
                if let (Some(port), false) = (port, s.one_shot) {
                    s.port = Some(port);
                    s.ready = Gate::Port { port, timeout_ms: DEFAULT_TIMEOUT_MS };
                }
                s
            })
            .collect();
        Some(self.source(file, scripts))
    }

    fn procfile(&self) -> Option<DetectedSource> {
        let text = self.read("Procfile")?;
        let scripts = text
            .lines()
            .filter_map(|l| {
                let (name, cmd) = l.split_once(':')?;
                let name = name.trim();
                let valid = !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                valid.then(|| self.script("Procfile", name, cmd.trim().to_string(), "", true))
            })
            .collect();
        Some(self.source("Procfile", scripts))
    }

    fn shell_scripts(&self) -> Option<DetectedSource> {
        let mut files: Vec<String> = fs::read_dir(self.dir)
            .ok()?
            .filter_map(Result::ok)
            .filter(|e| is_executable(&e.path()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".sh"))
            .collect();
        files.sort();
        let scripts = files
            .iter()
            .map(|f| {
                let stem = f.trim_end_matches(".sh");
                self.script(f, stem, format!("./{f}"), "", false)
            })
            .collect();
        Some(self.source("*.sh", scripts))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn package_manager(dir: &Path) -> Option<&'static str> {
    [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lockb", "bun"), ("bun.lock", "bun"), ("package-lock.json", "npm")]
        .iter()
        .find(|(lock, _)| dir.join(lock).is_file())
        .map(|(_, pm)| *pm)
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Port from explicit flags/addresses, else well-known tool defaults.
pub fn guess_port(text: &str) -> Option<u16> {
    let explicit = PORT_RES.iter().find_map(|re| {
        re.captures_iter(text).find_map(|c| c[1].parse::<u16>().ok().filter(|p| *p > 0))
    });
    explicit.or_else(|| {
        let w = words(text);
        let has = |x: &str| w.iter().any(|t| t == x);
        if text.contains("next dev") || text.contains("next start") {
            Some(3000)
        } else if has("vite") && !has("build") {
            Some(5173)
        } else if has("runserver") {
            Some(8000)
        } else {
            None
        }
    })
}

fn make_targets(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#']) {
            continue;
        }
        let Some((targets, rest)) = line.split_once(':') else { continue };
        // `VAR := x`, `VAR ::= x`, `VAR = a:b`
        if rest.starts_with([':', '=']) || targets.contains('=') {
            continue;
        }
        for t in targets.split_whitespace() {
            if t.starts_with('.') || t.contains(['%', '$']) {
                continue;
            }
            if seen.insert(t.to_string()) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Top-level `services:` keys plus the first published host port of each.
/// Deliberately minimal indentation-based parsing, not a YAML parser.
fn compose_services(text: &str) -> Vec<(String, Option<u16>)> {
    let mut out: Vec<(String, Option<u16>)> = Vec::new();
    let mut in_services = false;
    let mut service_indent = None;
    let mut ports_indent: Option<usize> = None;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent == 0 {
            in_services = trimmed == "services:";
            continue;
        }
        if !in_services {
            continue;
        }
        let svc_indent = *service_indent.get_or_insert(indent);
        if indent <= svc_indent {
            if let Some(name) = trimmed.strip_suffix(':') {
                out.push((name.trim_matches(['"', '\'']).to_string(), None));
            }
            ports_indent = None;
            continue;
        }
        if let Some(pi) = ports_indent {
            if indent < pi || (indent == pi && !trimmed.starts_with('-')) {
                ports_indent = None;
            }
        }
        if let Some(inline) = trimmed.strip_prefix("ports:") {
            ports_indent = Some(indent);
            let items = inline.trim().trim_start_matches('[').trim_end_matches(']');
            if let (Some(last), Some(first)) = (out.last_mut(), items.split(',').next()) {
                last.1 = last.1.or_else(|| host_port(first));
            }
            continue;
        }
        if ports_indent.is_some() {
            if let (Some(item), Some(last)) = (trimmed.strip_prefix('-'), out.last_mut()) {
                last.1 = last.1.or_else(|| host_port(item));
            }
        }
    }
    out
}

/// `"5432:5432"`, `127.0.0.1:8080:80`, `3000-3005:3000`, `6379` → host port.
fn host_port(item: &str) -> Option<u16> {
    let item = item.trim().trim_matches(['"', '\'']);
    let item = item.split('/').next()?;
    let parts: Vec<&str> = item.split(':').collect();
    let host = if parts.len() >= 2 { parts[parts.len() - 2] } else { parts[0] };
    host.split('-').next()?.trim().parse().ok()
}

/// Makes names unique across the whole scan (they key dependencies and TOML).
fn dedupe_names(sources: &mut [DetectedSource]) {
    let mut used = HashSet::new();
    for script in sources.iter_mut().flat_map(|s| s.scripts.iter_mut()) {
        let dir = script.cwd.trim_start_matches("./");
        let mut candidates = vec![script.name.clone()];
        if let (Some(label), false) = (&script.label, dir == ".") {
            candidates.push(format!("{dir}-{label}"));
        }
        let base = candidates.last().cloned().unwrap_or_default();
        let name = candidates
            .into_iter()
            .chain((2..).map(|n| format!("{base}-{n}")))
            .find(|c| !used.contains(c))
            .unwrap_or(base);
        used.insert(name.clone());
        script.name = name;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Minimal self-cleaning temp dir (avoids a dev-dependency).
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("scriptr-{tag}-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, contents).unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn find<'a>(r: &'a ScanResult, name: &str) -> &'a DetectedScript {
        r.sources
            .iter()
            .flat_map(|s| &s.scripts)
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no script {name}"))
    }

    #[test]
    fn scans_a_monorepo() {
        let t = TempDir::new("scan");
        t.write(".git/HEAD", "ref: refs/heads/feature/x\n");
        t.write("scriptr.toml", "");
        t.write(
            "web/package.json",
            r#"{"scripts":{"dev":"vite","build":"vite build","lint":"eslint ."},"devDependencies":{"vite":"^7"}}"#,
        );
        t.write("web/pnpm-lock.yaml", "");
        t.write(
            "api/pyproject.toml",
            "[tool.poetry]\nname='api'\n[tool.poetry.scripts]\nserve = 'api:main'\n",
        );
        t.write("api/manage.py", "");
        t.write("api/proj/celery.py", "");
        t.write("Makefile", ".PHONY: test\nVAR := x\ntest:\n\tcargo test\nseed db-reset: deps\n%.o: %.c\n");
        t.write(
            "docker-compose.yml",
            "version: '3'\nservices:\n  db:\n    image: postgres\n    ports:\n      - \"5433:5432\"\n  stripe-mock:\n    image: x\n    ports: [\"12111:12111\"]\nvolumes:\n  data:\n",
        );
        t.write("Procfile", "worker: celery -A proj worker\n");
        t.write("node_modules/x/package.json", r#"{"scripts":{"dev":"x"}}"#);

        let r = scan(&t.0).unwrap();
        assert_eq!(r.branch.as_deref(), Some("feature/x"));
        assert!(r.has_toml);

        let web = find(&r, "web");
        assert_eq!(web.cmd, "pnpm run dev");
        assert_eq!(web.label.as_deref(), Some("dev"));
        assert_eq!(web.cwd, "./web");
        assert_eq!(web.ready, Gate::Port { port: 5173, timeout_ms: 60_000 });
        assert!(web.suggested);

        let build = find(&r, "web-build");
        assert!(build.one_shot && !build.suggested);
        assert_eq!(build.ready, Gate::Exit { timeout_ms: 60_000 });

        let runserver = find(&r, "api");
        assert_eq!(runserver.label.as_deref(), Some("runserver"));
        assert_eq!(runserver.cmd, "poetry run python manage.py runserver 0.0.0.0:8000");
        assert_eq!(runserver.port, Some(8000));
        assert!(find(&r, "api-migrate").one_shot);
        assert_eq!(find(&r, "api-worker").cmd, "poetry run celery -A proj worker -l info");
        assert_eq!(find(&r, "api-serve").cmd, "poetry run serve");

        let db = find(&r, "db");
        assert_eq!(db.cmd, "docker compose up db");
        assert_eq!(db.port, Some(5433));
        assert!(db.suggested);
        assert_eq!(find(&r, "stripe-mock").port, Some(12111));

        assert!(find(&r, "test").one_shot);
        assert_eq!(find(&r, "db-reset").cmd, "make db-reset");
        // Procfile "worker" doesn't clash with anything at root.
        assert_eq!(find(&r, "worker").cmd, "celery -A proj worker");
        assert!(!r.sources.iter().any(|s| s.dir.contains("node_modules")));
    }

    #[test]
    fn port_guessing() {
        assert_eq!(guess_port("uvicorn app:app --port 9000"), Some(9000));
        assert_eq!(guess_port("serve -p 4000"), Some(4000));
        assert_eq!(guess_port("next dev"), Some(3000));
        assert_eq!(guess_port("python manage.py runserver"), Some(8000));
        assert_eq!(guess_port("echo hi"), None);
    }

    #[test]
    fn duplicate_names_get_suffixes() {
        let t = TempDir::new("dupes");
        t.write("package.json", r#"{"scripts":{"db":"x"}}"#);
        t.write("compose.yaml", "services:\n  db:\n    image: pg\n");
        let r = scan(&t.0).unwrap();
        let names: Vec<_> = r.sources.iter().flat_map(|s| &s.scripts).map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["db", "db-2"]);
    }
}
