//! Jev (TypeSafe System One) HTTP-wire client, behind the `jev` cargo
//! feature. The provider-neutral types live in [`crate::assist`]; this
//! module owns the wire contract, config resolution and the [`HttpJudge`].

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assist::{AssistUnavailable, UnavailableReason, Usage, assist_enabled, scrub};

/// Jev's hard per-call question limit; exceeding it is a config error
/// detected before any network call.
pub const MAX_QUESTIONS_PER_CALL: usize = 50;

/// Jev's hard per-`Choice`-question option limit.
pub const MAX_CHOICE_OPTIONS: usize = 255;

const TYPESAFE_BASE: &str = "https://api.typesafe.ai";
const TYPESAFE_MODEL: &str = "jev-latest";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api";
const OPENROUTER_MODEL: &str = "~typesafe/jev-latest";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// One question in a [`JevRequest`], tagged by `type` in lowercase.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevQuestion {
    Noul {
        instructions: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

impl JevQuestion {
    /// Builds a `Choice` question, enforcing Jev's `1..=255` option rule.
    /// A violation is a request-shape bug (`BadConfig`), never retryable.
    pub fn choice(
        instructions: impl Into<String>,
        criteria: BTreeMap<String, String>,
    ) -> Result<Self, AssistUnavailable> {
        let options = criteria.len();
        if options == 0 || options > MAX_CHOICE_OPTIONS {
            return Err(AssistUnavailable::new(
                UnavailableReason::BadConfig,
                format!("choice question needs 1..={MAX_CHOICE_OPTIONS} options, got {options}"),
            ));
        }
        Ok(JevQuestion::Choice {
            instructions: instructions.into(),
            criteria,
        })
    }
}

/// Boolean criteria for a `Noul` question; serialized as `true` / `false`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: String,
    #[serde(rename = "false")]
    pub no: String,
}

/// The POST body for `/v1/systemone`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevRequest {
    pub state: Value,
    pub model: String,
    pub questions: BTreeMap<String, JevQuestion>,
}

/// One decoded answer, tagged by `type` in lowercase.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        legend: BTreeMap<String, Value>,
        #[serde(default)]
        confidence: f64,
    },
}

/// A decoded Jev reply.
#[derive(Debug, Clone, PartialEq)]
pub struct JevReply {
    pub model: String,
    pub answers: BTreeMap<String, JevAnswer>,
    pub usage: Usage,
}

/// Lenient view of the response `usage` object: token counts default to 0
/// and `cost` may be a bare number or an object carrying `total`.
#[derive(Debug, Default, Deserialize)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cost: Option<Value>,
}

impl From<WireUsage> for Usage {
    fn from(wire: WireUsage) -> Self {
        Usage {
            input_tokens: wire.input_tokens,
            output_tokens: wire.output_tokens,
            cost_usd: wire.cost.as_ref().and_then(cost_usd),
        }
    }
}

