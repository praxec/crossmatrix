#![cfg(feature = "jev")]

use std::collections::BTreeMap;
use std::time::Duration;

use crossmatrix_mcp::assist::UnavailableReason;
use crossmatrix_mcp::assist::jev::{HttpJudge, JevAnswer, JevConfig, JevQuestion, Judge};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config_for(uri: &str, timeout_secs: u64) -> JevConfig {
    let timeout = timeout_secs.to_string();
    JevConfig::from_lookup(&|key| match key {
        "CROSSMATRIX_ASSIST" => Some("jev".to_string()),
        "TYPESAFE_JEV_API_KEY" => Some("sk-test".to_string()),
        "CROSSMATRIX_JEV_BASE_URL" => Some(uri.to_string()),
        "CROSSMATRIX_JEV_TIMEOUT_SECS" => Some(timeout.clone()),
        _ => None,
    })
    .expect("test config must build")
}

fn noul_question() -> JevQuestion {
    JevQuestion::Noul {
        instructions: "is it urgent?".to_string(),
        criteria: None,
    }
}

fn one_noul_question() -> BTreeMap<String, JevQuestion> {
    BTreeMap::from([("q".to_string(), noul_question())])
}

fn noul_questions(count: usize) -> BTreeMap<String, JevQuestion> {
    (0..count)
        .map(|i| (format!("q{i}"), noul_question()))
        .collect()
}

fn noul_reply() -> Value {
    json!({
        "model": "jev-latest",
        "answers": {"q": {"type": "noul", "noul": 0.25}},
        "usage": {"input_tokens": 1, "output_tokens": 0}
    })
}

async fn mount_success(server: &MockServer, body: Value) {
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_status(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

async fn sent_body(server: &MockServer) -> Value {
    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    requests[0].body_json().expect("request body is json")
}

#[tokio::test]
async fn typesafe_choice_reply_decodes_answer() {
    let server = MockServer::start().await;
    mount_success(
        &server,
        json!({
            "model": "jev-latest",
            "answers": {
                "q": {
                    "type": "choice",
                    "choice": "aida",
                    "probabilities": {"aida": 0.6, "pas": 0.4},
                    "confidence": 0.55
                }
            }
        }),
    )
    .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));
    let questions = BTreeMap::from([(
        "q".to_string(),
        JevQuestion::Choice {
            instructions: "family?".to_string(),
            criteria: BTreeMap::from([
                ("aida".to_string(), "d".to_string()),
                ("pas".to_string(), "d".to_string()),
            ]),
        },
    )]);

    let reply = judge
        .ask(json!({"text": "hi"}), questions)
        .await
        .expect("choice reply must decode");

    assert_eq!(
        reply.answers["q"],
        JevAnswer::Choice {
            choice: "aida".to_string(),
            probabilities: BTreeMap::from([("aida".to_string(), 0.6), ("pas".to_string(), 0.4),]),
            confidence: 0.55,
        }
    );
}

#[tokio::test]
async fn request_carries_bearer_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(noul_reply()))
        .mount(&server)
        .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let reply = judge.ask(json!({"text": "hi"}), one_noul_question()).await;

    assert!(reply.is_ok());
}

#[tokio::test]
async fn request_body_carries_model() {
    let server = MockServer::start().await;
    mount_success(&server, noul_reply()).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let _ = judge.ask(json!({"text": "hi"}), one_noul_question()).await;
    let body = sent_body(&server).await;

    assert_eq!(body["model"], "jev-latest");
}

#[tokio::test]
async fn request_body_carries_questions() {
    let server = MockServer::start().await;
    mount_success(&server, noul_reply()).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let _ = judge.ask(json!({"text": "hi"}), one_noul_question()).await;
    let body = sent_body(&server).await;

    assert_eq!(body["questions"]["q"]["type"], "noul");
}

#[tokio::test]
async fn request_body_carries_state() {
    let server = MockServer::start().await;
    mount_success(&server, noul_reply()).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let _ = judge.ask(json!({"text": "hi"}), one_noul_question()).await;
    let body = sent_body(&server).await;

    assert_eq!(body["state"], json!({"text": "hi"}));
}

