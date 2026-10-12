//! Performance budget tests (plain debug `cargo test --workspace`): each test
//! builds a large store fixture (10k locations, 200 features, 29k cells), opens
//! it through `Session` + `model.open`, and asserts that one operation alone
//! completes within its budget. Fixture construction is untimed; a process-wide
//! lock keeps the per-test timings stable under parallel test execution.

use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossmatrix_mcp::queries;
use crossmatrix_mcp::repo::RootSource;
use crossmatrix_mcp::schema_types::{
    CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState,
};
use crossmatrix_mcp::session::Session;
use crossmatrix_mcp::store::{ConfigStore, DimensionsStore, StateStore};
use serde_json::{Value, json};

const BUDGET: Duration = Duration::from_secs(1);
const LOCATION_COUNT: usize = 10_000;
const FEATURE_COUNT: usize = 200;

fn perf_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Recover from a poisoned lock so a failed test does not cascade.
fn lock_perf() -> std::sync::MutexGuard<'static, ()> {
    perf_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// The `locations` dimension: 1_000 directories, each containing 9 files.
fn location_members() -> Vec<Value> {
    let mut members = Vec::with_capacity(LOCATION_COUNT);
    for directory in 0..1_000usize {
        members.push(json!({
            "id": format!("dir_{directory}"),
            "sourceRef": {
                "uri": format!("file://src/dir_{directory}/"),
                "hash": format!("sha256:{:064x}", directory),
            },
        }));
        for file in 0..9usize {
            let index = 1_000 + directory * 9 + file;
            members.push(json!({
                "id": format!("file_{directory}_{file}"),
                "sourceRef": {
                    "uri": format!("file://src/file_{directory}_{file}.rs"),
                    "hash": format!("sha256:{:064x}", index),
                },
            }));
        }
    }
    members
}

fn feature_members() -> Vec<Value> {
    (0..FEATURE_COUNT)
        .map(|index| json!({ "id": format!("feature_{index}") }))
        .collect()
}

/// Build the split Configuration/Dimensions/State documents directly.
fn fixture() -> (Value, Value, Value) {
    let locations = location_members();
    let features = feature_members();

    let config = json!({
        "schemaVersion": "0.2.0",
        "configId": "cfg",
        "version": "1.0.0",
        "aspect": "hoq",
        "dimensionSetRef": { "dimensionSetId": "ds", "version": "1.0.0" },
        "scales": [{ "id": "s", "levels": [{ "observation": "on", "value": 1, "order": 0 }] }],
        "relations": [
            { "id": "contains", "from": "locations", "to": "locations", "relationType": "contains", "scale": "s" },
            { "id": "implements", "from": "locations", "to": "features", "relationType": "implements", "scale": "s" }
        ],
        "contractions": [],
    });

    let dimensions = json!({
        "schemaVersion": "0.2.0",
        "dimensionSetId": "ds",
        "version": "1.0.0",
        "dimensions": [
            { "id": "locations", "order": 0, "members": locations },
            { "id": "features", "order": 1, "members": features },
        ],
    });

    let mut cells: Vec<Value> = Vec::with_capacity(29_000);
    for directory in 0..1_000usize {
        for file in 0..9usize {
            cells.push(json!({
                "relationId": "contains",
                "from": format!("dir_{directory}"),
                "to": format!("file_{directory}_{file}"),
                "observations": [{ "observation": "on" }],
            }));
        }
    }
    for directory in 0..1_000usize {
        for offset in 0..2usize {
            let feature = (directory * 2 + offset) % FEATURE_COUNT;
            cells.push(json!({
                "relationId": "implements",
                "from": format!("dir_{directory}"),
                "to": format!("feature_{feature}"),
                "observations": [{ "observation": "on" }],
            }));
        }
        for file in 0..9usize {
            for offset in 0..2usize {
                let feature = ((directory * 9 + file) * 2 + offset) % FEATURE_COUNT;
                cells.push(json!({
                    "relationId": "implements",
                    "from": format!("file_{directory}_{file}"),
                    "to": format!("feature_{feature}"),
                    "observations": [{ "observation": "on" }],
                }));
            }
        }
    }

    let state = json!({
        "schemaVersion": "0.2.0",
        "stateId": "perf",
        "version": "1.0.0",
        "revision": 0,
        "configRef": { "configId": "cfg", "version": "1.0.0" },
        "cells": cells,
    });

    (config, dimensions, state)
}

