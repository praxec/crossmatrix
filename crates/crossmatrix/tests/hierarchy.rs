//! Hierarchical-roster behavior: `contains` validation, `ancestors`, `rolled_up`,
//! and `SyncSummary::is_noop`. One atomic assertion per test (TDD discipline).
use crossmatrix::write::{MemberSpec, SyncSummary, sync_members};
use crossmatrix::{Axis, Model, ValidationError};

fn cell(from: &str, to: &str, token: &str) -> String {
    format!(r#"{{"from":"{from}","to":"{to}","observations":[{{"observation":"{token}"}}]}}"#)
}

/// locations × features with a `contains` self-relation on locations and an
/// `implements` relation locations→features.
fn fixture(locations: &str, contains_cells: &str, implements_cells: &str) -> Model {
    let json = format!(
        r#"{{
          "schemaVersion":"0.2.0","modelId":"hier",
          "dimensions":[
            {{"id":"locations","order":0,"members":[{locations}]}},
            {{"id":"features","order":1,"weightScale":"importance","members":[
              {{"id":"feat_x","weightObservation":"high"}}]}}
          ],
          "scales":[
            {{"id":"s","levels":[
              {{"observation":"none","value":0,"order":0}},
              {{"observation":"link","value":0,"order":0}},
              {{"observation":"weak","value":1,"order":1}},
              {{"observation":"moderate","value":5,"order":2}},
              {{"observation":"strong","value":9,"order":3}}]}},
            {{"id":"importance","levels":[
              {{"observation":"low","value":1,"order":0}},
              {{"observation":"high","value":9,"order":1}}]}}
          ],
          "relations":[
            {{"id":"contains_loc","from":"locations","to":"locations","relationType":"contains",
              "scale":"s","cells":[{contains_cells}]}},
            {{"id":"implements","from":"locations","to":"features","relationType":"implements",
              "scale":"s","cells":[{implements_cells}]}}
          ]}}"#
    );
    Model::load(&json).expect("fixture must load")
}

// ── `contains` validation ──

#[test]
fn contains_with_from_not_equal_to_is_invalid_hierarchy() {
    let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
      "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                    {"id":"d2","order":1,"members":[{"id":"b"}]}],
      "relations":[{"id":"h","from":"d1","to":"d2","relationType":"contains","scale":"s","cells":[]}]}"#;
    assert!(matches!(
        Model::load(json),
        Err(ValidationError::InvalidHierarchy { .. })
    ));
}

#[test]
fn a_three_cycle_is_cyclic_hierarchy_naming_the_cycle() {
    let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
      "dimensions":[{"id":"d","order":0,"members":[{"id":"a"},{"id":"b"},{"id":"c"}]}],
      "scales":[{"id":"s","levels":[{"observation":"link","value":1,"order":0}]}],
      "relations":[{"id":"h","from":"d","to":"d","relationType":"contains","scale":"s",
        "cells":[
          {"from":"a","to":"b","observations":[{"observation":"link"}]},
          {"from":"b","to":"c","observations":[{"observation":"link"}]},
          {"from":"c","to":"a","observations":[{"observation":"link"}]}]}]}"#;
    assert!(matches!(
        Model::load(json),
        Err(ValidationError::CyclicHierarchy { members, .. })
            if members.iter().map(String::as_str).collect::<Vec<_>>() == vec!["a", "b", "c"]
    ));
}

#[test]
fn a_ten_thousand_deep_chain_loads_without_stack_overflow() {
    let depth = 10_000usize;
    let members: Vec<String> = (0..=depth).map(|i| format!(r#"{{"id":"m{i}"}}"#)).collect();
    let cells: Vec<String> = (0..depth)
        .map(|i| {
            format!(
                r#"{{"from":"m{i}","to":"m{}","observations":[{{"observation":"link"}}]}}"#,
                i + 1
            )
        })
        .collect();
    let json = format!(
        r#"{{"schemaVersion":"0.2.0","modelId":"deep",
          "dimensions":[{{"id":"d","order":0,"members":[{}]}}],
          "scales":[{{"id":"s","levels":[{{"observation":"link","value":1,"order":0}}]}}],
          "relations":[{{"id":"h","from":"d","to":"d","relationType":"contains","scale":"s",
            "cells":[{}]}}]}}"#,
        members.join(","),
        cells.join(",")
    );
    assert!(Model::load(&json).is_ok());
}

// ── ancestors ──

#[test]
fn ancestors_of_a_leaf_in_a_three_level_tree_are_parent_then_grandparent() {
    let model = fixture(
        r#"{"id":"grandparent"},{"id":"parent"},{"id":"leaf"}"#,
        &format!(
            "{},{}",
            cell("grandparent", "parent", "link"),
            cell("parent", "leaf", "link")
        ),
        "",
    );
    assert_eq!(
        model
            .ancestors("leaf")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["parent", "grandparent"]
    );
}

// ── rolled_up ──

