//! Behavioural tests for the `crossmatrix.query` read/analysis surface. Each
//! test drives the public `queries::query` entry point over a `Session` and
//! makes exactly one behavioural assertion.

use std::path::Path;

use crossmatrix::Model;
use crossmatrix_mcp::queries;
use crossmatrix_mcp::repo::RootSource;
use crossmatrix_mcp::session::{Rejection, Session};
use serde_json::{Value, json};

fn q(session: &Session, query: Value) -> Result<Value, Rejection> {
    queries::query(session, &json!({ "query": query }))
}

fn load(json_text: &str) -> Model {
    Model::load(json_text).expect("fixture must load + pass integrity")
}

/// The canonical travel-mug example as an ephemeral session.
fn travel_mug() -> Session {
    Session::ephemeral(load(include_str!(
        "../../../examples/travel-mug-3axis.json"
    )))
}

/// A persisted-root session (ephemeral model) so `readFiles` resolves under the
/// project directory. The root is `<project>/.crossmatrix`.
fn session_at(project: &Path, model: Value) -> Session {
    let mut session = Session::new(project.join(".crossmatrix"), RootSource::Env);
    session
        .command(&json!({ "requestId": "import-1", "model": model }))
        .expect("import must succeed");
    session
}

/// A hierarchy fixture: a `feat` dimension implementing a `loc` dimension that
/// carries a `contains` hierarchy (`dir_x` contains `file_a`).
const HIERARCHY: &str = r#"{
  "schemaVersion": "0.2.0",
  "modelId": "hierarchy",
  "scales": [
    {"id": "s", "levels": [{"observation": "on", "value": 1, "order": 0}]},
    {"id": "w", "levels": [{"observation": "high", "value": 9, "order": 0}]}
  ],
  "dimensions": [
    {"id": "loc", "order": 0, "members": [
      {"id": "file_a", "sourceRef": {"uri": "file://src/a.txt", "hash": "sha256:dead"}},
      {"id": "dir_x"}
    ]},
    {"id": "feat", "order": 1, "weightScale": "w", "members": [
      {"id": "feature_a", "weightObservation": "high"}
    ]}
  ],
  "relations": [
    {"id": "rel_contains", "from": "loc", "to": "loc", "relationType": "contains", "scale": "s",
     "cells": [{"from": "dir_x", "to": "file_a", "observations": [{"observation": "on"}]}]},
    {"id": "rel_implements", "from": "feat", "to": "loc", "relationType": "implements", "scale": "s",
     "cells": [{"from": "feature_a", "to": "file_a", "observations": [{"observation": "on"}]}]}
  ]
}"#;

/// A paging fixture: `a` fans out to three members of `d2`.
const PAGING: &str = r#"{
  "schemaVersion": "0.2.0",
  "modelId": "paging",
  "scales": [{"id": "s", "levels": [{"observation": "on", "value": 1, "order": 0}]}],
  "dimensions": [
    {"id": "d1", "order": 0, "members": [{"id": "a"}]},
    {"id": "d2", "order": 1, "members": [{"id": "b"}, {"id": "c"}, {"id": "d"}]}
  ],
  "relations": [
    {"id": "r", "from": "d1", "to": "d2", "relationType": "supports", "scale": "s",
     "cells": [
       {"from": "a", "to": "b", "observations": [{"observation": "on"}]},
       {"from": "a", "to": "c", "observations": [{"observation": "on"}]},
       {"from": "a", "to": "d", "observations": [{"observation": "on"}]}
     ]}
  ]
}"#;

// ── conflicts / report / export ──

#[test]
fn conflicts_returns_the_travel_mug_conflict() {
    let session = travel_mug();
    let result = q(&session, json!({ "kind": "conflicts" })).expect("conflicts must succeed");
    assert!(
        result["findings"].as_array().is_some_and(|a| !a.is_empty()),
        "travel-mug has a tension/conflict finding"
    );
}

#[test]
fn report_with_relation_id_contains_the_relationship_matrix_heading() {
    let session = travel_mug();
    let result = q(
        &session,
        json!({ "kind": "report", "relationId": "rel_need_part" }),
    )
    .expect("report must succeed");
    assert!(
        result["markdown"]
            .as_str()
            .is_some_and(|md| md.contains("## Relationship matrix")),
        "relation report must contain the relationship matrix"
    );
}

#[test]
fn report_without_relation_id_contains_the_aspect_relation_section() {
    let session = travel_mug();
    let result = q(&session, json!({ "kind": "report" })).expect("report must succeed");
    assert!(
        result["markdown"]
            .as_str()
            .is_some_and(|md| md.contains("House of Quality — rel_need_part")),
        "aspect report must embed the rel_need_part section"
    );
}

