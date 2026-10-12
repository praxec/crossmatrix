//! The `rig`-feature OpenRouter [`TextModel`] implementation.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use rig_core::client::CompletionClient;
use rig_core::completion::{CompletionError, CompletionModel};
use rig_core::http_client::Error as HttpError;
use rig_core::message::{AssistantContent, Message};
use rig_core::providers::openrouter;
use serde_json::{Map, Value, json};

use super::{Completion, RigUnavailable, RigUsage, TextModel};

/// Default OpenRouter model when `CROSSMATRIX_RIG_MODEL` is unset.
pub const DEFAULT_MODEL: &str = "openai/gpt-5-mini";
/// Default request timeout in seconds when `CROSSMATRIX_RIG_TIMEOUT_SECS` is unset.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 300;
const MAX_MODEL_CHARS: usize = 128;

/// A [`TextModel`] backed by rig-core's OpenRouter completion client.
pub struct OpenRouterTextModel {
    client: openrouter::Client,
    model: String,
    timeout: Duration,
    api_key: String,
}

impl OpenRouterTextModel {
    /// Build the model from a name→value lookup (typically the process env).
    ///
    /// Requires `CROSSMATRIX_ASSIST` to enable the assist (`rig`, `all`, or `1`)
    /// and `OPENROUTER_API_KEY`. `CROSSMATRIX_RIG_MODEL` (default
    /// [`DEFAULT_MODEL`]) and `CROSSMATRIX_RIG_TIMEOUT_SECS` (default
    /// [`DEFAULT_TIMEOUT_SECS`], `1..=300`) are validated.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self, RigUnavailable> {
        let assist = lookup("CROSSMATRIX_ASSIST").unwrap_or_default();
        if !assist_enabled(&assist) {
            return Err(RigUnavailable::new(
                "disabled",
                "CROSSMATRIX_ASSIST does not enable the rig assist",
            ));
        }

        let key = lookup("OPENROUTER_API_KEY")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| RigUnavailable::new("no_key", "OPENROUTER_API_KEY is not set"))?;

        let model = lookup("CROSSMATRIX_RIG_MODEL")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !valid_model(&model) {
            return Err(RigUnavailable::new(
                "bad_config",
                format!("CROSSMATRIX_RIG_MODEL is invalid: {model}"),
            ));
        }

        let timeout_secs = match lookup("CROSSMATRIX_RIG_TIMEOUT_SECS")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            None => DEFAULT_TIMEOUT_SECS,
            Some(raw) => match raw.parse::<u64>() {
                Ok(secs) if (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs) => secs,
                _ => {
                    return Err(RigUnavailable::new(
                        "bad_config",
                        format!("CROSSMATRIX_RIG_TIMEOUT_SECS is invalid: {raw}"),
                    ));
                }
            },
        };

        let client = openrouter::Client::builder()
            .api_key(key.clone())
            .build()
            .map_err(|error| RigUnavailable::new("bad_config", scrub(&error.to_string(), &key)))?;

        Ok(Self {
            client,
            model,
            timeout: Duration::from_secs(timeout_secs),
            api_key: key,
        })
    }

    /// Build the model from the process environment.
    pub fn from_env() -> Result<Self, RigUnavailable> {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// The configured model id.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn map_error(&self, error: CompletionError) -> RigUnavailable {
        let (reason, detail) = match error {
            CompletionError::HttpError(HttpError::InvalidStatusCodeWithMessage(status, body)) => {
                (status_reason(status.as_u16()), body)
            }
            CompletionError::HttpError(HttpError::InvalidStatusCode(status)) => {
                (status_reason(status.as_u16()), status.to_string())
            }
            CompletionError::HttpError(other) => ("network".to_string(), other.to_string()),
            CompletionError::JsonError(error) => ("bad_response".to_string(), error.to_string()),
            CompletionError::ResponseError(detail) => ("bad_response".to_string(), detail),
            CompletionError::ProviderError(detail) => ("bad_response".to_string(), detail),
            CompletionError::UrlError(error) => ("bad_config".to_string(), error.to_string()),
            CompletionError::RequestError(error) => ("bad_config".to_string(), error.to_string()),
        };
        RigUnavailable::new(reason, scrub(&detail, &self.api_key))
    }
}

impl std::fmt::Debug for OpenRouterTextModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenRouterTextModel")
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl TextModel for OpenRouterTextModel {
    fn complete(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Completion, RigUnavailable>> + Send + '_>> {
        let system = system.to_string();
        let user = user.to_string();
        Box::pin(async move {
            let model = self.client.completion_model(self.model.clone());

            // rig-core 0.39 drops `max_tokens` for OpenRouter, so carry it through
            // the additional-parameter wire path as well.
            let mut params = Map::new();
            params.insert("max_tokens".to_string(), json!(max_tokens));

            let request = model
                .completion_request(Message::user(user))
                .preamble(system)
                .max_tokens(u64::from(max_tokens))
                .additional_params(Value::Object(params));

            let response = match tokio::time::timeout(self.timeout, request.send()).await {
                Err(_) => {
                    return Err(RigUnavailable::new(
                        "timeout",
                        format!("completion exceeded {}s", self.timeout.as_secs()),
                    ));
                }
                Ok(Err(error)) => return Err(self.map_error(error)),
                Ok(Ok(response)) => response,
            };

            let text: String = response
                .choice
                .iter()
                .filter_map(|content| match content {
                    AssistantContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                return Err(RigUnavailable::new(
                    "bad_response",
                    "completion contained no text",
                ));
            }

            let raw = serde_json::to_value(&response.raw_response)
                .map_err(|error| RigUnavailable::new("bad_response", error.to_string()))?;
            let usage = RigUsage {
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
                cost_usd: raw
                    .pointer("/usage/cost")
                    .and_then(Value::as_f64)
                    .filter(|cost| cost.is_finite()),
            };

            Ok(Completion {
                text,
                usage,
                model: self.model.clone(),
            })
        })
    }
}

fn status_reason(status: u16) -> String {
    if (400..500).contains(&status) {
        "http_4xx".to_string()
    } else if (500..600).contains(&status) {
        "http_5xx".to_string()
    } else {
        "network".to_string()
    }
}

fn assist_enabled(value: &str) -> bool {
    value
        .split([',', ';', ' ', '\t', '\n'])
        .any(|token| matches!(token.trim(), "rig" | "all" | "1"))
}

fn valid_model(model: &str) -> bool {
    let length = model.chars().count();
    (1..=MAX_MODEL_CHARS).contains(&length)
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '~' | '-'))
}

fn scrub(detail: &str, key: &str) -> String {
    if key.is_empty() {
        detail.to_string()
    } else {
        detail.replace(key, "[redacted]")
    }
}
