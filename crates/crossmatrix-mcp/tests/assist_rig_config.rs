#![cfg(feature = "rig")]

//! Configuration tests for the feature-gated OpenRouter text model. No network
//! calls: the client is only constructed, never dispatched.

use std::collections::HashMap;

use crossmatrix_mcp::assist_rig::OpenRouterTextModel;

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

#[test]
fn disabled_without_crossmatrix_assist() {
    let error = OpenRouterTextModel::from_lookup(&lookup(&[])).expect_err("must be disabled");
    assert_eq!(error.reason, "disabled");
}

#[test]
fn no_key_without_openrouter_api_key() {
    let error = OpenRouterTextModel::from_lookup(&lookup(&[("CROSSMATRIX_ASSIST", "rig")]))
        .expect_err("must report no_key");
    assert_eq!(error.reason, "no_key");
}

#[test]
fn default_model_is_openai_gpt_5_mini() {
    let model = OpenRouterTextModel::from_lookup(&lookup(&[
        ("CROSSMATRIX_ASSIST", "rig"),
        ("OPENROUTER_API_KEY", "sk-test"),
    ]))
    .expect("valid config must build");
    assert_eq!(model.model(), "openai/gpt-5-mini");
}

#[test]
fn bad_timeout_is_bad_config() {
    let error = OpenRouterTextModel::from_lookup(&lookup(&[
        ("CROSSMATRIX_ASSIST", "rig"),
        ("OPENROUTER_API_KEY", "sk-test"),
        ("CROSSMATRIX_RIG_TIMEOUT_SECS", "9999"),
    ]))
    .expect_err("out-of-range timeout must be bad_config");
    assert_eq!(error.reason, "bad_config");
}
