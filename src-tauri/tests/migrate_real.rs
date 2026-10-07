//! Migration smoke test against a real database, run on demand:
//!
//! ```sh
//! SCRIPTR_DB="$HOME/Library/Application Support/scriptr/scriptr.db" \
//!   cargo test --test migrate_real -- --ignored --nocapture
//! ```
//!
//! It works on a copy, so the original is never touched. Ignored by default
//! because it needs a database only the developer's machine has.

use std::path::PathBuf;

use scriptr_lib::db::Db;

#[test]
#[ignore = "needs SCRIPTR_DB pointing at a real database"]
fn migrates_a_copy_of_a_real_database() {
    let Ok(source) = std::env::var("SCRIPTR_DB") else {
        panic!("set SCRIPTR_DB to a scriptr.db path");
    };
    let source = PathBuf::from(source);
    assert!(source.is_file(), "{} is not a file", source.display());

    // Read the JSON columns as they are now, before any migration runs.
    let before = legacy_counts(&source);

    // A live database is in WAL mode: a plain file copy would miss everything
    // still in the -wal file. VACUUM INTO writes a consistent snapshot.
    let copy = std::env::temp_dir().join(format!("scriptr-migrate-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&copy);
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute("VACUUM INTO ?1", [copy.to_string_lossy()])
        .expect("snapshot the source database");
    let db = Db::open(&copy).unwrap();

    let scripts = db.scripts().unwrap();
    let groups = db.groups().unwrap();
    let deps: usize = scripts.iter().map(|s| s.after.len()).sum();
    let members: usize = groups.iter().map(|g| g.script_ids.len()).sum();

    println!(
        "projects {} · scripts {} · groups {} · dependency edges {deps} (was {}) · group members {members} (was {})",
        db.projects().unwrap().len(),
        scripts.len(),
        groups.len(),
        before.0,
        before.1
    );

    // Opening again must be a no-op: the columns are gone, so nothing re-runs.
    drop(db);
    let again = Db::open(&copy).unwrap();
    assert_eq!(again.scripts().unwrap().iter().map(|s| s.after.len()).sum::<usize>(), deps, "migration is not idempotent");
    assert_eq!(again.groups().unwrap().iter().map(|g| g.script_ids.len()).sum::<usize>(), members);

    // Edges only disappear if they pointed at something already deleted.
    assert!(deps <= before.0 && members <= before.1, "migration invented links");
    let ids: Vec<String> = again.scripts().unwrap().into_iter().map(|s| s.id).collect();
    for s in again.scripts().unwrap() {
        for dep in &s.after {
            assert!(ids.contains(dep), "script {} still points at a missing {dep}", s.name);
        }
    }
    let _ = std::fs::remove_file(&copy);
}

/// Counts links in the pre-migration JSON columns, if they are still there.
fn legacy_counts(path: &PathBuf) -> (usize, usize) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let count = |sql: &str| -> usize {
        let Ok(mut stmt) = conn.prepare(sql) else { return 0 };
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map(|rows| {
            rows.flatten()
                .map(|json| serde_json::from_str::<Vec<String>>(&json).map(|v| v.len()).unwrap_or(0))
                .sum::<usize>()
        });
        rows.unwrap_or(0)
    };
    (count("SELECT after FROM scripts"), count("SELECT script_ids FROM groups"))
}
