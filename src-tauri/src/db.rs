//! SQLite storage. A single connection behind a mutex; the data set is tiny.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, Row};

use crate::model::{Group, HistoryEntry, Project, Script, Settings, Task};

pub type DbResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn to_json<T: serde::Serialize>(v: &T) -> DbResult<String> {
    serde_json::to_string(v).map_err(err)
}


/// Reads a JSON text column.
fn json<T: serde::de::DeserializeOwned>(r: &Row<'_>, i: usize) -> rusqlite::Result<T> {
    let s: String = r.get(i)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    })
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    path        TEXT NOT NULL,
    branch      TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS scripts (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    label       TEXT,
    cmd         TEXT NOT NULL,
    cwd         TEXT NOT NULL,
    shell       TEXT,
    env         TEXT NOT NULL,
    env_file    TEXT,
    after       TEXT NOT NULL,
    ready       TEXT NOT NULL,
    restart     TEXT NOT NULL,
    port        INTEGER,
    source      TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    script_ids  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS run_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    script_id   TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    exit_code   INTEGER
);
CREATE INDEX IF NOT EXISTS run_history_script ON run_history(script_id, started_at);
-- AI tasks (docs/AI-PM.md). after / verify / labels and the enum columns hold
-- JSON text, like scripts.ready / scripts.restart.
CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    goal            TEXT NOT NULL,
    after           TEXT NOT NULL,
    status          TEXT NOT NULL,
    priority        INTEGER NOT NULL DEFAULT 0,
    assignee        TEXT,
    labels          TEXT NOT NULL,
    issue_url       TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    sort_order      INTEGER NOT NULL DEFAULT 0
);
-- Relationships are rows, not JSON: the database enforces that a dependency
-- points at something real and removes the edge when either end is deleted.
-- The foreign keys are DEFERRABLE so a batch (a scriptr.toml import) can write
-- a script that waits for one later in the same transaction.
CREATE TABLE IF NOT EXISTS script_deps (
    script_id   TEXT NOT NULL REFERENCES scripts(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    depends_on  TEXT NOT NULL REFERENCES scripts(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (script_id, depends_on)
);
CREATE INDEX IF NOT EXISTS script_deps_on ON script_deps(depends_on);
CREATE TABLE IF NOT EXISTS group_scripts (
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    script_id   TEXT NOT NULL REFERENCES scripts(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, script_id)
);
CREATE INDEX IF NOT EXISTS group_scripts_script ON group_scripts(script_id);
CREATE TABLE IF NOT EXISTS task_deps (
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    depends_on  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    position    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (task_id, depends_on)
);
CREATE INDEX IF NOT EXISTS task_deps_on ON task_deps(depends_on);
"#;

/// One link table's shape: `links(left, right)` ordered by position.
struct Link {
    table: &'static str,
    left: &'static str,
    right: &'static str,
    /// The table `right` points at, used to skip dead edges when migrating.
    right_table: &'static str,
    /// The JSON column it replaced, dropped once its rows are migrated.
    legacy: (&'static str, &'static str),
}

const LINKS: [Link; 3] = [
    Link { table: "script_deps", left: "script_id", right: "depends_on", right_table: "scripts", legacy: ("scripts", "after") },
    Link { table: "group_scripts", left: "group_id", right: "script_id", right_table: "scripts", legacy: ("groups", "script_ids") },
    Link { table: "task_deps", left: "task_id", right: "depends_on", right_table: "tasks", legacy: ("tasks", "after") },
];

pub struct Db {
    conn: Mutex<Connection>,
    path: PathBuf,
}

/// `~/Library/Application Support/scriptr/scriptr.db` on macOS.
pub fn default_path() -> DbResult<PathBuf> {
    let dir = dirs::data_dir().ok_or("no data directory on this system")?.join("scriptr");
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.join("scriptr.db"))
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode = WAL;").map_err(err)?;
        Self::init(conn, path.to_path_buf())
    }

    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory().map_err(err)?, PathBuf::from(":memory:"))
    }

    fn init(mut conn: Connection, path: PathBuf) -> DbResult<Self> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        Self::migrate_links(&mut conn)?;
        Self::migrate_board(&mut conn)?;
        // Any patch-up for a newly added column goes here — after the
        // migrations, never before, or a column a migration just dropped comes
        // straight back.
        Ok(Self { conn: Mutex::new(conn), path })
    }

    /// Moves relationships out of their old JSON columns into link tables, then
    /// drops the columns. Runs once per database: after the drop, `has_column`
    /// is false and this is a no-op.
    fn migrate_links(conn: &mut Connection) -> DbResult<()> {
        let tx = conn.transaction().map_err(err)?;
        for link in &LINKS {
            let (table, column) = link.legacy;
            if !Self::has_column(&tx, table, column)? {
                continue;
            }
            let rows: Vec<(String, String)> = {
                let mut stmt = tx.prepare(&format!("SELECT id, {column} FROM {table}")).map_err(err)?;
                let mapped = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .map_err(err)?;
                mapped.collect::<Result<_, _>>().map_err(err)?
            };
            let mut moved = 0usize;
            for (id, json_ids) in rows {
                // A dangling id was possible before: JSON could point at a
                // deleted row. Those edges are dropped rather than migrated.
                for (i, target) in serde_json::from_str::<Vec<String>>(&json_ids).unwrap_or_default().iter().enumerate()
                {
                    // Deferred foreign keys would only fail at COMMIT, taking the
                    // whole migration with them, so a dead id is skipped here.
                    let sql = format!(
                        "INSERT OR IGNORE INTO {} ({}, {}, position) \
                         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM {} WHERE id = ?2)",
                        link.table, link.left, link.right, link.right_table
                    );
                    match tx.execute(&sql, params![id, target, i as i64]).map_err(err)? {
                        0 => log::warn!("dropped dead {}.{column} link {id} -> {target}", table),
                        _ => moved += 1,
                    }
                }
            }
            tx.execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), []).map_err(err)?;
            log::info!("migrated {moved} {}.{column} links into {}", table, link.table);
        }
        tx.commit().map_err(err)
    }

    /// Rebuilds `tasks` as a board card when it still carries the agent-era
    /// columns, mapping the runner's phases onto the board's columns. One way:
    /// an older build cannot read the result, which is the point — those
    /// columns described a runner that no longer exists.
    fn migrate_board(conn: &mut Connection) -> DbResult<()> {
        if !Self::has_column(conn, "tasks", "agent_id")? {
            return Ok(());
        }
        let tx = conn.transaction().map_err(err)?;
        tx.execute_batch(
            r#"
CREATE TABLE tasks_new (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    goal            TEXT NOT NULL,
    status          TEXT NOT NULL,
    priority        INTEGER NOT NULL DEFAULT 0,
    assignee        TEXT,
    labels          TEXT NOT NULL,
    issue_url       TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    sort_order      INTEGER NOT NULL DEFAULT 0
);
-- Enum columns hold JSON, so the values carry their quotes: comparing
-- against a bare 'queued' matches nothing and silently backlogs the board.
INSERT INTO tasks_new
SELECT id, project_id, title, goal,
       CASE status
         WHEN '"queued"'     THEN '"todo"'
         WHEN '"working"'    THEN '"doing"'
         WHEN '"verifying"'  THEN '"doing"'
         WHEN '"publishing"' THEN '"doing"'
         WHEN '"failed"'     THEN '"backlog"'
         WHEN '"cancelled"'  THEN '"backlog"'
         WHEN '"review"'     THEN '"review"'
         WHEN '"done"'       THEN '"done"'
         WHEN '"backlog"'    THEN '"backlog"'
         ELSE '"backlog"'
       END,
       priority, assignee, labels, issue_url, created_at, updated_at, sort_order
FROM tasks;
DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;
DROP TABLE IF EXISTS task_runs;
DROP TABLE IF EXISTS task_verify;
"#,
        )
        .map_err(err)?;
        let moved: i64 = tx.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0)).map_err(err)?;
        tx.commit().map_err(err)?;
        log::info!("migrated {moved} task(s) to board cards; dropped the agent-era columns");
        Ok(())
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> DbResult<bool> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).map_err(err)?;
        let cols: Vec<String> =
            stmt.query_map([], |r| r.get::<_, String>(1)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        Ok(cols.iter().any(|c| c == column))
    }

    /// All rows of a link table as `left -> [right…]`, in position order.
    fn link_map(conn: &Connection, link: &Link) -> DbResult<HashMap<String, Vec<String>>> {
        let sql = format!(
            "SELECT {}, {} FROM {} ORDER BY {}, position",
            link.left, link.right, link.table, link.left
        );
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (left, right) = row.map_err(err)?;
            out.entry(left).or_default().push(right);
        }
        Ok(out)
    }

    /// Rewrites one row's links. The caller holds the transaction, so a bad id
    /// fails the whole write rather than leaving half an edge.
    fn replace_links(conn: &Connection, link: &Link, left: &str, rights: &[String]) -> DbResult<()> {
        conn.execute(&format!("DELETE FROM {} WHERE {} = ?1", link.table, link.left), [left]).map_err(err)?;
        let sql = format!(
            "INSERT OR IGNORE INTO {} ({}, {}, position) VALUES (?1, ?2, ?3)",
            link.table, link.left, link.right
        );
        for (i, right) in rights.iter().enumerate() {
            conn.execute(&sql, params![left, right, i as i64]).map_err(err)?;
        }
        Ok(())
    }


    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite inconsistent.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- projects -------------------------------------------------------

    pub fn projects(&self) -> DbResult<Vec<Project>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT id, name, path, branch, sort_order FROM projects ORDER BY sort_order, name")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    branch: r.get(3)?,
                    sort_order: r.get(4)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)
    }

    pub fn project(&self, id: &str) -> DbResult<Project> {
        self.projects()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("project {id} not found"))
    }

    pub fn insert_project(&self, p: &Project) -> DbResult<()> {
        self.conn()
            .execute(
                "INSERT INTO projects (id, name, path, branch, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![p.id, p.name, p.path, p.branch, p.sort_order],
            )
            .map(drop)
            .map_err(err)
    }

    pub fn delete_project(&self, id: &str) -> DbResult<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM run_history WHERE script_id IN (SELECT id FROM scripts WHERE project_id = ?1)",
            [id],
        )
        .map_err(err)?;
        conn.execute("DELETE FROM projects WHERE id = ?1", [id]).map(drop).map_err(err)
    }

    pub fn next_project_order(&self) -> DbResult<i64> {
        self.conn()
            .query_row("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM projects", [], |r| r.get(0))
            .map_err(err)
    }

    // ---- scripts --------------------------------------------------------

    const SCRIPT_COLS: &'static str = "id, project_id, name, label, cmd, cwd, shell, env, env_file, \
         ready, restart, port, source, sort_order";

    /// `after` is filled from `script_deps` by the caller.
    fn script_from_row(r: &Row<'_>) -> rusqlite::Result<Script> {
        Ok(Script {
            id: r.get(0)?,
            project_id: r.get(1)?,
            name: r.get(2)?,
            label: r.get(3)?,
            cmd: r.get(4)?,
            cwd: r.get(5)?,
            shell: r.get(6)?,
            env: json(r, 7)?,
            env_file: r.get(8)?,
            after: Vec::new(),
            ready: json(r, 9)?,
            restart: json(r, 10)?,
            port: r.get(11)?,
            source: r.get(12)?,
            sort_order: r.get(13)?,
        })
    }

    fn query_scripts(&self, filter: &str, arg: Option<&str>) -> DbResult<Vec<Script>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {} FROM scripts {filter} ORDER BY project_id, sort_order, name",
            Self::SCRIPT_COLS
        );
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = match arg {
            Some(a) => stmt.query_map([a], Self::script_from_row),
            None => stmt.query_map([], Self::script_from_row),
        }
        .map_err(err)?;
        let mut scripts: Vec<Script> = rows.collect::<Result<_, _>>().map_err(err)?;
        let deps = Self::link_map(&conn, &LINKS[0])?;
        for s in &mut scripts {
            s.after = deps.get(&s.id).cloned().unwrap_or_default();
        }
        Ok(scripts)
    }

    pub fn scripts(&self) -> DbResult<Vec<Script>> {
        self.query_scripts("", None)
    }

    pub fn project_scripts(&self, project_id: &str) -> DbResult<Vec<Script>> {
        self.query_scripts("WHERE project_id = ?1", Some(project_id))
    }

    pub fn script(&self, id: &str) -> DbResult<Script> {
        self.query_scripts("WHERE id = ?1", Some(id))?
            .pop()
            .ok_or_else(|| format!("script {id} not found"))
    }

    pub fn upsert_script(&self, s: &Script) -> DbResult<()> {
        self.upsert_scripts(std::slice::from_ref(s))
    }

    /// Writes scripts and their dependencies in one transaction, so a batch may
    /// reference a script that appears later in the same batch (an import).
    pub fn upsert_scripts(&self, scripts: &[Script]) -> DbResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(err)?;
        let sql = format!(
            "INSERT OR REPLACE INTO scripts ({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            Self::SCRIPT_COLS
        );
        for s in scripts {
            tx.execute(
                &sql,
                params![
                    s.id,
                    s.project_id,
                    s.name,
                    s.label,
                    s.cmd,
                    s.cwd,
                    s.shell,
                    to_json(&s.env)?,
                    s.env_file,
                    to_json(&s.ready)?,
                    to_json(&s.restart)?,
                    s.port,
                    s.source,
                    s.sort_order
                ],
            )
            .map_err(err)?;
        }
        for s in scripts {
            Self::replace_links(&tx, &LINKS[0], &s.id, &s.after)?;
        }
        tx.commit().map_err(err)
    }

    /// Deletes a script. `ON DELETE CASCADE` removes its dependency edges,
    /// group membership and any task that verified against it.
    pub fn delete_script(&self, id: &str) -> DbResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM run_history WHERE script_id = ?1", [id]).map_err(err)?;
        conn.execute("DELETE FROM scripts WHERE id = ?1", [id]).map(drop).map_err(err)
    }

    // ---- groups ---------------------------------------------------------

    fn query_groups(&self, filter: &str, arg: Option<&str>) -> DbResult<Vec<Group>> {
        let conn = self.conn();
        let sql = format!("SELECT id, project_id, name FROM groups {filter} ORDER BY name");
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let map = |r: &Row<'_>| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?));
        let rows: Vec<(String, String, String)> = match arg {
            Some(a) => stmt.query_map([a], map),
            None => stmt.query_map([], map),
        }
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)?;
        let members = Self::link_map(&conn, &LINKS[1])?;
        Ok(rows
            .into_iter()
            .map(|(id, project_id, name)| {
                let script_ids = members.get(&id).cloned().unwrap_or_default();
                Group { id, project_id, name, script_ids }
            })
            .collect())
    }

    pub fn groups(&self) -> DbResult<Vec<Group>> {
        self.query_groups("", None)
    }

    pub fn project_groups(&self, project_id: &str) -> DbResult<Vec<Group>> {
        self.query_groups("WHERE project_id = ?1", Some(project_id))
    }

    pub fn group(&self, id: &str) -> DbResult<Group> {
        self.query_groups("WHERE id = ?1", Some(id))?
            .pop()
            .ok_or_else(|| format!("group {id} not found"))
    }

    pub fn upsert_group(&self, g: &Group) -> DbResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(err)?;
        tx.execute(
            "INSERT OR REPLACE INTO groups (id, project_id, name) VALUES (?1, ?2, ?3)",
            params![g.id, g.project_id, g.name],
        )
        .map_err(err)?;
        Self::replace_links(&tx, &LINKS[1], &g.id, &g.script_ids)?;
        tx.commit().map_err(err)
    }

    pub fn delete_group(&self, id: &str) -> DbResult<()> {
        self.conn().execute("DELETE FROM groups WHERE id = ?1", [id]).map(drop).map_err(err)
    }

    // ---- settings -------------------------------------------------------

    /// Settings are stored as one JSON value per key so new fields fall back
    /// to defaults on older databases.
    pub fn settings(&self) -> DbResult<Settings> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT key, value FROM settings").map_err(err)?;
        let mut obj = serde_json::to_value(Settings::default()).map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        for row in rows {
            let (k, v) = row.map_err(err)?;
            if let (Some(map), Ok(val)) = (obj.as_object_mut(), serde_json::from_str(&v)) {
                if map.contains_key(&k) {
                    map.insert(k, val);
                }
            }
        }
        // A malformed stored value falls back to defaults rather than bricking the app.
        Ok(serde_json::from_value(obj).unwrap_or_default())
    }

    pub fn set_settings(&self, s: &Settings) -> DbResult<()> {
        let value = serde_json::to_value(s).map_err(err)?;
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(err)?;
        for (k, v) in value.as_object().into_iter().flatten() {
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![k, v.to_string()],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    // ---- run history ----------------------------------------------------

    pub fn history_start(&self, script_id: &str, started_at: i64) -> DbResult<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO run_history (script_id, started_at) VALUES (?1, ?2)",
            params![script_id, started_at],
        )
        .map_err(err)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn history_end(&self, row_id: i64, ended_at: i64, exit_code: Option<i32>) -> DbResult<()> {
        self.conn()
            .execute(
                "UPDATE run_history SET ended_at = ?1, exit_code = ?2 WHERE id = ?3",
                params![ended_at, exit_code, row_id],
            )
            .map(drop)
            .map_err(err)
    }

    pub fn history(&self, script_id: &str) -> DbResult<Vec<HistoryEntry>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT started_at, ended_at, exit_code FROM run_history WHERE script_id = ?1 \
                 ORDER BY started_at DESC LIMIT 100",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([script_id], |r| {
                Ok(HistoryEntry { started_at: r.get(0)?, ended_at: r.get(1)?, exit_code: r.get(2)? })
            })
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)
    }

    // ---- tasks ----------------------------------------------------------

    const TASK_COLS: &'static str =
        "id, project_id, title, goal, status, priority, assignee, labels, issue_url, created_at, updated_at, sort_order";

    fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Task> {
        Ok(Task {
            id: r.get(0)?,
            project_id: r.get(1)?,
            title: r.get(2)?,
            goal: r.get(3)?,
            after: Vec::new(),
            status: json(r, 4)?,
            priority: r.get(5)?,
            assignee: r.get(6)?,
            labels: json(r, 7)?,
            issue_url: r.get(8)?,
            created_at: r.get(9)?,
            updated_at: r.get(10)?,
            sort_order: r.get(11)?,
        })
    }

    fn query_tasks(&self, filter: &str, arg: Option<&str>) -> DbResult<Vec<Task>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {} FROM tasks {filter} ORDER BY sort_order, created_at",
            Self::TASK_COLS
        );
        let mut stmt = conn.prepare(&sql).map_err(err)?;
        let rows = match arg {
            Some(a) => stmt.query_map([a], Self::task_from_row),
            None => stmt.query_map([], Self::task_from_row),
        }
        .map_err(err)?;
        let mut tasks: Vec<Task> = rows.collect::<Result<_, _>>().map_err(err)?;
        let deps = Self::link_map(&conn, &LINKS[2])?;
        for t in &mut tasks {
            t.after = deps.get(&t.id).cloned().unwrap_or_default();
        }
        Ok(tasks)
    }

    /// Every card, across projects.
    pub fn tasks(&self) -> DbResult<Vec<Task>> {
        self.query_tasks("", None)
    }

    pub fn project_tasks(&self, project_id: &str) -> DbResult<Vec<Task>> {
        self.query_tasks("WHERE project_id = ?1", Some(project_id))
    }

    pub fn task(&self, id: &str) -> DbResult<Task> {
        self.query_tasks("WHERE id = ?1", Some(id))?
            .pop()
            .ok_or_else(|| format!("task {id} not found"))
    }

    pub fn upsert_task(&self, t: &Task) -> DbResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(err)?;
        tx.execute(
            &format!(
                "INSERT OR REPLACE INTO tasks ({}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                Self::TASK_COLS
            ),
            params![
                t.id,
                t.project_id,
                t.title,
                t.goal,
                to_json(&t.status)?,
                t.priority,
                t.assignee,
                to_json(&t.labels)?,
                t.issue_url,
                t.created_at,
                t.updated_at,
                t.sort_order
            ],
        )
        .map_err(err)?;
        Self::replace_links(&tx, &LINKS[2], &t.id, &t.after)?;
        tx.commit().map_err(err)
    }

    /// Deletes a task. `ON DELETE CASCADE` takes its runs, its dependency edges
    /// and any edge pointing at it.
    pub fn delete_task(&self, id: &str) -> DbResult<()> {
        self.conn().execute("DELETE FROM tasks WHERE id = ?1", [id]).map(drop).map_err(err)
    }


    /// Writes a consistent copy of the database to `dest`.
    pub fn backup(&self, dest: &str) -> DbResult<()> {
        self.conn().execute("VACUUM INTO ?1", [dest]).map(drop).map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Gate, RestartPolicy, TaskStatus};

    /// A database with one project, ready for scripts and tasks.
    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.insert_project(&Project { id: "p".into(), name: "p".into(), path: "/tmp".into(), branch: None,
        sort_order: 0 })
            .unwrap();
        db
    }

    fn sample_script(id: &str, project_id: &str, name: &str) -> Script {
        Script {
            id: id.into(),
            project_id: project_id.into(),
            name: name.into(),
            label: None,
            cmd: "echo hi".into(),
            cwd: ".".into(),
            shell: None,
            env: Default::default(),
            env_file: None,
            after: vec![],
            ready: Gate::Instant,
            restart: RestartPolicy::default(),
            port: None,
            source: None,
            sort_order: 0,
        }
    }

    #[test]
    fn crud_roundtrip_and_delete_scrubs_references() {
        let db = Db::open_in_memory().unwrap();
        db.insert_project(&Project { id: "p".into(), name: "P".into(), path: "/tmp".into(), branch: None,
        sort_order: 0 })
            .unwrap();
        let a = sample_script("a", "p", "db");
        let mut b = sample_script("b", "p", "api");
        b.after = vec!["a".into()];
        b.ready = Gate::Port { port: 8000, timeout_ms: 1000 };
        db.upsert_script(&a).unwrap();
        db.upsert_script(&b).unwrap();
        db.upsert_group(&Group { id: "g".into(), project_id: "p".into(), name: "G".into(), script_ids: vec!["a".into(), "b".into()] })
            .unwrap();
        assert_eq!(db.script("b").unwrap(), b);

        db.delete_script("a").unwrap();
        assert!(db.script("b").unwrap().after.is_empty());
        assert_eq!(db.group("g").unwrap().script_ids, vec!["b".to_string()]);

        let mut s = db.settings().unwrap();
        s.keep_toml_in_sync = true;
        db.set_settings(&s).unwrap();
        assert!(db.settings().unwrap().keep_toml_in_sync);

        let row = db.history_start("b", 1).unwrap();
        db.history_end(row, 2, Some(3)).unwrap();
        assert_eq!(db.history("b").unwrap()[0].exit_code, Some(3));

        db.delete_project("p").unwrap();
        assert!(db.scripts().unwrap().is_empty());
        assert!(db.groups().unwrap().is_empty());
    }

    fn sample_task(id: &str) -> Task {
        Task {
            id: id.into(),
            title: "Fix the flaky test".into(),
            goal: "The \"api\" suite fails\nsometimes. Fix it.".into(),
            project_id: "p".into(),
            after: vec![],
            status: TaskStatus::Backlog,
            priority: 3,
            assignee: Some("me".into()),
            labels: vec!["bug".into(), "ci".into()],
            issue_url: Some("https://github.com/x/y/issues/1".into()),
            created_at: 1,
            updated_at: 2,
            sort_order: 0,
        }
    }

    #[test]
    fn tasks_round_trip_every_field() {
        let db = seeded();
        let a = sample_task("a");
        let mut b = sample_task("b");
        b.id = "b".into();
        b.after = vec!["a".into()];
        b.status = TaskStatus::Review;
        b.assignee = None;
        b.sort_order = 1;
        db.upsert_task(&a).unwrap();
        db.upsert_task(&b).unwrap();
        assert_eq!(db.task("a").unwrap(), a);
        assert_eq!(db.task("b").unwrap(), b);
        assert_eq!(db.project_tasks("p").unwrap(), vec![a.clone(), b.clone()]);

        // Deleting a card scrubs it from whatever it was blocking.
        db.delete_task("a").unwrap();
        assert!(db.task("b").unwrap().after.is_empty());
        assert!(db.task("a").is_err());
    }

    // ---- link tables ----------------------------------------------------

    /// What JSON columns could not do: the database refuses an edge to nothing.
    #[test]
    fn a_dependency_on_a_missing_script_is_refused() {
        let db = seeded();
        let mut s = sample_script("ghost-dep", "p", "ghost-dep");
        s.after = vec!["does-not-exist".into()];
        let err = db.upsert_script(&s).unwrap_err();
        assert!(err.to_lowercase().contains("foreign key"), "{err}");
        assert!(db.script("ghost-dep").is_err(), "the script must not be half-written");
    }

    /// And it removes the edge when either end goes, with no hand-written scrub.
    #[test]
    fn deleting_a_script_takes_its_edges_with_it() {
        let db = seeded();
        let db_script = sample_script("db", "p", "db");
        db.upsert_script(&db_script).unwrap();
        let mut api = sample_script("api", "p", "api");
        api.after = vec!["db".into()];
        db.upsert_script(&api).unwrap();
        db.upsert_group(&Group {
            id: "g1".into(),
            project_id: "p".into(),
            name: "Full stack".into(),
            script_ids: vec!["db".into(), "api".into()],
        })
        .unwrap();

        db.delete_script("db").unwrap();

        assert_eq!(db.script("api").unwrap().after, Vec::<String>::new(), "the dangling edge is gone");
        assert_eq!(db.group("g1").unwrap().script_ids, vec!["api".to_string()], "and so is the membership");
    }

    /// Order is data: a group lists its scripts in the order you arranged them.
    #[test]
    fn link_order_survives_a_round_trip() {
        let db = seeded();
        for name in ["a", "b", "c"] {
            db.upsert_script(&sample_script(name, "p", name)).unwrap();
        }
        let ids: Vec<String> = vec!["c".into(), "a".into(), "b".into()];
        db.upsert_group(&Group {
            id: "g1".into(),
            project_id: "p".into(),
            name: "G".into(),
            script_ids: ids.clone(),
        })
        .unwrap();
        assert_eq!(db.group("g1").unwrap().script_ids, ids);

        // Rewriting replaces the set rather than appending to it.
        db.upsert_group(&Group {
            id: "g1".into(),
            project_id: "p".into(),
            name: "G".into(),
            script_ids: vec!["b".into()],
        })
        .unwrap();
        assert_eq!(db.group("g1").unwrap().script_ids, vec!["b".to_string()]);
    }

    /// An import may mention a script defined further down the same file.
    #[test]
    fn a_batch_may_reference_a_script_later_in_the_batch() {
        let db = seeded();
        let mut api = sample_script("api", "p", "api");
        api.after = vec!["db".into()];
        let db_script = sample_script("db", "p", "db");
        db.upsert_scripts(&[api, db_script]).unwrap();
        assert_eq!(db.script("api").unwrap().after, vec!["db".to_string()]);
    }

    #[test]
    fn a_blocked_card_loses_the_edge_when_its_blocker_goes() {
        let db = seeded();
        db.upsert_task(&sample_task("t1")).unwrap();
        let mut second = sample_task("t2");
        second.after = vec!["t1".into()];
        db.upsert_task(&second).unwrap();
        assert_eq!(db.task("t2").unwrap().after, vec!["t1".to_string()]);

        db.delete_task("t1").unwrap();
        assert_eq!(db.task("t2").unwrap().after, Vec::<String>::new());

        // And a card cannot be blocked by one that was never there.
        let mut ghost = sample_task("t3");
        ghost.after = vec!["nope".into()];
        assert!(db.upsert_task(&ghost).is_err());
    }

    /// The agent era's tasks become cards, and the columns it needed are gone.
    #[test]
    fn the_migration_turns_agent_tasks_into_board_cards() {
        let path = std::env::temp_dir().join(format!("scriptr-board-mig-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        // The shape before this change. Status is an enum column, so the values
        // carry their JSON quotes — the thing that made the first attempt wrong.
        conn.execute_batch(
            r#"
            CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL, branch TEXT, sort_order INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE tasks (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL, goal TEXT NOT NULL,
                agent_id TEXT NOT NULL, model TEXT, autonomy TEXT NOT NULL, workspace TEXT NOT NULL, branch TEXT,
                base_branch TEXT, pr_url TEXT, pr_number INTEGER, status TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 0,
                assignee TEXT, labels TEXT NOT NULL, issue_url TEXT, budget_tokens INTEGER, budget_seconds INTEGER,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0, effort TEXT);
            CREATE TABLE task_runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, state TEXT NOT NULL);
            INSERT INTO projects VALUES ('p','p','/tmp',NULL,0);
            INSERT INTO tasks VALUES ('a','p','Queued one','g','claude-code',NULL,'"ask"','"in-place"',NULL,NULL,NULL,NULL,'"queued"',1,NULL,'[]',NULL,NULL,NULL,1,2,0,NULL);
            INSERT INTO tasks VALUES ('b','p','Working one','g','claude-code',NULL,'"ask"','"in-place"',NULL,NULL,NULL,NULL,'"working"',2,NULL,'[]',NULL,NULL,NULL,1,2,1,NULL);
            INSERT INTO tasks VALUES ('c','p','Failed one','g','claude-code',NULL,'"ask"','"in-place"',NULL,NULL,NULL,NULL,'"failed"',3,NULL,'[]',NULL,NULL,NULL,1,2,2,NULL);
            INSERT INTO tasks VALUES ('d','p','Done one','g','claude-code',NULL,'"ask"','"in-place"',NULL,NULL,NULL,NULL,'"done"',0,NULL,'[]',NULL,NULL,NULL,1,2,3,NULL);
            INSERT INTO tasks VALUES ('e','p','Publishing one','g','claude-code',NULL,'"ask"','"in-place"',NULL,NULL,NULL,NULL,'"publishing"',1,NULL,'[]',NULL,NULL,NULL,1,2,4,NULL);
            INSERT INTO task_runs VALUES ('r','a','"stopped"');
        "#,
        )
        .unwrap();
        drop(conn);

        let db = Db::open(&path).unwrap();
        let by_id = |id: &str| db.task(id).unwrap();
        // The runner's phases map onto the board's columns.
        assert_eq!(by_id("a").status, TaskStatus::Todo, "queued is groomed work");
        assert_eq!(by_id("b").status, TaskStatus::Doing);
        assert_eq!(by_id("e").status, TaskStatus::Doing, "publishing was still in flight");
        assert_eq!(by_id("c").status, TaskStatus::Backlog, "a failed run is work still to do");
        assert_eq!(by_id("d").status, TaskStatus::Done);
        // Nothing else about the card is disturbed.
        assert_eq!(by_id("c").title, "Failed one");
        assert_eq!(by_id("c").priority, 3);
        assert_eq!(db.tasks().unwrap().len(), 5);

        // The agent-era columns and tables are gone, and stay gone.
        let conn = rusqlite::Connection::open(&path).unwrap();
        for column in ["agent_id", "autonomy", "workspace", "pr_url", "effort"] {
            assert!(!Db::has_column(&conn, "tasks", column).unwrap(), "tasks.{column} survived");
        }
        let runs: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE name='task_runs'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0, "task_runs survived");
        drop(conn);

        // Re-opening is a no-op rather than a second migration.
        drop(db);
        let again = Db::open(&path).unwrap();
        assert_eq!(again.tasks().unwrap().len(), 5);
        assert_eq!(again.task("a").unwrap().status, TaskStatus::Todo);
        let _ = std::fs::remove_file(&path);
    }

    /// An older database keeps its relationships and loses the JSON columns.
    #[test]
    fn the_migration_moves_json_columns_into_link_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // The shape before this change: relationships as JSON text.
        conn.execute_batch(
            r#"
            CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL, branch TEXT, sort_order INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE scripts (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, label TEXT,
                cmd TEXT NOT NULL, cwd TEXT NOT NULL, shell TEXT, env TEXT NOT NULL, env_file TEXT,
                after TEXT NOT NULL, ready TEXT NOT NULL, restart TEXT NOT NULL, port INTEGER, source TEXT,
                sort_order INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE groups (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, script_ids TEXT NOT NULL);
            INSERT INTO projects VALUES ('p','p','/tmp',NULL,0);
            INSERT INTO scripts VALUES ('db','p','db',NULL,'x','.',NULL,'{}',NULL,'[]','{"kind":"instant"}','{"on":"never","max":5,"backoffMs":2000,"backoffMaxMs":32000}',NULL,NULL,0);
            INSERT INTO scripts VALUES ('api','p','api',NULL,'x','.',NULL,'{}',NULL,'["db","deleted-long-ago"]','{"kind":"instant"}','{"on":"never","max":5,"backoffMs":2000,"backoffMaxMs":32000}',NULL,NULL,1);
            INSERT INTO groups VALUES ('g','p','G','["api","db"]');
            "#,
        )
        .unwrap();

        let db = Db::init(conn, PathBuf::from(":memory:")).unwrap();

        assert_eq!(db.script("api").unwrap().after, vec!["db".to_string()], "real edges survive");
        assert_eq!(db.group("g").unwrap().script_ids, vec!["api".to_string(), "db".to_string()], "order survives");
        // The JSON columns are gone, so there is one source of truth.
        let conn = db.conn();
        assert!(!Db::has_column(&conn, "scripts", "after").unwrap());
        assert!(!Db::has_column(&conn, "groups", "script_ids").unwrap());
    }
}
