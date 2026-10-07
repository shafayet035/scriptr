//! Agent adapters: data, not code. One TOML descriptor per coding-agent CLI,
//! with `{prompt}` / `{model}` / `{session}` placeholders in the argv
//! templates. Built-ins are embedded; users and projects can add or override
//! them. Nothing here ever goes through a shell — argv is passed straight to
//! the binary, so quotes and newlines in a goal are safe.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use tokio::task::JoinSet;

use crate::config::FILE_NAME;
use crate::model::{AgentAdapter, AgentSource, AgentStream, Autonomy, Effort};

/// How long `<bin> --version` may take before we give up on it.
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);

/// Built-in descriptors, embedded so a packaged app ships them.
const BUILTIN: &[&str] = &[
    include_str!("../agents/claude-code.toml"),
    include_str!("../agents/opencode.toml"),
    include_str!("../agents/cursor-agent.toml"),
    include_str!("../agents/gemini.toml"),
    include_str!("../agents/aider.toml"),
];

// ---- descriptor file ------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFile {
    id: String,
    name: Option<String>,
    bin: String,
    #[serde(default)]
    interactive_args: Vec<String>,
    #[serde(default)]
    headless_args: Vec<String>,
    #[serde(default)]
    resume_args: Vec<String>,
    #[serde(default)]
    model_args: Vec<String>,
    #[serde(default)]
    stream: AgentStream,
    #[serde(default)]
    models: Vec<String>,
    docs_url: Option<String>,
    #[serde(default)]
    autonomy: BTreeMap<Autonomy, Vec<String>>,
    #[serde(default)]
    effort: BTreeMap<Effort, Vec<String>>,
    /// Levels a CLI opts into by prompt keyword rather than a flag.
    #[serde(default)]
    effort_prompt: BTreeMap<Effort, String>,
}

/// Only the `[[agent]]` tables of a project's `scriptr.toml`; everything else
/// in that file belongs to `config.rs`.
#[derive(Debug, Default, Deserialize)]
struct ProjectAgents {
    #[serde(default)]
    agent: Vec<AgentFile>,
}

impl AgentFile {
    fn into_adapter(self, source: AgentSource) -> Result<AgentAdapter, String> {
        if self.id.trim().is_empty() {
            return Err("an agent needs a non-empty id".into());
        }
        if self.bin.trim().is_empty() {
            return Err(format!("agent \"{}\" needs a bin", self.id));
        }
        if self.interactive_args.is_empty() && self.headless_args.is_empty() {
            return Err(format!("agent \"{}\" needs interactive_args or headless_args", self.id));
        }
        // Every autonomy level must be present for `Record<Autonomy, string[]>`.
        let autonomy_args = Autonomy::ALL
            .into_iter()
            .map(|a| (a, self.autonomy.get(&a).cloned().unwrap_or_default()))
            .collect();
        // Same for effort, where an empty list additionally means "unsupported":
        // a level is offered when it has flags or prompt text.
        let effort_args =
            Effort::ALL.into_iter().map(|e| (e, self.effort.get(&e).cloned().unwrap_or_default())).collect();
        Ok(AgentAdapter {
            name: self.name.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            bin: self.bin,
            interactive_args: self.interactive_args,
            headless_args: self.headless_args,
            resume_args: self.resume_args,
            model_args: self.model_args,
            autonomy_args,
            effort_args,
            effort_prompt: self.effort_prompt,
            stream: self.stream,
            models: self.models,
            docs_url: self.docs_url,
            source,
            available: false,
            version: None,
        })
    }
}

/// Parses one descriptor. Callers log and skip the error rather than failing.
pub fn parse(text: &str, source: AgentSource) -> Result<AgentAdapter, String> {
    toml::from_str::<AgentFile>(text).map_err(|e| e.to_string())?.into_adapter(source)
}

// ---- loading & merging ----------------------------------------------------

/// `~/.config/scriptr/agents`.
pub fn user_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".config").join("scriptr").join("agents"))
}