/// Write the fixture to `root` and open it through `model.open`.
fn open_perf_session(root: &Path) -> Session {
    let (config_value, dims_value, state_value) = fixture();
    let config: CrossMatrixConfiguration =
        serde_json::from_value(config_value).expect("config fixture");
    let dimensions: CrossMatrixDimensions =
        serde_json::from_value(dims_value).expect("dimensions fixture");
    let state: CrossMatrixState = serde_json::from_value(state_value).expect("state fixture");

    ConfigStore::new(root.to_path_buf())
        .put(&config)
        .expect("put config");
    DimensionsStore::new(root.to_path_buf())
        .put(&dimensions)
        .expect("put dimensions");
    let state_store = StateStore::new(root.to_path_buf());
    state_store.put(&state).expect("put state");
    state_store.put_current(&state).expect("put current state");

    let mut session = Session::new(root.to_path_buf(), RootSource::Env);
    session
        .command(&json!({
            "schemaVersion": "0.2.0",
            "requestId": "open-1",
            "modelId": "perf",
            "actor": { "actorType": "llm", "persona": "model_admin" },
            "op": { "kind": "model.open", "stateId": "perf" },
        }))
        .expect("model.open must succeed");
    session
}

#[test]
fn members_sync_of_ten_thousand_locations_stays_under_budget() {
    let _guard = lock_perf();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(".crossmatrix");
    let mut session = open_perf_session(&root);

    let mut roster = location_members();
    for (index, member) in roster.iter_mut().take(100).enumerate() {
        member["sourceRef"]["hash"] = json!(format!("sha256:changed-{index}"));
    }
    let request = json!({
        "schemaVersion": "0.2.0",
        "requestId": "sync-1",
        "modelId": "perf",
        "actor": { "actorType": "llm", "persona": "model_admin" },
        "op": { "kind": "members.sync", "dimensionId": "locations", "members": roster },
    });

    let started = Instant::now();
    let result = session
        .command(&request)
        .expect("members.sync must succeed");
    let elapsed = started.elapsed();
    assert!(
        result["ok"] == json!(true) && elapsed < BUDGET,
        "members.sync of 10k locations took {elapsed:?} (budget {BUDGET:?})"
    );
}

#[test]
fn stale_with_hashes_for_ten_thousand_members_stays_under_budget() {
    let _guard = lock_perf();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(".crossmatrix");
    let session = open_perf_session(&root);

    let members = location_members();
    let mut hashes = serde_json::Map::new();
    for (index, member) in members.iter().enumerate() {
        let uri = member["sourceRef"]["uri"]
            .as_str()
            .expect("uri")
            .to_string();
        let hash = if index % 100 == 0 {
            format!("sha256:changed-{index}")
        } else {
            member["sourceRef"]["hash"]
                .as_str()
                .expect("hash")
                .to_string()
        };
        hashes.insert(uri, json!(hash));
    }
    let request = json!({ "query": { "kind": "stale", "hashes": hashes } });

    let started = Instant::now();
    let result = queries::query(&session, &request).expect("stale must succeed");
    let elapsed = started.elapsed();
    assert!(
        result["changed"].as_array().is_some_and(|c| !c.is_empty()) && elapsed < BUDGET,
        "stale over 10k members took {elapsed:?} (budget {BUDGET:?})"
    );
}

#[test]
fn trace_by_uri_with_rollup_stays_under_budget() {
    let _guard = lock_perf();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(".crossmatrix");
    let session = open_perf_session(&root);

    let request = json!({
        "query": { "kind": "trace", "member": "file://src/file_0_0.rs", "rollup": true },
    });
    let started = Instant::now();
    let result = queries::query(&session, &request).expect("trace must succeed");
    let elapsed = started.elapsed();
    assert!(
        result["member"] == json!("file_0_0") && elapsed < BUDGET,
        "trace by uri with rollup took {elapsed:?} (budget {BUDGET:?})"
    );
}

#[test]
fn trace_of_one_feature_member_stays_under_budget() {
    let _guard = lock_perf();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(".crossmatrix");
    let session = open_perf_session(&root);

    let request = json!({ "query": { "kind": "trace", "member": "feature_0" } });
    let started = Instant::now();
    let result = queries::query(&session, &request).expect("trace must succeed");
    let elapsed = started.elapsed();
    assert!(
        result["hops"].as_array().is_some_and(|h| !h.is_empty()) && elapsed < BUDGET,
        "trace of a feature member took {elapsed:?} (budget {BUDGET:?})"
    );
}
