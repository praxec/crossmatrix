//! Behavioral tests for the pure, typed write API. Each test makes exactly one
//! assertion against one observable outcome, exercising only public interfaces.

use crossmatrix::write::{
    ContractionSpec, DimensionSpec, MemberSpec, RelationSpec, TargetKind, WriteError,
    attach_evidence, declare_contraction, declare_relation, declare_scale, deprecate, observe,
    propose_member, register_dimension, sync_members,
};
use crossmatrix::{Evidence, Model, Observation, ScaleDef, SourceRef, ValidationError};

// ── fixtures ──

fn base() -> Model {
    Model::load(
        r#"{
          "schemaVersion":"0.2.0","modelId":"t",
          "scales":[
            {"id":"qfd","levels":[
              {"observation":"none","value":0,"order":0},
              {"observation":"weak","value":1,"order":1},
              {"observation":"strong","value":9,"order":3}]},
            {"id":"importance","levels":[
              {"observation":"low","value":1,"order":0},
              {"observation":"high","value":9,"order":1}]},
            {"id":"measured","levels":[
              {"observation":"small","value":1,"order":0,"range":{"min":0,"max":5}},
              {"observation":"large","value":9,"order":1,"range":{"min":5.0001,"max":10}}]}
          ],
          "dimensions":[
            {"id":"d1","order":0,"weightScale":"importance","members":[
              {"id":"a","weightObservation":"low"},
              {"id":"b","weightObservation":"high"}]},
            {"id":"d2","order":1,"members":[{"id":"x"}]}
          ],
          "relations":[
            {"id":"r1","from":"d1","to":"d2","relationType":"supports","scale":"qfd",
             "cells":[{"from":"a","to":"x","observations":[{"observation":"strong"}]}]}
          ]
        }"#,
    )
    .expect("base fixture must load")
}

fn with_measured_relation() -> Model {
    declare_relation(
        &base(),
        &RelationSpec {
            id: "r_meas".into(),
            from: "d1".into(),
            to: "d2".into(),
            relation_type: "supports".into(),
            scale: "measured".into(),
            coverage_policy: None,
        },
    )
    .expect("measured relation declaration")
}

fn source_ref(system: &str, path: &str, hash: &str) -> SourceRef {
    serde_json::from_value(serde_json::json!({
        "system": system,
        "sourceRef": path,
        "contentHash": hash,
    }))
    .expect("source ref")
}

fn member(
    id: &str,
    label: Option<&str>,
    weight_observation: Option<&str>,
    weight: Option<f64>,
    source: Option<SourceRef>,
) -> MemberSpec {
    MemberSpec {
        id: id.into(),
        label: label.map(str::to_string),
        weight_observation: weight_observation.map(str::to_string),
        weight,
        source_ref: source,
        attributes: None,
    }
}

fn observation(token: &str) -> Observation {
    serde_json::from_value(serde_json::json!({ "observation": token })).expect("observation")
}

fn measured(token: &str, value: f64, provenance: &str, with_source: bool) -> Observation {
    let mut json = serde_json::json!({
        "observation": token,
        "measuredValue": value,
        "provenanceClass": provenance,
    });
    if with_source {
        json["sourceRef"] = serde_json::json!({ "system": "s" });
    }
    serde_json::from_value(json).expect("measured observation")
}

fn evidence() -> Evidence {
    serde_json::from_value(serde_json::json!({
        "sourceRef": { "system": "s", "sourceRef": "p" }
    }))
    .expect("evidence")
}

fn scale(id: &str, levels: serde_json::Value) -> ScaleDef {
    serde_json::from_value(serde_json::json!({ "id": id, "levels": levels })).expect("scale")
}

fn dimension(id: &str) -> DimensionSpec {
    DimensionSpec {
        id: id.into(),
        kind: None,
        label: None,
        system: None,
        weight_scale: None,
        weight_semantics: None,
        order: None,
    }
}

