//! Behavioural tests for the pure `report` query/report module (ADR-0003
//! deferred reads). Each test makes exactly one behavioural assertion and
//! exercises only the public interface.

use std::collections::BTreeMap;

use crossmatrix::Model;
use crossmatrix_mcp::report;

/// Arrange: the canonical travel-mug example, loaded through the public API.
fn example() -> Model {
    let json = include_str!("../../../examples/travel-mug-3axis.json");
    Model::load(json).expect("travel-mug example must load + pass integrity")
}

// ── conflicts ──

#[test]
fn conflicts_without_filter_returns_findings() {
    assert!(!report::conflicts(&example(), None).is_empty());
}

#[test]
fn conflicts_filter_returns_findings_for_the_named_relation() {
    assert!(!report::conflicts(&example(), Some("rel_part_internal_conflict")).is_empty());
}

#[test]
fn conflicts_filter_keeps_only_findings_with_a_matching_contributor() {
    let findings = report::conflicts(&example(), Some("rel_part_internal_conflict"));
    assert!(
        findings.iter().all(|f| f
            .contributors
            .iter()
            .any(|c| c.starts_with("rel_part_internal_conflict:"))),
        "every returned finding must cite a contributor from the requested relation"
    );
}

#[test]
fn conflicts_filter_for_an_unknown_relation_is_empty() {
    assert!(report::conflicts(&example(), Some("nope")).is_empty());
}

// ── export_model ──

#[test]
fn export_model_omits_the_computed_findings_key() {
    assert!(report::export_model(&example()).get("findings").is_none());
}

#[test]
fn export_model_round_trips_through_model_load() {
    let exported = report::export_model(&example());
    let reloaded = Model::load(&exported.to_string()).expect("export must reload");
    assert_eq!(report::export_model(&reloaded), exported);
}

// ── stale_against ──

#[test]
fn stale_against_matching_hash_is_fresh() {
    let mut given = BTreeMap::new();
    given.insert("FM-101".to_string(), "xxh3:a1b2".to_string());
    let stale = report::stale_against(&example(), &given);
    assert_eq!(stale.fresh, vec!["FM-101".to_string()]);
}

#[test]
fn stale_against_different_hash_is_stale_with_location() {
    let mut given = BTreeMap::new();
    given.insert("FM-101".to_string(), "changed".to_string());
    let stale = report::stale_against(&example(), &given);
    assert_eq!(
        stale.stale,
        vec![report::StaleRef {
            source_ref: "FM-101".to_string(),
            stored: "xxh3:a1b2".to_string(),
            given: "changed".to_string(),
            location: "dimensions/dim_fail/members/fail_burn".to_string(),
        }]
    );
}

#[test]
fn stale_against_absent_entry_is_unchecked() {
    let stale = report::stale_against(&example(), &BTreeMap::new());
    assert_eq!(
        stale.unchecked,
        vec![
            "FM-101".to_string(),
            "FM-102".to_string(),
            "FM-103".to_string(),
        ]
    );
}

// ── relation_report_markdown ──

#[test]
fn relation_report_includes_relationship_matrix_heading() {
    let md = report::relation_report_markdown(&example(), "rel_need_part").unwrap();
    assert!(md.contains("## Relationship matrix"));
}

#[test]
fn relation_report_relationship_matrix_holds_the_expected_row() {
    let md = report::relation_report_markdown(&example(), "rel_need_part").unwrap();
    assert!(md.contains("| Keeps drink hot for hours | strong | · | · |"));
}

#[test]
fn relation_report_includes_the_self_relation_roof_sub_heading() {
    let md = report::relation_report_markdown(&example(), "rel_need_part").unwrap();
    assert!(md.contains("### rel_part_internal_conflict (opposes)"));
}

#[test]
fn relation_report_ranks_the_highest_marginalized_member_first() {
    let md = report::relation_report_markdown(&example(), "rel_need_part").unwrap();
    assert!(md.contains("| 1 | part_lid_seal | Silicone lid seal | 81 | 100% |"));
}

#[test]
fn relation_report_for_an_unknown_relation_is_err() {
    assert!(report::relation_report_markdown(&example(), "nope").is_err());
}

#[test]
fn relation_report_is_deterministic() {
    let model = example();
    assert_eq!(
        report::relation_report_markdown(&model, "rel_need_part").unwrap(),
        report::relation_report_markdown(&model, "rel_need_part").unwrap()
    );
}

#[test]
fn relation_report_targets_table_lists_member_with_target_attribute() {
    let json = r#"{
      "schemaVersion": "0.2.0",
      "modelId": "targets",
      "scales": [
        {"id":"qfd","levels":[{"observation":"strong","value":9,"order":0}]},
        {"id":"importance","levels":[{"observation":"high","value":9,"order":0}]}
      ],
      "dimensions": [
        {"id":"d1","order":0,"weightScale":"importance","members":[{"id":"a","weightObservation":"high"}]},
        {"id":"d2","order":1,"members":[{"id":"x","label":"Target X","attributes":{"target":5,"unit":"mm"}}]}
      ],
      "relations": [
        {"id":"r1","from":"d1","to":"d2","relationType":"satisfies","scale":"qfd",
         "cells":[{"from":"a","to":"x","observations":[{"observation":"strong"}]}]}
      ]
    }"#;
    let model = Model::load(json).expect("targets fixture must load");
    let md = report::relation_report_markdown(&model, "r1").unwrap();
    assert!(md.contains("| x | Target X | 5 | mm |"));
}

// ── aspect_report_markdown ──

#[test]
fn aspect_report_embeds_a_demoted_relation_section() {
    assert!(
        report::aspect_report_markdown(&example(), None)
            .contains("## House of Quality — rel_need_part")
    );
}

#[test]
fn aspect_report_omits_a_section_for_the_self_relation() {
    assert!(
        !report::aspect_report_markdown(&example(), None)
            .contains("## House of Quality — rel_part_internal_conflict")
    );
}