#[tokio::test]
async fn numeric_usage_cost_decodes_to_cost_usd() {
    let server = MockServer::start().await;
    mount_success(
        &server,
        json!({
            "model": "jev-latest",
            "answers": {"q": {"type": "noul", "noul": 0.25}},
            "usage": {"input_tokens": 3, "output_tokens": 0, "cost": 0.25}
        }),
    )
    .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let reply = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect("reply must decode");

    assert_eq!(reply.usage.cost_usd, Some(0.25));
}

#[tokio::test]
async fn object_usage_cost_decodes_to_cost_usd() {
    let server = MockServer::start().await;
    mount_success(
        &server,
        json!({
            "model": "jev-latest",
            "answers": {"q": {"type": "noul", "noul": 0.25}},
            "usage": {"input_tokens": 3, "output_tokens": 0, "cost": {"total": 0.5}}
        }),
    )
    .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let reply = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect("reply must decode");

    assert_eq!(reply.usage.cost_usd, Some(0.5));
}

#[tokio::test]
async fn http_401_yields_http4xx() {
    let server = MockServer::start().await;
    mount_status(&server, 401).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("401 must fail");

    assert_eq!(err.reason, UnavailableReason::Http4xx);
}

#[tokio::test]
async fn http_401_detail_scrubs_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid key sk-test"))
        .mount(&server)
        .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("401 must fail");

    assert!(!err.detail.contains("sk-test"));
}

#[tokio::test]
async fn http_429_yields_http4xx() {
    let server = MockServer::start().await;
    mount_status(&server, 429).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("429 must fail");

    assert_eq!(err.reason, UnavailableReason::Http4xx);
}

#[tokio::test]
async fn http_503_yields_http5xx() {
    let server = MockServer::start().await;
    mount_status(&server, 503).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("503 must fail");

    assert_eq!(err.reason, UnavailableReason::Http5xx);
}

#[tokio::test]
async fn invalid_json_yields_bad_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{not json"))
        .mount(&server)
        .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("invalid json must fail");

    assert_eq!(err.reason, UnavailableReason::BadResponse);
}

#[tokio::test]
async fn missing_answer_id_yields_bad_response() {
    let server = MockServer::start().await;
    mount_success(&server, noul_reply()).await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));
    let mut questions = one_noul_question();
    questions.insert("q2".to_string(), noul_question());

    let err = judge
        .ask(json!({"text": "hi"}), questions)
        .await
        .expect_err("missing answer id must fail");

    assert_eq!(err.reason, UnavailableReason::BadResponse);
}

#[tokio::test]
async fn delayed_reply_yields_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(noul_reply())
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let judge = HttpJudge::new(config_for(&server.uri(), 1));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("delay must time out");

    assert_eq!(err.reason, UnavailableReason::Timeout);
}

#[tokio::test]
async fn refused_connection_yields_network() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("ephemeral addr").port();
    drop(listener);
    let judge = HttpJudge::new(config_for(&format!("http://127.0.0.1:{port}"), 5));

    let err = judge
        .ask(json!({"text": "hi"}), one_noul_question())
        .await
        .expect_err("refused connection must fail");

    assert_eq!(err.reason, UnavailableReason::Network);
}

#[tokio::test]
async fn fifty_one_questions_yields_bad_config() {
    let server = MockServer::start().await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let err = judge
        .ask(json!({"text": "hi"}), noul_questions(51))
        .await
        .expect_err("too many questions must fail");

    assert_eq!(err.reason, UnavailableReason::BadConfig);
}

#[tokio::test]
async fn fifty_one_questions_sends_no_request() {
    let server = MockServer::start().await;
    let judge = HttpJudge::new(config_for(&server.uri(), 5));

    let _ = judge.ask(json!({"text": "hi"}), noul_questions(51)).await;
    let requests = server.received_requests().await.expect("recording enabled");

    assert!(requests.is_empty());
}