/// Later sources win on id; a new id is appended, so built-in order is stable.
fn merge(into: &mut Vec<AgentAdapter>, adapter: AgentAdapter) {
    match into.iter().position(|a| a.id == adapter.id) {
        Some(i) => into[i] = adapter,
        None => into.push(adapter),
    }
}

pub fn builtins() -> Vec<AgentAdapter> {
    let mut out = Vec::with_capacity(BUILTIN.len());
    for text in BUILTIN {
        // A broken built-in is a bug in this repo, not in the user's setup.
        match parse(text, AgentSource::Builtin) {
            Ok(a) => merge(&mut out, a),
            Err(e) => log::error!("built-in agent descriptor: {e}"),
        }
    }
    out
}

/// Reads `*.toml` from the user directory, creating it lazily. A missing
/// directory, an unreadable file or a malformed descriptor is logged and
/// skipped.
fn user_agents(into: &mut Vec<AgentAdapter>) {
    let Some(dir) = user_dir() else { return };
    if !dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::debug!("agents dir {}: {e}", dir.display());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        log::warn!("cannot read {}", dir.display());
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for path in files {
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()) {
            Ok(text) => match parse(&text, AgentSource::User) {
                Ok(a) => merge(into, a),
                Err(e) => log::warn!("agent {}: {e}", path.display()),
            },
            Err(e) => log::warn!("agent {}: {e}", path.display()),
        }
    }
}

/// `[[agent]]` tables in a project's `scriptr.toml` (read-only).
fn project_agents(into: &mut Vec<AgentAdapter>, project_path: &Path) {
    let path = project_path.join(FILE_NAME);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let file: ProjectAgents = match toml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            return;
        }
    };
    for agent in file.agent {
        match agent.into_adapter(AgentSource::Project) {
            Ok(a) => merge(into, a),
            Err(e) => log::warn!("{}: {e}", path.display()),
        }
    }
}

/// Built-in → `~/.config/scriptr/agents/*.toml` → a project's `scriptr.toml`.
/// `available` / `version` are left unresolved; see [`resolve`].
pub fn load(project_path: Option<&Path>) -> Vec<AgentAdapter> {
    let mut out = builtins();
    user_agents(&mut out);
    if let Some(path) = project_path {
        project_agents(&mut out, path);
    }
    out
}

pub fn find(adapters: Vec<AgentAdapter>, id: &str) -> Result<AgentAdapter, String> {
    adapters
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("unknown agent \"{id}\" — check its descriptor"))
}

// ---- availability & version ----------------------------------------------

/// Extra places GUI apps miss: a bundled app's PATH is not a login shell's.
fn extra_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join("bin"));
    }
    dirs
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

/// Looks `bin` up on PATH (plus [`extra_bin_dirs`]). An absolute or relative
/// path is taken as-is.
pub fn resolve_bin(bin: &str) -> Option<PathBuf> {
    if bin.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(bin);
        return executable(&path).then_some(path);
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path_var)
        .chain(extra_bin_dirs())
        .map(|dir| dir.join(bin))
        .find(|candidate| executable(candidate))
}

fn version_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// `<bin> --version`, first line only, 2 s budget, cached per process.
async fn version(bin: &str, path: &Path) -> Option<String> {
    if let Some(hit) = crate::lock(version_cache()).get(bin) {
        return hit.clone();
    }
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg("--version").kill_on_drop(true);
    let found = match tokio::time::timeout(VERSION_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| l.chars().take(80).collect()),
        _ => None,
    };
    crate::lock(version_cache()).insert(bin.to_string(), found.clone());
    found
}

