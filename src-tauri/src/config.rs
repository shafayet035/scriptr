//! `scriptr.toml` export / import. Dependencies and group members are stored
//! by script name; durations as `"60s"` / `"500ms"`; env values are never
//! exported (only `env_file`).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::db::Db;
use crate::model::{Gate, Group, ImportMode, ImportReport, RestartOn, RestartPolicy, Script, DEFAULT_TIMEOUT_MS};

pub const FILE_NAME: &str = "scriptr.toml";
const HEADER: &str = "# scriptr.toml — commit this at the project root\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    script: Vec<FileScript>,
    #[serde(default)]
    group: Vec<FileGroup>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileScript {
    name: String,
    label: Option<String>,
    cmd: String,
    cwd: Option<String>,
    shell: Option<String>,
    env_file: Option<String>,
    port: Option<u16>,
    after: Option<Vec<String>>,
    ready: Option<FileReady>,
    restart: Option<FileRestart>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReady {
    log: Option<String>,
    port: Option<u16>,
    http: Option<String>,
    exit: Option<i64>,
    timeout: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRestart {
    on: Option<RestartOn>,
    max: Option<u32>,
    backoff: Option<String>,
    backoff_max: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileGroup {
    name: String,
    scripts: Vec<String>,
}

// ---- durations & quoting ----------------------------------------------------------

/// `"500ms"`, `"2s"`, `"1.5s"`, `"5m"`, `"1h"` → milliseconds.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let bad = || format!("invalid duration \"{s}\" (use e.g. \"500ms\", \"2s\", \"5m\")");
    let (num, unit) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000.0)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000.0)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000.0)
    } else {
        return Err(bad());
    };
    let value: f64 = num.trim().parse().map_err(|_| bad())?;
    if !value.is_finite() || value < 0.0 {
        return Err(bad());
    }
    Ok((value * unit).round() as u64)
}

pub fn format_duration(ms: u64) -> String {
    if ms.is_multiple_of(1000) { format!("{}s", ms / 1000) } else { format!("{ms}ms") }
}

/// TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn quote_list<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let items: Vec<String> = items.into_iter().map(quote).collect();
    format!("[{}]", items.join(", "))
}

// ---- export ---------------------------------------------------------------------------

fn render_ready(gate: &Gate) -> Option<String> {
    let timeout = |ms: &u64| quote(&format_duration(*ms));
    Some(match gate {
        Gate::Instant => return None,
        Gate::Log { pattern, timeout_ms } => format!("{{ log = {}, timeout = {} }}", quote(pattern), timeout(timeout_ms)),
        Gate::Port { port, timeout_ms } => format!("{{ port = {port}, timeout = {} }}", timeout(timeout_ms)),
        Gate::Http { url, timeout_ms } => format!("{{ http = {}, timeout = {} }}", quote(url), timeout(timeout_ms)),
        Gate::Exit { timeout_ms } => format!("{{ exit = 0, timeout = {} }}", timeout(timeout_ms)),
    })
}

fn render_restart(p: &RestartPolicy) -> Option<String> {
    let default = RestartPolicy::default();
    if *p == default {
        return None;
    }
    let mut s = format!(
        "{{ on = {}, max = {}, backoff = {}",
        quote(p.on.as_str()),
        p.max,
        quote(&format_duration(p.backoff_ms))
    );
    if p.backoff_max_ms != default.backoff_max_ms {
        let _ = write!(s, ", backoff_max = {}", quote(&format_duration(p.backoff_max_ms)));
    }
    s.push_str(" }");
    Some(s)
}

/// Writes `key = value` lines with `=` aligned within the block.
fn push_block(out: &mut String, header: &str, fields: &[(&str, String)]) {
    let width = fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let _ = writeln!(out, "\n{header}");
    for (k, v) in fields {
        let _ = writeln!(out, "{k:<width$} = {v}");
    }
}

