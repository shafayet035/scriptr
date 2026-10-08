//! The loopback API an outside agent reaches through the scriptr-mcp bridge.
//! Real HTTP on an ephemeral port, real SQLite — no Tauri window.

use std::sync::{Arc, Mutex};

use scriptr_lib::db::Db;
use scriptr_lib::mcp_api::{self, Hooks};
use scriptr_lib::model::{Project, Task, TaskStatus};

struct Harness {
    db: Arc<Db>,
    base: String,
    token: String,
    notified: Arc<Mutex<Vec<String>>>,
    /// What the app was asked to do, in order.
    acted: Arc<Mutex<Vec<String>>>,
    /// Card orders announced to the UI.
    announced: Arc<Mutex<Vec<String>>>,
}

async fn setup() -> Harness {
    let db = Arc::new(Db::open_in_memory().unwrap());
    db.insert_project(&Project {
        id: "p1".into(),
        name: "acme-platform".into(),
        path: "/Users/dev/code/acme-platform".into(),
        branch: None,
        sort_order: 0,
    })
    .unwrap();

    let notified = Arc::new(Mutex::new(Vec::new()));
    let seen = notified.clone();
    let acted = Arc::new(Mutex::new(Vec::new()));
    let announced = Arc::new(Mutex::new(Vec::new()));

    let mut hooks = Hooks::none();
    hooks.running_scripts = Arc::new(|| 3);
    hooks.on_task = Arc::new(move |t: &Task| seen.lock().unwrap().push(t.title.clone()));
    hooks.start_script = {
        let log = acted.clone();
        Arc::new(move |id: String| {
            log.lock().unwrap().push(format!("start {id}"));
            Ok(())
        })
    };
    hooks.stop_script = {
        let log = acted.clone();
        Arc::new(move |id: String| {
            log.lock().unwrap().push(format!("stop {id}"));
            Box::pin(async {}) as _
        })
    };
    hooks.move_task = {
        let db = db.clone();
        Arc::new(move |id: String, status, before: Option<String>| {
            let db = db.clone();
            Box::pin(async move { scriptr_lib::tasks::move_task(&db, &id, status, before.as_deref()) }) as _
        })
    };
    hooks.on_order = {
        let seen = announced.clone();
        Arc::new(move |project_id: &str, tasks: &[Task]| {
            seen.lock()
                .unwrap()
                .push(format!("{project_id}: {}", tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join(",")))
        })
    };
    hooks.logs = Arc::new(|key: &str, lines: usize| Ok(format!("{key}: {lines} lines requested\nsecond line")));
    let token = "test-token".to_string();
    let router = mcp_api::router(db.clone(), token.clone(), hooks);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Harness { db, base: format!("http://127.0.0.1:{port}"), token, notified, acted, announced }
}

impl Harness {
    async fn get(&self, path: &str, token: Option<&str>) -> (u16, String) {
        self.send(reqwest_like::Method::Get, path, token, None).await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: serde_json::Value) -> (u16, String) {
        self.send(reqwest_like::Method::Post, path, token, Some(body)).await
    }

    async fn patch(&self, path: &str, token: Option<&str>, body: serde_json::Value) -> (u16, String) {
        self.send(reqwest_like::Method::Patch, path, token, Some(body)).await
    }

    fn acted(&self) -> Vec<String> {
        self.acted.lock().unwrap().clone()
    }

    async fn send(
        &self,
        method: reqwest_like::Method,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (u16, String) {
        let url = format!("{}{path}", self.base);
        let token = token.map(str::to_string);
        tokio::task::spawn_blocking(move || reqwest_like::send(method, &url, token.as_deref(), body))
            .await
            .unwrap()
    }
}

/// A 30-line blocking HTTP/1.1 client: the test must not depend on the bridge's
/// own client to prove the server works.
mod reqwest_like {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    pub enum Method {
        Get,
        Post,
        Patch,
    }

