//! End-to-end acceptance tests: rebuild `examples/travel-mug-3axis.json` purely
//! through the `crossmatrix.command` ops of a real `crossmatrix-mcp` process over
//! stdio JSON-RPC, restart, and compare findings against the legacy import.
//!
//! Each test makes exactly one behavioral assertion. Shared helpers drive the
//! public MCP boundary (spawn the binary, send JSON-RPC, read structured content).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Map, Value, json};

/// A synchronous MCP stdio client over one `crossmatrix-mcp` process.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpServer {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_crossmatrix-mcp"))
            .env("CROSSMATRIX_DIR", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn crossmatrix-mcp");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        let mut server = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let init = server.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "e2e-travel-mug", "version": "1.0.0" },
            }),
        );
        assert!(
            init.get("result").is_some(),
            "initialize must return a result: {init}"
        );
        server.notify("notifications/initialized", json!({}));
        server
    }

    fn notify(&mut self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.write_line(&message);
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_line(&message);
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read stdout");
            if read == 0 {
                panic!("server closed stdout before responding to {method}");
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed)
                .unwrap_or_else(|e| panic!("invalid JSON-RPC line ({e}): {trimmed}"));
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
        }
    }

    fn write_line(&mut self, value: &Value) {
        self.stdin
            .write_all(value.to_string().as_bytes())
            .expect("write stdin");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush stdin");
    }

    /// Send a tool call and return its `result` object.
    fn call(&mut self, tool: &str, request: Value) -> Value {
        let response = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": { "request": request } }),
        );
        response
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("tools/call returned no result: {response}"))
    }

    /// The `structuredContent` of a tool call (never null for our tools).
    fn structured(&mut self, tool: &str, request: Value) -> Value {
        self.call(tool, request)
            .get("structuredContent")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A command that must succeed; panics with the rejection otherwise.
    fn command_ok(&mut self, request: Value) -> Value {
        let structured = self.structured("crossmatrix.command", request);
        assert_eq!(
            structured.get("ok").and_then(Value::as_bool),
            Some(true),
            "command must succeed: {structured}"
        );
        structured
    }

    fn command_raw(&mut self, request: Value) -> Value {
        self.structured("crossmatrix.command", request)
    }

    fn query(&mut self, query: Value) -> Value {
        self.structured("crossmatrix.query", json!({ "query": query }))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const EXAMPLE: &str = include_str!("../../../examples/travel-mug-3axis.json");

/// A uniquely-keyed `crossmatrix.command` envelope.
fn envelope(request_id: &str, op: Value) -> Value {
    json!({
        "schemaVersion": "0.2.0",
        "requestId": request_id,
        "modelId": "travel-mug",
        "actor": { "actorType": "llm", "persona": "model_admin" },
        "op": op,
    })
}

fn next_id(seq: &mut usize) -> String {
    *seq += 1;
    format!("tm-{seq:03}")
}

fn dimension_op(dim: &Value) -> Value {
    let mut op = Map::new();
    op.insert("kind".to_string(), json!("dimension.register"));
    op.insert("dimensionId".to_string(), dim["id"].clone());
    op.insert("dimensionKind".to_string(), dim["kind"].clone());
    for key in ["label", "system", "weightScale", "weightSemantics", "order"] {
        if let Some(value) = dim.get(key) {
            op.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(op)
}

fn relation_op(relation: &Value) -> Value {
    let mut op = Map::new();
    op.insert("kind".to_string(), json!("relation.declare"));
    op.insert("relationId".to_string(), relation["id"].clone());
    op.insert("from".to_string(), relation["from"].clone());
    op.insert("to".to_string(), relation["to"].clone());
    op.insert("relationType".to_string(), relation["relationType"].clone());
    op.insert("scale".to_string(), relation["scale"].clone());
    if let Some(policy) = relation.get("coveragePolicy") {
        op.insert("coveragePolicy".to_string(), policy.clone());
    }
    Value::Object(op)
}

fn contraction_op(contraction: &Value) -> Value {
    let mut op = Map::new();
    op.insert("kind".to_string(), json!("contraction.declare"));
    op.insert("contractionId".to_string(), contraction["id"].clone());
    op.insert("chain".to_string(), contraction["chain"].clone());
    for key in ["label", "weightCombination", "weightBySourceMembers"] {
        if let Some(value) = contraction.get(key) {
            op.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(op)
}

fn observe_op(relation: &Value, cell: &Value, observation: &Value) -> Value {
    let mut op = observation.as_object().cloned().unwrap_or_default();
    op.insert("kind".to_string(), json!("observe"));
    op.insert("relationId".to_string(), relation["id"].clone());
    op.insert("from".to_string(), cell["from"].clone());
    op.insert("to".to_string(), cell["to"].clone());
    Value::Object(op)
}

/// What a full travel-mug build did, for the atomic assertions.
struct BuildReport {
    /// Accepted commands, including `model.open`.
    commands: usize,
    /// Accepted mutations (everything but `model.open`).
    mutations: u64,
    /// A recorded mutation request, replayable by id in a later process.
    replay_request: Value,
    replay_version: u64,
}

/// Drive a full travel-mug House of Quality through `crossmatrix.command`.
fn build_travel_mug(server: &mut McpServer) -> BuildReport {
    let example: Value = serde_json::from_str(EXAMPLE).expect("example JSON");
    let mut seq = 0usize;
    let mut commands = 0usize;
    let mut mutations: u64 = 0;
    let mut replay_request = Value::Null;
    let mut replay_version: u64 = 0;

    // model.open
    let open_id = next_id(&mut seq);
    let open = server.command_ok(envelope(
        &open_id,
        json!({ "kind": "model.open", "stateId": "travel-mug" }),
    ));
    assert_eq!(open.get("created").and_then(Value::as_bool), Some(true));
    commands += 1;

    // scale.declare x2 (in example order)
    for scale in example["scales"].as_array().expect("scales array") {
        let id = next_id(&mut seq);
        server.command_ok(envelope(
            &id,
            json!({ "kind": "scale.declare", "scale": scale }),
        ));
        commands += 1;
        mutations += 1;
    }

    // dimension.register x3 (in example order)
    for dim in example["dimensions"].as_array().expect("dimensions array") {
        let id = next_id(&mut seq);
        server.command_ok(envelope(&id, dimension_op(dim)));
        commands += 1;
        mutations += 1;
    }

    // members.sync x3 (example rosters)
    for dim in example["dimensions"].as_array().expect("dimensions array") {
        let id = next_id(&mut seq);
        server.command_ok(envelope(
            &id,
            json!({
                "kind": "members.sync",
                "dimensionId": dim["id"],
                "members": dim["members"],
            }),
        ));
        commands += 1;
        mutations += 1;
    }

    // relation.declare x4 (in example order)
    for relation in example["relations"].as_array().expect("relations array") {
        let id = next_id(&mut seq);
        server.command_ok(envelope(&id, relation_op(relation)));
        commands += 1;
        mutations += 1;
    }

    // observe for every observation of every cell, in example order
    for relation in example["relations"].as_array().expect("relations array") {
        for cell in relation["cells"].as_array().expect("cells array") {
            for observation in cell["observations"].as_array().expect("observations array") {
                let id = next_id(&mut seq);
                let request = envelope(&id, observe_op(relation, cell, observation));
                let response = server.command_ok(request.clone());
                if replay_request.is_null() {
                    replay_request = request;
                    replay_version = response
                        .get("version")
                        .and_then(Value::as_u64)
                        .expect("observe version");
                }
                commands += 1;
                mutations += 1;
            }
        }
    }

    // contraction.declare x1
    for contraction in example["contractions"]
        .as_array()
        .expect("contractions array")
    {
        let id = next_id(&mut seq);
        server.command_ok(envelope(&id, contraction_op(contraction)));
        commands += 1;
        mutations += 1;
    }

    BuildReport {
        commands,
        mutations,
        replay_request,
        replay_version,
    }
}

fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(".crossmatrix");
    (tmp, root)
}

// ── the restart e2e ──

#[test]
fn restarting_a_command_built_travel_mug_matches_the_legacy_import_findings() {
    // Process 1: build the model purely through crossmatrix.command ops.
    let (_tmp, root) = temp_root();
    let mut first = McpServer::start(&root);
    build_travel_mug(&mut first);
    drop(first);

    // Process 2: reopen the persisted state and read engine findings.
    let mut second = McpServer::start(&root);
    let reopened = second.command_ok(envelope(
        "reopen-1",
        json!({ "kind": "model.open", "stateId": "travel-mug" }),
    ));
    assert_eq!(
        reopened.get("created").and_then(Value::as_bool),
        Some(false)
    );
    let rebuilt_findings = second.query(json!({ "kind": "analyze.findings" }));
    drop(second);

    // Process 3: import the example model and read engine findings.
    let mut third = McpServer::start(&root);
    third.command_ok(json!({
        "requestId": "import-1",
        "model": serde_json::from_str::<Value>(EXAMPLE).expect("example JSON"),
    }));
    let imported_findings = third.query(json!({ "kind": "analyze.findings" }));
    drop(third);

    assert_eq!(rebuilt_findings["findings"], imported_findings["findings"]);
}

// ── atomic acceptance tests ──

#[test]
fn reopened_version_equals_the_number_of_accepted_mutations() {
    let (_tmp, root) = temp_root();
    let built = {
        let mut server = McpServer::start(&root);
        build_travel_mug(&mut server)
    };
    let mut reopened = McpServer::start(&root);
    let open = reopened.command_ok(envelope(
        "reopen-2",
        json!({ "kind": "model.open", "stateId": "travel-mug" }),
    ));
    assert_eq!(
        open.get("version").and_then(Value::as_u64),
        Some(built.mutations)
    );
}

#[test]
fn check_dir_exits_zero_after_building_the_store() {
    let (tmp, root) = temp_root();
    {
        let mut server = McpServer::start(&root);
        build_travel_mug(&mut server);
    }
    let status = Command::new(env!("CARGO_BIN_EXE_crossmatrix-mcp"))
        .args(["check", "--dir"])
        .arg(tmp.path())
        .status()
        .expect("run crossmatrix-mcp check");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn audit_log_has_one_line_per_accepted_command() {
    let (_tmp, root) = temp_root();
    let built = {
        let mut server = McpServer::start(&root);
        build_travel_mug(&mut server)
    };
    let audit = std::fs::read_to_string(root.join("audit/travel-mug.jsonl")).expect("read audit");
    let lines = audit.lines().filter(|line| !line.trim().is_empty()).count();
    assert_eq!(lines, built.commands);
}

#[test]
fn replaying_a_request_id_in_a_new_process_returns_the_recorded_version() {
    let (_tmp, root) = temp_root();
    let built = {
        let mut server = McpServer::start(&root);
        build_travel_mug(&mut server)
    };
    let mut server = McpServer::start(&root);
    server.command_ok(envelope(
        "reopen-3",
        json!({ "kind": "model.open", "stateId": "travel-mug" }),
    ));
    let replayed = server.command_ok(built.replay_request.clone());
    assert_eq!(
        replayed.get("version").and_then(Value::as_u64),
        Some(built.replay_version)
    );
}

#[test]
fn replaying_a_request_id_in_a_new_process_does_not_append_an_audit_line() {
    let (_tmp, root) = temp_root();
    let built = {
        let mut server = McpServer::start(&root);
        build_travel_mug(&mut server)
    };
    let audit_path = root.join("audit/travel-mug.jsonl");
    let before = std::fs::read_to_string(&audit_path).expect("read audit before");
    let mut server = McpServer::start(&root);
    server.command_ok(envelope(
        "reopen-4",
        json!({ "kind": "model.open", "stateId": "travel-mug" }),
    ));
    server.command_ok(built.replay_request.clone());
    let after = std::fs::read_to_string(&audit_path).expect("read audit after");
    assert_eq!(after.lines().count(), before.lines().count());
}

#[test]
fn a_stale_expected_version_is_rejected_with_a_version_conflict_code() {
    let (_tmp, root) = temp_root();
    let mut server = McpServer::start(&root);
    build_travel_mug(&mut server);
    let mut request = envelope(
        "stale-1",
        json!({ "kind": "dimension.register", "dimensionId": "d_extra", "dimensionKind": "criterion" }),
    );
    request["expectedVersion"] = json!(0);
    let structured = server.command_raw(request);
    assert_eq!(structured["error"]["code"], json!("version_conflict"));
}

#[test]
fn a_stale_expected_version_rejection_carries_non_empty_links() {
    let (_tmp, root) = temp_root();
    let mut server = McpServer::start(&root);
    build_travel_mug(&mut server);
    let mut request = envelope(
        "stale-2",
        json!({ "kind": "dimension.register", "dimensionId": "d_extra", "dimensionKind": "criterion" }),
    );
    request["expectedVersion"] = json!(0);
    let structured = server.command_raw(request);
    assert!(
        structured["links"]
            .as_array()
            .is_some_and(|links| !links.is_empty())
    );
}

#[test]
fn observing_an_unknown_token_is_rejected_with_an_unknown_token_code() {
    let (_tmp, root) = temp_root();
    let mut server = McpServer::start(&root);
    build_travel_mug(&mut server);
    let structured = server.command_raw(envelope(
        "token-1",
        json!({
            "kind": "observe",
            "relationId": "rel_need_part",
            "from": "need_keeps_hot",
            "to": "part_vacuum_wall",
            "observation": "bogus",
            "provenanceClass": "normative",
        }),
    ));
    assert_eq!(structured["error"]["code"], json!("unknown_token"));
}

#[test]
fn syncing_a_numeric_member_weight_without_a_source_is_rejected_as_observation_only() {
    let (_tmp, root) = temp_root();
    let mut server = McpServer::start(&root);
    build_travel_mug(&mut server);
    let structured = server.command_raw(envelope(
        "obs-1",
        json!({
            "kind": "members.sync",
            "dimensionId": "dim_part",
            "members": [ { "id": "part_vacuum_wall", "weight": 5 } ],
        }),
    ));
    assert_eq!(structured["error"]["code"], json!("observation_only"));
}

#[test]
fn a_command_without_an_open_model_reports_ok_false() {
    let (_tmp, root) = temp_root();
    let mut server = McpServer::start(&root);
    let structured = server.command_raw(envelope(
        "nomodel-1",
        json!({ "kind": "dimension.register", "dimensionId": "d1", "dimensionKind": "criterion" }),
    ));
    assert_eq!(structured["ok"], json!(false));
}