/// Fills `available` / `version` for every adapter, in parallel. Never called
/// on the startup path.
pub async fn resolve(adapters: &mut [AgentAdapter]) {
    let mut set = JoinSet::new();
    for (i, a) in adapters.iter().enumerate() {
        let bin = a.bin.clone();
        set.spawn(async move {
            let found = tokio::task::spawn_blocking({
                let bin = bin.clone();
                move || resolve_bin(&bin)
            })
            .await
            .ok()
            .flatten();
            let version = match &found {
                Some(path) => version(&bin, path).await,
                None => None,
            };
            (i, found.is_some(), version)
        });
    }
    while let Some(joined) = set.join_next().await {
        if let Ok((i, available, version)) = joined {
            adapters[i].available = available;
            adapters[i].version = version;
        }
    }
}

// ---- argv templating ------------------------------------------------------

/// Values the placeholders stand for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Vars<'a> {
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub session: Option<&'a str>,
}

impl Vars<'_> {
    fn value(&self, placeholder: &str) -> Option<&str> {
        match placeholder {
            "{prompt}" => Some(self.prompt),
            "{model}" => self.model,
            "{session}" => self.session,
            _ => None,
        }
    }
}

const PLACEHOLDERS: [&str; 3] = ["{prompt}", "{model}", "{session}"];

/// Substitutes placeholders in one argv template. Returns `None` when the
/// template needs a value this run doesn't have, so the caller drops the whole
/// group instead of leaving a dangling flag like a bare `--model`.
pub fn render_args(template: &[String], vars: &Vars) -> Option<Vec<String>> {
    template
        .iter()
        .map(|arg| {
            PLACEHOLDERS.iter().try_fold(arg.clone(), |acc, p| {
                if !acc.contains(p) {
                    return Some(acc);
                }
                Some(acc.replace(p, vars.value(p)?))
            })
        })
        .collect()
}

/// True when the agent can express this effort level at all — by flag or by
/// prompt keyword. The picker greys out the rest.
pub fn supports_effort(adapter: &AgentAdapter, effort: Effort) -> bool {
    adapter.effort_args.get(&effort).is_some_and(|a| !a.is_empty())
        || adapter.effort_prompt.contains_key(&effort)
}

/// Text this level wants appended to the prompt, for CLIs whose mode is
/// keyword-triggered (Claude Code's ultracode) rather than a flag.
pub fn effort_prompt(adapter: &AgentAdapter, effort: Option<Effort>) -> Option<&str> {
    effort.and_then(|e| adapter.effort_prompt.get(&e)).map(String::as_str)
}

