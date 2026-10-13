//! Scale/performance guards for large hierarchical rosters. Each test measures
//! ONLY the operation and asserts a single time bound. Fixtures are built
//! programmatically by the shared helpers below.
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossmatrix::write::{MemberSpec, sync_members};
use crossmatrix::{CrossMatrixModel, Model};

const DIRS: usize = 1_000;
const FILES_PER_DIR: usize = 9;
/// directories + files
const LOCATIONS: usize = DIRS * (FILES_PER_DIR + 1);
const FEATURES: usize = 200;
/// distinct linked files per feature -> 20_000 `implements` cells
const FILE_LINKS: usize = 100;
const LIMIT: Duration = Duration::from_secs(1);

/// Serialize the measured sections so parallel test threads cannot inflate a
/// single operation's wall time.
static PERF_LOCK: Mutex<()> = Mutex::new(());

/// Roster index -> member id: a directory every 10th slot, its 9 files between.
fn location_id(index: usize) -> String {
    let dir = index / (FILES_PER_DIR + 1);
    let within = index % (FILES_PER_DIR + 1);
    if within == 0 {
        format!("dir_{dir}")
    } else {
        format!("file_{dir}_{}", within - 1)
    }
}

fn source_ref_value(index: usize) -> serde_json::Value {
    serde_json::json!({
        "uri": format!("file://src/f{index}.rs"),
        "hash": format!("sha256:{:064x}", index),
    })
}

fn source_ref(index: usize) -> crossmatrix::SourceRef {
    serde_json::from_value(source_ref_value(index)).expect("valid source ref")
}

/// Build the shared large model document once.
fn build_model_value() -> serde_json::Value {
    let locations: Vec<serde_json::Value> = (0..LOCATIONS)
        .map(|i| serde_json::json!({"id": location_id(i), "sourceRef": source_ref_value(i)}))
        .collect();

    let contains_cells: Vec<serde_json::Value> = (0..DIRS)
        .flat_map(|dir| {
            (0..FILES_PER_DIR).map(move |file| {
                serde_json::json!({
                    "from": format!("dir_{dir}"),
                    "to": format!("file_{dir}_{file}"),
                    "observations": [{"observation": "strong"}],
                })
            })
        })
        .collect();

    let implements_cells: Vec<serde_json::Value> = (0..FEATURES)
        .flat_map(|feature| {
            (0..FILE_LINKS).map(move |link| {
                let dir = link / FILES_PER_DIR;
                let file = link % FILES_PER_DIR;
                serde_json::json!({
                    "from": format!("file_{dir}_{file}"),
                    "to": format!("feat_{feature}"),
                    "observations": [{"observation": "strong"}],
                })
            })
        })
        .collect();

    let features: Vec<serde_json::Value> = (0..FEATURES)
        .map(|feature| serde_json::json!({"id": format!("feat_{feature}")}))
        .collect();

    serde_json::json!({
        "schemaVersion": "0.2.0",
        "modelId": "perf",
        "scales": [{"id": "s", "levels": [
            {"observation": "none", "value": 0, "order": 0},
            {"observation": "strong", "value": 9, "order": 1}
        ]}],
        "dimensions": [
            {"id": "locations", "order": 0, "members": locations},
            {"id": "features", "order": 1, "members": features}
        ],
        "relations": [
            {"id": "contains_loc", "from": "locations", "to": "locations",
             "relationType": "contains", "scale": "s", "cells": contains_cells},
            {"id": "implements", "from": "locations", "to": "features",
             "relationType": "implements", "scale": "s", "cells": implements_cells}
        ]
    })
}

fn model_value() -> &'static serde_json::Value {
    static VALUE: OnceLock<serde_json::Value> = OnceLock::new();
    VALUE.get_or_init(build_model_value)
}

fn raw_model() -> CrossMatrixModel {
    serde_json::from_value(model_value().clone()).expect("valid model document")
}

fn loaded_model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| Model::from_doc(raw_model()).expect("model validates"))
}

fn roster(changed: &HashSet<usize>) -> Vec<MemberSpec> {
    (0..LOCATIONS)
        .map(|i| MemberSpec {
            id: location_id(i),
            label: None,
            weight_observation: None,
            weight: None,
            source_ref: Some(if changed.contains(&i) {
                source_ref(i + LOCATIONS)
            } else {
                source_ref(i)
            }),
            attributes: None,
        })
        .collect()
}

#[test]
fn from_doc_of_a_ten_thousand_member_model_finishes_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let doc = raw_model();
    let start = Instant::now();
    let model = Model::from_doc(doc).expect("validates");
    let elapsed = start.elapsed();
    std::hint::black_box(&model);
    assert!(elapsed < LIMIT, "Model::from_doc took {elapsed:?}");
}

#[test]
fn sync_members_with_hundred_changed_hashes_finishes_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let model = loaded_model();
    let changed: HashSet<usize> = (0..100).collect();
    let roster = roster(&changed);
    let start = Instant::now();
    let (_model, _summary) = sync_members(model, "locations", &roster).expect("sync succeeds");
    let elapsed = start.elapsed();
    assert!(elapsed < LIMIT, "sync_members (changed) took {elapsed:?}");
}

#[test]
fn sync_members_of_an_identical_roster_finishes_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let model = loaded_model();
    let roster = roster(&HashSet::new());
    let start = Instant::now();
    let (_model, summary) = sync_members(model, "locations", &roster).expect("sync succeeds");
    let elapsed = start.elapsed();
    let _ = summary.is_noop();
    assert!(elapsed < LIMIT, "sync_members (identical) took {elapsed:?}");
}

#[test]
fn rolled_up_of_a_large_hierarchy_finishes_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let model = loaded_model();
    let start = Instant::now();
    let rolled = model.rolled_up();
    let elapsed = start.elapsed();
    std::hint::black_box(&rolled);
    assert!(elapsed < LIMIT, "rolled_up took {elapsed:?}");
}

#[test]
fn ancestors_for_one_thousand_leaves_finish_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let leaves: Vec<String> = (0..DIRS).map(|dir| format!("file_{dir}_0")).collect();
    let model = loaded_model();
    let start = Instant::now();
    let result: Vec<Vec<String>> = leaves.iter().map(|leaf| model.ancestors(leaf)).collect();
    let elapsed = start.elapsed();
    std::hint::black_box(&result);
    assert!(elapsed < LIMIT, "ancestors took {elapsed:?}");
}

#[test]
fn trace_of_one_feature_member_finishes_within_one_second() {
    let _guard = PERF_LOCK.lock().expect("perf lock");
    let model = loaded_model();
    let start = Instant::now();
    let hops = model.trace("feat_0");
    let elapsed = start.elapsed();
    std::hint::black_box(&hops);
    assert!(elapsed < LIMIT, "trace took {elapsed:?}");
}
