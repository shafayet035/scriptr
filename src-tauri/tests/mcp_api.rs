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
    let hooks = Hooks {
        running_scripts: Arc::new(|| 3),
        on_task: Arc::new(move |t: &Task| seen.lock().unwrap().push(t.title.clone())),
    };
    let token = "test-token".to_string();
    let router = mcp_api::router(db.clone(), token.clone(), hooks);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Harness { db, base: format!("http://127.0.0.1:{port}"), token, notified }
}

impl Harness {
    async fn get(&self, path: &str, token: Option<&str>) -> (u16, String) {
        self.send(reqwest_like::Method::Get, path, token, None).await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: serde_json::Value) -> (u16, String) {
        self.send(reqwest_like::Method::Post, path, token, Some(body)).await
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
    }

    pub fn send(method: Method, url: &str, token: Option<&str>, body: Option<serde_json::Value>) -> (u16, String) {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
        let mut stream = TcpStream::connect(host).unwrap();
        let payload = body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default();
        let verb = match method {
            Method::Get => "GET",
            Method::Post => "POST",
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
                "effort": "high",
                "priority": 3
            }),
        )
        .await;
    assert_eq!(code, 200, "{body}");

    let tasks = h.db.project_tasks("p1").unwrap();
    assert_eq!(tasks.len(), 1);
    let t = &tasks[0];
    assert_eq!(t.title, "Rate-limit the checkout endpoint");
    // The whole safety story: filed, never started, and never full autonomy.
    assert_eq!(t.status, TaskStatus::Backlog);
    assert_eq!(t.autonomy, scriptr_lib::model::Autonomy::Ask);
    assert_eq!(t.effort, Some(scriptr_lib::model::Effort::High));
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
async fn an_empty_goal_is_refused() {
    let h = setup().await;
    let (code, body) = h
        .post("/v1/tasks", Some(&h.token), serde_json::json!({"project": "p1", "title": "t", "goal": "   "}))
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
