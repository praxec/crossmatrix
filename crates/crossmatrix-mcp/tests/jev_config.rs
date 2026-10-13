#![cfg(feature = "jev")]

use crossmatrix_mcp::assist::UnavailableReason;
use crossmatrix_mcp::assist::jev::{JevConfig, JevProvider};

fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        pairs
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| (*value).to_string())
    }
}

#[test]
fn disabled_when_assist_unset() {
    let lookup = lookup_from(&[]);

    let err = JevConfig::from_lookup(&lookup).expect_err("assist must be disabled");

    assert_eq!(err.reason, UnavailableReason::Disabled);
}

#[test]
fn no_key_when_enabled_without_keys() {
    let lookup = lookup_from(&[("CROSSMATRIX_ASSIST", "jev")]);

    let err = JevConfig::from_lookup(&lookup).expect_err("no key must be reported");

    assert_eq!(err.reason, UnavailableReason::NoKey);
}

#[test]
fn typesafe_default_endpoint_is_systemone() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("TYPESAFE_JEV_API_KEY", "sk-x"),
    ]);

    let config = JevConfig::from_lookup(&lookup).expect("typesafe config");

    assert_eq!(config.endpoint, "https://api.typesafe.ai/v1/systemone");
}

#[test]
fn typesafe_default_model_is_jev_latest() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("TYPESAFE_JEV_API_KEY", "sk-x"),
    ]);

    let config = JevConfig::from_lookup(&lookup).expect("typesafe config");

    assert_eq!(config.model, "jev-latest");
}

#[test]
fn openrouter_default_endpoint_is_openrouter_systemone() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("OPENROUTER_API_KEY", "sk-or"),
    ]);

    let config = JevConfig::from_lookup(&lookup).expect("openrouter config");

    assert_eq!(config.endpoint, "https://openrouter.ai/api/v1/systemone");
}

#[test]
fn openrouter_default_model_uses_openrouter_namespace() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("OPENROUTER_API_KEY", "sk-or"),
    ]);

    let config = JevConfig::from_lookup(&lookup).expect("openrouter config");

    assert_eq!(config.model, "~typesafe/jev-latest");
}

#[test]
fn typesafe_key_wins_over_openrouter_key() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("TYPESAFE_JEV_API_KEY", "sk-ts"),
        ("OPENROUTER_API_KEY", "sk-or"),
    ]);

    let config = JevConfig::from_lookup(&lookup).expect("typesafe config");

    assert_eq!(config.provider, JevProvider::Typesafe);
}

#[test]
fn zero_timeout_yields_bad_config() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("TYPESAFE_JEV_API_KEY", "sk-x"),
        ("CROSSMATRIX_JEV_TIMEOUT_SECS", "0"),
    ]);

    let err = JevConfig::from_lookup(&lookup).expect_err("zero timeout must fail");

    assert_eq!(err.reason, UnavailableReason::BadConfig);
}

#[test]
fn debug_never_contains_api_key() {
    let lookup = lookup_from(&[
        ("CROSSMATRIX_ASSIST", "jev"),
        ("TYPESAFE_JEV_API_KEY", "sk-secret-value"),
    ]);
    let config = JevConfig::from_lookup(&lookup).expect("typesafe config");

    assert!(!format!("{config:?}").contains("sk-secret-value"));
}