/// Renders scripts (in order) and groups as `scriptr.toml`.
pub fn render(scripts: &[Script], groups: &[Group]) -> String {
    let names: HashMap<&str, &str> = scripts.iter().map(|s| (s.id.as_str(), s.name.as_str())).collect();
    let name_list = |ids: &[String]| quote_list(ids.iter().filter_map(|id| names.get(id.as_str()).copied()));
    let mut out = HEADER.to_string();
    for s in scripts {
        let mut f: Vec<(&str, String)> = vec![("name", quote(&s.name))];
        if let Some(label) = &s.label {
            f.push(("label", quote(label)));
        }
        f.push(("cmd", quote(&s.cmd)));
        if !matches!(s.cwd.as_str(), "" | "." | "./") {
            f.push(("cwd", quote(&s.cwd)));
        }
        if let Some(shell) = &s.shell {
            f.push(("shell", quote(shell)));
        }
        if let Some(env_file) = &s.env_file {
            f.push(("env_file", quote(env_file)));
        }
        if let Some(port) = s.port {
            f.push(("port", port.to_string()));
        }
        if !s.after.is_empty() {
            f.push(("after", name_list(&s.after)));
        }
        if let Some(ready) = render_ready(&s.ready) {
            f.push(("ready", ready));
        }
        if let Some(restart) = render_restart(&s.restart) {
            f.push(("restart", restart));
        }
        push_block(&mut out, "[[script]]", &f);
    }
    for g in groups {
        push_block(&mut out, "[[group]]", &[("name", quote(&g.name)), ("scripts", name_list(&g.script_ids))]);
    }
    out
}

/// Exports a project to `<project>/scriptr.toml`, returning the path.
pub fn export(db: &Db, project_id: &str) -> Result<PathBuf, String> {
    let project = db.project(project_id)?;
    let text = render(&db.project_scripts(project_id)?, &db.project_groups(project_id)?);
    let path = Path::new(&project.path).join(FILE_NAME);
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

// ---- import ---------------------------------------------------------------------------

/// Database changes an import would make.
pub struct ImportPlan {
    pub upserts: Vec<Script>,
    pub deletes: Vec<String>,
    pub group_upserts: Vec<Group>,
    pub group_deletes: Vec<String>,
    pub report: ImportReport,
}

fn parse_ready(r: &FileReady) -> Result<Gate, String> {
    let timeout_ms = r.timeout.as_deref().map(parse_duration).transpose()?.unwrap_or(DEFAULT_TIMEOUT_MS);
    let kinds = [r.log.is_some(), r.port.is_some(), r.http.is_some(), r.exit.is_some()];
    if kinds.iter().filter(|k| **k).count() > 1 {
        return Err("ready may only set one of log, port, http, exit".into());
    }
    Ok(if let Some(pattern) = &r.log {
        Gate::Log { pattern: pattern.clone(), timeout_ms }
    } else if let Some(port) = r.port {
        Gate::Port { port, timeout_ms }
    } else if let Some(url) = &r.http {
        Gate::Http { url: url.clone(), timeout_ms }
    } else if let Some(code) = r.exit {
        if code != 0 {
            return Err(format!("ready.exit must be 0 (got {code})"));
        }
        Gate::Exit { timeout_ms }
    } else {
        Gate::Instant
    })
}

fn parse_restart(r: &FileRestart, base: RestartPolicy) -> Result<RestartPolicy, String> {
    Ok(RestartPolicy {
        on: r.on.unwrap_or(base.on),
        max: r.max.unwrap_or(base.max),
        backoff_ms: r.backoff.as_deref().map(parse_duration).transpose()?.unwrap_or(base.backoff_ms),
        backoff_max_ms: r.backoff_max.as_deref().map(parse_duration).transpose()?.unwrap_or(base.backoff_max_ms),
    })
}

fn changed_fields(a: &Script, b: &Script) -> Vec<&'static str> {
    [
        ("cmd", a.cmd != b.cmd),
        ("label", a.label != b.label),
        ("cwd", a.cwd != b.cwd),
        ("shell", a.shell != b.shell),
        ("env_file", a.env_file != b.env_file),
        ("port", a.port != b.port),
        ("after", a.after != b.after),
        ("ready", a.ready != b.ready),
        ("restart", a.restart != b.restart),
    ]
    .into_iter()
    .filter_map(|(name, changed)| changed.then_some(name))
    .collect()
}

