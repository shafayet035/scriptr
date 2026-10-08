//! Board tasks.
//!
//! A task is a card on a Kanban board: a title, free-text goal, a column, a
//! priority, labels, and the tasks it is blocked by. Scriptr stores and shows
//! them; the work of actually doing them happens elsewhere, usually in an AI
//! client driving the board over MCP.
//!
//! `create` is shared by the UI command and the MCP API so a card filed from
//! outside gets exactly the same validation as one typed into the composer.

use crate::db::Db;
use crate::model::{now_ms, Task, TaskStatus};

/// What a caller must supply to file a task.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    /// project id, name or path
    pub project: String,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub status: Option<TaskStatus>,
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub issue_url: Option<String>,
}

/// Creates a task. `origin` records who filed it ("mcp", "mcp-offline") and
/// becomes a label, so the board shows where a card came from.
pub fn create(db: &Db, project_id: &str, input: &NewTask, origin: &str) -> Result<Task, String> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err("a task needs a title".into());
    }
    db.project(project_id)?;

    let mut labels = input.labels.clone();
    if !origin.is_empty() && !labels.iter().any(|l| l == origin) {
        labels.push(origin.to_string());
    }
    let now = now_ms();
    let task = Task {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: project_id.to_string(),
        title: title.to_string(),
        goal: input.goal.trim().to_string(),
        after: vec![],
        status: input.status.unwrap_or(TaskStatus::Backlog),
        priority: input.priority.unwrap_or(1).clamp(0, 3),
        assignee: input.assignee.clone(),
        labels,
        issue_url: input.issue_url.clone(),
        created_at: now,
        updated_at: now,
        sort_order: db.project_tasks(project_id).map(|t| t.len() as i64).unwrap_or(0),
    };
    db.upsert_task(&task)?;
    Ok(task)
}

/// Moves a card to a column and to a position within it, which is what a
/// drag-and-drop ends in. Order is `sort_order` across the project, so a move
/// renumbers the cards that shifted and leaves the rest alone.
///
/// Returns the project's tasks in their new order.
pub fn move_task(db: &Db, task_id: &str, status: TaskStatus, before: Option<&str>) -> Result<Vec<Task>, String> {
    let mut task = db.task(task_id)?;
    if before == Some(task_id) {
        return Err("a card cannot be placed before itself".into());
    }
    let project_id = task.project_id.clone();
    task.status = status;
    task.updated_at = now_ms();
    db.upsert_task(&task)?;

    let mut rest: Vec<Task> = db.project_tasks(&project_id)?.into_iter().filter(|t| t.id != task_id).collect();
    let at = match before {
        Some(id) => rest.iter().position(|t| t.id == id).ok_or_else(|| format!("no task {id} to place this before"))?,
        None => rest.len(),
    };
    rest.insert(at, task);
    for (i, t) in rest.iter_mut().enumerate() {
        if t.sort_order != i as i64 {
            t.sort_order = i as i64;
            db.upsert_task(t)?;
        }
    }
    Ok(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.insert_project(&Project {
            id: "p".into(),
            name: "p".into(),
            path: "/tmp".into(),
            branch: None,
            sort_order: 0,
        })
        .unwrap();
        db
    }

    fn input(title: &str) -> NewTask {
        NewTask {
            project: "p".into(),
            title: title.into(),
            goal: String::new(),
            status: None,
            priority: None,
            labels: vec![],
            assignee: None,
            issue_url: None,
        }
    }

    #[test]
    fn a_card_needs_a_title_but_not_a_goal() {
        let db = seeded();
        // A goal is optional: half the cards on a real board are just a title.
        let t = create(&db, "p", &input("Ship the thing"), "").unwrap();
        assert_eq!(t.goal, "");
        assert_eq!(t.status, TaskStatus::Backlog);
        assert_eq!(t.priority, 1);

        for bad in ["", "   "] {
            assert!(create(&db, "p", &input(bad), "").is_err(), "{bad:?} should be refused");
        }
        assert_eq!(db.project_tasks("p").unwrap().len(), 1);
    }

    #[test]
    fn the_origin_becomes_a_label_once() {
        let db = seeded();
        let t = create(&db, "p", &input("From Claude"), "mcp").unwrap();
        assert_eq!(t.labels, vec!["mcp".to_string()]);

        let mut again = input("Also from Claude");
        again.labels = vec!["mcp".into(), "bug".into()];
        let t = create(&db, "p", &again, "mcp").unwrap();
        assert_eq!(t.labels, vec!["mcp".to_string(), "bug".to_string()], "no duplicate origin label");
    }

    #[test]
    fn priority_is_clamped_and_cards_keep_their_filing_order() {
        let db = seeded();
        let mut hot = input("Urgent");
        hot.priority = Some(99);
        assert_eq!(create(&db, "p", &hot, "").unwrap().priority, 3);
        let mut cold = input("Whenever");
        cold.priority = Some(-4);
        assert_eq!(create(&db, "p", &cold, "").unwrap().priority, 0);

        let order: Vec<i64> = db.project_tasks("p").unwrap().iter().map(|t| t.sort_order).collect();
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn a_card_can_be_filed_straight_into_a_column() {
        let db = seeded();
        let mut ready = input("Already groomed");
        ready.status = Some(TaskStatus::Todo);
        assert_eq!(create(&db, "p", &ready, "mcp").unwrap().status, TaskStatus::Todo);
    }

    #[test]
    fn moving_a_card_sets_its_column_and_renumbers_the_rest() {
        let db = seeded();
        let a = create(&db, "p", &input("A"), "").unwrap();
        let b = create(&db, "p", &input("B"), "").unwrap();
        let c = create(&db, "p", &input("C"), "").unwrap();

        // C to the front of Doing: the column changes and the order follows.
        let order = move_task(&db, &c.id, TaskStatus::Doing, Some(&a.id)).unwrap();
        assert_eq!(order.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["C", "A", "B"]);
        assert_eq!(order.iter().map(|t| t.sort_order).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(db.task(&c.id).unwrap().status, TaskStatus::Doing);
        // …and it survives a reload, rather than living in the UI only.
        let reloaded = db.project_tasks("p").unwrap();
        assert_eq!(reloaded.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["C", "A", "B"]);

        // No `before` means last.
        let order = move_task(&db, &c.id, TaskStatus::Done, None).unwrap();
        assert_eq!(order.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);

        // Nonsense is refused rather than silently dropping the card.
        assert!(move_task(&db, &a.id, TaskStatus::Todo, Some("ghost")).is_err());
        assert!(move_task(&db, &a.id, TaskStatus::Todo, Some(&a.id)).is_err());
        assert_eq!(db.project_tasks("p").unwrap().len(), 3);
        let _ = b;
    }

    #[test]
    fn an_unknown_project_is_refused_before_anything_is_written() {
        let db = seeded();
        assert!(create(&db, "nope", &input("Orphan"), "").is_err());
        assert!(db.project_tasks("p").unwrap().is_empty());
    }
}
