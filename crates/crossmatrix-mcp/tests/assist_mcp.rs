//! Behavioral tests for the AI-assist query dispatch (X07/X08). Each test makes
//! exactly one behavioral assertion and drives the shared public entry point
//! `crossmatrix_mcp::assist_dispatch::query` with in-process fakes (no network).

#![cfg(all(feature = "jev", feature = "test-fakes"))]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crossmatrix::Model;
use crossmatrix_mcp::assist::UnavailableReason;
use crossmatrix_mcp::assist::jev::fake::FakeJudge;
use crossmatrix_mcp::assist::jev::{JevAnswer, JevQuestion};
use crossmatrix_mcp::assist_dispatch;
use crossmatrix_mcp::assist_rig::{Completion, RigUnavailable, RigUsage, TextModel};
use crossmatrix_mcp::queries;
use crossmatrix_mcp::session::Session;
use serde_json::{Value, json};

/// A `TextModel` that returns one canned reply.
struct FakeTextModel {
    reply: String,
}

impl FakeTextModel {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
        }
    }
}

impl TextModel for FakeTextModel {
    fn complete(
        &self,
        _system: &str,
        _user: &str,
        _max_tokens: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Completion, RigUnavailable>> + Send + '_>> {
        let text = self.reply.clone();
        Box::pin(async move {
            Ok(Completion {
                text,
                usage: RigUsage::default(),
                model: "fake/rig".to_string(),
            })
        })
    }
}

const ASSIST_MODEL: &str = r#"{
  "schemaVersion": "0.2.0",
  "modelId": "assist-test",
  "scales": [
    {"id": "qfd", "levels": [
      {"observation": "none", "description": "No relationship.", "value": 0, "order": 0},
      {"observation": "weak", "description": "Minor/indirect effect.", "value": 1, "order": 1},
      {"observation": "strong", "description": "Primary mechanism.", "value": 9, "order": 3}
    ]},
    {"id": "importance", "levels": [
      {"observation": "high", "description": "Critical.", "value": 9, "order": 0}
    ]}
  ],
  "dimensions": [
    {"id": "d1", "order": 0, "weightScale": "importance", "members": [
      {"id": "a", "label": "Alpha", "weightObservation": "high"},
      {"id": "b", "label": "Beta", "weightObservation": "high"}
    ]},
    {"id": "d2", "order": 1, "members": [
      {"id": "x", "label": "Xray"},
      {"id": "y", "label": "Yankee"}
    ]}
  ],
  "relations": [
    {"id": "r", "from": "d1", "to": "d2", "relationType": "supports", "scale": "qfd",
     "cells": [
       {"from": "a", "to": "x", "observations": [{"observation": "weak", "provenanceClass": "normative"}]}
     ]}
  ]
}"#;

/// A model with a relation that has NO cells, so `gaps.next` yields four
/// candidate cells in a deterministic order.
const GAPS_MODEL: &str = r#"{
  "schemaVersion": "0.2.0",
  "modelId": "assist-gaps",
  "scales": [
    {"id": "qfd", "levels": [
      {"observation": "weak", "value": 1, "order": 0},
      {"observation": "strong", "value": 9, "order": 1}
    ]},
    {"id": "importance", "levels": [
      {"observation": "high", "value": 9, "order": 0}
    ]}
  ],
  "dimensions": [
    {"id": "d1", "order": 0, "weightScale": "importance", "members": [
      {"id": "a", "weightObservation": "high"},
      {"id": "b", "weightObservation": "high"}
    ]},
    {"id": "d2", "order": 1, "members": [{"id": "x"}, {"id": "y"}]}
  ],
  "relations": [
    {"id": "r", "from": "d1", "to": "d2", "relationType": "supports", "scale": "qfd", "cells": []}
  ]
}"#;

fn session(fixture: &str) -> Mutex<Session> {
    Mutex::new(Session::ephemeral(
        Model::load(fixture).expect("fixture must load"),
    ))
}

fn strong_judge() -> FakeJudge {
    FakeJudge::answering(|_id, _question| JevAnswer::Choice {
        choice: "strong".to_string(),
        probabilities: BTreeMap::from([(String::from("strong"), 0.9)]),
        confidence: 0.9,
    })
}

