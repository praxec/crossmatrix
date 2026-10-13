//! Provider-neutral "assist" surface: a small set of types shared by every
//! optional LLM assist backend. This module is ALWAYS compiled; the concrete
//! backends live behind cargo features (see [`jev`]).

#[cfg(feature = "jev")]
pub mod jev;

use serde::Serialize;

/// Why an assist backend could not answer. Serialized `snake_case`; the two
/// HTTP classes get explicit `http_4xx` / `http_5xx` spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    Disabled,
    NoKey,
    Timeout,
    #[serde(rename = "http_4xx")]
    Http4xx,
    #[serde(rename = "http_5xx")]
    Http5xx,
    Network,
    BadResponse,
    BadConfig,
    FeatureNotBuilt,
}

/// A provider-neutral "assist is not available" result. `detail` is bounded
/// and must never carry a credential (callers scrub before constructing).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistUnavailable {
    pub reason: UnavailableReason,
    pub detail: String,
}

/// Maximum length (in `char`s) of an [`AssistUnavailable::detail`].
const MAX_DETAIL_CHARS: usize = 300;

impl AssistUnavailable {
    /// Builds an unavailable result, truncating `detail` to the bounded
    /// length on a `char` boundary.
    pub fn new(reason: UnavailableReason, detail: impl Into<String>) -> Self {
        let detail = detail.into().chars().take(MAX_DETAIL_CHARS).collect();
        Self { reason, detail }
    }

    /// Renders the wire shape used by the MCP report:
    /// `{"ok": false, "status": "assist_unavailable", ...}`.
    pub fn to_value(&self, links: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "ok": false,
            "status": "assist_unavailable",
            "reason": self.reason,
            "detail": self.detail,
            "links": links,
        })
    }
}

/// Replaces every occurrence of `secret` with `[redacted]`. An empty secret
/// leaves the text untouched (it would otherwise match everywhere).
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "[redacted]")
}

/// Provider-neutral token accounting for one assist call.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

/// The explicit assist opt-in. `CROSSMATRIX_ASSIST` is a comma list; a
/// backend is enabled when the list contains its `feature`, the wildcard
/// `all`, or the sentinel `1`. Unset or empty means disabled.
pub fn assist_enabled(lookup: &dyn Fn(&str) -> Option<String>, feature: &str) -> bool {
    let Some(raw) = lookup("CROSSMATRIX_ASSIST") else {
        return false;
    };
    if raw.trim().is_empty() {
        return false;
    }
    raw.split(',')
        .map(str::trim)
        .any(|token| !token.is_empty() && (token == feature || token == "all" || token == "1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_lookup(value: &str) -> impl Fn(&str) -> Option<String> + '_ {
        move |_| Some(value.to_string())
    }

    #[test]
    fn assist_enabled_recognizes_jev_in_comma_list() {
        assert!(assist_enabled(&fixed_lookup("jev,rig"), "jev"));
    }

    #[test]
    fn assist_enabled_recognizes_rig_in_comma_list() {
        assert!(assist_enabled(&fixed_lookup("jev,rig"), "rig"));
    }

    #[test]
    fn to_value_reports_assist_unavailable_status() {
        let unavailable = AssistUnavailable::new(UnavailableReason::NoKey, "no key");
        let value = unavailable.to_value(&[]);
        assert_eq!(value["status"], "assist_unavailable");
    }
}
