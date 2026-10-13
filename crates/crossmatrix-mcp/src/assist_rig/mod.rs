//! LLM assist functions for `crossmatrix-mcp`.
//!
//! The pure, model-agnostic logic (`propose_members`, `narrate`) is always
//! compiled and depends only on the [`TextModel`] seam, so it is fully testable
//! without a provider. The real OpenRouter-backed [`TextModel`] lives behind the
//! `rig` cargo feature (see [`openrouter`]) and is OFF by default.
//!
//! These functions never apply anything: `propose_members` returns ready
//! `member.propose` op payloads for a caller (the manager) to dispatch later.

use serde::Serialize;
use serde_json::{Map, Value, json};

/// Maximum number of source characters handed to the member-extraction model.
pub const SOURCE_CHAR_LIMIT: usize = 60_000;
/// Maximum number of findings summarized for the narrative model.
pub const FINDINGS_LIMIT: usize = 30;
/// Maximum number of contributors kept per finding in the narrative summary.
pub const CONTRIBUTORS_LIMIT: usize = 5;

/// A structured, user-facing failure of an assist call.
///
/// `reason` is one of `disabled`, `no_key`, `timeout`, `http_4xx`, `http_5xx`,
/// `network`, `bad_response`, or `bad_config`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RigUnavailable {
    /// Machine-readable reason code.
    pub reason: String,
    /// Human-readable detail (never contains an API key).
    pub detail: String,
}

impl RigUnavailable {
    /// Build an unavailable result.
    pub fn new(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            detail: detail.into(),
        }
    }

    /// Render the canonical `assist_unavailable` tool payload.
    pub fn to_value(&self, links: &[&str]) -> Value {
        json!({
            "ok": false,
            "status": "assist_unavailable",
            "reason": self.reason,
            "detail": self.detail,
            "links": links,
        })
    }
}

/// Token and cost accounting for one completion.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RigUsage {
    /// Prompt tokens reported by the provider.
    pub input_tokens: u64,
    /// Completion tokens reported by the provider.
    pub output_tokens: u64,
    /// Provider-reported cost in USD, when available.
    pub cost_usd: Option<f64>,
}

/// A raw completion returned by a [`TextModel`].
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    /// The assistant text.
    pub text: String,
    /// Provider usage.
    pub usage: RigUsage,
    /// The model that actually served the request.
    pub model: String,
}

/// The model seam: a minimal completion call, object-safe and free of
/// `async-trait` so the pure logic compiles without the `rig` feature.
pub trait TextModel: Send + Sync {
    /// Complete `system` + `user`, bounded to `max_tokens` output tokens.
    fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Completion, RigUnavailable>> + Send + '_>,
    >;
}

/// One accepted member proposal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberProposal {
    /// Proposed member id (snake_case).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Optional description.
    pub description: Option<String>,
    /// Source reference: `{uri?, sourceRef?, locator: "chars <start>-<end>", excerpt}`.
    pub source_ref: Value,
    /// Ready `member.propose` op payload for the manager to dispatch.
    pub propose_op: Value,
}

/// Input for [`propose_members`].
#[derive(Debug, Clone)]
pub struct MembersRequest {
    /// Target dimension id.
    pub dimension_id: String,
    /// Optional dimension label.
    pub dimension_label: Option<String>,
    /// Member ids that already exist and must not be re-proposed.
    pub existing_member_ids: Vec<String>,
    /// Authoritative source text to extract members from.
    pub source_text: String,
    /// Optional URI of the source document.
    pub source_uri: Option<String>,
    /// Maximum accepted proposals (clamped to `1..=50`).
    pub max_members: usize,
    /// Output token budget (clamped to `64..=4096`).
    pub max_tokens: u32,
}

impl Default for MembersRequest {
    fn default() -> Self {
        Self {
            dimension_id: String::new(),
            dimension_label: None,
            existing_member_ids: Vec::new(),
            source_text: String::new(),
            source_uri: None,
            max_members: 20,
            max_tokens: 1024,
        }
    }
}

/// Output for [`propose_members`]. Proposals are never applied.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersResult {
    /// Accepted proposals (each with a ready `proposeOp`).
    pub proposals: Vec<MemberProposal>,
    /// Rejected items as `{item, reason}`.
    pub rejected: Vec<Value>,
    /// Usage reported by the model.
    pub usage: RigUsage,
    /// Model that served the request.
    pub model: String,
}