async fn dispatch(
    session: &Mutex<Session>,
    judge: Option<&dyn crossmatrix_mcp::assist::jev::Judge>,
    text_model: Option<&dyn TextModel>,
    query: Value,
) -> Value {
    let request = json!({ "query": query });
    assist_dispatch::query(session, judge, text_model, &request)
        .await
        .expect("assist kind must be dispatched")
        .expect("dispatch must return a value")
}

fn recorded_version(session: &Mutex<Session>) -> Option<u64> {
    session.lock().expect("session mutex").status()["version"].as_u64()
}

// ── assist.suggest ──

#[tokio::test]
async fn suggest_returns_one_proposal_per_candidate_cell() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let result = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r",
               "cells": [{"from": "a", "to": "x"}, {"from": "b", "to": "y"}]}),
    )
    .await;
    assert_eq!(result["proposals"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn suggest_proposals_carry_the_judge_choice() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let result = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r",
               "cells": [{"from": "a", "to": "x"}, {"from": "b", "to": "y"}]}),
    )
    .await;
    assert!(
        result["proposals"]
            .as_array()
            .expect("proposals array")
            .iter()
            .all(|proposal| proposal["token"] == "strong")
    );
}

#[tokio::test]
async fn suggest_sends_a_state_containing_both_member_labels() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let _ = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r",
               "cells": [{"from": "b", "to": "y"}]}),
    )
    .await;
    let states = judge.states();
    assert_eq!(
        (
            states[0]["from"]["label"].as_str(),
            states[0]["to"]["label"].as_str(),
        ),
        (Some("Beta"), Some("Yankee"))
    );
}

#[tokio::test]
async fn suggest_caps_proposals_at_max_cells() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let result = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r", "maxCells": 2,
               "cells": [{"from": "b", "to": "x"}, {"from": "b", "to": "y"}, {"from": "a", "to": "y"}]}),
    )
    .await;
    assert_eq!(result["proposals"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn suggest_record_true_stores_proposed_observations_listed_by_proposals_query() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let _ = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r", "record": true,
               "cells": [{"from": "b", "to": "y"}]}),
    )
    .await;
    let proposals = {
        let guard = session.lock().expect("session mutex");
        queries::query(
            &guard,
            &json!({"query": {"kind": "proposals", "relationId": "r"}}),
        )
        .expect("proposals query")
    };
    assert!(
        proposals["proposals"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "recorded proposed observations must be listed by the proposals query"
    );
}

#[tokio::test]
async fn recorded_proposals_do_not_change_analyze_marginalize() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let _ = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r", "record": true,
               "cells": [{"from": "b", "to": "y"}]}),
    )
    .await;
    let result = {
        let guard = session.lock().expect("session mutex");
        queries::query(
            &guard,
            &json!({"query": {"kind": "analyze.marginalize", "relationId": "r", "axis": "from"}}),
        )
        .expect("marginalize query")
    };
    assert!(
        !result["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|finding| finding["member"] == "y"),
        "a proposed observation must not enter the weighted analysis"
    );
}

#[tokio::test]
async fn ordinary_observe_confirms_a_proposal_and_changes_marginalize() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let _ = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r", "record": true,
               "cells": [{"from": "b", "to": "y"}]}),
    )
    .await;
    {
        let mut guard = session.lock().expect("session mutex");
        guard
            .command(&json!({
                "schemaVersion": "0.2.0",
                "requestId": "confirm-b-y",
                "modelId": "assist-test",
                "actor": {"actorType": "llm", "persona": "observer"},
                "op": {"kind": "observe", "relationId": "r", "from": "b", "to": "y",
                       "observation": "strong", "provenanceClass": "normative"}
            }))
            .expect("confirming observe must succeed");
    }
    let result = {
        let guard = session.lock().expect("session mutex");
        queries::query(
            &guard,
            &json!({"query": {"kind": "analyze.marginalize", "relationId": "r", "axis": "from"}}),
        )
        .expect("marginalize query")
    };
    assert!(
        result["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|finding| finding["member"] == "y"),
        "a confirmed observation must enter the weighted analysis"
    );
}

