//! crossmatrix-mcp — the LLM-facing MCP projection of the `crossmatrix` core
//! (ADR 0003). Two tools: `crossmatrix.command` (mutation-first writes) and
//! `crossmatrix.query` (reads/analyses). Observation-only at the boundary;
//! responses set `structured_content`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};

use crossmatrix_mcp::repo::RootSource;
use crossmatrix_mcp::session::{Rejection, Session};

type McpError = rmcp::ErrorData;

/// The handler. The command engine (and its persisted/ephemeral state) lives in
/// an `Arc<Mutex<Session>>` so a `&self` handler can serve both write and read
/// calls against the same open model. Cloning `S` shares the same session.
#[derive(Clone)]
struct S {
    session: Arc<Mutex<Session>>,
}

impl Default for S {
    fn default() -> Self {
        Self::with_session(Session::from_process())
    }
}

impl S {
    #[allow(dead_code)]
    fn new(model: crossmatrix::Model) -> Self {
        Self::with_session(Session::ephemeral(model))
    }

    #[allow(dead_code)]
    fn with_root(root: PathBuf) -> Self {
        Self::with_session(Session::new(root, RootSource::Env))
    }

    fn with_session(session: Session) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
        }
    }

    fn bad_request(message: impl Into<String>) -> Rejection {
        Rejection::new(
            "bad_request",
            message,
            None,
            vec!["model.status".to_string()],
        )
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
                 model.open, model.status, dimension.register, scale.declare, relation.declare, \
                 contraction.declare, members.sync, observe, member.propose, evidence.attach, \
                 deprecate. Rejects numeric weights + unknown tokens (fail-fast, recoverable).",
                json!({ "type": "object", "properties": { "request": { "type": "object" } }, "required": ["request"] }),
            ),
            Self::make_tool(
                "crossmatrix.query",
                "Read / analysis (engine-computed, never mutating). query{kind,...}: model.status, \
                 slice, describe, trace, explain, gaps.next, gaps.orphans, coverage, stale, \
                 conflicts, report, export, analyze.marginalize, analyze.contract, \
                 analyze.findings, validate. Optional rollup: true runs analyses over the derived \
                 rolled-up model; slice/trace/stale accept member-id or sourceRef uri lookups and \
                 limit/cursor paging (trace direction in|out|both). Reads are scoped slices.",
                json!({ "type": "object", "properties": { "request": { "type": "object" } }, "required": ["request"] }),
            ),
        ]
    }

    fn call_tool_impl(&self, name: &str, args: Value) -> Result<Value, Rejection> {
        let request = args.get("request").cloned().unwrap_or(Value::Null);
        match name {
            "crossmatrix.command" => self.session.lock().unwrap().command(&request),
            "crossmatrix.query" => {
                let session = self.session.lock().unwrap();
                crossmatrix_mcp::queries::query(&session, &request)
            }
            other => Err(Self::bad_request(format!("unknown tool: {other}"))),
        }
    }
}
impl ServerHandler for S {
    fn get_info(&self) -> ServerInfo {
        let mut result = InitializeResult::default();
        result.capabilities = ServerCapabilities::builder().enable_tools().build();
        result.instructions = Some(
            "crossmatrix-mcp: a sparse N-dimensional weighted cross-reference engine.\n\
             Tools: crossmatrix.command (writes, observation-only) and crossmatrix.query \
             (reads/analyses: model.status, slice, trace (member-id or sourceRef uri lookups; \
             direction, rollup, limit/cursor paging), describe, explain, gaps.next, gaps.orphans, \
             coverage, stale (hashes/readFiles, paging), conflicts, report, export, \
             analyze.marginalize, analyze.contract, analyze.findings, validate)."
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
            Err(rejection) => {
                let mut r = CallToolResult::error(vec![Content::text(rejection.message.clone())]);
                r.structured_content = Some(rejection.to_value());
                Ok(r)
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match crossmatrix_mcp::cli::run(
        &std::env::args().skip(1).collect::<Vec<_>>(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ) {
        crossmatrix_mcp::cli::CliAction::Exit(code) => std::process::exit(code),
        crossmatrix_mcp::cli::CliAction::Serve => {}
    }

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

    /// Arrange: an S over the split example fixtures (shared by the
    /// traceability-query dispatch tests).
    fn example_server() -> S {
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

        S::new(model)
    }

    fn query(s: &S, q: serde_json::Value) -> Result<Value, Rejection> {
        s.call_tool_impl("crossmatrix.query", json!({"request": {"query": q}}))
    }

    /// The three split example fixtures merged into the flat engine model doc
    /// that `crossmatrix.command` imports (the shape `merge()` emits).
    fn merged_example_model() -> serde_json::Value {
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
        crossmatrix_mcp::merge::merge(&config, &dims, &state).expect("merge must succeed")
    }

    /// Import the merged example model into `s` via `crossmatrix.command`
    /// (no `S::new` preload) and return the import response.
    fn import_example(s: &S) -> Value {
        s.call_tool_impl(
            "crossmatrix.command",
            json!({
                "request": {
                    "requestId": "hoq-import-1",
                    "modelId": "state_demo",
                    "model": merged_example_model(),
                }
            }),
        )
        .expect("import command must succeed")
    }

    #[test]
    fn command_import_persists_model_so_query_validate_is_true() {
        // Arrange: a fresh server with no preloaded model.
        let s = S::default();
        // Act: import the whole model via command, then validate via query.
        let _ = import_example(&s);
        let result = query(&s, json!({"kind": "validate"})).expect("validate must succeed");
        // Assert: the imported model persisted across the command→query round-trip.
        assert_eq!(
            result.pointer("/validated").and_then(|v| v.as_bool()),
            Some(true),
            "imported model must persist so a later query validates true"
        );
    }

    /// E4 — the one-call HOQ round-trip: import the whole engine model via
    /// `crossmatrix.command`, then rank the HOWs via `analyze.marginalize`. This
    /// is exactly the two-tool drive the pack capability performs, headless.
    ///
    /// Expected weighted per-HOW importance (weight × max qfd, from the fixtures):
    ///   char_response_latency = high(9) × strong(9) = 81
    ///   char_encryption       = high(9) × strong(9) = 81
    ///   char_idempotent_api   = high(9) × moderate(3) = 27
    ///   char_return_workflow  = medium(3) × strong(9) = 27
    #[test]
    fn one_call_hoq_roundtrip_ranks_high_importance_how_on_top() {
        // Arrange + Act: import then marginalize the WHAT axis of rel_req_char.
        let s = S::default();
        let _ = import_example(&s);
        let result = query(
            &s,
            json!({"kind": "analyze.marginalize", "relationId": "rel_req_char", "axis": "from"}),
        )
        .expect("marginalize must succeed");
        let findings = result
            .pointer("/findings")
            .and_then(|v| v.as_array())
            .expect("findings array");

        // The top-ranked HOW must be a high-importance-driven characteristic at 81,
        // outranking char_idempotent_api (a weakly-related HOW at 27).
        let top = &findings[0];
        let top_member = top.get("member").and_then(|v| v.as_str()).unwrap_or("");
        let top_value = top.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let idempotent = findings
            .iter()
            .find(|f| f.get("member").and_then(|v| v.as_str()) == Some("char_idempotent_api"))
            .and_then(|f| f.get("value").and_then(|v| v.as_f64()))
            .expect("char_idempotent_api must appear in the ranking");

        // Assert: the ranking is weighted-importance driven, not agent-invented.
        assert!(
            (top_member == "char_response_latency" || top_member == "char_encryption")
                && (top_value - 81.0).abs() < f64::EPSILON
                && top_value > idempotent,
            "top HOW must be a high-importance characteristic at 81 outranking the \
             weakly-related char_idempotent_api ({idempotent}); got {top_member}={top_value}"
        );
    }

    #[test]
    fn query_analyze_marginalize_returns_ranked_member_value_pairs() {
        // Arrange: import the example model (no S::new preload).
        let s = S::default();
        let _ = import_example(&s);
        // Act: the genuine per-HOW weighted rollup — roll up the WHAT axis of
        // rel_req_char; survivors are the HOWs (dim_char).
        let result = query(
            &s,
            json!({"kind": "analyze.marginalize", "relationId": "rel_req_char", "axis": "from"}),
        )
        .expect("analyze.marginalize must succeed");
        // Assert: a non-empty array of {member, value} for the HOW axis.
        let findings = result.pointer("/findings").and_then(|v| v.as_array());
        assert!(
            findings
                .map(|a| !a.is_empty()
                    && a.iter()
                        .all(|f| f.get("member").is_some() && f.get("value").is_some()))
                .unwrap_or(false),
            "analyze.marginalize must return non-empty {{member, value}} pairs"
        );
    }

    #[test]
    fn command_import_persists_model_so_analyze_contract_sees_it() {
        // Arrange: a fresh server; persistence must reach the analysis arms.
        let s = S::default();
        // Act: import, then analyze.contract — WITHOUT any S::new preload.
        let _ = import_example(&s);
        let result =
            query(&s, json!({"kind": "analyze.contract"})).expect("analyze.contract must succeed");
        // Assert: the req→fail exposure findings are computed off the imported model.
        let findings = result.pointer("/findings").and_then(|v| v.as_array());
        assert!(
            findings.map(|a| !a.is_empty()).unwrap_or(false),
            "analyze.contract must see the imported model and return findings"
        );
    }

    #[test]
    fn query_trace_returns_related_members() {
        let s = example_server();
        let result = query(&s, json!({"kind": "trace", "member": "req_secure_payment"}))
            .expect("trace must succeed");
        let hops = result.pointer("/hops").and_then(|v| v.as_array());
        assert!(
            hops.map(|a| !a.is_empty()).unwrap_or(false),
            "trace of a related member must return non-empty hops"
        );
    }

    #[test]
    fn query_trace_unknown_member_is_a_recoverable_error() {
        let s = example_server();
        assert!(
            query(&s, json!({"kind": "trace", "member": "req_nope"})).is_err(),
            "trace of an unknown member must fail fast"
        );
    }

    #[test]
    fn query_coverage_reports_uncovered_members() {
        let s = example_server();
        let result = query(&s, json!({"kind": "coverage"})).expect("coverage must succeed");
        // rel_char_exposes_fail leaves char_response_latency + char_return_workflow
        // uncovered on its from-axis, so the summary must be non-zero.
        assert!(
            result
                .pointer("/summary/uncovered")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                > 0,
            "example matrix has uncovered axis members"
        );
    }

    #[test]
    fn query_gaps_orphans_is_empty_on_fully_related_example() {
        let s = example_server();
        let result = query(&s, json!({"kind": "gaps.orphans"})).expect("orphans must succeed");
        assert_eq!(
            result
                .pointer("/orphans")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(0),
            "every member of the split example appears in a cell"
        );
    }

    #[test]
    fn query_describe_summarizes_the_model() {
        let s = example_server();
        let result = query(&s, json!({"kind": "describe"})).expect("describe must succeed");
        assert_eq!(
            result
                .pointer("/describe/dimensions")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(3),
            "describe must report the example's 3 dimensions"
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