fn relation(id: &str) -> RelationSpec {
    RelationSpec {
        id: id.into(),
        from: "d1".into(),
        to: "d2".into(),
        relation_type: "supports".into(),
        scale: "qfd".into(),
        coverage_policy: None,
    }
}

// ── register_dimension ──

#[test]
fn register_dimension_appends_a_new_dimension() {
    let model = register_dimension(&base(), &dimension("d3")).expect("register");
    assert!(model.dimensions().iter().any(|d| d.id == "d3"));
}

#[test]
fn register_dimension_defaults_order_to_max_plus_one() {
    let model = register_dimension(&base(), &dimension("d3")).expect("register");
    assert_eq!(
        model
            .dimensions()
            .iter()
            .find(|d| d.id == "d3")
            .unwrap()
            .order,
        2
    );
}

#[test]
fn register_dimension_accepts_an_open_kind_outside_the_old_enum() {
    let mut spec = dimension("d3");
    spec.kind = Some("competitor".into());
    let model = register_dimension(&base(), &spec).expect("register");
    assert_eq!(
        model
            .dimensions()
            .iter()
            .find(|d| d.id == "d3")
            .unwrap()
            .kind
            .as_deref(),
        Some("competitor")
    );
}

#[test]
fn register_dimension_upsert_overwrites_some_metadata() {
    let mut spec = dimension("d1");
    spec.label = Some("Renamed".into());
    let model = register_dimension(&base(), &spec).expect("register");
    assert_eq!(model.dimensions()[0].label.as_deref(), Some("Renamed"));
}

#[test]
fn register_dimension_upsert_leaves_members_untouched() {
    let model = register_dimension(&base(), &dimension("d1")).expect("register");
    assert_eq!(model.member_count("d1"), 2);
}

#[test]
fn register_dimension_rejects_an_unknown_weight_scale() {
    let mut spec = dimension("d3");
    spec.weight_scale = Some("nope".into());
    assert!(matches!(
        register_dimension(&base(), &spec),
        Err(WriteError::UnknownScale(_))
    ));
}

// ── declare_scale ──

#[test]
fn declare_scale_adds_a_new_scale() {
    let model = declare_scale(
        &base(),
        scale(
            "extra",
            serde_json::json!([{ "observation": "on", "value": 1 }]),
        ),
    )
    .expect("declare");
    assert!(model.scales().iter().any(|s| s.id == "extra"));
}

#[test]
fn declare_scale_replaces_an_existing_scale() {
    let model = declare_scale(
        &base(),
        scale(
            "importance",
            serde_json::json!([{ "observation": "low", "value": 7 }]),
        ),
    )
    .expect("declare");
    assert_eq!(model.scales()[1].levels[0].value, 7.0);
}

#[test]
fn declare_scale_rejects_duplicate_level_tokens() {
    let duplicate = scale(
        "dup",
        serde_json::json!([
            { "observation": "x", "value": 1 },
            { "observation": "x", "value": 2 }
        ]),
    );
    assert!(matches!(
        declare_scale(&base(), duplicate),
        Err(WriteError::DuplicateToken { .. })
    ));
}

#[test]
fn declare_scale_orphaning_a_used_token_is_invalid() {
    let replacement = scale(
        "qfd",
        serde_json::json!([{ "observation": "none", "value": 0 }]),
    );
    assert!(matches!(
        declare_scale(&base(), replacement),
        Err(WriteError::Invalid(_))
    ));
}

// ── declare_relation ──

#[test]
fn declare_relation_adds_a_new_relation() {
    let model = declare_relation(&base(), &relation("r2")).expect("declare");
    assert!(model.relations().iter().any(|r| r.id == "r2"));
}

#[test]
fn declare_relation_upsert_preserves_existing_cells() {
    let mut spec = relation("r1");
    spec.relation_type = "undermines".into();
    let model = declare_relation(&base(), &spec).expect("declare");
    assert_eq!(model.relations()[0].cells.len(), 1);
}