    pub fn send(method: Method, url: &str, token: Option<&str>, body: Option<serde_json::Value>) -> (u16, String) {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
        let mut stream = TcpStream::connect(host).unwrap();
        let payload = body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default();
        let verb = match method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
        };
        let mut req = format!("{verb} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        if let Some(t) = token {
            req.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if !payload.is_empty() {
            req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(&payload).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let status = raw.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let body = raw.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        (status, body)
    }
}

#[tokio::test]
async fn an_agent_files_a_task_into_the_backlog() {
    let h = setup().await;

    let (code, body) = h
        .post(
            "/v1/tasks",
            Some(&h.token),
            serde_json::json!({
                "project": "/Users/dev/code/acme-platform/",
                "title": "Rate-limit the checkout endpoint",
                "goal": "10 req/min per account, 429 with Retry-After.",
                "priority": 3
            }),
        )
        .await;
    assert_eq!(code, 200, "{body}");

    let tasks = h.db.project_tasks("p1").unwrap();
    assert_eq!(tasks.len(), 1);
    let t = &tasks[0];
    assert_eq!(t.title, "Rate-limit the checkout endpoint");
    assert_eq!(t.status, TaskStatus::Backlog, "a filed card starts in the backlog");
    assert_eq!(t.priority, 3);
    assert!(t.labels.iter().any(|l| l == "mcp"), "origin must be visible on the card: {:?}", t.labels);
    // The board hears about it without a reload.
    assert_eq!(h.notified.lock().unwrap().as_slice(), ["Rate-limit the checkout endpoint"]);
}

#[tokio::test]
async fn the_token_is_required() {
    let h = setup().await;
    assert_eq!(h.get("/v1/projects", None).await.0, 401);
    assert_eq!(h.get("/v1/projects", Some("wrong")).await.0, 401);
    assert_eq!(h.get("/v1/projects", Some(&h.token)).await.0, 200);

    let (code, _) = h
        .post("/v1/tasks", Some("wrong"), serde_json::json!({"project": "p1", "title": "t", "goal": "g"}))
        .await;
    assert_eq!(code, 401);
    assert!(h.db.project_tasks("p1").unwrap().is_empty(), "an unauthorized call must not write");
}

#[tokio::test]
async fn an_unknown_project_names_the_ones_that_exist() {
    let h = setup().await;
    let (code, body) = h
        .post("/v1/tasks", Some(&h.token), serde_json::json!({"project": "typo", "title": "t", "goal": "g"}))
        .await;
    assert_eq!(code, 404);
    assert!(body.contains("acme-platform"), "the error should list known projects: {body}");
}

#[tokio::test]
async fn an_empty_title_is_refused() {
    let h = setup().await;
    let (code, body) = h
        .post("/v1/tasks", Some(&h.token), serde_json::json!({"project": "p1", "title": "   ", "goal": "g"}))
        .await;
    assert_eq!(code, 400, "{body}");
    assert!(h.db.project_tasks("p1").unwrap().is_empty());
}

#[tokio::test]
async fn status_and_lists_report_the_app() {
    let h = setup().await;
    let (code, body) = h.get("/v1/status", Some(&h.token)).await;
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["projects"], 1);
    assert_eq!(v["runningScripts"], 3);

