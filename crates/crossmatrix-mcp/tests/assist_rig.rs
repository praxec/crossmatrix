//! Behavioral tests for the always-compiled, model-agnostic `assist_rig` logic.
//!
//! Each test makes exactly one behavioral assertion and drives the public
//! `TextModel` seam with a canned in-process fake (no network, no key).

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use crossmatrix_mcp::assist_rig::{
    Completion, MembersRequest, NarrativeRequest, RigUnavailable, RigUsage, TextModel, narrate,
    propose_members,
};
use serde_json::{Value, json};

/// A `TextModel` that returns a canned reply and records the user prompt.
struct FakeTextModel {
    reply: String,
    usage: RigUsage,
    last_user: Mutex<String>,
}

impl FakeTextModel {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
            usage: RigUsage::default(),
            last_user: Mutex::new(String::new()),
        }
    }

    fn with_usage(reply: impl Into<String>, usage: RigUsage) -> Self {
        Self {
            reply: reply.into(),
            usage,
            last_user: Mutex::new(String::new()),
        }
    }
}

impl TextModel for FakeTextModel {
    fn complete(
        &self,
        _system: &str,
        user: &str,
        _max_tokens: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Completion, RigUnavailable>> + Send + '_>> {
        *self.last_user.lock().expect("lock user prompt") = user.to_string();
        let text = self.reply.clone();
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(Completion {
                text,
                usage,
                model: "fake-model".to_string(),
            })
        })
    }
}

fn members_request(source: &str) -> MembersRequest {
    MembersRequest {
        dimension_id: "d1".to_string(),
        dimension_label: None,
        existing_member_ids: Vec::new(),
        source_text: source.to_string(),
        source_uri: None,
        max_members: 20,
        max_tokens: 1024,
    }
}

fn narrative_request(findings: Vec<Value>) -> NarrativeRequest {
    NarrativeRequest {
        title: "Title".to_string(),
        findings: Value::Array(findings),
        priorities: None,
        max_words: 200,
        max_tokens: 512,
    }
}

#[tokio::test]
async fn propose_members_keeps_proposal_whose_excerpt_occurs_verbatim() {
    let fake =
        FakeTextModel::new(r#"[{"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."}]"#);
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.proposals[0].id, "alpha");
}

#[tokio::test]
async fn propose_members_computes_locator_span_from_found_position() {
    let fake =
        FakeTextModel::new(r#"[{"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."}]"#);
    let result = propose_members(&fake, &members_request("Intro text. Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.proposals[0].source_ref["locator"], "chars 12-31");
}

#[tokio::test]
async fn propose_members_drops_hallucinated_excerpt_with_reason_excerpt_not_found() {
    let fake = FakeTextModel::new(
        r#"[{"id":"alpha","label":"Alpha","excerpt":"Not present in the source."}]"#,
    );
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.rejected[0]["reason"], "excerpt_not_found");
}

#[tokio::test]
async fn propose_members_drops_bad_id_with_reason_bad_id() {
    let fake =
        FakeTextModel::new(r#"[{"id":"Bad-Id","label":"Bad","excerpt":"Alpha reduces cost."}]"#);
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.rejected[0]["reason"], "bad_id");
}

#[tokio::test]
async fn propose_members_drops_existing_id_with_reason_duplicate() {
    let fake =
        FakeTextModel::new(r#"[{"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."}]"#);
    let mut request = members_request("Alpha reduces cost.");
    request.existing_member_ids = vec!["alpha".to_string()];
    let result = propose_members(&fake, &request)
        .await
        .expect("propose must succeed");
    assert_eq!(result.rejected[0]["reason"], "duplicate");
}

#[tokio::test]
async fn propose_members_caps_at_max_members() {
    let fake = FakeTextModel::new(
        r#"[
            {"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."},
            {"id":"beta","label":"Beta","excerpt":"Beta improves speed."},
            {"id":"gamma","label":"Gamma","excerpt":"Gamma lowers risk."}
        ]"#,
    );
    let mut request =
        members_request("Alpha reduces cost. Beta improves speed. Gamma lowers risk.");
    request.max_members = 2;
    let result = propose_members(&fake, &request)
        .await
        .expect("propose must succeed");
    assert_eq!(result.proposals.len(), 2);
}

#[tokio::test]
async fn propose_members_tolerates_json_fenced_reply() {
    let fake = FakeTextModel::new(
        "```json\n[{\"id\":\"alpha\",\"label\":\"Alpha\",\"excerpt\":\"Alpha reduces cost.\"}]\n```",
    );
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.proposals.len(), 1);
}

#[tokio::test]
async fn propose_members_returns_bad_response_on_prose_without_json() {
    let fake = FakeTextModel::new("I cannot help with that.");
    let error = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect_err("prose must fail");
    assert_eq!(error.reason, "bad_response");
}

#[tokio::test]
async fn propose_members_attaches_member_propose_op_payload() {
    let fake =
        FakeTextModel::new(r#"[{"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."}]"#);
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(result.proposals[0].propose_op["kind"], "member.propose");
}

#[tokio::test]
async fn propose_members_reports_usage_from_model() {
    let fake = FakeTextModel::with_usage(
        r#"[{"id":"alpha","label":"Alpha","excerpt":"Alpha reduces cost."}]"#,
        RigUsage {
            input_tokens: 11,
            output_tokens: 22,
            cost_usd: Some(0.5),
        },
    );
    let result = propose_members(&fake, &members_request("Alpha reduces cost."))
        .await
        .expect("propose must succeed");
    assert_eq!(
        result.usage,
        RigUsage {
            input_tokens: 11,
            output_tokens: 22,
            cost_usd: Some(0.5),
        }
    );
}

#[tokio::test]
async fn narrate_truncates_to_max_words() {
    let fake = FakeTextModel::new(
        (0..60)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" "),
    );
    let mut request = narrative_request(Vec::new());
    request.max_words = 50;
    let result = narrate(&fake, &request)
        .await
        .expect("narrate must succeed");
    assert_eq!(result.narrative.split_whitespace().count(), 50);
}

#[tokio::test]
async fn narrate_sends_at_most_thirty_findings_to_model() {
    let findings = (0..40)
        .map(|i| {
            json!({
                "kind": "tension",
                "severity": "warn",
                "members": [format!("m{i}")],
                "explanation": "x",
            })
        })
        .collect();
    let fake = FakeTextModel::new("narrative");
    let mut request = narrative_request(findings);
    request.max_words = 50;
    narrate(&fake, &request)
        .await
        .expect("narrate must succeed");
    let sent = fake.last_user.lock().expect("lock user prompt").clone();
    assert_eq!(sent.matches("\"kind\"").count(), 30);
}