#[test]
fn declare_relation_accepts_a_self_relation() {
    let mut spec = relation("r_self");
    spec.to = "d1".into();
    spec.relation_type = "opposes".into();
    assert!(declare_relation(&base(), &spec).is_ok());
}

#[test]
fn declare_relation_rejects_an_unknown_from_dimension() {
    let mut spec = relation("r2");
    spec.from = "nope".into();
    assert!(matches!(
        declare_relation(&base(), &spec),
        Err(WriteError::UnknownDimension(_))
    ));
}

#[test]
fn declare_relation_rejects_an_unknown_scale() {
    let mut spec = relation("r2");
    spec.scale = "nope".into();
    assert!(matches!(
        declare_relation(&base(), &spec),
        Err(WriteError::UnknownScale(_))
    ));
}

#[test]
fn declare_relation_rejects_an_unknown_relation_type() {
    let mut spec = relation("r2");
    spec.relation_type = "nope".into();
    assert!(matches!(
        declare_relation(&base(), &spec),
        Err(WriteError::UnknownRelationType(_))
    ));
}

#[test]
fn declare_relation_rejects_an_unknown_coverage_policy() {
    let mut spec = relation("r2");
    spec.coverage_policy = Some("nope".into());
    assert!(matches!(
        declare_relation(&base(), &spec),
        Err(WriteError::UnknownCoveragePolicy(_))
    ));
}

// ── declare_contraction ──

#[test]
fn declare_contraction_adds_a_chain() {
    let model = declare_contraction(
        &base(),
        &ContractionSpec {
            id: "c1".into(),
            chain: vec!["r1".into()],
            label: None,
            weight_combination: None,
            weight_by_source_members: None,
        },
    )
    .expect("declare");
    assert_eq!(model.contraction_ids(), vec!["c1"]);
}

#[test]
fn declare_contraction_rejects_an_unknown_relation() {
    assert!(matches!(
        declare_contraction(
            &base(),
            &ContractionSpec {
                id: "c1".into(),
                chain: vec!["nope".into()],
                label: None,
                weight_combination: None,
                weight_by_source_members: None,
            },
        ),
        Err(WriteError::UnknownRelation(_))
    ));
}

#[test]
fn declare_contraction_rejects_an_unknown_weight_combination() {
    assert!(matches!(
        declare_contraction(
            &base(),
            &ContractionSpec {
                id: "c1".into(),
                chain: vec!["r1".into()],
                label: None,
                weight_combination: Some("nope".into()),
                weight_by_source_members: None,
            },
        ),
        Err(WriteError::UnknownWeightCombination(_))
    ));
}

// ── sync_members ──

#[test]
fn sync_members_adds_new_members() {
    let (_, summary) = sync_members(
        &base(),
        "d1",
        &[
            member("a", None, Some("low"), None, None),
            member("b", None, Some("high"), None, None),
            member("c", None, Some("low"), None, None),
        ],
    )
    .expect("sync");
    assert_eq!(summary.added, vec!["c".to_string()]);
}

#[test]
fn sync_members_updates_a_changed_member() {
    let (_, summary) = sync_members(
        &base(),
        "d1",
        &[
            member("a", Some("changed"), Some("low"), None, None),
            member("b", None, Some("high"), None, None),
        ],
    )
    .expect("sync");
    assert_eq!(summary.updated, vec!["a".to_string()]);
}