/// Input for [`narrate`].
#[derive(Debug, Clone)]
pub struct NarrativeRequest {
    /// Report title.
    pub title: String,
    /// The `analyze.findings` array.
    pub findings: Value,
    /// Optional `marginalize` pairs.
    pub priorities: Option<Value>,
    /// Maximum narrative length in words (clamped to `50..=600`).
    pub max_words: usize,
    /// Output token budget.
    pub max_tokens: u32,
}

impl Default for NarrativeRequest {
    fn default() -> Self {
        Self {
            title: String::new(),
            findings: Value::Array(Vec::new()),
            priorities: None,
            max_words: 200,
            max_tokens: 1024,
        }
    }
}

/// Output for [`narrate`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NarrativeResult {
    /// The factual narrative.
    pub narrative: String,
    /// Usage reported by the model.
    pub usage: RigUsage,
    /// Model that served the request.
    pub model: String,
}

const PROPOSE_SYSTEM: &str = "You extract structured members from a source text. \
Return ONLY a JSON array (no prose, no markdown fences). Each element is an object with \
exactly the keys \"id\" (snake_case, matching ^[a-z][a-z0-9_]{0,63}$), \"label\" (short \
human-readable name), \"description\" (optional one-sentence summary), and \"excerpt\" \
(a VERBATIM quote of at most 200 characters copied exactly from the source text that \
justifies the member). Never invent text: if you cannot quote the source, omit the member.";

const NARRATIVE_SYSTEM: &str = "You write plain, factual narrative prose about a \
cross-reference analysis. Cite member ids exactly as given. Invent no numbers beyond those \
present in the provided JSON. Do not use markdown headings or bullet points.";

/// Extract member proposals from `request.source_text`.
///
/// Deterministic post-processing is the important part: excerpts must occur
/// verbatim in the source, ids must be valid snake_case, duplicates are dropped,
/// spans are computed from the found position, and proposals are capped.
pub async fn propose_members(
    model: &dyn TextModel,
    request: &MembersRequest,
) -> Result<MembersResult, RigUnavailable> {
    let max_members = request.max_members.clamp(1, 50);
    let max_tokens = request.max_tokens.clamp(64, 4096);
    let truncated = request.source_text.chars().count() > SOURCE_CHAR_LIMIT;
    let source: String = request
        .source_text
        .chars()
        .take(SOURCE_CHAR_LIMIT)
        .collect();

    let user = build_propose_user(request, &source, max_members, truncated);
    let completion = model.complete(PROPOSE_SYSTEM, &user, max_tokens).await?;

    let items = first_json_array(&completion.text).ok_or_else(|| {
        RigUnavailable::new("bad_response", "model reply did not contain a JSON array")
    })?;

    let mut proposals: Vec<MemberProposal> = Vec::new();
    let mut rejected: Vec<Value> = Vec::new();
    let mut accepted_ids: Vec<String> = Vec::new();

    for item in items {
        let Some(object) = item.as_object() else {
            rejected.push(json!({ "item": item, "reason": "bad_response" }));
            continue;
        };
        let id = string_field(object, "id");
        let label = string_field(object, "label");
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        let excerpt = string_field(object, "excerpt");

        if !valid_id(&id) {
            rejected.push(json!({ "item": item, "reason": "bad_id" }));
            continue;
        }
        if request.existing_member_ids.iter().any(|seen| seen == &id)
            || accepted_ids.iter().any(|seen| seen == &id)
        {
            rejected.push(json!({ "item": item, "reason": "duplicate" }));
            continue;
        }
        let found = if excerpt.is_empty() {
            None
        } else {
            source.find(&excerpt)
        };
        let Some(byte_start) = found else {
            rejected.push(json!({ "item": item, "reason": "excerpt_not_found" }));
            continue;
        };
        if proposals.len() >= max_members {
            rejected.push(json!({ "item": item, "reason": "over_cap" }));
            continue;
        }

        let char_start = source[..byte_start].chars().count();
        let char_end = char_start + excerpt.chars().count();
        let label = if label.is_empty() { id.clone() } else { label };

        let mut source_ref = Map::new();
        if let Some(uri) = &request.source_uri {
            source_ref.insert("uri".to_string(), Value::String(uri.clone()));
        }
        source_ref.insert(
            "locator".to_string(),
            Value::String(format!("chars {char_start}-{char_end}")),
        );
        source_ref.insert("excerpt".to_string(), Value::String(excerpt.clone()));
        let source_ref = Value::Object(source_ref);

        let propose_op = json!({
            "kind": "member.propose",
            "dimensionId": request.dimension_id,
            "memberId": id,
            "label": label,
            "sourceRef": source_ref,
        });

        accepted_ids.push(id.clone());
        proposals.push(MemberProposal {
            id,
            label,
            description,
            source_ref,
            propose_op,
        });
    }

    Ok(MembersResult {
        proposals,
        rejected,
        usage: completion.usage,
        model: completion.model,
    })
}