#[test]
fn export_format_model_has_no_findings_key() {
    let session = travel_mug();
    let result =
        q(&session, json!({ "kind": "export", "format": "model" })).expect("export must succeed");
    assert!(result["model"].get("findings").is_none());
}

#[test]
fn export_unknown_format_is_bad_request_with_links() {
    let session = travel_mug();
    let rejection = q(&session, json!({ "kind": "export", "format": "bogus" }))
        .expect_err("unknown export format must be rejected");
    assert_eq!(
        (rejection.code.as_str(), rejection.links.is_empty()),
        ("bad_request", false)
    );
}

// ── trace / slice ──

#[test]
fn trace_by_uri_resolves_the_member() {
    let session = Session::ephemeral(load(HIERARCHY));
    let result = q(
        &session,
        json!({ "kind": "trace", "member": "file://src/a.txt" }),
    )
    .expect("trace by uri must succeed");
    assert_eq!(result["member"].as_str(), Some("file_a"));
}

#[test]
fn trace_paging_returns_next_cursor_and_the_second_page_continues_without_overlap() {
    let session = Session::ephemeral(load(PAGING));
    let first = q(
        &session,
        json!({ "kind": "trace", "member": "a", "limit": 2 }),
    )
    .expect("first page must succeed");
    let cursor = first["nextCursor"].as_str().map(str::to_string);
    let second = q(
        &session,
        json!({ "kind": "trace", "member": "a", "limit": 2, "cursor": cursor }),
    )
    .expect("second page must succeed");
    let key = |value: &Value| {
        value["hops"]
            .as_array()
            .map(|hops| {
                hops.iter()
                    .map(|hop| {
                        (
                            hop["relation"].as_str().unwrap_or("").to_string(),
                            hop["member"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default()
    };
    assert!(
        first["nextCursor"].is_string() && key(&first).is_disjoint(&key(&second)),
        "trace paging must advertise a cursor and keep pages disjoint"
    );
}

#[test]
fn slice_by_uri_returns_that_members_cells() {
    let session = Session::ephemeral(load(HIERARCHY));
    let result = q(
        &session,
        json!({ "kind": "slice", "relationId": "rel_implements", "to": "file://src/a.txt" }),
    )
    .expect("slice by uri must succeed");
    assert!(
        result["cells"]
            .as_array()
            .is_some_and(|cells| !cells.is_empty() && cells.iter().all(|c| c["to"] == "file_a")),
        "slice by uri must return the uri member's cells"
    );
}

// ── rollup ──

#[test]
fn trace_with_rollup_on_a_directory_returns_the_feature_implemented_by_its_child() {
    let session = Session::ephemeral(load(HIERARCHY));
    let result = q(
        &session,
        json!({ "kind": "trace", "member": "dir_x", "rollup": true }),
    )
    .expect("rollup trace must succeed");
    assert!(
        result["hops"]
            .as_array()
            .is_some_and(|hops| hops.iter().any(|hop| hop["member"] == "feature_a")),
        "rolled-up trace of a directory must reach the feature implementing its child"
    );
}

#[test]
fn marginalize_with_rollup_includes_a_directory() {
    let session = Session::ephemeral(load(HIERARCHY));
    let result = q(
        &session,
        json!({ "kind": "analyze.marginalize", "relationId": "rel_implements", "axis": "from", "rollup": true }),
    )
    .expect("rollup marginalize must succeed");
    assert!(
        result["findings"]
            .as_array()
            .is_some_and(|findings| findings.iter().any(|f| f["member"] == "dir_x")),
        "rolled-up marginalize must rank the contains-directory ancestor"
    );
}

// ── stale ──

#[test]
fn stale_with_one_changed_hash_returns_that_member() {
    let session = travel_mug();
    let result = q(
        &session,
        json!({ "kind": "stale", "hashes": { "FM-101": "changed" } }),
    )
    .expect("stale must succeed");
    assert!(
        result["changed"]
            .as_array()
            .is_some_and(|changed| changed.iter().any(|c| c["member"] == "fail_burn")),
        "a changed sourceRef hash must surface its member"
    );
}

#[test]
fn stale_returns_the_affected_cells_of_a_changed_member() {
    let session = travel_mug();
    let result = q(
        &session,
        json!({ "kind": "stale", "hashes": { "FM-101": "changed" } }),
    )
    .expect("stale must succeed");
    assert!(
        result["affectedCells"].as_array().is_some_and(|cells| cells
            .iter()
            .any(|c| c["from"] == "fail_burn" || c["to"] == "fail_burn")),
        "affected cells must reference the changed member"
    );
}

#[test]
fn stale_paging_respects_limit_and_next_cursor() {
    let session = travel_mug();
    let hashes = json!({ "FM-101": "a", "FM-102": "b", "FM-103": "c" });
    let first = q(
        &session,
        json!({ "kind": "stale", "hashes": hashes, "limit": 2 }),
    )
    .expect("first page must succeed");
    let cursor = first["nextCursor"].as_str().map(str::to_string);
    let second = q(
        &session,
        json!({ "kind": "stale", "hashes": { "FM-101": "a", "FM-102": "b", "FM-103": "c" }, "limit": 2, "cursor": cursor }),
    )
    .expect("second page must succeed");
    assert!(
        first["nextCursor"].is_string() && second["nextCursor"].is_null(),
        "stale paging must advertise a cursor until the last page"
    );
}

#[test]
fn stale_read_files_detects_a_changed_file_under_the_project_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("src")).expect("mkdir src");
    std::fs::write(tmp.path().join("src/a.txt"), "fresh content").expect("write source");
    let session = session_at(
        tmp.path(),
        serde_json::from_str(HIERARCHY).expect("fixture json"),
    );
    let result = q(&session, json!({ "kind": "stale", "readFiles": true }))
        .expect("stale readFiles must succeed");
    assert!(
        result["changed"].as_array().is_some_and(|changed| {
            changed.iter().any(|c| {
                c["member"] == "file_a"
                    && c["current"]
                        .as_str()
                        .is_some_and(|h| h.starts_with("sha256:"))
            })
        }),
        "readFiles must hash an in-project file and report it changed"
    );
}

#[test]
fn stale_read_files_skips_a_path_outside_the_project_directory() {
    let outside = r#"{
      "schemaVersion": "0.2.0",
      "modelId": "outside",
      "dimensions": [
        {"id": "loc", "order": 0, "members": [
          {"id": "escape", "sourceRef": {"uri": "file://../outside.txt", "hash": "sha256:dead"}}
        ]}
      ],
      "relations": []
    }"#;
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = session_at(
        tmp.path(),
        serde_json::from_str(outside).expect("fixture json"),
    );
    let result = q(&session, json!({ "kind": "stale", "readFiles": true }))
        .expect("stale readFiles must succeed");
    assert!(
        result["skipped"]
            .as_array()
            .is_some_and(|skipped| skipped.iter().any(|s| s["reason"] == "outside_project_dir")),
        "a file uri escaping the project directory must be skipped"
    );
}

#[test]
fn legacy_stale_without_params_keeps_returning_findings() {
    let session = travel_mug();
    let result = q(&session, json!({ "kind": "stale" })).expect("legacy stale must succeed");
    assert!(
        result["findings"].as_array().is_some_and(|f| !f.is_empty()),
        "legacy stale must keep returning engine findings"
    );
}

// ── malformed / unrecognized query requests ──

/// Drive the raw `crossmatrix.query` entry point with a whole tool request.
fn raw(session: &Session, request: Value) -> Result<Value, Rejection> {
    queries::query(session, &request)
}

#[test]
fn query_without_kind_is_rejected_as_invalid_params() {
    let session = travel_mug();
    let rejection =
        raw(&session, json!({ "query": {} })).expect_err("missing query.kind must be rejected");
    assert_eq!(rejection.code, "invalid_params");
}

#[test]
fn query_with_op_instead_of_kind_is_rejected_as_invalid_params() {
    let session = travel_mug();
    let rejection = raw(&session, json!({ "op": "validate" }))
        .expect_err("a request without query.kind must be rejected");
    assert_eq!(rejection.code, "invalid_params");
}

#[test]
fn query_without_kind_error_names_the_expected_parameter() {
    let session = travel_mug();
    let rejection =
        raw(&session, json!({ "query": {} })).expect_err("missing query.kind must be rejected");
    assert!(
        rejection.message.contains("query.kind"),
        "the error must name the expected shape, got: {}",
        rejection.message
    );
}

#[test]
fn unknown_query_kind_is_rejected_as_invalid_params() {
    let session = travel_mug();
    let rejection =
        q(&session, json!({ "kind": "bogus" })).expect_err("unknown query kind must be rejected");
    assert_eq!(rejection.code, "invalid_params");
}

#[test]
fn unknown_query_kind_error_lists_a_supported_kind() {
    let session = travel_mug();
    let rejection =
        q(&session, json!({ "kind": "bogus" })).expect_err("unknown query kind must be rejected");
    assert!(
        rejection.message.contains("slice"),
        "the error must list supported kinds, got: {}",
        rejection.message
    );
}

#[test]
fn validate_query_still_succeeds() {
    let session = travel_mug();
    let result = q(&session, json!({ "kind": "validate" })).expect("validate must succeed");
    assert_eq!(result["validated"], json!(true));
}

#[test]
fn command_without_op_or_model_is_rejected_as_invalid_params() {
    let mut session = travel_mug();
    let rejection = session
        .command(&json!({ "request": { "op": "validate" } }))
        .expect_err("a command with neither op nor model must be rejected");
    assert_eq!(rejection.code, "invalid_params");
}
