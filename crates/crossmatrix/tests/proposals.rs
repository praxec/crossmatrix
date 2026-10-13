//! Behavioural tests for PROPOSED observations (X07 prerequisite): a
//! suggestion recorded for review, kept in the document but excluded from every
//! analysis until confirmed by an ordinary (active) observation.
//!
//! One assertion per test, through the public engine/write API.

use crossmatrix::write;
use crossmatrix::{Axis, Model, Observation, Proposal, ValidationError};
use serde_json::json;

/// Arrange: `d1{a,b}` × `d2{x}` with one relation `r1` of the given valence and
/// scale `qfd` (`weak`=1, `strong`=9). `cell` is raw JSON for the sole cell, or
/// empty for no cells.
fn raw_json(relation_type: &str, cell: &str) -> String {
    format!(
        r#"{{"schemaVersion":"0.2.0","modelId":"proposals",
          "scales":[
            {{"id":"qfd","levels":[
              {{"observation":"none","value":0,"order":0}},
              {{"observation":"weak","value":1,"order":1}},
              {{"observation":"strong","value":9,"order":3}}]}},
            {{"id":"importance","levels":[
              {{"observation":"low","value":1,"order":0}}]}}
          ],
          "dimensions":[
            {{"id":"d1","order":0,"weightScale":"importance","members":[
              {{"id":"a","weightObservation":"low"}},
              {{"id":"b","weightObservation":"low"}}]}},
            {{"id":"d2","order":1,"members":[{{"id":"x"}}]}}
          ],
          "relations":[{{"id":"r1","from":"d1","to":"d2","relationType":"{relation_type}",
            "scale":"qfd","cells":[{cell}]}}]}}"#
    )
}

fn fixture(relation_type: &str, cell: &str) -> Model {
    Model::load(&raw_json(relation_type, cell)).expect("fixture must load")
}

/// A cell JSON with a single `proposed` observation.
fn proposed(from: &str, to: &str, token: &str) -> String {
    format!(
        r#"{{"from":"{from}","to":"{to}","observations":[{{"observation":"{token}","status":"proposed"}}]}}"#
    )
}

fn observation(value: serde_json::Value) -> Observation {
    serde_json::from_value(value).expect("observation json")
}

#[test]
fn a_proposed_observation_round_trips_through_load_and_doc() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    let value = serde_json::to_value(model.doc()).expect("serialize doc");
    assert_eq!(
        value["relations"][0]["cells"][0]["observations"][0]["status"],
        "proposed"
    );
}

#[test]
fn a_proposed_observation_does_not_create_coverage() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    let from = model
        .coverage()
        .into_iter()
        .find(|c| c.axis == "from")
        .expect("from-axis coverage");
    assert_eq!(from.uncovered, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn a_proposed_observation_does_not_change_marginalize_scores() {
    let with_proposal = fixture("supports", &proposed("a", "x", "strong"));
    let without = fixture("supports", "");
    assert_eq!(
        with_proposal
            .marginalize("r1", Axis::From)
            .expect("marginalize"),
        without.marginalize("r1", Axis::From).expect("marginalize")
    );
}

#[test]
fn a_proposed_observation_does_not_appear_in_trace_hops() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    assert!(model.trace("a").is_empty());
}

#[test]
fn a_cell_holding_only_a_proposal_is_absent_from_slice() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    let cells = model.slice("r1", None, None).expect("relation must exist");
    assert!(cells.is_empty());
}

#[test]
fn tensions_ignore_an_opposing_proposal() {
    let model = fixture("undermines", &proposed("a", "x", "strong"));
    assert!(model.tensions().is_empty());
}

#[test]
fn proposals_lists_a_proposal_with_relation_and_endpoints() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    assert_eq!(
        model.proposals(None),
        vec![Proposal {
            relation_id: "r1".to_string(),
            from: "a".to_string(),
            to: "x".to_string(),
            observation: observation(json!({"observation":"strong","status":"proposed"})),
        }]
    );
}

#[test]
fn rolled_up_keeps_proposals_in_the_full_document() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    let value = serde_json::to_value(model.rolled_up().doc()).expect("serialize rolled-up doc");
    assert_eq!(
        value["relations"][0]["cells"][0]["observations"][0]["status"],
        "proposed"
    );
}

#[test]
fn confirming_a_proposal_with_the_same_token_makes_it_count_in_marginalize() {
    let model = fixture("supports", &proposed("a", "x", "strong"));
    let confirmed = write::observe(
        &model,
        "r1",
        "a",
        "x",
        observation(json!({"observation":"strong"})),
    )
    .expect("confirm the proposal");
    assert_eq!(
        confirmed
            .marginalize("r1", Axis::From)
            .expect("marginalize"),
        vec![("x".to_string(), 9.0)]
    );
}

#[test]
fn observe_accepts_a_proposed_observation() {
    let model = fixture("supports", "");
    assert!(
        write::observe(
            &model,
            "r1",
            "a",
            "x",
            observation(json!({"observation":"strong","status":"proposed"})),
        )
        .is_ok()
    );
}

#[test]
fn an_unknown_token_in_a_proposal_still_fails_validation() {
    let json = raw_json("supports", &proposed("a", "x", "imaginary"));
    assert!(matches!(
        Model::load(&json),
        Err(ValidationError::UnknownObservationToken { .. })
    ));
}

#[test]
fn the_travel_mug_findings_are_unchanged_by_a_proposal() {
    let loaded =
        Model::load(include_str!("fixtures/travel-mug-3axis.json")).expect("travel-mug must load");
    let with_proposal = write::observe(
        &loaded,
        "rel_need_part",
        "need_keeps_hot",
        "part_vacuum_wall",
        observation(json!({"observation":"strong","status":"proposed"})),
    )
    .expect("record proposal");
    assert_eq!(
        serde_json::to_value(loaded.findings()).expect("findings"),
        serde_json::to_value(with_proposal.findings()).expect("findings")
    );
}