/// Computes what importing `text` would change. `merge`/`preview`: the file
/// wins for fields it sets, local-only scripts and groups are kept.
/// `replace`: the file is the whole truth (except env values, which never
/// live in the file).
pub fn plan_import(
    project_id: &str,
    existing: &[Script],
    existing_groups: &[Group],
    text: &str,
    mode: ImportMode,
) -> Result<ImportPlan, String> {
    let file: FileConfig = toml::from_str(text).map_err(|e| format!("{FILE_NAME}: {e}"))?;
    let replace = mode == ImportMode::Replace;

    let mut seen = HashSet::new();
    if let Some(dup) = file.script.iter().find(|s| !seen.insert(s.name.as_str())) {
        return Err(format!("{FILE_NAME}: script \"{}\" is defined twice", dup.name));
    }

    // Names resolvable in dependency lists, mapped to (existing or new) ids.
    let mut ids: HashMap<String, String> = HashMap::new();
    if !replace {
        ids.extend(existing.iter().map(|s| (s.name.clone(), s.id.clone())));
    }
    for fs in &file.script {
        let id = existing
            .iter()
            .find(|s| s.name == fs.name)
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), |s| s.id.clone());
        ids.insert(fs.name.clone(), id);
    }
    let resolve = |owner: &str, names: &[String]| -> Result<Vec<String>, String> {
        names
            .iter()
            .map(|n| ids.get(n).cloned().ok_or_else(|| format!("{owner}: unknown script \"{n}\"")))
            .collect()
    };

    let mut report = ImportReport::default();
    let mut lines = Vec::new();
    let mut upserts = Vec::new();
    let mut next_order = existing.iter().map(|s| s.sort_order + 1).max().unwrap_or(0);

    for fs in &file.script {
        let current = existing.iter().find(|s| s.name == fs.name);
        let mut s = match current {
            Some(c) if !replace => c.clone(),
            _ => {
                let order = current.map_or_else(
                    || {
                        next_order += 1;
                        next_order - 1
                    },
                    |c| c.sort_order,
                );
                Script {
                    id: ids[&fs.name].clone(),
                    project_id: project_id.to_string(),
                    name: fs.name.clone(),
                    label: None,
                    cmd: String::new(),
                    cwd: ".".into(),
                    shell: None,
                    env: current.map(|c| c.env.clone()).unwrap_or_default(),
                    env_file: None,
                    after: vec![],
                    ready: Gate::Instant,
                    restart: RestartPolicy::default(),
                    port: None,
                    source: Some(FILE_NAME.into()),
                    sort_order: order,
                }
            }
        };
        let owner = format!("script \"{}\"", fs.name);
        s.cmd = fs.cmd.clone();
        // Merge keeps local values for absent keys; replace resets them above.
        if fs.label.is_some() {
            s.label = fs.label.clone();
        }
        if let Some(cwd) = &fs.cwd {
            s.cwd = cwd.clone();
        }
        if fs.shell.is_some() {
            s.shell = fs.shell.clone();
        }
        if fs.env_file.is_some() {
            s.env_file = fs.env_file.clone();
        }
        if fs.port.is_some() {
            s.port = fs.port;
        }
        if let Some(after) = &fs.after {
            s.after = resolve(&owner, after)?;
        }
        if let Some(ready) = &fs.ready {
            s.ready = parse_ready(ready).map_err(|e| format!("{owner}: {e}"))?;
        }
        if let Some(restart) = &fs.restart {
            s.restart = parse_restart(restart, s.restart.clone()).map_err(|e| format!("{owner}: {e}"))?;
        }

        match current {
            None => {
                report.added += 1;
                lines.push(format!("+ script {}", s.name));
                upserts.push(s);
            }
            Some(c) if *c != s => {
                report.updated += 1;
                let fields = changed_fields(c, &s);
                lines.push(format!("~ script {}: {}", s.name, fields.join(", ")));
                upserts.push(s);
            }
            Some(_) => {}
        }
    }

    let mut deletes = Vec::new();
    if replace {
        for s in existing.iter().filter(|s| !file.script.iter().any(|f| f.name == s.name)) {
            report.removed += 1;
            lines.push(format!("- script {}", s.name));
            deletes.push(s.id.clone());
        }
    }

    let mut group_upserts = Vec::new();
    for fg in &file.group {
        let script_ids = resolve(&format!("group \"{}\"", fg.name), &fg.scripts)?;
        match existing_groups.iter().find(|g| g.name == fg.name) {
            Some(g) if g.script_ids == script_ids => {}
            Some(g) => {
                report.updated += 1;
                lines.push(format!("~ group {}: scripts", fg.name));
                group_upserts.push(Group { script_ids, ..g.clone() });
            }
            None => {
                report.added += 1;
                lines.push(format!("+ group {}", fg.name));
                group_upserts.push(Group {
                    id: uuid::Uuid::new_v4().to_string(),
                    project_id: project_id.to_string(),
                    name: fg.name.clone(),
                    script_ids,
                });
            }
        }
    }
    let mut group_deletes = Vec::new();
    if replace {
        for g in existing_groups.iter().filter(|g| !file.group.iter().any(|f| f.name == g.name)) {
            report.removed += 1;
            lines.push(format!("- group {}", g.name));
            group_deletes.push(g.id.clone());
        }
    }

    if mode == ImportMode::Preview {
        report.preview = Some(if lines.is_empty() { "no changes".into() } else { lines.join("\n") });
    }
    Ok(ImportPlan { upserts, deletes, group_upserts, group_deletes, report })
}