fn cost_usd(cost: &Value) -> Option<f64> {
    let value = match cost {
        Value::Number(number) => number.as_f64(),
        Value::Object(map) => map.get("total").and_then(Value::as_f64),
        _ => None,
    }?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

#[derive(Debug, Deserialize)]
struct WireReply {
    #[serde(default)]
    model: String,
    answers: BTreeMap<String, JevAnswer>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

/// Decodes a reply body, requiring the answer ids to match the question
/// ids exactly, and maps `usage` leniently.
fn decode_reply(
    body: &str,
    questions: &BTreeMap<String, JevQuestion>,
) -> Result<JevReply, AssistUnavailable> {
    let wire: WireReply = serde_json::from_str(body).map_err(|error| {
        AssistUnavailable::new(
            UnavailableReason::BadResponse,
            format!("invalid reply json: {error}"),
        )
    })?;
    let ids_match = wire.answers.len() == questions.len()
        && questions.keys().all(|id| wire.answers.contains_key(id));
    if !ids_match {
        return Err(AssistUnavailable::new(
            UnavailableReason::BadResponse,
            "reply answer ids do not match question ids",
        ));
    }
    Ok(JevReply {
        model: wire.model,
        answers: wire.answers,
        usage: wire.usage.map(Usage::from).unwrap_or_default(),
    })
}

/// Which vendor contract a [`JevConfig`] targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JevProvider {
    Typesafe,
    Openrouter,
}

/// A credential that never prints itself. No `Display` and no `Serialize`
/// on purpose; only the HTTP layer may read it back via `expose`.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Resolved Jev endpoint, model, credential and timeout.
#[derive(Debug, Clone)]
pub struct JevConfig {
    pub endpoint: String,
    pub model: String,
    key: ApiKey,
    pub timeout: Duration,
    pub provider: JevProvider,
}

impl JevConfig {
    /// Resolves config from an injectable env-style lookup. See the module
    /// contract for precedence and validation rules.
    pub fn from_lookup(
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<JevConfig, AssistUnavailable> {
        if !assist_enabled(lookup, "jev") {
            return Err(AssistUnavailable::new(
                UnavailableReason::Disabled,
                "assist disabled; CROSSMATRIX_ASSIST must include 'jev'",
            ));
        }

        let (provider, default_base, default_model, key) =
            match non_blank(lookup("TYPESAFE_JEV_API_KEY")) {
                Some(key) => (JevProvider::Typesafe, TYPESAFE_BASE, TYPESAFE_MODEL, key),
                None => match non_blank(lookup("OPENROUTER_API_KEY")) {
                    Some(key) => (
                        JevProvider::Openrouter,
                        OPENROUTER_BASE,
                        OPENROUTER_MODEL,
                        key,
                    ),
                    None => {
                        return Err(AssistUnavailable::new(
                            UnavailableReason::NoKey,
                            "neither TYPESAFE_JEV_API_KEY nor OPENROUTER_API_KEY is set",
                        ));
                    }
                },
            };

        let base = match non_blank(lookup("CROSSMATRIX_JEV_BASE_URL")) {
            Some(value) => {
                if !valid_base(&value) {
                    return Err(AssistUnavailable::new(
                        UnavailableReason::BadConfig,
                        "CROSSMATRIX_JEV_BASE_URL must be https:// or loopback http://",
                    ));
                }
                value
            }
            None => default_base.to_string(),
        };

        let model = match non_blank(lookup("CROSSMATRIX_JEV_MODEL")) {
            Some(value) => {
                if !valid_model(&value) {
                    return Err(AssistUnavailable::new(
                        UnavailableReason::BadConfig,
                        "CROSSMATRIX_JEV_MODEL must be 1..=128 of [A-Za-z0-9._:/~-]",
                    ));
                }
                value
            }
            None => default_model.to_string(),
        };

        let timeout = match non_blank(lookup("CROSSMATRIX_JEV_TIMEOUT_SECS")) {
            Some(value) => match value.parse::<u64>() {
                Ok(seconds) if (1..=300).contains(&seconds) => Duration::from_secs(seconds),
                _ => {
                    return Err(AssistUnavailable::new(
                        UnavailableReason::BadConfig,
                        "CROSSMATRIX_JEV_TIMEOUT_SECS must be an integer in 1..=300",
                    ));
                }
            },
            None => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };

        let endpoint = format!("{}/v1/systemone", base.trim_end_matches('/'));

        Ok(JevConfig {
            endpoint,
            model,
            key: ApiKey::new(key),
            timeout,
            provider,
        })
    }

    /// [`JevConfig::from_lookup`] over the process environment.
    pub fn from_env() -> Result<JevConfig, AssistUnavailable> {
        Self::from_lookup(&|key| std::env::var(key).ok())
    }
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|candidate| !candidate.trim().is_empty())
}

fn valid_base(base: &str) -> bool {
    if base.starts_with("https://") {
        return true;
    }
    ["http://127.0.0.1", "http://localhost"]
        .iter()
        .any(|prefix| match base.strip_prefix(prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'),
            None => false,
        })
}

fn valid_model(model: &str) -> bool {
    let length = model.chars().count();
    (1..=128).contains(&length)
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/~-".contains(c))
}

fn excerpt(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// The provider-neutral judgement interface.
#[async_trait]
pub trait Judge: Send + Sync {
    async fn ask(
        &self,
        state: Value,
        questions: BTreeMap<String, JevQuestion>,
    ) -> Result<JevReply, AssistUnavailable>;
}

/// HTTP [`Judge`] over Jev's `/v1/systemone`. One pooled [`reqwest::Client`]
/// is reused for every call.
pub struct HttpJudge {
    http: reqwest::Client,
    config: JevConfig,
}

impl HttpJudge {
    /// Builds a judge from resolved config, with a 10 s connect timeout on a
    /// single reused client.
    pub fn new(config: JevConfig) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { http, config }
    }

    fn validate(
        state: &Value,
        questions: &BTreeMap<String, JevQuestion>,
    ) -> Result<(), AssistUnavailable> {
        if questions.len() > MAX_QUESTIONS_PER_CALL {
            return Err(AssistUnavailable::new(
                UnavailableReason::BadConfig,
                format!(
                    "at most {MAX_QUESTIONS_PER_CALL} questions per call, got {}",
                    questions.len()
                ),
            ));
        }
        if !(state.is_object() || state.is_string()) {
            return Err(AssistUnavailable::new(
                UnavailableReason::BadConfig,
                "state must be a JSON object or string",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl Judge for HttpJudge {
    async fn ask(
        &self,
        state: Value,
        questions: BTreeMap<String, JevQuestion>,
    ) -> Result<JevReply, AssistUnavailable> {
        Self::validate(&state, &questions)?;

        let request = JevRequest {
            state,
            model: self.config.model.clone(),
            questions,
        };
        let body = serde_json::to_string(&request).map_err(|error| {
            AssistUnavailable::new(
                UnavailableReason::BadConfig,
                format!("request encoding failed: {error}"),
            )
        })?;
        let key = self.config.key.expose().to_string();
        let endpoint = self.config.endpoint.clone();
        let http = self.http.clone();

        let call = async move {
            let response = http
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|error| {
                    AssistUnavailable::new(
                        UnavailableReason::Network,
                        scrub(&format!("transport error: {error}"), &key),
                    )
                })?;

            let status = response.status();
            if status.is_success() {
                let text = response.text().await.map_err(|error| {
                    AssistUnavailable::new(
                        UnavailableReason::Network,
                        scrub(&format!("transport error: {error}"), &key),
                    )
                })?;
                return decode_reply(&text, &request.questions);
            }

            let code = status.as_u16();
            let text = response.text().await.unwrap_or_default();
            let body_excerpt = scrub(&excerpt(&text, 200), &key);
            let reason = if (400..=499).contains(&code) {
                UnavailableReason::Http4xx
            } else if (500..=599).contains(&code) {
                UnavailableReason::Http5xx
            } else {
                UnavailableReason::BadResponse
            };
            Err(AssistUnavailable::new(
                reason,
                format!("status {code}: {body_excerpt}"),
            ))
        };

        match tokio::time::timeout(self.config.timeout, call).await {
            Ok(result) => result,
            Err(_) => Err(AssistUnavailable::new(
                UnavailableReason::Timeout,
                "request timed out",
            )),
        }
    }
}

/// Deterministic in-memory [`Judge`] for tests and downstream fakes.
#[cfg(any(test, feature = "test-fakes"))]
pub mod fake {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::Value;

    use super::{JevAnswer, JevQuestion, JevReply, Judge};
    use crate::assist::{AssistUnavailable, UnavailableReason, Usage};

    type Responder = Box<dyn Fn(&str, &JevQuestion) -> JevAnswer + Send + Sync>;

    /// Records every call and answers from an injected closure.
    pub struct FakeJudge {
        responder: Option<Responder>,
        failure: Option<UnavailableReason>,
        calls: AtomicUsize,
        states: Mutex<Vec<Value>>,
    }

    impl FakeJudge {
        /// A judge that answers each question with `responder(id, question)`.
        pub fn answering(
            responder: impl Fn(&str, &JevQuestion) -> JevAnswer + Send + Sync + 'static,
        ) -> Self {
            Self {
                responder: Some(Box::new(responder)),
                failure: None,
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
            }
        }

        /// A judge that fails every call with `reason`.
        pub fn failing(reason: UnavailableReason) -> Self {
            Self {
                responder: None,
                failure: Some(reason),
                calls: AtomicUsize::new(0),
                states: Mutex::new(Vec::new()),
            }
        }

        /// Number of calls observed.
        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// States sent, in call order.
        pub fn states(&self) -> Vec<Value> {
            self.states.lock().expect("fake states mutex").clone()
        }
    }

    #[async_trait]
    impl Judge for FakeJudge {
        async fn ask(
            &self,
            state: Value,
            questions: BTreeMap<String, JevQuestion>,
        ) -> Result<JevReply, AssistUnavailable> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.states.lock().expect("fake states mutex").push(state);
            if let Some(reason) = self.failure {
                return Err(AssistUnavailable::new(reason, "fake judge failure"));
            }
            let responder = self
                .responder
                .as_ref()
                .expect("answering fake must have a responder");
            let answers = questions
                .iter()
                .map(|(id, question)| (id.clone(), responder(id, question)))
                .collect();
            Ok(JevReply {
                model: "fake/jev".to_string(),
                answers,
                usage: Usage {
                    input_tokens: questions.len() as u64,
                    output_tokens: 0,
                    cost_usd: None,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_constructor_accepts_a_single_option() {
        let criteria = BTreeMap::from([("only".to_string(), "d".to_string())]);
        assert!(JevQuestion::choice("pick one", criteria).is_ok());
    }

    #[test]
    fn choice_constructor_rejects_zero_options() {
        let err = JevQuestion::choice("pick one", BTreeMap::new()).expect_err("zero must fail");
        assert_eq!(err.reason, UnavailableReason::BadConfig);
    }

    #[test]
    fn choice_constructor_rejects_more_than_255_options() {
        let criteria: BTreeMap<String, String> = (0..256)
            .map(|index| (format!("o{index}"), "d".to_string()))
            .collect();
        let err = JevQuestion::choice("pick one", criteria).expect_err("too many must fail");
        assert_eq!(err.reason, UnavailableReason::BadConfig);
    }
}