#[tokio::test]
async fn recording_twice_leaves_the_version_unchanged() {
    let session = session(ASSIST_MODEL);
    let judge = strong_judge();
    let request = json!({"kind": "assist.suggest", "relationId": "r", "record": true,
                         "cells": [{"from": "b", "to": "y"}]});
    let _ = dispatch(&session, Some(&judge), None, request.clone()).await;
    let before = recorded_version(&session);
    let _ = dispatch(&session, Some(&judge), None, request).await;
    assert_eq!(recorded_version(&session), before);
}

#[tokio::test]
async fn a_failing_judge_returns_partial_with_the_reason() {
    let session = session(ASSIST_MODEL);
    let judge = FakeJudge::failing(UnavailableReason::Timeout);
    let result = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "assist.suggest", "relationId": "r",
               "cells": [{"from": "b", "to": "y"}]}),
    )
    .await;
    assert_eq!(result["partial"]["reason"], "timeout");
}

#[tokio::test]
async fn suggest_without_a_judge_returns_assist_unavailable_status() {
    let session = session(ASSIST_MODEL);
    let result = dispatch(
        &session,
        None,
        None,
        json!({"kind": "assist.suggest", "relationId": "r"}),
    )
    .await;
    assert_eq!(result["status"], "assist_unavailable");
}

#[tokio::test]
async fn suggest_without_a_judge_returns_non_empty_links() {
    let session = session(ASSIST_MODEL);
    let result = dispatch(
        &session,
        None,
        None,
        json!({"kind": "assist.suggest", "relationId": "r"}),
    )
    .await;
    assert!(
        result["links"]
            .as_array()
            .is_some_and(|links| !links.is_empty()),
        "assist_unavailable must advertise legal next ops"
    );
}

// ── gaps.next rankBy=uncertainty ──

#[tokio::test]
async fn gaps_uncertainty_orders_by_one_minus_confidence() {
    let session = session(GAPS_MODEL);
    let counter = Arc::new(AtomicUsize::new(0));
    let judge = FakeJudge::answering(move |_id, _question: &JevQuestion| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        let confidence = match index {
            0 => 0.9,
            1 => 0.2,
            _ => 0.5,
        };
        JevAnswer::Choice {
            choice: "strong".to_string(),
            probabilities: BTreeMap::from([(String::from("strong"), confidence)]),
            confidence,
        }
    });
    let result = dispatch(
        &session,
        Some(&judge),
        None,
        json!({"kind": "gaps.next", "rankBy": "uncertainty", "relationId": "r"}),
    )
    .await;
    assert_eq!(result["gaps"][0]["cell"], json!({"from": "a", "to": "y"}));
}

// ── assist.members / assist.narrative / report narrative ──

#[tokio::test]
async fn members_never_adds_members_to_the_dimension() {
    let session = session(ASSIST_MODEL);
    let text_model =
        FakeTextModel::new(r#"[{"id":"new_member","label":"New","excerpt":"New member text."}]"#);
    let _ = dispatch(
        &session,
        None,
        Some(&text_model),
        json!({"kind": "assist.members", "dimensionId": "d1", "sourceText": "New member text."}),
    )
    .await;
    let describe = {
        let guard = session.lock().expect("session mutex");
        queries::query(&guard, &json!({"query": {"kind": "describe"}})).expect("describe query")
    };
    let count = describe["describe"]["dimensions"]
        .as_array()
        .expect("dimensions array")
        .iter()
        .find(|dimension| dimension["id"] == "d1")
        .and_then(|dimension| dimension["members"].as_u64());
    assert_eq!(count, Some(2));
}

#[tokio::test]
async fn narrative_returns_the_fake_models_text() {
    let session = session(ASSIST_MODEL);
    let text_model = FakeTextModel::new("The matrix is stable.");
    let result = dispatch(
        &session,
        None,
        Some(&text_model),
        json!({"kind": "assist.narrative"}),
    )
    .await;
    assert_eq!(result["narrative"], "The matrix is stable.");
}

#[tokio::test]
async fn report_with_narrative_appends_a_narrative_section() {
    let session = session(ASSIST_MODEL);
    let text_model = FakeTextModel::new("Narrative body.");
    let result = dispatch(
        &session,
        None,
        Some(&text_model),
        json!({"kind": "report", "narrative": true}),
    )
    .await;
    assert!(
        result["markdown"]
            .as_str()
            .is_some_and(|markdown| markdown.contains("## Narrative")),
        "report narrative:true must append a Narrative section"
    );
}