#[test]
fn sync_members_reports_an_identical_member_unchanged() {
    let (_, summary) = sync_members(
        &base(),
        "d1",
        &[
            member("a", None, Some("low"), None, None),
            member("b", None, Some("high"), None, None),
        ],
    )
    .expect("sync");
    assert_eq!(summary.unchanged, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn sync_members_deprecates_a_member_absent_from_the_roster() {
    let (_, summary) =
        sync_members(&base(), "d1", &[member("a", None, Some("low"), None, None)]).expect("sync");
    assert_eq!(summary.deprecated, vec!["b".to_string()]);
}

#[test]
fn sync_members_reactivates_a_relisted_member() {
    let (once, _) =
        sync_members(&base(), "d1", &[member("a", None, Some("low"), None, None)]).expect("sync");
    let (_, summary) = sync_members(
        &once,
        "d1",
        &[
            member("a", None, Some("low"), None, None),
            member("b", None, Some("high"), None, None),
        ],
    )
    .expect("sync");
    assert_eq!(summary.reactivated, vec!["b".to_string()]);
}

#[test]
fn sync_members_rejects_an_unknown_dimension() {
    assert!(matches!(
        sync_members(&base(), "nope", &[]),
        Err(WriteError::UnknownDimension(_))
    ));
}

#[test]
fn sync_members_rejects_a_member_id_used_in_another_dimension() {
    assert!(matches!(
        sync_members(&base(), "d1", &[member("x", None, None, None, None)]),
        Err(WriteError::DuplicateMember { .. })
    ));
}

#[test]
fn sync_members_rejects_weight_without_a_deterministic_source() {
    assert!(matches!(
        sync_members(&base(), "d1", &[member("c", None, None, Some(5.0), None)]),
        Err(WriteError::ObservationOnly(_))
    ));
}

#[test]
fn sync_members_accepts_weight_with_a_source_system() {
    let (_, summary) = sync_members(
        &base(),
        "d1",
        &[member(
            "c",
            None,
            None,
            Some(5.0),
            Some(source_ref("fmeca", "FM-1", "h")),
        )],
    )
    .expect("sync");
    assert_eq!(summary.added, vec!["c".to_string()]);
}

#[test]
fn sync_members_rejects_weight_observation_without_a_weight_scale() {
    assert!(matches!(
        sync_members(&base(), "d2", &[member("y", None, Some("low"), None, None)]),
        Err(WriteError::MissingWeightScale { .. })
    ));
}

#[test]
fn sync_members_rejects_an_unknown_weight_observation_token() {
    assert!(matches!(
        sync_members(
            &base(),
            "d1",
            &[member("a", None, Some("nope"), None, None)]
        ),
        Err(WriteError::UnknownToken { .. })
    ));
}

// ── propose_member ──

#[test]
fn propose_member_adds_one_member() {
    let model = propose_member(&base(), "d1", &member("c", None, Some("low"), None, None))
        .expect("propose");
    assert_eq!(model.member_count("d1"), 3);
}

#[test]
fn propose_member_rejects_an_existing_member() {
    assert!(matches!(
        propose_member(&base(), "d1", &member("a", None, None, None, None)),
        Err(WriteError::DuplicateMember { .. })
    ));
}

#[test]
fn propose_member_rejects_an_unknown_dimension() {
    assert!(matches!(
        propose_member(&base(), "nope", &member("c", None, None, None, None)),
        Err(WriteError::UnknownDimension(_))
    ));
}

// ── observe ──

#[test]
fn observe_appends_a_second_distinct_observation() {
    let model = observe(&base(), "r1", "a", "x", observation("weak")).expect("observe");
    assert_eq!(model.relations()[0].cells[0].observations.len(), 2);
}

#[test]
fn observe_ignores_an_exact_duplicate_observation() {
    let model = observe(&base(), "r1", "a", "x", observation("strong")).expect("observe");
    assert_eq!(model.relations()[0].cells[0].observations.len(), 1);
}

#[test]
fn observe_creates_the_cell_when_absent() {
    let model = observe(&base(), "r1", "b", "x", observation("weak")).expect("observe");
    assert_eq!(model.relations()[0].cells.len(), 2);
}

#[test]
fn observe_rejects_an_unknown_relation() {
    assert!(matches!(
        observe(&base(), "nope", "a", "x", observation("weak")),
        Err(WriteError::UnknownRelation(_))
    ));
}

#[test]
fn observe_rejects_a_deprecated_relation() {
    let deprecated = deprecate(&base(), "r1", Some(TargetKind::Relation), None).expect("deprecate");
    assert!(matches!(
        observe(&deprecated, "r1", "a", "x", observation("weak")),
        Err(WriteError::RelationDeprecated(_))
    ));
}

#[test]
fn observe_rejects_an_unknown_member() {
    assert!(matches!(
        observe(&base(), "r1", "z", "x", observation("weak")),
        Err(WriteError::UnknownMember { .. })
    ));
}

#[test]
fn observe_rejects_a_token_outside_the_scale() {
    assert!(matches!(
        observe(&base(), "r1", "a", "x", observation("nope")),
        Err(WriteError::UnknownToken { .. })
    ));
}

#[test]
fn observe_rejects_measured_value_without_a_source_ref() {
    assert!(matches!(
        observe(
            &with_measured_relation(),
            "r_meas",
            "a",
            "x",
            measured("small", 3.0, "observed", false)
        ),
        Err(WriteError::ObservationOnly(_))
    ));
}

#[test]
fn observe_rejects_measured_value_without_observed_provenance() {
    assert!(matches!(
        observe(
            &with_measured_relation(),
            "r_meas",
            "a",
            "x",
            measured("small", 3.0, "normative", true)
        ),
        Err(WriteError::ObservationOnly(_))
    ));
}

#[test]
fn observe_rejects_a_token_that_mismatches_the_measured_bucket() {
    assert!(matches!(
        observe(
            &with_measured_relation(),
            "r_meas",
            "a",
            "x",
            measured("small", 8.0, "observed", true)
        ),
        Err(WriteError::MeasuredMismatch { .. })
    ));
}

#[test]
fn observe_accepts_a_token_that_matches_the_measured_bucket() {
    assert!(
        observe(
            &with_measured_relation(),
            "r_meas",
            "a",
            "x",
            measured("large", 8.0, "observed", true)
        )
        .is_ok()
    );
}

// ── attach_evidence ──

#[test]
fn attach_evidence_appends_to_the_last_observation_by_default() {
    let model = attach_evidence(&base(), "r1", "a", "x", None, evidence()).expect("attach");
    assert_eq!(
        model.relations()[0].cells[0].observations[0].evidence.len(),
        1
    );
}

#[test]
fn attach_evidence_rejects_an_unknown_cell() {
    assert!(matches!(
        attach_evidence(&base(), "r1", "b", "x", None, evidence()),
        Err(WriteError::UnknownCell { .. })
    ));
}

#[test]
fn attach_evidence_rejects_an_out_of_range_index() {
    assert!(matches!(
        attach_evidence(&base(), "r1", "a", "x", Some(5), evidence()),
        Err(WriteError::ObservationIndexOutOfRange { .. })
    ));
}

// ── deprecate ──

#[test]
fn deprecate_marks_a_member_deprecated() {
    let model = deprecate(&base(), "a", Some(TargetKind::Member), None).expect("deprecate");
    let value = serde_json::to_value(model.doc()).expect("serialize");
    assert_eq!(
        value["dimensions"][0]["members"][0]["status"],
        serde_json::json!("deprecated")
    );
}

#[test]
fn deprecate_sets_superseded_by() {
    let model = deprecate(&base(), "b", Some(TargetKind::Member), Some("a")).expect("deprecate");
    let value = serde_json::to_value(model.doc()).expect("serialize");
    assert_eq!(
        value["dimensions"][0]["members"][1]["supersededBy"],
        serde_json::json!("a")
    );
}

#[test]
fn deprecate_marks_a_relation_deprecated() {
    let model = deprecate(&base(), "r1", Some(TargetKind::Relation), None).expect("deprecate");
    let value = serde_json::to_value(model.doc()).expect("serialize");
    assert_eq!(
        value["relations"][0]["status"],
        serde_json::json!("deprecated")
    );
}

#[test]
fn deprecate_rejects_an_unknown_target() {
    assert!(matches!(
        deprecate(&base(), "nope", None, None),
        Err(WriteError::UnknownTarget(_))
    ));
}

#[test]
fn deprecate_rejects_an_ambiguous_target() {
    let model = propose_member(&base(), "d1", &member("r1", None, None, None, None))
        .expect("propose member colliding with relation id");
    assert!(matches!(
        deprecate(&model, "r1", None, None),
        Err(WriteError::AmbiguousTarget(_))
    ));
}

#[test]
fn deprecate_rejects_an_unknown_superseded_by() {
    assert!(matches!(
        deprecate(&base(), "a", Some(TargetKind::Member), Some("nope")),
        Err(WriteError::UnknownTarget(_))
    ));
}

// ── purity & error codes ──

#[test]
fn a_write_does_not_mutate_the_input_model() {
    let original = base();
    let before = serde_json::to_value(original.doc()).expect("serialize");
    let _ = register_dimension(&original, &dimension("d3")).expect("register");
    let after = serde_json::to_value(original.doc()).expect("serialize");
    assert_eq!(before, after);
}

#[test]
fn write_error_code_is_stable_snake_case_for_every_variant() {
    let cases = [
        (
            WriteError::UnknownDimension("x".into()),
            "unknown_dimension",
        ),
        (WriteError::UnknownRelation("x".into()), "unknown_relation"),
        (WriteError::UnknownScale("x".into()), "unknown_scale"),
        (
            WriteError::UnknownMember {
                dimension: "d".into(),
                member: "m".into(),
            },
            "unknown_member",
        ),
        (
            WriteError::UnknownToken {
                scale: "s".into(),
                token: "t".into(),
            },
            "unknown_token",
        ),
        (
            WriteError::UnknownRelationType("x".into()),
            "unknown_relation_type",
        ),
        (
            WriteError::UnknownCoveragePolicy("x".into()),
            "unknown_coverage_policy",
        ),
        (
            WriteError::UnknownWeightCombination("x".into()),
            "unknown_weight_combination",
        ),
        (
            WriteError::DuplicateMember {
                dimension: "d".into(),
                member: "m".into(),
            },
            "duplicate_member",
        ),
        (
            WriteError::DuplicateToken {
                scale: "s".into(),
                token: "t".into(),
            },
            "duplicate_token",
        ),
        (
            WriteError::MissingWeightScale {
                dimension: "d".into(),
            },
            "missing_weight_scale",
        ),
        (WriteError::ObservationOnly("x".into()), "observation_only"),
        (
            WriteError::MeasuredMismatch {
                token: "t".into(),
                bucket: "b".into(),
            },
            "measured_mismatch",
        ),
        (
            WriteError::RelationDeprecated("x".into()),
            "relation_deprecated",
        ),
        (
            WriteError::UnknownCell {
                relation: "r".into(),
                from: "f".into(),
                to: "t".into(),
            },
            "unknown_cell",
        ),
        (
            WriteError::ObservationIndexOutOfRange { index: 0, len: 0 },
            "observation_index_out_of_range",
        ),
        (WriteError::UnknownTarget("x".into()), "unknown_target"),
        (WriteError::AmbiguousTarget("x".into()), "ambiguous_target"),
        (
            WriteError::Invalid(ValidationError::DanglingCell(vec![])),
            "invalid",
        ),
    ];
    let codes: Vec<&str> = cases.iter().map(|(error, _)| error.code()).collect();
    let expected: Vec<&str> = cases.iter().map(|(_, code)| *code).collect();
    assert_eq!(codes, expected);
}

// ── end-to-end acceptance ──

#[test]
fn building_the_travel_mug_model_reproduces_the_loaded_findings() {
    let mut model = Model::load(
        r#"{"schemaVersion":"0.2.0","modelId":"travel-mug-qfd-fmeca","dimensions":[],"relations":[]}"#,
    )
    .expect("empty model");

    model = declare_scale(
        &model,
        scale(
            "qfd",
            serde_json::json!([
                { "observation": "none", "value": 0, "order": 0 },
                { "observation": "weak", "value": 1, "order": 1 },
                { "observation": "moderate", "value": 3, "order": 2 },
                { "observation": "strong", "value": 9, "order": 3 }
            ]),
        ),
    )
    .expect("scale qfd");
    model = declare_scale(
        &model,
        scale(
            "importance",
            serde_json::json!([
                { "observation": "low", "value": 1, "order": 0 },
                { "observation": "medium", "value": 3, "order": 1 },
                { "observation": "high", "value": 9, "order": 2 }
            ]),
        ),
    )
    .expect("scale importance");

    model = register_dimension(
        &model,
        &DimensionSpec {
            id: "dim_need".into(),
            kind: Some("customer_requirement".into()),
            label: Some("Customer needs".into()),
            system: None,
            weight_scale: Some("importance".into()),
            weight_semantics: Some("customer_importance".into()),
            order: Some(0),
        },
    )
    .expect("dim_need");
    model = register_dimension(
        &model,
        &DimensionSpec {
            id: "dim_part".into(),
            kind: Some("engineering_characteristic".into()),
            label: Some("Engineering characteristics".into()),
            system: None,
            weight_scale: None,
            weight_semantics: None,
            order: Some(1),
        },
    )
    .expect("dim_part");
    model = register_dimension(
        &model,
        &DimensionSpec {
            id: "dim_fail".into(),
            kind: Some("failure_mode".into()),
            label: Some("Failure modes (FMECA)".into()),
            system: Some("fmeca".into()),
            weight_scale: None,
            weight_semantics: Some("fmeca_criticality".into()),
            order: Some(2),
        },
    )
    .expect("dim_fail");

    model = sync_members(
        &model,
        "dim_need",
        &[
            member(
                "need_keeps_hot",
                Some("Keeps drink hot for hours"),
                Some("high"),
                None,
                None,
            ),
            member(
                "need_no_spill",
                Some("Does not spill in a bag"),
                Some("high"),
                None,
                None,
            ),
            member(
                "need_easy_clean",
                Some("Easy to clean"),
                Some("medium"),
                None,
                None,
            ),
        ],
    )
    .expect("sync dim_need")
    .0;
    model = sync_members(
        &model,
        "dim_part",
        &[
            member(
                "part_vacuum_wall",
                Some("Double-wall vacuum insulation"),
                None,
                None,
                None,
            ),
            member("part_lid_seal", Some("Silicone lid seal"), None, None, None),
            member(
                "part_wide_mouth",
                Some("Wide mouth opening"),
                None,
                None,
                None,
            ),
        ],
    )
    .expect("sync dim_part")
    .0;
    model = sync_members(
        &model,
        "dim_fail",
        &[
            member(
                "fail_burn",
                Some("Burn injury from hot exterior"),
                None,
                Some(9.0),
                Some(source_ref("fmeca", "FM-101", "xxh3:a1b2")),
            ),
            member(
                "fail_leak",
                Some("Leak or spill into bag"),
                None,
                Some(9.0),
                Some(source_ref("fmeca", "FM-102", "xxh3:c3d4")),
            ),
            member(
                "fail_mold",
                Some("Mold from incomplete cleaning"),
                None,
                Some(3.0),
                Some(source_ref("fmeca", "FM-103", "xxh3:e5f6")),
            ),
        ],
    )
    .expect("sync dim_fail")
    .0;

    model = declare_relation(
        &model,
        &RelationSpec {
            id: "rel_need_part".into(),
            from: "dim_need".into(),
            to: "dim_part".into(),
            relation_type: "satisfies".into(),
            scale: "qfd".into(),
            coverage_policy: None,
        },
    )
    .expect("rel_need_part");
    model = declare_relation(
        &model,
        &RelationSpec {
            id: "rel_part_exposes_fail".into(),
            from: "dim_part".into(),
            to: "dim_fail".into(),
            relation_type: "exposes".into(),
            scale: "qfd".into(),
            coverage_policy: None,
        },
    )
    .expect("rel_part_exposes_fail");
    model = declare_relation(
        &model,
        &RelationSpec {
            id: "rel_part_mitigates_fail".into(),
            from: "dim_part".into(),
            to: "dim_fail".into(),
            relation_type: "mitigates".into(),
            scale: "qfd".into(),
            coverage_policy: None,
        },
    )
    .expect("rel_part_mitigates_fail");
    model = declare_relation(
        &model,
        &RelationSpec {
            id: "rel_part_internal_conflict".into(),
            from: "dim_part".into(),
            to: "dim_part".into(),
            relation_type: "opposes".into(),
            scale: "qfd".into(),
            coverage_policy: None,
        },
    )
    .expect("rel_part_internal_conflict");

    let obs = |token: &str, provenance: &str, rationale: Option<&str>| -> Observation {
        let mut json = serde_json::json!({
            "observation": token,
            "provenanceClass": provenance,
        });
        if let Some(text) = rationale {
            json["rationale"] = serde_json::json!(text);
        }
        serde_json::from_value(json).expect("observation")
    };

    model = observe(
        &model,
        "rel_need_part",
        "need_keeps_hot",
        "part_vacuum_wall",
        obs("strong", "normative", None),
    )
    .expect("cell 1");
    model = observe(
        &model,
        "rel_need_part",
        "need_no_spill",
        "part_lid_seal",
        obs("strong", "normative", None),
    )
    .expect("cell 2");
    model = observe(
        &model,
        "rel_need_part",
        "need_easy_clean",
        "part_wide_mouth",
        obs("strong", "normative", None),
    )
    .expect("cell 3");
    model = observe(
        &model,
        "rel_part_exposes_fail",
        "part_vacuum_wall",
        "fail_burn",
        obs(
            "weak",
            "observed",
            Some("Even double-wall designs leave a hot rim near the lid."),
        ),
    )
    .expect("cell 4");
    model = observe(
        &model,
        "rel_part_exposes_fail",
        "part_wide_mouth",
        "fail_leak",
        obs(
            "strong",
            "observed",
            Some("A wider mouth is harder to seal reliably."),
        ),
    )
    .expect("cell 5");
    model = observe(
        &model,
        "rel_part_mitigates_fail",
        "part_lid_seal",
        "fail_leak",
        obs(
            "strong",
            "normative",
            Some("A compliant silicone seal mitigates leaks."),
        ),
    )
    .expect("cell 6");
    model = observe(
        &model,
        "rel_part_mitigates_fail",
        "part_wide_mouth",
        "fail_mold",
        obs(
            "strong",
            "normative",
            Some("A wide mouth lets a brush reach all surfaces, mitigating mold."),
        ),
    )
    .expect("cell 7");
    model = observe(
        &model,
        "rel_part_internal_conflict",
        "part_wide_mouth",
        "part_lid_seal",
        obs(
            "moderate",
            "observed",
            Some(
                "Intra-axis trade-off as a SELF-relation: a wider mouth makes a reliable seal harder to achieve.",
            ),
        ),
    )
    .expect("cell 8");

    model = declare_contraction(
        &model,
        &ContractionSpec {
            id: "ctr_need_failure_exposure".into(),
            chain: vec!["rel_need_part".into(), "rel_part_exposes_fail".into()],
            label: Some(
                "Need -> failure exposure (support chain: rel_need_part  contracted with  rel_part_exposes_fail)"
                    .into(),
            ),
            weight_combination: Some("product".into()),
            weight_by_source_members: Some(true),
        },
    )
    .expect("contraction");

    let expected =
        Model::load(include_str!("../../../examples/travel-mug-3axis.json")).expect("example");
    assert_eq!(
        serde_json::to_value(model.findings()).expect("built findings"),
        serde_json::to_value(expected.findings()).expect("loaded findings")
    );
}