    h.post("/v1/tasks", Some(&h.token), serde_json::json!({"project": "p1", "title": "one", "goal": "g"}))
        .await;
    let (_, body) = h.get("/v1/tasks?project=acme-platform", Some(&h.token)).await;
    assert!(body.contains("\"title\":\"one\""), "{body}");
}

/// Files a task through the API and returns its id.
async fn file(h: &Harness, title: &str) -> String {
    let (_, body) = h
        .post("/v1/tasks", Some(&h.token), serde_json::json!({"project": "p1", "title": title, "goal": "g"}))
        .await;
    serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_card_can_be_edited_from_outside() {
    let h = setup().await;
    let id = file(&h, "Rate-limit checkout").await;

    let (code, body) = h
        .patch(
            &format!("/v1/tasks/{id}"),
            Some(&h.token),
            serde_json::json!({"status": "todo", "priority": 3, "assignee": "me", "labels": ["urgent"]}),
        )
        .await;
    assert_eq!(code, 200, "{body}");

    let t = h.db.task(&id).unwrap();
    assert_eq!(t.status, TaskStatus::Todo);
    assert_eq!(t.priority, 3);
    assert_eq!(t.assignee.as_deref(), Some("me"));
    assert_eq!(t.labels, vec!["urgent".to_string()]);
    // Untouched fields stay as they were: a patch is not a replace.
    assert_eq!(t.title, "Rate-limit checkout");
    assert_eq!(t.goal, "g");

    // The board hears about it, so a card moves without a reload.
    assert!(h.notified.lock().unwrap().iter().any(|t| t == "Rate-limit checkout"));
}

#[tokio::test]
async fn an_empty_title_cannot_be_patched_in() {
    let h = setup().await;
    let id = file(&h, "Keep me").await;
    let (code, body) = h.patch(&format!("/v1/tasks/{id}"), Some(&h.token), serde_json::json!({"title": "  "})).await;
    assert_eq!(code, 400, "{body}");
    assert_eq!(h.db.task(&id).unwrap().title, "Keep me");

    // A goal, unlike a title, may be cleared: plenty of cards are a title.
    let (code, _) = h.patch(&format!("/v1/tasks/{id}"), Some(&h.token), serde_json::json!({"goal": ""})).await;
    assert_eq!(code, 200);
    assert_eq!(h.db.task(&id).unwrap().goal, "");
}



#[tokio::test]
async fn scripts_are_controlled_by_name_and_unknown_ones_list_the_choices() {
    let h = setup().await;
    h.db.upsert_script(&script("s1", "p1", "web")).unwrap();
    h.db.upsert_script(&script("s2", "p1", "worker")).unwrap();

    let (code, body) = h.post("/v1/scripts/web/start", Some(&h.token), serde_json::json!({})).await;
    assert_eq!(code, 200, "{body}");
    let (code, _) = h.post("/v1/scripts/s2/stop", Some(&h.token), serde_json::json!({})).await;
    assert_eq!(code, 200);
    assert_eq!(h.acted(), vec!["start s1".to_string(), "stop s2".to_string()], "resolved by name, then by id");

    let (code, body) = h.post("/v1/scripts/nope/start", Some(&h.token), serde_json::json!({})).await;
    assert_eq!(code, 404);
    assert!(body.contains("web") && body.contains("worker"), "a 404 should name the options: {body}");

    let (_, body) = h.get("/v1/scripts?project=acme-platform", Some(&h.token)).await;
    assert!(body.contains("\"name\":\"web\"") && body.contains("\"state\":\"idle\""), "{body}");
}

#[tokio::test]
async fn logs_resolve_their_target_and_refuse_nonsense() {
    let h = setup().await;
    h.db.upsert_script(&script("s1", "p1", "web")).unwrap();

    let (code, body) = h.get("/v1/logs?target=script:web&lines=50", Some(&h.token)).await;
    assert_eq!(code, 200, "{body}");
    // The name was resolved to an id before the app was asked.
    assert!(body.contains("script:s1") && body.contains("50 lines requested"), "{body}");

    let (code, _) = h.get("/v1/logs?target=script:ghost", Some(&h.token)).await;
    assert_eq!(code, 404);
    let (code, body) = h.get("/v1/logs?target=garbage", Some(&h.token)).await;
    assert_eq!(code, 400, "{body}");

    let id = file(&h, "has logs").await;
    let (code, body) = h.get(&format!("/v1/logs?target=task:{id}"), Some(&h.token)).await;
    assert_eq!(code, 200);
    assert!(body.contains("200 lines requested"), "the default tail is 200: {body}");
}

#[tokio::test]
async fn every_new_route_demands_the_token() {
    let h = setup().await;
    let id = file(&h, "guarded").await;
    h.db.upsert_script(&script("s1", "p1", "web")).unwrap();

    for (path, patch) in [
        (format!("/v1/tasks/{id}"), false),
        ("/v1/scripts".to_string(), false),
        ("/v1/groups".to_string(), false),
        ("/v1/logs?target=script:web".to_string(), false),
    ] {
        let (code, _) = if patch {
            h.patch(&path, None, serde_json::json!({})).await
        } else {
            h.get(&path, None).await
        };
        assert_eq!(code, 401, "{path} was reachable without a token");
    }
    for path in [format!("/v1/tasks/{id}/move"), "/v1/scripts/web/start".to_string()] {
        let (code, _) = h.post(&path, None, serde_json::json!({})).await;
        assert_eq!(code, 401, "{path} was reachable without a token");
    }
    let (code, _) = h.patch(&format!("/v1/tasks/{id}"), None, serde_json::json!({"priority": 1})).await;
    assert_eq!(code, 401);
}

/// A minimal script row.
fn script(id: &str, project: &str, name: &str) -> scriptr_lib::model::Script {
    scriptr_lib::model::Script {
        id: id.into(),
        project_id: project.into(),
        name: name.into(),
        label: None,
        cmd: format!("run {name}"),
        cwd: "/Users/dev/code/acme-platform".into(),
        shell: None,
        env: Default::default(),
        env_file: None,
        after: vec![],
        ready: scriptr_lib::model::Gate::Instant,
        restart: Default::default(),
        port: None,
        source: None,
        sort_order: 0,
    }
}


#[tokio::test]
async fn moving_a_card_sets_its_column_and_its_place() {
    let h = setup().await;
    let a = file(&h, "A").await;
    let b = file(&h, "B").await;

    let (code, body) = h
        .post(&format!("/v1/tasks/{b}/move"), Some(&h.token), serde_json::json!({"status": "doing", "before": a}))
        .await;
    assert_eq!(code, 200, "{body}");
    let order: Vec<String> = h.db.project_tasks("p1").unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(order, ["B", "A"], "the card moved to the front");
    assert_eq!(h.db.task(&b).unwrap().status, TaskStatus::Doing);

    // The board has to hear about it, or the card moves in the database and the
    // UI keeps showing the old column until something reloads it.
    assert_eq!(
        h.announced.lock().unwrap().as_slice(),
        ["p1: B,A"],
        "a move must announce the project's new order"
    );

    // A column is required; a missing one is a bad request, not a silent default.
    let (code, _) = h.post(&format!("/v1/tasks/{a}/move"), Some(&h.token), serde_json::json!({})).await;
    assert_eq!(code, 400);
    // And placing a card before a stranger is refused rather than guessed at.
    let (code, _) = h
        .post(&format!("/v1/tasks/{a}/move"), Some(&h.token), serde_json::json!({"status": "todo", "before": "ghost"}))
        .await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn a_card_cannot_be_blocked_by_itself() {
    let h = setup().await;
    let a = file(&h, "A").await;
    let b = file(&h, "B").await;

    let (code, _) = h.patch(&format!("/v1/tasks/{b}"), Some(&h.token), serde_json::json!({"after": [a]})).await;
    assert_eq!(code, 200);
    assert_eq!(h.db.task(&b).unwrap().after, vec![a.clone()]);

    let (code, body) = h.patch(&format!("/v1/tasks/{b}"), Some(&h.token), serde_json::json!({"after": [b]})).await;
    assert_eq!(code, 400, "{body}");
    assert!(body.contains("itself"), "{body}");
}