#[test]
fn rolled_up_gives_a_directory_a_derived_cell_when_one_child_has_a_cell() {
    let rolled = fixture(
        r#"{"id":"dir_d"},{"id":"file_f"}"#,
        &cell("dir_d", "file_f", "link"),
        &cell("file_f", "feat_x", "strong"),
    )
    .rolled_up();
    assert_eq!(
        rolled
            .slice("implements", Some("dir_d"), Some("feat_x"))
            .map(|cells| cells.len()),
        Some(1)
    );
}

#[test]
fn the_derived_token_is_the_strongest_child_token() {
    let rolled = fixture(
        r#"{"id":"dir_d"},{"id":"file_lo"},{"id":"file_hi"}"#,
        &format!(
            "{},{}",
            cell("dir_d", "file_lo", "link"),
            cell("dir_d", "file_hi", "link")
        ),
        &format!(
            "{},{}",
            cell("file_lo", "feat_x", "weak"),
            cell("file_hi", "feat_x", "strong")
        ),
    )
    .rolled_up();
    let cells = rolled
        .slice("implements", Some("dir_d"), Some("feat_x"))
        .expect("relation exists");
    assert_eq!(cells[0].observations[0].observation, "strong");
}

#[test]
fn a_real_ancestor_cell_is_left_untouched() {
    let rolled = fixture(
        r#"{"id":"dir_d"},{"id":"file_f"}"#,
        &cell("dir_d", "file_f", "link"),
        &format!(
            "{},{}",
            cell("dir_d", "feat_x", "moderate"),
            cell("file_f", "feat_x", "strong")
        ),
    )
    .rolled_up();
    assert_eq!(
        rolled
            .slice("implements", Some("dir_d"), Some("feat_x"))
            .map(|cells| cells.len()),
        Some(1)
    );
}

#[test]
fn a_deprecated_child_contributes_nothing() {
    let rolled = fixture(
        r#"{"id":"dir_d"},{"id":"file_f","status":"deprecated"}"#,
        &cell("dir_d", "file_f", "link"),
        &cell("file_f", "feat_x", "strong"),
    )
    .rolled_up();
    assert_eq!(
        rolled
            .slice("implements", Some("dir_d"), Some("feat_x"))
            .map(|cells| cells.len()),
        Some(0)
    );
}

#[test]
fn rolled_up_without_contains_equals_the_original_model() {
    let json = r#"{"schemaVersion":"0.2.0","modelId":"flat",
      "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                    {"id":"d2","order":1,"members":[{"id":"x"}]}],
      "scales":[{"id":"s","levels":[{"observation":"strong","value":9,"order":0}]}],
      "relations":[{"id":"r","from":"d1","to":"d2","relationType":"supports","scale":"s",
        "cells":[{"from":"a","to":"x","observations":[{"observation":"strong"}]}]}]}"#;
    let model = Model::load(json).expect("flat model loads");
    assert_eq!(
        serde_json::to_value(model.rolled_up().doc()).expect("serialize rolled"),
        serde_json::to_value(model.doc()).expect("serialize original")
    );
}

#[test]
fn marginalize_on_the_rolled_model_ranks_a_directory_at_least_as_high_as_its_child() {
    let rolled = fixture(
        r#"{"id":"dir_d"},{"id":"file_lo"},{"id":"file_hi"}"#,
        &format!(
            "{},{}",
            cell("dir_d", "file_lo", "link"),
            cell("dir_d", "file_hi", "link")
        ),
        &format!(
            "{},{}",
            cell("file_lo", "feat_x", "weak"),
            cell("file_hi", "feat_x", "strong")
        ),
    )
    .rolled_up();
    let ranked = rolled
        .marginalize("implements", Axis::To)
        .expect("marginalize succeeds");
    let score = |id: &str| {
        ranked
            .iter()
            .find(|(member, _)| member == id)
            .map(|(_, value)| *value)
            .expect("member ranked")
    };
    assert!(score("dir_d") >= score("file_hi"));
}

// ── SyncSummary::is_noop ──

fn member(id: &str, label: &str) -> MemberSpec {
    MemberSpec {
        id: id.to_string(),
        label: Some(label.to_string()),
        weight_observation: None,
        weight: None,
        source_ref: None,
        attributes: None,
    }
}

fn one_member_model() -> Model {
    let json = r#"{"schemaVersion":"0.2.0","modelId":"sync",
      "dimensions":[{"id":"d1","order":0,"members":[{"id":"a","label":"A"}]}],
      "relations":[]}"#;
    Model::load(json).expect("sync fixture loads")
}

fn resync(label: &str) -> SyncSummary {
    let (_, summary) =
        sync_members(&one_member_model(), "d1", &[member("a", label)]).expect("sync succeeds");
    summary
}

#[test]
fn is_noop_is_true_for_an_identical_re_sync() {
    assert!(resync("A").is_noop());
}

#[test]
fn is_noop_is_false_when_one_label_changes() {
    assert!(!resync("B").is_noop());
}
