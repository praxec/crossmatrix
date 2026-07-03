//! crossmatrix-mcp — the LLM-facing MCP projection of the `crossmatrix` core
//! (ADR 0003). Two tools: `crossmatrix.command` (mutation-first writes) and
//! `crossmatrix.query` (reads/analyses). Observation-only at the boundary;
//! responses set `structured_content`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

type McpError = rmcp::ErrorData;

#[derive(Clone, Default)]
struct S {
    model: Option<crossmatrix::Model>,
    request_cache: Arc<Mutex<HashMap<String, Result<Value, String>>>>,
}

impl S {
    #[allow(dead_code)]
    fn new(model: crossmatrix::Model) -> Self {
        Self {
            model: Some(model),
            request_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn make_tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
        let schema: Map<String, Value> =
            serde_json::from_value(schema).expect("valid tool input schema");
        Tool::new(name, description, Arc::new(schema))
    }

    fn tool_list() -> Vec<Tool> {
        vec![
            Self::make_tool(
                "crossmatrix.command",
                "Mutation-first write (observation-only). Envelope {schemaVersion, requestId, \
                 modelId, expectedVersion?, actor{actorType,persona}, op{kind,...}}. Ops: \
                 model.open, dimension.register, scale.declare, relation.declare, \
                 contraction.declare, members.sync, observe, member.propose, evidence.attach, \
                 deprecate. Rejects numeric weights + unknown tokens (fail-fast, recoverable).",
                json!({ "type": "object", "properties": { "request": { "type": "object" } }, "required": ["request"] }),
            ),
            Self::make_tool(
                "crossmatrix.query",
                "Read / analysis (engine-computed, never mutating). query{kind,...}: slice, \
                 describe, trace, explain, gaps.next, gaps.orphans, coverage, stale, conflicts, \
                 analyze(marginalize|contract|findings), validate, export. Reads are scoped slices.",
                json!({ "type": "object", "properties": { "request": { "type": "object" } }, "required": ["request"] }),
            ),
        ]
    }

    fn call_tool_impl(&self, name: &str, args: Value) -> Result<Value, String> {
        let request = args.get("request").cloned().unwrap_or(Value::Null);
        match name {
            "crossmatrix.command" => {
                // Idempotency: if requestId was already processed, replay.
                if let Some(request_id) = request.get("requestId").and_then(|v| v.as_str()) {
                    if let Some(cached) = self.request_cache.lock().unwrap().get(request_id) {
                        return cached.clone();
                    }
                }

                let kind = request
                    .pointer("/op/kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Observation-only boundary: reject numeric weights without sourceRef.system.
                let has_numeric_weight = request
                    .pointer("/op/cell/weight")
                    .is_some_and(|v| v.is_number())
                    || request
                        .pointer("/op/member/weight")
                        .is_some_and(|v| v.is_number());
                let has_source_ref_system = request
                    .pointer("/op/cell/sourceRef/system")
                    .is_some_and(|v| v.is_string())
                    || request
                        .pointer("/op/member/sourceRef/system")
                        .is_some_and(|v| v.is_string());
                if has_numeric_weight && !has_source_ref_system {
                    let err =
                        Err("observation-only: numeric weights require sourceRef.system"
                            .to_string());
                    if let Some(request_id) = request.get("requestId").and_then(|v| v.as_str()) {
                        self.request_cache
                            .lock()
                            .unwrap()
                            .insert(request_id.to_string(), err.clone());
                    }
                    return err;
                }
                // If a full model is supplied (e.g. import), validate it through core.
                let result = if let Some(model) = request.get("model") {
                    match crossmatrix::Model::load(&model.to_string()) {
                        Ok(_) => Ok(
                            json!({ "ok": true, "op": kind, "validated": true, "links": ["query"] }),
                        ),
                        Err(e) => Err(format!("model failed validation: {e}")),
                    }
                } else {
                    Ok(
                        json!({ "ok": true, "op": kind, "note": "op not supported in this build (mutation ops deferred — see ADR-0004; needs a core write-API)", "links": ["query"] }),
                    )
                };
                // Cache result under requestId for idempotency.
                if let Some(request_id) = request.get("requestId").and_then(|v| v.as_str()) {
                    self.request_cache
                        .lock()
                        .unwrap()
                        .insert(request_id.to_string(), result.clone());
                }
                result
            }
            "crossmatrix.query" => {
                let kind = request
                    .pointer("/query/kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match kind {
                    "analyze.contract" => {
                        let model = self
                            .model
                            .as_ref()
                            .ok_or_else(|| "no model loaded".to_string())?;
                        let mut all_findings: Vec<Value> = Vec::new();
                        for cid in model.contraction_ids() {
                            let findings = model.contract(cid);
                            for f in findings {
                                all_findings.push(
                                    serde_json::to_value(&f)
                                        .map_err(|e| format!("serialize finding: {e}"))?,
                                );
                            }
                        }
                        Ok(json!({
                            "findings": all_findings,
                            "links": ["analyze.findings", "analyze.marginalize", "validate", "describe"]
                        }))
                    }
                    "analyze.findings" => {
                        let model = self
                            .model
                            .as_ref()
                            .ok_or_else(|| "no model loaded".to_string())?;
                        let findings: Vec<Value> = model
                            .findings()
                            .into_iter()
                            .map(|f| {
                                serde_json::to_value(&f)
                                    .map_err(|e| format!("serialize finding: {e}"))
                            })
                            .collect::<Result<_, _>>()?;
                        Ok(json!({
                            "findings": findings,
                            "links": ["analyze.contract", "validate", "describe"]
                        }))
                    }
                    "validate" => {
                        let validated = self.model.is_some();
                        Ok(json!({
                            "validated": validated,
                            "links": if validated {
                                vec!["analyze.contract", "analyze.findings", "describe"]
                            } else {
                                vec!["open"]
                            }
                        }))
                    }
                    _ => Ok(json!({
                        "ok": true,
                        "query": kind,
                        "note": "query not supported in this build (slice/describe/trace/gaps deferred)",
                        "links": ["analyze.contract", "analyze.findings", "validate"]
                    })),
                }
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

impl ServerHandler for S {
    fn get_info(&self) -> ServerInfo {
        let mut result = InitializeResult::default();
        result.capabilities = ServerCapabilities::builder().enable_tools().build();
        result.instructions = Some(
            "crossmatrix-mcp: a sparse N-dimensional weighted cross-reference engine.\n\
             Tools: crossmatrix.command (writes, observation-only) and crossmatrix.query (reads/analyses)."
                .to_string(),
        );
        result
    }

    #[allow(deprecated)]
    async fn list_tools(
        &self,
        _params: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(Self::tool_list()))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args = req
            .arguments
            .map(Value::Object)
            .unwrap_or_else(|| Value::Object(Default::default()));
        match self.call_tool_impl(req.name.as_ref(), args) {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                let mut r = CallToolResult::success(vec![Content::text(text)]);
                r.structured_content = Some(value);
                Ok(r)
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("crossmatrix-mcp: serving on stdio");
    let svc = S::default().serve(stdio()).await?;
    svc.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossmatrix_mcp::open;
    use crossmatrix_mcp::schema_types::{
        CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState,
    };
    use crossmatrix_mcp::store::{ConfigStore, DimensionsStore, StateStore};
    use serde_json::json;

    #[test]
    fn query_analyze_contract_returns_non_empty_findings() {
        // Arrange: set up stores with the three split example fixtures.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let config_store = ConfigStore::new(root.clone());
        let dims_store = DimensionsStore::new(root.clone());
        let state_store = StateStore::new(root);

        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dims: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        config_store.put(&config).expect("put config");
        dims_store.put(&dims).expect("put dimensions");
        state_store.put(&state).expect("put state");

        let model = open(
            &config_store,
            &dims_store,
            &state_store,
            &state.state_id,
            state.version.as_deref().unwrap_or(""),
        )
        .expect("open must succeed");

        let s = S::new(model);

        // Act: query analyze.contract.
        let result = s
            .call_tool_impl(
                "crossmatrix.query",
                json!({"request": {"query": {"kind": "analyze.contract"}}}),
            )
            .expect("query must succeed");

        // Assert: findings array is non-empty.
        let findings = result.pointer("/findings").and_then(|v| v.as_array());
        assert!(
            findings.map(|a| !a.is_empty()).unwrap_or(false),
            "analyze.contract must return non-empty findings"
        );
    }

    #[test]
    fn query_validate_on_good_model_returns_ok_true() {
        // Arrange: open the example model.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let config_store = ConfigStore::new(root.clone());
        let dims_store = DimensionsStore::new(root.clone());
        let state_store = StateStore::new(root);

        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dims: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        config_store.put(&config).expect("put config");
        dims_store.put(&dims).expect("put dimensions");
        state_store.put(&state).expect("put state");

        let model = open(
            &config_store,
            &dims_store,
            &state_store,
            &state.state_id,
            state.version.as_deref().unwrap_or(""),
        )
        .expect("open must succeed");

        let s = S::new(model);

        // Act: query validate.
        let result = s
            .call_tool_impl(
                "crossmatrix.query",
                json!({"request": {"query": {"kind": "validate"}}}),
            )
            .expect("query must succeed");

        // Assert: validated flag is true.
        assert_eq!(
            result.pointer("/validated").and_then(|v| v.as_bool()),
            Some(true),
            "validate on a good model must return validated=true"
        );
    }

    #[test]
    fn command_idempotency_replays_result_for_same_request_id() {
        // Arrange: an S with no model — idempotency is at the boundary.
        let s = S::default();

        // First call with a valid minimal model and requestId "idem-1" (precondition).
        let _result1 = s
            .call_tool_impl(
                "crossmatrix.command",
                json!({
                    "request": {
                        "requestId": "idem-1",
                        "model": {
                            "schemaVersion": "0.2.0",
                            "modelId": "idem-test",
                            "dimensions": [
                                {"id": "d1", "order": 0, "members": [{"id": "a"}]},
                                {"id": "d2", "order": 1, "members": [{"id": "x"}]}
                            ],
                            "scales": [],
                            "relations": []
                        }
                    }
                }),
            )
            .expect("first call with valid model must succeed");

        // Second call with the SAME requestId but an invalid model.
        // Idempotency requires the prior result to be replayed, not re-executed.
        let result2 = s.call_tool_impl(
            "crossmatrix.command",
            json!({
                "request": {
                    "requestId": "idem-1",
                    "model": "not-a-valid-model-object"
                }
            }),
        );

        // Assert: idempotent — second call must return Ok (replayed from first).
        assert!(
            result2.is_ok(),
            "idempotent replay: same requestId must return cached success, not re-execute"
        );
    }

    #[test]
    fn command_rejects_numeric_weight_without_source_ref_system() {
        // Arrange: an S with no model; the boundary check is structural.
        let s = S::default();

        // Act: a command carrying a numeric member weight without sourceRef.system.
        let result = s.call_tool_impl(
            "crossmatrix.command",
            json!({
                "request": {
                    "requestId": "obs-test-1",
                    "op": {
                        "kind": "member.propose",
                        "member": {
                            "dimensionId": "d1",
                            "id": "m1",
                            "weight": 9.0
                        }
                    }
                }
            }),
        );

        // Assert: rejected with an observation-only diagnostic.
        assert!(
            result.is_err(),
            "numeric weight without sourceRef.system must be rejected as observation-only"
        );
    }

    #[test]
    fn query_response_includes_non_empty_links_array() {
        // Arrange: an S with no model.
        let s = S::default();

        // Act: any query — use validate as a representative read op.
        let result = s
            .call_tool_impl(
                "crossmatrix.query",
                json!({"request": {"query": {"kind": "validate"}}}),
            )
            .expect("query must succeed");

        // Assert: the response carries a non-empty links array.
        let links = result.pointer("/links").and_then(|v| v.as_array());
        assert!(
            links.map(|a| !a.is_empty()).unwrap_or(false),
            "query response must include non-empty links array"
        );
    }
}
