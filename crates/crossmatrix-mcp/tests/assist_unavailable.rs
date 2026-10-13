//! Always-compiled tests for the assist-unavailable path. No features, no
//! network: the jev-backed kinds must degrade to a success value carrying
//! `status: assist_unavailable` with non-empty links.

use std::path::PathBuf;
use std::sync::Mutex;

use crossmatrix_mcp::assist_dispatch;
use crossmatrix_mcp::repo::RootSource;
use crossmatrix_mcp::session::Session;
use serde_json::{Value, json};

fn session() -> Mutex<Session> {
    Mutex::new(Session::new(PathBuf::from("."), RootSource::Cwd))
}

async fn suggest_unavailable() -> Value {
    let session = session();
    assist_dispatch::query(
        &session,
        None,
        None,
        &json!({"query": {"kind": "assist.suggest", "relationId": "r"}}),
    )
    .await
    .expect("assist.suggest must be dispatched")
    .expect("assist.suggest must be a success value")
}

#[tokio::test]
async fn assist_suggest_without_the_jev_backend_reports_assist_unavailable() {
    assert_eq!(suggest_unavailable().await["status"], "assist_unavailable");
}

#[tokio::test]
async fn assist_suggest_without_the_jev_backend_reports_non_empty_links() {
    assert!(
        suggest_unavailable().await["links"]
            .as_array()
            .is_some_and(|links| !links.is_empty())
    );
}
