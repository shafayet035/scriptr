//! Migration smoke test against a real database, run on demand:
//!
//! ```sh
//! SCRIPTR_DB="$HOME/Library/Application Support/scriptr/scriptr.db" \
//!   cargo test --test migrate_real -- --ignored --nocapture
//! ```
//!
//! It works on a copy, so the original is never touched. Ignored by default
//! because it needs a database only the developer's machine has.

use std::path::{Path, PathBuf};

use scriptr_lib::db::Db;

#[test]
#[ignore = "needs SCRIPTR_DB pointing at a real database"]
fn migrates_a_copy_of_a_real_database() {
    let Ok(source) = std::env::var("SCRIPTR_DB") else {
        panic!("set SCRIPTR_DB to a scriptr.db path");
    };
    let source = PathBuf::from(source);
    assert!(source.is_file(), "{} is not a file", source.display());

    // A live database is in WAL mode: a plain file copy would miss everything
    // still in the -wal file. VACUUM INTO writes a consistent snapshot.
    let copy = std::env::temp_dir().join(format!("scriptr-migrate-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&copy);
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute("VACUUM INTO ?1", [copy.to_string_lossy()])
        .expect("snapshot the source database");

    // What the agent-era table held, before anything runs.
    let before = counts(&copy);
    println!("before: {} task(s), statuses {:?}", before.0, before.1);

    let db = Db::open(&copy).unwrap();
    let tasks = db.tasks().unwrap();
    let scripts = db.scripts().unwrap();
    let groups = db.groups().unwrap();

    let mut after: Vec<String> = tasks.iter().map(|t| format!("{:?}", t.status).to_lowercase()).collect();
    after.sort();
    after.dedup();
    println!(
        "after:  {} task(s), statuses {after:?} · {} script(s) · {} group(s) · {} blocker edge(s)",
        tasks.len(),
        scripts.len(),
        groups.len(),
        tasks.iter().map(|t| t.after.len()).sum::<usize>(),
    );

    // Nothing is dropped: a card per row, whatever column it was in.
    assert_eq!(tasks.len(), before.0, "cards were lost in the migration");
    // Every card lands in a column the board can actually render.
    for t in &tasks {
        let s = format!("{:?}", t.status).to_lowercase();
        assert!(
            ["backlog", "todo", "doing", "review", "done"].contains(&s.as_str()),
            "card {:?} is in {s}, which is not a column",
            t.title
        );
    }
    // The agent-era columns are gone, and so are the tables that served them.
    let conn = rusqlite::Connection::open(&copy).unwrap();
    for column in ["agent_id", "autonomy", "workspace", "branch", "pr_url", "effort"] {
        assert!(!has_column(&conn, "tasks", column), "tasks.{column} survived");
    }
    for table in ["task_runs", "task_verify"] {
        let n: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [table], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table} survived");
    }

    // Re-opening is a no-op: the columns are gone, so nothing re-runs.
    drop(db);
    let again = Db::open(&copy).unwrap();
    assert_eq!(again.tasks().unwrap().len(), tasks.len(), "migration is not idempotent");
    assert_eq!(again.scripts().unwrap().len(), scripts.len());

    let _ = std::fs::remove_file(&copy);
}

/// Task count and the distinct statuses, read straight from the table.
fn counts(path: &Path) -> (usize, Vec<String>) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let n: usize = conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize;
    let mut stmt = conn.prepare("SELECT DISTINCT status FROM tasks ORDER BY status").unwrap();
    let statuses = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    (n, statuses)
}

fn has_column(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.flatten().any(|c| c == column))
        .unwrap_or(false)
}