/// Imports `path` into a project. `preview` makes no writes.
pub fn import(db: &Db, project_id: &str, path: &Path, mode: ImportMode) -> Result<ImportReport, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let plan = plan_import(project_id, &db.project_scripts(project_id)?, &db.project_groups(project_id)?, &text, mode)?;
    if mode != ImportMode::Preview {
        for id in &plan.deletes {
            db.delete_script(id)?;
        }
        for id in &plan.group_deletes {
            db.delete_group(id)?;
        }
        for s in &plan.upserts {
            db.upsert_script(s)?;
        }
        for g in &plan.group_upserts {
            db.upsert_group(g)?;
        }
    }
    Ok(plan.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"# scriptr.toml — commit this at the project root
[[script]]
name    = "db"
cmd     = "docker compose up db"
ready   = { port = 5432 }

[[script]]
name    = "migrate"
cmd     = "poetry run python manage.py migrate"
cwd     = "./api"
after   = ["db"]
ready   = { exit = 0 }

[[script]]
name    = "api"
cmd     = "poetry run python manage.py runserver 0.0.0.0:8000"
cwd     = "./api"
after   = ["db", "migrate"]
ready   = { log = "Starting development server at", timeout = "60s" }
restart = { on = "crash", max = 5, backoff = "2s" }

[[group]]
name    = "Full stack"
scripts = ["db", "migrate", "api"]
"#;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("60s"), Ok(60_000));
        assert_eq!(parse_duration("500ms"), Ok(500));
        assert_eq!(parse_duration("1.5s"), Ok(1_500));
        assert_eq!(parse_duration("2m"), Ok(120_000));
        assert!(parse_duration("soon").is_err());
        assert_eq!(format_duration(2_000), "2s");
        assert_eq!(format_duration(250), "250ms");
    }

    #[test]
    fn imports_example_and_round_trips() {
        let plan = plan_import("p", &[], &[], EXAMPLE, ImportMode::Merge).unwrap();
        assert_eq!(plan.report.added, 4);
        let api = plan.upserts.iter().find(|s| s.name == "api").unwrap();
        let db_id = &plan.upserts[0].id;
        assert_eq!(api.after, vec![db_id.clone(), plan.upserts[1].id.clone()]);
        assert_eq!(api.ready, Gate::Log { pattern: "Starting development server at".into(), timeout_ms: 60_000 });
        assert_eq!(api.restart, RestartPolicy { on: RestartOn::Crash, max: 5, backoff_ms: 2_000, backoff_max_ms: 32_000 });
        assert_eq!(plan.upserts[1].ready, Gate::Exit { timeout_ms: 60_000 });

        let rendered = render(&plan.upserts, &plan.group_upserts);
        assert!(rendered.starts_with(HEADER));
        assert!(rendered.contains(
            "name    = \"api\"\ncmd     = \"poetry run python manage.py runserver 0.0.0.0:8000\"\ncwd     = \"./api\"\nafter   = [\"db\", \"migrate\"]\nready   = { log = \"Starting development server at\", timeout = \"60s\" }\nrestart = { on = \"crash\", max = 5, backoff = \"2s\" }\n"
        ));
        assert!(rendered.contains("[[group]]\nname    = \"Full stack\"\nscripts = [\"db\", \"migrate\", \"api\"]\n"));

        // Re-importing the export over the same data changes nothing.
        let again = plan_import("p", &plan.upserts, &plan.group_upserts, &rendered, ImportMode::Preview).unwrap();
        assert_eq!(again.report, ImportReport { preview: Some("no changes".into()), ..Default::default() });
        // And a fresh import of the export reproduces the same scripts.
        let fresh = plan_import("p", &[], &[], &rendered, ImportMode::Merge).unwrap();
        let strip = |v: &[Script]| -> Vec<(String, String, String, Gate, RestartPolicy, usize)> {
            v.iter().map(|s| (s.name.clone(), s.cmd.clone(), s.cwd.clone(), s.ready.clone(), s.restart.clone(), s.after.len())).collect()
        };
        assert_eq!(strip(&fresh.upserts), strip(&plan.upserts));
    }

    #[test]
    fn merge_keeps_local_and_replace_removes() {
        let base = plan_import("p", &[], &[], EXAMPLE, ImportMode::Merge).unwrap();
        let mut local = base.upserts.clone();
        let mut extra = local[0].clone();
        extra.id = "local".into();
        extra.name = "local-only".into();
        local.push(extra);
        let changed = EXAMPLE.replace("docker compose up db", "docker compose up postgres");

        let merge = plan_import("p", &local, &base.group_upserts, &changed, ImportMode::Preview).unwrap();
        assert_eq!(merge.report.updated, 1);
        assert_eq!(merge.report.removed, 0);
        assert_eq!(merge.report.preview.as_deref(), Some("~ script db: cmd"));

        let replace = plan_import("p", &local, &base.group_upserts, &changed, ImportMode::Replace).unwrap();
        assert_eq!(replace.deletes, vec!["local".to_string()]);
        assert_eq!(replace.report.removed, 1);
        // Existing ids are preserved so run history survives.
        assert_eq!(replace.upserts[0].id, base.upserts[0].id);
    }

    #[test]
    fn rejects_unknown_dependency() {
        let bad = "[[script]]\nname = \"a\"\ncmd = \"x\"\nafter = [\"ghost\"]\n";
        let err = plan_import("p", &[], &[], bad, ImportMode::Merge).err().unwrap();
        assert!(err.contains("ghost"), "{err}");
    }
}