/// The full argv for one run: the interactive (or headless) form, then
/// `model_args` when a model is set, `resume_args` when resuming, the effort
/// flags, then the autonomy flags.
pub fn argv(
    adapter: &AgentAdapter,
    vars: &Vars,
    autonomy: Autonomy,
    effort: Option<Effort>,
    headless: bool,
) -> Result<Vec<String>, String> {
    let template = if headless { &adapter.headless_args } else { &adapter.interactive_args };
    let mut args = render_args(template, vars).ok_or_else(|| {
        format!("agent \"{}\": its argv template needs a value this run has not got", adapter.id)
    })?;
    if vars.model.is_some() {
        args.extend(render_args(&adapter.model_args, vars).unwrap_or_default());
    }
    if vars.session.is_some() {
        args.extend(render_args(&adapter.resume_args, vars).unwrap_or_default());
    }
    if let Some(extra) = effort.and_then(|e| adapter.effort_args.get(&e)) {
        args.extend(render_args(extra, vars).unwrap_or_default());
    }
    if let Some(extra) = adapter.autonomy_args.get(&autonomy) {
        args.extend(render_args(extra, vars).unwrap_or_default());
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> AgentAdapter {
        find(builtins(), "claude-code").unwrap()
    }

    #[test]
    fn builtins_parse_with_every_autonomy_level() {
        let all = builtins();
        let ids: Vec<&str> = all.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["claude-code", "opencode", "cursor-agent", "gemini", "aider"]);
        for a in &all {
            assert_eq!(a.autonomy_args.len(), 3, "{} is missing an autonomy level", a.id);
            assert!(!a.autonomy_args[&Autonomy::Full].is_empty(), "{} has no full flags", a.id);
            assert_eq!(a.source, AgentSource::Builtin);
            assert!(!a.available, "availability must be resolved separately");
        }
        assert_eq!(claude().stream, AgentStream::ClaudeJson);
    }

    #[test]
    fn substitutes_prompt_with_quotes_and_newlines() {
        let prompt = "fix the \"broken\" test\nthen run `pnpm test`";
        let vars = Vars { prompt, model: Some("opus"), session: None };
        let args = argv(&claude(), &vars, Autonomy::Full, None, false).unwrap();
        assert_eq!(
            args,
            vec![prompt, "--model", "opus", "--dangerously-skip-permissions"]
        );
        // Spawned as argv, never as a shell string: nothing is quoted or escaped.
        assert_eq!(args[0], prompt);
    }

    #[test]
    fn effort_maps_claude_levels_and_translates_extra_to_xhigh() {
        let claude = claude();
        let vars = Vars { prompt: "go", model: None, session: None };

        // "extra" is our name for what the CLI calls xhigh.
        assert_eq!(
            argv(&claude, &vars, Autonomy::Ask, Some(Effort::Extra), false).unwrap(),
            vec!["go", "--effort", "xhigh", "--permission-mode", "default"]
        );
        // Effort flags sit before the permission flags.
        assert_eq!(
            argv(&claude, &vars, Autonomy::Full, Some(Effort::Low), false).unwrap(),
            vec!["go", "--effort", "low", "--dangerously-skip-permissions"]
        );
        // No effort chosen → no effort flags at all.
        assert_eq!(
            argv(&claude, &vars, Autonomy::Full, None, false).unwrap(),
            vec!["go", "--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn ultracode_is_max_effort_plus_a_prompt_keyword() {
        let claude = claude();
        // Claude Code's multi-agent mode is opted into by the keyword in the
        // prompt; `--effort ultracode` is not a value the CLI accepts.
        assert_eq!(effort_prompt(&claude, Some(Effort::Ultracode)), Some("ultracode"));
        assert_eq!(effort_prompt(&claude, Some(Effort::Max)), None);
        assert_eq!(effort_prompt(&claude, None), None);
        let vars = Vars { prompt: "go", model: None, session: None };
        let args = argv(&claude, &vars, Autonomy::Ask, Some(Effort::Ultracode), false).unwrap();
        assert!(args.windows(2).any(|w| w == ["--effort", "max"]), "{args:?}");
        assert!(!args.iter().any(|a| a == "ultracode"), "the keyword goes in the prompt, not argv: {args:?}");
    }

    #[test]
    fn unsupported_levels_are_reported_so_the_picker_can_grey_them() {
        let claude = claude();
        for level in Effort::ALL {
            assert!(supports_effort(&claude, level), "claude should offer {}", level.as_str());
        }

        let opencode = find(builtins(), "opencode").unwrap();
        assert!(supports_effort(&opencode, Effort::Low), "opencode maps low to --variant minimal");
        // medium is the provider default: no flag to send, so we don't pretend.
        assert!(!supports_effort(&opencode, Effort::Medium));
        assert!(!supports_effort(&opencode, Effort::Ultracode));

        let gemini = find(builtins(), "gemini").unwrap();
        assert!(Effort::ALL.into_iter().all(|l| !supports_effort(&gemini, l)), "gemini has no effort flag");
        // Every adapter still carries a complete map, so the UI never sees a hole.
        assert_eq!(gemini.effort_args.len(), Effort::ALL.len());
    }

    #[test]
    fn missing_model_drops_the_whole_model_group() {
        let vars = Vars { prompt: "go", model: None, session: None };
        let args = argv(&claude(), &vars, Autonomy::Ask, None, false).unwrap();
        assert_eq!(args, vec!["go", "--permission-mode", "default"]);
        assert!(render_args(&["--model".into(), "{model}".into()], &vars).is_none());

        // Headless keeps the stream flags; a session appends resume_args.
        let vars = Vars { prompt: "go", model: None, session: Some("abc") };
        assert_eq!(
            argv(&claude(), &vars, Autonomy::AutoEdit, None, true).unwrap(),
            vec![
                "-p",
                "go",
                "--output-format",
                "stream-json",
                "--verbose",
                "--resume",
                "abc",
                "--permission-mode",
                "acceptEdits"
            ]
        );
    }

    #[test]
    fn placeholder_inside_a_larger_argument() {
        let template = vec!["--prompt={prompt}".to_string(), "m:{model}".to_string()];
        let vars = Vars { prompt: "a b", model: Some("x"), session: None };
        assert_eq!(render_args(&template, &vars).unwrap(), vec!["--prompt=a b", "m:x"]);
    }

    #[test]
    fn later_sources_win_by_id_and_new_ids_append() {
        let mut all = builtins();
        let before = all.len();
        let override_claude = parse(
            "id = \"claude-code\"\nname = \"Claude (mine)\"\nbin = \"claude\"\n\
             interactive_args = [\"{prompt}\"]\n[autonomy]\nfull = [\"--go\"]\n",
            AgentSource::User,
        )
        .unwrap();
        merge(&mut all, override_claude);
        let mine = parse(
            "id = \"mine\"\nbin = \"echo\"\ninteractive_args = [\"{prompt}\"]\n",
            AgentSource::Project,
        )
        .unwrap();
        merge(&mut all, mine);

        assert_eq!(all.len(), before + 1);
        let claude = all.iter().find(|a| a.id == "claude-code").unwrap();
        assert_eq!((claude.name.as_str(), claude.source), ("Claude (mine)", AgentSource::User));
        // Overriding replaces the descriptor outright: the old models are gone.
        assert!(claude.models.is_empty());
        assert_eq!(claude.autonomy_args[&Autonomy::Ask], Vec::<String>::new());
        // Position is kept, so the picker order doesn't jump around.
        assert_eq!(all[0].id, "claude-code");
        let mine = all.last().unwrap();
        assert_eq!((mine.name.as_str(), mine.source, mine.stream), ("mine", AgentSource::Project, AgentStream::None));
    }

    #[test]
    fn malformed_descriptors_are_errors_not_panics() {
        for bad in [
            "id = \"x\"\nbin =",                                        // syntax error
            "name = \"no id\"\nbin = \"x\"\ninteractive_args = [\"a\"]", // missing id
            "id = \"x\"\nbin = \"x\"",                                   // no argv at all
            "id = \"x\"\nbin = \"x\"\ninteractive_args = [\"a\"]\nwat = 1", // unknown field
            "id = \"x\"\nbin = \"x\"\ninteractive_args = [\"a\"]\n[autonomy]\nyolo = []", // bad level
        ] {
            assert!(parse(bad, AgentSource::User).is_err(), "should have failed: {bad}");
        }
        // And the loader keeps going: built-ins survive a broken user file.
        assert!(!builtins().is_empty());
    }

    #[test]
    fn resolve_bin_finds_a_real_binary() {
        assert!(resolve_bin("sh").is_some());
        assert!(resolve_bin("definitely-not-a-real-binary-9f3a").is_none());
        assert!(resolve_bin("/bin/sh").is_some());
        assert!(resolve_bin("/bin/nope-9f3a").is_none());
    }

    #[tokio::test]
    async fn resolve_fills_availability_and_caches_versions() {
        let mut adapters = vec![
            parse("id = \"sh\"\nbin = \"sh\"\ninteractive_args = [\"-c\", \"{prompt}\"]\n", AgentSource::User).unwrap(),
            parse("id = \"ghost\"\nbin = \"nope-9f3a\"\ninteractive_args = [\"{prompt}\"]\n", AgentSource::User).unwrap(),
        ];
        resolve(&mut adapters).await;
        assert!(adapters[0].available);
        assert!(!adapters[1].available);
        assert!(adapters[1].version.is_none());
        // Cached: a second pass must not re-run the binary.
        assert!(crate::lock(version_cache()).contains_key("sh"));
    }
}