/// Write a plain narrative summarizing at most the first 30 findings.
pub async fn narrate(
    model: &dyn TextModel,
    request: &NarrativeRequest,
) -> Result<NarrativeResult, RigUnavailable> {
    let max_words = request.max_words.clamp(50, 600);
    let max_tokens = request.max_tokens.max(1);
    let summary = summarize_findings(&request.findings);
    let user = build_narrative_user(request, &summary);

    let completion = model.complete(NARRATIVE_SYSTEM, &user, max_tokens).await?;
    let narrative = truncate_words(completion.text.trim(), max_words);

    Ok(NarrativeResult {
        narrative,
        usage: completion.usage,
        model: completion.model,
    })
}

fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.as_str().len() <= 63
        && chars
            .as_str()
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn build_propose_user(
    request: &MembersRequest,
    source: &str,
    max_members: usize,
    truncated: bool,
) -> String {
    let mut user = String::new();
    user.push_str(&format!("Dimension id: {}\n", request.dimension_id));
    if let Some(label) = &request.dimension_label {
        user.push_str(&format!("Dimension label: {label}\n"));
    }
    if !request.existing_member_ids.is_empty() {
        user.push_str(&format!(
            "Existing member ids to avoid: {}\n",
            request.existing_member_ids.join(", ")
        ));
    }
    user.push_str(&format!(
        "Return at most {max_members} members as a JSON array.\n"
    ));
    if truncated {
        user.push_str(&format!(
            "NOTE: the source text below was truncated to the first {SOURCE_CHAR_LIMIT} characters.\n"
        ));
    }
    user.push_str("Source text:\n<<<\n");
    user.push_str(source);
    user.push_str("\n>>>\n");
    user
}

fn build_narrative_user(request: &NarrativeRequest, summary: &Value) -> String {
    let mut user = String::new();
    user.push_str(&format!("Title: {}\n", request.title));
    if let Some(priorities) = &request.priorities {
        user.push_str(&format!(
            "Priorities: {}\n",
            serde_json::to_string(priorities).unwrap_or_else(|_| "null".to_string())
        ));
    }
    user.push_str("Findings summary (JSON):\n");
    user.push_str(&serde_json::to_string(summary).unwrap_or_else(|_| "[]".to_string()));
    user
}

fn summarize_findings(findings: &Value) -> Value {
    let empty = Vec::new();
    let array = findings.as_array().unwrap_or(&empty);
    let summarized: Vec<Value> = array
        .iter()
        .take(FINDINGS_LIMIT)
        .map(|finding| {
            let mut object = Map::new();
            for key in ["kind", "severity", "members", "net", "tension"] {
                if let Some(value) = finding.get(key) {
                    object.insert(key.to_string(), value.clone());
                }
            }
            if let Some(contributors) = finding.get("contributors").and_then(Value::as_array) {
                object.insert(
                    "contributors".to_string(),
                    Value::Array(
                        contributors
                            .iter()
                            .take(CONTRIBUTORS_LIMIT)
                            .cloned()
                            .collect(),
                    ),
                );
            }
            if let Some(explanation) = finding.get("explanation") {
                object.insert("explanation".to_string(), explanation.clone());
            }
            Value::Object(object)
        })
        .collect();
    Value::Array(summarized)
}

fn truncate_words(text: &str, max_words: usize) -> String {
    text.split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Return the first JSON array in `reply`, tolerating prose and code fences.
fn first_json_array(reply: &str) -> Option<Vec<Value>> {
    for (index, _) in reply.char_indices().filter(|(_, c)| *c == '[') {
        let mut stream = serde_json::Deserializer::from_str(&reply[index..]).into_iter::<Value>();
        if let Some(Ok(Value::Array(array))) = stream.next() {
            return Some(array);
        }
    }
    None
}

#[cfg(feature = "rig")]
mod openrouter;
#[cfg(feature = "rig")]
pub use openrouter::OpenRouterTextModel;
