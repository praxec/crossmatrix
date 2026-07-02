//! Vets that the engine is NOT House-of-Quality-specific: a second, independently
//! authored model in a different generic domain (a travel mug: customer needs ×
//! engineering characteristics × FMECA failure modes) loads and analyzes through
//! the same fixed operations. Proves configurability — new domain = new axes +
//! cells, same engine. One atomic assertion per test (TDD discipline).
use crossmatrix::Model;

fn travel_mug() -> Model {
    Model::load(include_str!("fixtures/travel-mug-3axis.json"))
        .expect("travel-mug sample model must load")
}

#[test]
fn travel_mug_sample_has_three_dimensions() {
    assert_eq!(travel_mug().dimensions().len(), 3);
}

#[test]
fn travel_mug_sample_contracts_non_empty() {
    assert!(!travel_mug().contract("ctr_need_failure_exposure").is_empty());
}

#[test]
fn travel_mug_sample_marginalizes_a_relation() {
    assert!(travel_mug()
        .marginalize("rel_need_part", crossmatrix::Axis::From)
        .is_ok());
}
