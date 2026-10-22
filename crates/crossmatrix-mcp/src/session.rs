//! Command engine: the MCP-layer state machine behind `crossmatrix.command`.
//!
//! A [`Session`] owns the store root, the currently-open engine model (persisted
//! through the three ADR-0004 layers, or ephemeral for an imported model), the
//! monotonic revision used for optimistic CAS, and the `requestId` replay cache.
//! It is deliberately free of `rmcp` so the whole write path is unit-testable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Value, json};

use crossmatrix::write::{
    self, ContractionSpec, DimensionSpec, MemberSpec, RelationSpec, TargetKind, WriteError,
};
use crossmatrix::{Evidence, Model, Observation, ScaleDef, SourceRef};

use crate::merge;
use crate::repo::RootSource;
use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};
use crate::store::{AuditEntry, AuditLog, ConfigStore, DimensionsStore, StateStore, now_rfc3339};

/// The backing documents of the currently-open model.
struct PersistedDocs {
    config: CrossMatrixConfiguration,
    dims: CrossMatrixDimensions,
    state: CrossMatrixState,
}

enum Backing {
    Persisted(Box<PersistedDocs>),
    Ephemeral,
}

/// A recoverable, structured rejection. Every rejection carries a stable `code`
/// and a non-empty `links` array of legal next ops.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rejection {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
    pub links: Vec<String>,
}

impl Rejection {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
        links: Vec<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details,
            links,
        }
    }

    /// The wire shape: `{"ok": false, "error": {"code", "message", "details"?}, "links": [...]}`.
    pub fn to_value(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(details) = &self.details {
            error["details"] = details.clone();
        }
        json!({ "ok": false, "error": error, "links": self.links })
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The command engine. Create with [`Session::new`] / [`Session::from_process`].
pub struct Session {
    root: PathBuf,
    source: RootSource,
    model: Option<Model>,
    backing: Backing,
    state_id: Option<String>,
    revision: u64,
    locked: bool,
    request_cache: HashMap<String, Result<Value, Rejection>>,
    /// requestId -> afterVersion for persisted audit replays (loaded at model.open).
    audit_replays: HashMap<String, u64>,
}

impl Session {
    /// A session over an explicit store root. No directory is created until a
    /// persisted write happens (lazy-safe).
    pub fn new(root: PathBuf, source: RootSource) -> Self {
        Self {
            root,
            source,
            model: None,
            backing: Backing::Ephemeral,
            state_id: None,
            revision: 0,
            locked: false,
            request_cache: HashMap::new(),
            audit_replays: HashMap::new(),
        }
    }

    /// A session over the process-resolved store root (env/ancestor/xdg/home/cwd).
    pub fn from_process() -> Self {
        let (root, source) = crate::repo::resolve_root_from_process();
        Self::new(root, source)
    }

    /// A session preloaded with an imported model; writes stay in memory.
    pub fn ephemeral(model: Model) -> Self {
        let mut session = Self::new(PathBuf::from("."), RootSource::Cwd);
        session.set_ephemeral(model);
        session
    }

    /// The currently-open model, if any.
    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
    }

    /// The store root this session reads/writes (e.g. `<project>/.crossmatrix`).
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// The default report title: the persisted configuration's `label`, else its
    /// `aspect`; `None` for an ephemeral (imported) model.
    pub fn config_default_title(&self) -> Option<String> {
        match &self.backing {
            Backing::Persisted(docs) => docs
                .config
                .label
                .clone()
                .or_else(|| Some(docs.config.aspect.clone())),
            Backing::Ephemeral => None,
        }
    }

    /// The non-mutating `model.status` body (shared by the command op and the
    /// `model.status` query).
    pub fn status(&self) -> Value {
        match &self.model {
            None => json!({
                "ok": true,
                "op": "model.status",
                "open": false,
                "modelId": null,
                "version": null,
                "persisted": null,
                "root": self.root.display().to_string(),
                "rootSource": self.source,
                "configRef": null,
                "dimensionSetRef": null,
                "counts": { "dimensions": 0, "members": 0, "relations": 0, "cells": 0 },
                "links": ["model.open"],
            }),
            Some(model) => {
                let (persisted, config_ref, dimension_set_ref) = match &self.backing {
                    Backing::Persisted(docs) => (
                        true,
                        Some(json!({
                            "configId": docs.config.config_id,
                            "version": docs.config.version,
                        })),
                        Some(json!({
                            "dimensionSetId": docs.config.dimension_set_ref.dimension_set_id,
                            "version": docs.config.dimension_set_ref.version,
                        })),
                    ),
                    Backing::Ephemeral => (false, None, None),
                };
                let dimensions = model.dimensions();
                let members: usize = dimensions.iter().map(|d| d.members.len()).sum();
                let relations = model.relations();
                let cells: usize = relations.iter().map(|r| r.cells.len()).sum();
                json!({
                    "ok": true,
                    "op": "model.status",
                    "open": true,
                    "modelId": model.doc().model_id,
                    "version": self.revision,
                    "persisted": persisted,
                    "root": self.root.display().to_string(),
                    "rootSource": self.source,
                    "configRef": config_ref,
                    "dimensionSetRef": dimension_set_ref,
                    "counts": {
                        "dimensions": dimensions.len(),
                        "members": members,
                        "relations": relations.len(),
                        "cells": cells,
                    },
                    "links": ["query", "model.status", "describe"],
                })
            }
        }
    }

    /// Execute one `crossmatrix.command` request. Results (Ok and Err) are
    /// cached per `requestId` for idempotent replay.
    pub fn command(&mut self, request: &Value) -> Result<Value, Rejection> {
        let request_id = request
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(id) = &request_id
            && let Some(cached) = self.request_cache.get(id)
        {
            return cached.clone();
        }
        let kind = request
            .pointer("/op/kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let result = self.dispatch(request, &kind);
        if let Some(id) = request_id {
            self.request_cache.insert(id, result.clone());
        }
        result
    }

    fn dispatch(&mut self, request: &Value, kind: &str) -> Result<Value, Rejection> {
        // (a) persisted audit replay: the requestId ran in a previous process.
        if let Some(id) = request.get("requestId").and_then(Value::as_str)
            && self.persisted_open()
            && let Some(after) = self.audit_replays.get(id).copied()
        {
            return Ok(json!({
                "ok": true,
                "op": kind,
                "replayed": true,
                "version": after,
                "links": success_links(kind),
            }));
        }

        // (b) observation-only boundary (exact legacy behavior).
        let has_numeric_weight = request
            .pointer("/op/cell/weight")
            .is_some_and(Value::is_number)
            || request
                .pointer("/op/member/weight")
                .is_some_and(Value::is_number);
        let has_source_ref_system = request
            .pointer("/op/cell/sourceRef/system")
            .is_some_and(Value::is_string)
            || request
                .pointer("/op/member/sourceRef/system")
                .is_some_and(Value::is_string);
        if has_numeric_weight && !has_source_ref_system {
            return Err(Rejection::new(
                "observation_only",
                "observation-only: numeric weights require sourceRef.system",
                None,
                vec!["describe".to_string(), "model.status".to_string()],
            ));
        }

        // (c) import path (backward compatible, ephemeral).
        if let Some(model_value) = request.get("model") {
            return match Model::load(&model_value.to_string()) {
                Ok(loaded) => {
                    self.set_ephemeral(loaded);
                    Ok(json!({ "ok": true, "op": kind, "validated": true, "links": ["query"] }))
                }
                Err(error) => Err(Rejection::new(
                    "invalid_model",
                    format!("model failed validation: {error}"),
                    None,
                    vec!["model.open".to_string(), "validate".to_string()],
                )),
            };
        }

        // (d) neither an op nor an import model: reject, naming the envelope.
        if request.get("op").is_none() {
            return Err(Rejection::new(
                "invalid_params",
                "command requires 'op' (envelope: schemaVersion, requestId, modelId, actor, op{kind}) or an import 'model'",
                None,
                vec!["model.status".to_string(), "describe".to_string()],
            ));
        }

        // (e) validate the request against $defs/CommandRequest.
        let errors: Vec<Value> = command_validator()
            .iter_errors(request)
            .map(|error| {
                json!({
                    "instancePath": error.instance_path().to_string(),
                    "message": error.to_string(),
                })
            })
            .collect();
        if !errors.is_empty() {
            let mut links = Vec::new();
            if !kind.is_empty() {
                links.push(kind.to_string());
            }
            links.push("model.status".to_string());
            return Err(Rejection::new(
                "schema_invalid",
                "request failed schema validation",
                Some(Value::Array(errors)),
                links,
            ));
        }

        // (f) model.status is non-mutating and does not require an open model.
        if kind == "model.status" {
            return Ok(self.status());
        }

        // (g) model.open.
        if kind == "model.open" {
            return self.open_model(request);
        }

        // (h) every other op requires an open model + CAS.
        if self.model.is_none() {
            return Err(Rejection::new(
                "no_model",
                "no model loaded",
                None,
                vec!["model.open".to_string()],
            ));
        }
        if self.locked {
            return Err(Rejection::new(
                "model_locked",
                "model is locked",
                None,
                vec!["model.status".to_string(), kind.to_string()],
            ));
        }
        if let Some(expected) = request.get("expectedVersion").and_then(Value::as_u64)
            && expected != self.revision
        {
            return Err(Rejection::new(
                "version_conflict",
                "expectedVersion does not match the current revision",
                Some(json!({ "expectedVersion": expected, "currentVersion": self.revision })),
                vec!["model.status".to_string(), kind.to_string()],
            ));
        }

        self.apply_mutation(request, kind)
    }

    fn open_model(&mut self, request: &Value) -> Result<Value, Rejection> {
        let op = request.get("op").cloned().unwrap_or(Value::Null);
        let state_id = op
            .get("stateId")
            .and_then(Value::as_str)
            .or_else(|| request.get("modelId").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let open_failed = |error: String| {
            Rejection::new("open_failed", error, None, vec!["model.open".to_string()])
        };

        let state_store = StateStore::new(self.root.clone());
        if state_store.exists_current(&state_id) {
            let state = state_store
                .get_current(&state_id)
                .map_err(|e| open_failed(format!("read state '{state_id}': {e}")))?;
            let config = ConfigStore::new(self.root.clone())
                .get(&state.config_ref.config_id, &state.config_ref.version)
                .map_err(|e| open_failed(format!("resolve configuration: {e}")))?;
            let dims = DimensionsStore::new(self.root.clone())
                .get(
                    &config.dimension_set_ref.dimension_set_id,
                    &config.dimension_set_ref.version,
                )
                .map_err(|e| open_failed(format!("resolve dimensions: {e}")))?;
            let model_json = merge::merge(&config, &dims, &state)
                .map_err(|e| open_failed(format!("merge: {e}")))?;
            let model = Model::load(&model_json.to_string())
                .map_err(|e| open_failed(format!("load: {e}")))?;
            let revision = state.revision.unwrap_or(0);
            self.locked = matches!(
                state_status(&state).as_deref(),
                Some("locked") | Some("archived")
            );
            self.audit_replays = self.load_audit_replays(&state_id);
            self.model = Some(model);
            self.backing = Backing::Persisted(Box::new(PersistedDocs {
                config,
                dims,
                state,
            }));
            self.state_id = Some(state_id.clone());
            self.revision = revision;
            return Ok(json!({
                "ok": true,
                "op": "model.open",
                "created": false,
                "modelId": state_id,
                "version": revision,
                "root": self.root.display().to_string(),
                "rootSource": self.source,
                "links": ["query", "model.status", "describe"],
            }));
        }

        // Create a fresh model: reuse existing config/dims docs by id@version
        // when present, else build empty ones.
        let config_id = op
            .get("configId")
            .and_then(Value::as_str)
            .unwrap_or(&state_id)
            .to_string();
        let config_version = op
            .get("configVersion")
            .and_then(Value::as_str)
            .unwrap_or("0.1.0")
            .to_string();
        let dims_id = op
            .get("dimensionSetId")
            .and_then(Value::as_str)
            .unwrap_or(&state_id)
            .to_string();
        let dims_version = op
            .get("dimensionSetVersion")
            .and_then(Value::as_str)
            .unwrap_or("0.1.0")
            .to_string();
        let aspect = op
            .get("aspect")
            .and_then(Value::as_str)
            .unwrap_or("custom")
            .to_string();
        let title = op.get("title").and_then(Value::as_str).map(str::to_string);
        let status = op.get("status").and_then(Value::as_str).map(str::to_string);

        let config_store = ConfigStore::new(self.root.clone());
        let dims_store = DimensionsStore::new(self.root.clone());
        let config = match config_store.get(&config_id, &config_version) {
            Ok(config) => config,
            Err(_) => serde_json::from_value(json!({
                "schemaVersion": "0.2.0",
                "configId": config_id,
                "version": config_version,
                "aspect": aspect,
                "label": title,
                "dimensionSetRef": { "dimensionSetId": dims_id, "version": dims_version },
                "scales": [],
                "relations": [],
                "contractions": [],
            }))
            .map_err(|e| open_failed(format!("build configuration: {e}")))?,
        };
        let dims = match dims_store.get(&dims_id, &dims_version) {
            Ok(dims) => dims,
            Err(_) => serde_json::from_value(json!({
                "schemaVersion": "0.2.0",
                "dimensionSetId": dims_id,
                "version": dims_version,
                "dimensions": [],
            }))
            .map_err(|e| open_failed(format!("build dimensions: {e}")))?,
        };
        let mut state_json = json!({
            "schemaVersion": "0.2.0",
            "stateId": state_id,
            "configRef": { "configId": config_id, "version": config_version },
            "cells": [],
            "revision": 0,
        });
        if let Some(title) = &title {
            state_json["title"] = json!(title);
        }
        if let Some(status) = &status {
            state_json["status"] = json!(status);
        }
        let state: CrossMatrixState = serde_json::from_value(state_json)
            .map_err(|e| open_failed(format!("build state: {e}")))?;

        config_store
            .put(&config)
            .map_err(|e| open_failed(format!("persist configuration: {e}")))?;
        dims_store
            .put(&dims)
            .map_err(|e| open_failed(format!("persist dimensions: {e}")))?;
        state_store
            .put_current(&state)
            .map_err(|e| open_failed(format!("persist state: {e}")))?;

        let model_json =
            merge::merge(&config, &dims, &state).map_err(|e| open_failed(format!("merge: {e}")))?;
        let model =
            Model::load(&model_json.to_string()).map_err(|e| open_failed(format!("load: {e}")))?;

        let entry = AuditEntry {
            ts: now_rfc3339(),
            request_id: request
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            actor: request.get("actor").cloned().unwrap_or(Value::Null),
            op: "model.open".to_string(),
            before_version: 0,
            after_version: 0,
            summary: format!("created {state_id}"),
        };
        AuditLog::new(self.root.clone())
            .append(&state_id, &entry)
            .map_err(|e| open_failed(format!("append audit: {e}")))?;

        self.locked = matches!(status.as_deref(), Some("locked") | Some("archived"));
        self.audit_replays = self.load_audit_replays(&state_id);
        self.model = Some(model);
        self.backing = Backing::Persisted(Box::new(PersistedDocs {
            config,
            dims,
            state,
        }));
        self.state_id = Some(state_id.clone());
        self.revision = 0;
        Ok(json!({
            "ok": true,
            "op": "model.open",
            "created": true,
            "modelId": state_id,
            "version": 0,
            "root": self.root.display().to_string(),
            "rootSource": self.source,
            "links": ["query", "model.status", "describe"],
        }))
    }

    fn apply_mutation(&mut self, request: &Value, kind: &str) -> Result<Value, Rejection> {
        let op = request.get("op").cloned().unwrap_or(Value::Null);
        let model = self.model.clone().ok_or_else(|| {
            Rejection::new(
                "no_model",
                "no model loaded",
                None,
                vec!["model.open".to_string()],
            )
        })?;

        let (new_model, summary, sync) = match kind {
            "dimension.register" => {
                let spec = DimensionSpec {
                    id: required_str(&op, "dimensionId")?,
                    kind: optional_str(&op, "dimensionKind"),
                    label: optional_str(&op, "label"),
                    system: optional_str(&op, "system"),
                    weight_scale: optional_str(&op, "weightScale"),
                    weight_semantics: optional_str(&op, "weightSemantics"),
                    order: op.get("order").and_then(Value::as_i64),
                };
                let id = spec.id.clone();
                let next = write::register_dimension(&model, &spec).map_err(reject_write)?;
                (next, format!("register dimension {id}"), None)
            }
            "scale.declare" => {
                let scale: ScaleDef =
                    serde_json::from_value(op.get("scale").cloned().unwrap_or(Value::Null))
                        .map_err(|e| invalid_payload(format!("scale: {e}")))?;
                let id = scale.id.clone();
                let next = write::declare_scale(&model, scale).map_err(reject_write)?;
                (next, format!("declare scale {id}"), None)
            }
            "relation.declare" => {
                let spec = RelationSpec {
                    id: required_str(&op, "relationId")?,
                    from: required_str(&op, "from")?,
                    to: required_str(&op, "to")?,
                    relation_type: required_str(&op, "relationType")?,
                    scale: required_str(&op, "scale")?,
                    coverage_policy: optional_str(&op, "coveragePolicy"),
                };
                let id = spec.id.clone();
                let next = write::declare_relation(&model, &spec).map_err(reject_write)?;
                (next, format!("declare relation {id}"), None)
            }
            "contraction.declare" => {
                let spec = ContractionSpec {
                    id: required_str(&op, "contractionId")?,
                    chain: op
                        .get("chain")
                        .and_then(Value::as_array)
                        .map(|chain| {
                            chain
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                    label: optional_str(&op, "label"),
                    weight_combination: optional_str(&op, "weightCombination"),
                    weight_by_source_members: op
                        .get("weightBySourceMembers")
                        .and_then(Value::as_bool),
                };
                let id = spec.id.clone();
                let next = write::declare_contraction(&model, &spec).map_err(reject_write)?;
                (next, format!("declare contraction {id}"), None)
            }
            "members.sync" => {
                let dimension_id = required_str(&op, "dimensionId")?;
                let roster_values = op
                    .get("members")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let roster: Vec<MemberSpec> = roster_values
                    .iter()
                    .map(|item| member_spec(item, "id"))
                    .collect::<Result<_, _>>()?;
                let (next, sync) =
                    write::sync_members(&model, &dimension_id, &roster).map_err(reject_write)?;
                let summary = format!(
                    "sync {dimension_id}: +{} ~{} -{}",
                    sync.added.len(),
                    sync.updated.len(),
                    sync.deprecated.len()
                );
                (next, summary, Some(sync))
            }
            "member.propose" => {
                let dimension_id = required_str(&op, "dimensionId")?;
                let spec = member_spec(&op, "memberId")?;
                let id = spec.id.clone();
                let next =
                    write::propose_member(&model, &dimension_id, &spec).map_err(reject_write)?;
                (next, format!("propose member {id} in {dimension_id}"), None)
            }
            "observe" => {
                let relation_id = required_str(&op, "relationId")?;
                let from = required_str(&op, "from")?;
                let to = required_str(&op, "to")?;
                let mut observation_value = op.as_object().cloned().unwrap_or_default();
                observation_value.remove("kind");
                observation_value.remove("relationId");
                observation_value.remove("from");
                observation_value.remove("to");
                let observation: Observation =
                    serde_json::from_value(Value::Object(observation_value))
                        .map_err(|e| invalid_payload(format!("observation: {e}")))?;
                let token = observation.observation.clone();
                let next = write::observe(&model, &relation_id, &from, &to, observation)
                    .map_err(reject_write)?;
                (
                    next,
                    format!("observe {relation_id} {from}->{to} {token}"),
                    None,
                )
            }
            "evidence.attach" => {
                let target = op.get("targetRef").cloned().unwrap_or(Value::Null);
                let relation_id = required_str(&target, "relationId")?;
                let from = required_str(&target, "from")?;
                let to = required_str(&target, "to")?;
                let observation_index = target
                    .get("observationIndex")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize);
                let evidence: Evidence =
                    serde_json::from_value(op.get("evidence").cloned().unwrap_or(Value::Null))
                        .map_err(|e| invalid_payload(format!("evidence: {e}")))?;
                let next = write::attach_evidence(
                    &model,
                    &relation_id,
                    &from,
                    &to,
                    observation_index,
                    evidence,
                )
                .map_err(reject_write)?;
                (
                    next,
                    format!("attach evidence {relation_id} {from}->{to}"),
                    None,
                )
            }
            "deprecate" => {
                let target_id = required_str(&op, "targetId")?;
                let reason = required_str(&op, "reason")?;
                let target_kind = match op.get("targetKind").and_then(Value::as_str) {
                    Some("member") => Some(TargetKind::Member),
                    Some("relation") => Some(TargetKind::Relation),
                    _ => None,
                };
                let superseded_by = optional_str(&op, "supersededBy");
                let next =
                    write::deprecate(&model, &target_id, target_kind, superseded_by.as_deref())
                        .map_err(reject_write)?;
                (next, format!("deprecate {target_id}: {reason}"), None)
            }
            other => {
                return Err(Rejection::new(
                    "schema_invalid",
                    format!("unknown op kind '{other}'"),
                    None,
                    vec!["model.status".to_string()],
                ));
            }
        };

        let before = self.revision;
        let persisted = matches!(self.backing, Backing::Persisted { .. });

        // A write that changes nothing is a NO-OP: no revision bump, no file
        // writes, no audit line. `members.sync` is authoritative via its
        // summary; every other write is compared by resulting document.
        let noop = match &sync {
            Some(sync) => sync.is_noop(),
            None => {
                serde_json::to_value(new_model.doc()).ok() == serde_json::to_value(model.doc()).ok()
            }
        };
        if noop {
            let model_id = new_model.doc().model_id.clone();
            self.model = Some(new_model);
            let mut response = json!({
                "ok": true,
                "op": kind,
                "modelId": model_id,
                "version": before,
                "persisted": persisted,
                "noop": true,
                "summary": summary,
                "links": success_links(kind),
            });
            if let Some(sync) = sync {
                response["sync"] = serde_json::to_value(&sync).unwrap_or(Value::Null);
            }
            return Ok(response);
        }

        let after = before + 1;

        if persisted {
            let mut next_backing = None;
            if let Backing::Persisted(docs) = &self.backing {
                let (config2, dims2, mut state2) =
                    merge::split(new_model.doc(), &docs.config, &docs.dims, &docs.state).map_err(
                        |e| {
                            Rejection::new(
                                "persist_failed",
                                format!("split model: {e}"),
                                None,
                                vec!["model.status".to_string()],
                            )
                        },
                    )?;
                state2.revision = Some(after);
                let config_store = ConfigStore::new(self.root.clone());
                let dims_store = DimensionsStore::new(self.root.clone());
                let state_store = StateStore::new(self.root.clone());
                // Only rewrite the layers this op can change (ADR-0004 split);
                // State always carries the new revision. `deprecate` may touch
                // either layer, so it is compared by content.
                let (config_changed, dims_changed) = match kind {
                    "scale.declare" | "relation.declare" | "contraction.declare" => (true, false),
                    "dimension.register" | "members.sync" | "member.propose" => (false, true),
                    "observe" | "evidence.attach" => (false, false),
                    _ => (
                        serde_json::to_value(&config2).ok()
                            != serde_json::to_value(&docs.config).ok(),
                        serde_json::to_value(&dims2).ok() != serde_json::to_value(&docs.dims).ok(),
                    ),
                };
                if config_changed {
                    config_store.put(&config2).map_err(persist_failed)?;
                }
                if dims_changed {
                    dims_store.put(&dims2).map_err(persist_failed)?;
                }
                state_store.put_current(&state2).map_err(persist_failed)?;
                next_backing = Some(Backing::Persisted(Box::new(PersistedDocs {
                    config: config2,
                    dims: dims2,
                    state: state2,
                })));
            }
            if let Some(backing) = next_backing {
                self.backing = backing;
            }

            let state_id = self.state_id.clone().unwrap_or_default();
            let entry = AuditEntry {
                ts: now_rfc3339(),
                request_id: request
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                actor: request.get("actor").cloned().unwrap_or(Value::Null),
                op: kind.to_string(),
                before_version: before,
                after_version: after,
                summary: summary.clone(),
            };
            AuditLog::new(self.root.clone())
                .append(&state_id, &entry)
                .map_err(persist_failed)?;
        }

        let model_id = new_model.doc().model_id.clone();
        self.model = Some(new_model);
        self.revision = after;

        let mut response = json!({
            "ok": true,
            "op": kind,
            "modelId": model_id,
            "version": after,
            "persisted": persisted,
            "summary": summary,
            "links": success_links(kind),
        });
        if let Some(sync) = sync {
            response["sync"] = serde_json::to_value(&sync).unwrap_or(Value::Null);
        }
        Ok(response)
    }

    fn set_ephemeral(&mut self, model: Model) {
        self.model = Some(model);
        self.backing = Backing::Ephemeral;
        self.state_id = None;
        self.revision = 0;
        self.locked = false;
        self.audit_replays.clear();
    }

    fn persisted_open(&self) -> bool {
        self.model.is_some() && matches!(self.backing, Backing::Persisted(_))
    }

    fn load_audit_replays(&self, state_id: &str) -> HashMap<String, u64> {
        let mut replays = HashMap::new();
        if let Ok(entries) = AuditLog::new(self.root.clone()).read(state_id) {
            for entry in entries {
                replays.insert(entry.request_id, entry.after_version);
            }
        }
        replays
    }
}

fn state_status(state: &CrossMatrixState) -> Option<String> {
    serde_json::to_value(state).ok().and_then(|value| {
        value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

/// Links advertized after a successful op, the HATEOAS "legal next ops".
fn success_links(kind: &str) -> Vec<String> {
    let links: &[&str] = match kind {
        "dimension.register" => &["members.sync", "scale.declare", "relation.declare"],
        "scale.declare" => &["relation.declare", "dimension.register"],
        "relation.declare" => &["gaps.next", "observe", "contraction.declare"],
        "contraction.declare" => &["analyze.contract"],
        "members.sync" | "member.propose" => &["gaps.next", "observe", "describe"],
        "observe" | "evidence.attach" => &["gaps.next", "observe", "analyze.findings", "conflicts"],
        "deprecate" => &["describe", "gaps.next"],
        "model.open" | "model.status" => &["query", "model.status"],
        _ => &["describe", "model.status"],
    };
    links.iter().map(|link| link.to_string()).collect()
}

/// Legal recovery ops for a core write failure, keyed by `WriteError::code()`.
fn recovery_links(code: &str) -> Vec<String> {
    let links: &[&str] = match code {
        "unknown_dimension" => &["dimension.register", "describe"],
        "unknown_scale" => &["scale.declare"],
        "unknown_relation" => &["relation.declare", "describe"],
        "unknown_member" => &["members.sync", "member.propose", "trace"],
        "unknown_token" => &["describe", "scale.declare"],
        _ => &["describe", "model.status"],
    };
    links.iter().map(|link| link.to_string()).collect()
}

fn reject_write(error: WriteError) -> Rejection {
    let code = error.code();
    Rejection::new(code, error.to_string(), None, recovery_links(code))
}

fn persist_failed(error: String) -> Rejection {
    Rejection::new(
        "persist_failed",
        format!("persist failed: {error}"),
        None,
        vec!["model.status".to_string()],
    )
}

fn invalid_payload(message: String) -> Rejection {
    Rejection::new(
        "schema_invalid",
        message,
        None,
        vec!["model.status".to_string()],
    )
}

fn required_str(value: &Value, key: &str) -> Result<String, Rejection> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            Rejection::new(
                "schema_invalid",
                format!("missing required string field '{key}'"),
                None,
                vec!["model.status".to_string()],
            )
        })
}

fn optional_str(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn member_spec(value: &Value, id_key: &str) -> Result<MemberSpec, Rejection> {
    let source_ref = match value.get("sourceRef") {
        Some(source) if !source.is_null() => Some(
            serde_json::from_value::<SourceRef>(source.clone())
                .map_err(|e| invalid_payload(format!("sourceRef: {e}")))?,
        ),
        _ => None,
    };
    Ok(MemberSpec {
        id: required_str(value, id_key)?,
        label: optional_str(value, "label"),
        weight_observation: optional_str(value, "weightObservation"),
        weight: value.get("weight").and_then(Value::as_f64),
        source_ref,
        attributes: value.get("attributes").and_then(Value::as_object).cloned(),
    })
}

fn command_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(include_str!("../schema/crossmatrix-mcp.schema.json"))
                .expect("embedded crossmatrix-mcp schema is valid JSON");
        let wrapper = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": schema.get("$defs").cloned().unwrap_or(Value::Null),
            "$ref": "#/$defs/CommandRequest",
        });
        jsonschema::validator_for(&wrapper).expect("command request schema compiles")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Value {
        json!({ "actorType": "llm", "persona": "model_admin" })
    }

    fn envelope(op: Value) -> Value {
        json!({
            "schemaVersion": "0.2.0",
            "requestId": format!("req-{}", uuid_like()),
            "modelId": "m",
            "actor": actor(),
            "op": op,
        })
    }

    fn uuid_like() -> String {
        // A cheap unique-ish token; no new deps.
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
    }

    fn open_session(root: &std::path::Path) -> Session {
        let mut session = Session::new(root.to_path_buf(), RootSource::Env);
        session
            .command(&envelope(json!({ "kind": "model.open" })))
            .expect("model.open must succeed");
        session
    }

    #[test]
    fn open_creates_a_persisted_store_and_reports_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        let result = session
            .command(&envelope(json!({ "kind": "model.open" })))
            .expect("open");
        assert_eq!(
            result.get("created").and_then(Value::as_bool),
            Some(true),
            "a fresh model.open must report created=true"
        );
    }

    #[test]
    fn reopening_a_persisted_state_reports_not_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut first = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        first
            .command(&envelope(json!({ "kind": "model.open" })))
            .expect("first open");
        let mut second = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        let result = second
            .command(&json!({
                "schemaVersion": "0.2.0",
                "requestId": uuid_like(),
                "modelId": "m",
                "actor": actor(),
                "op": { "kind": "model.open", "stateId": "m" },
            }))
            .expect("second open");
        assert_eq!(
            result.get("created").and_then(Value::as_bool),
            Some(false),
            "reopening an existing state must report created=false"
        );
    }

    #[test]
    fn registering_a_dimension_bumps_the_revision() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        let result = session
            .command(&envelope(json!({
                "kind": "dimension.register",
                "dimensionId": "d1",
                "dimensionKind": "criterion",
            })))
            .expect("register");
        assert_eq!(
            result.get("version").and_then(Value::as_u64),
            Some(1),
            "the first accepted mutation must move the revision to 1"
        );
    }

    #[test]
    fn status_on_an_empty_session_reports_not_open() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        assert_eq!(
            session.status().get("open").and_then(Value::as_bool),
            Some(false),
            "status with no model open must report open=false"
        );
    }

    #[test]
    fn rejecting_a_stale_expected_version_reports_a_conflict() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        let mut request = envelope(json!({
            "kind": "dimension.register",
            "dimensionId": "d1",
            "dimensionKind": "criterion",
        }));
        request["expectedVersion"] = json!(99);
        let rejection = session.command(&request).expect_err("stale CAS must fail");
        assert_eq!(rejection.code, "version_conflict");
    }

    #[test]
    fn replaying_a_request_id_returns_the_original_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        let request = envelope(json!({
            "kind": "dimension.register",
            "dimensionId": "d1",
            "dimensionKind": "criterion",
        }));
        session.command(&request).expect("first");
        let replay = session.command(&request).expect("replay");
        assert_eq!(
            replay.get("version").and_then(Value::as_u64),
            Some(1),
            "a replayed requestId must return the original revision"
        );
    }

    #[test]
    fn opening_a_locked_model_rejects_a_later_mutation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        session
            .command(&envelope(
                json!({ "kind": "model.open", "status": "locked" }),
            ))
            .expect("open locked");
        let rejection = session
            .command(&envelope(json!({
                "kind": "dimension.register",
                "dimensionId": "d1",
                "dimensionKind": "criterion",
            })))
            .expect_err("mutation on a locked model must be rejected");
        assert_eq!(rejection.code, "model_locked");
    }

    #[test]
    fn every_rejection_has_at_least_one_link() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = Session::new(tmp.path().to_path_buf(), RootSource::Env);
        let rejection = session
            .command(&json!({
                "schemaVersion": "0.2.0",
                "requestId": uuid_like(),
                "modelId": "m",
                "actor": actor(),
                "op": { "kind": "bogus" },
            }))
            .expect_err("unknown op kind must be rejected");
        assert!(
            !rejection.links.is_empty(),
            "every rejection must carry a non-empty links array"
        );
    }

    #[test]
    fn a_numeric_member_weight_without_source_is_rejected_as_observation_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        session
            .command(&envelope(json!({
                "kind": "dimension.register",
                "dimensionId": "d1",
                "dimensionKind": "criterion",
            })))
            .expect("register dimension");
        let rejection = session
            .command(&envelope(json!({
                "kind": "members.sync",
                "dimensionId": "d1",
                "members": [ { "id": "x", "weight": 9 } ],
            })))
            .expect_err("numeric weight without sourceRef.system must be rejected");
        assert_eq!(rejection.code, "observation_only");
    }

    #[test]
    fn the_command_validator_is_shared_and_fresh_requests_validate() {
        // Two independent sessions both accept a valid command; the once-compiled
        // validator is not poisoned by a prior rejection.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        let result = session.command(&envelope(json!({
            "kind": "scale.declare",
            "scale": { "id": "s", "levels": [ { "observation": "on", "value": 1 } ] },
        })));
        assert!(
            result.is_ok(),
            "a well-formed scale.declare must pass the shared schema validator"
        );
    }

    /// A persisted session with dimension `d1` and one synced member `x`.
    fn synced_session(root: &std::path::Path) -> Session {
        let mut session = open_session(root);
        session
            .command(&envelope(json!({
                "kind": "dimension.register",
                "dimensionId": "d1",
                "dimensionKind": "criterion",
            })))
            .expect("register d1");
        session
            .command(&envelope(json!({
                "kind": "members.sync",
                "dimensionId": "d1",
                "members": [ { "id": "x", "label": "X" } ],
            })))
            .expect("sync roster");
        session
    }

    fn identical_resync(session: &mut Session) -> Value {
        session
            .command(&envelope(json!({
                "kind": "members.sync",
                "dimensionId": "d1",
                "members": [ { "id": "x", "label": "X" } ],
            })))
            .expect("identical resync")
    }

    #[test]
    fn resyncing_an_identical_roster_keeps_the_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = synced_session(tmp.path());
        let before = session.status()["version"].as_u64();
        let result = identical_resync(&mut session);
        assert_eq!(result["version"].as_u64(), before);
    }

    #[test]
    fn resyncing_an_identical_roster_reports_noop_true() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = synced_session(tmp.path());
        let result = identical_resync(&mut session);
        assert_eq!(result["noop"], json!(true));
    }

    #[test]
    fn resyncing_an_identical_roster_writes_no_audit_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = synced_session(tmp.path());
        let audit_path = tmp.path().join("audit/m.jsonl");
        let before = std::fs::read_to_string(&audit_path)
            .expect("read audit before")
            .lines()
            .count();
        let _ = identical_resync(&mut session);
        let after = std::fs::read_to_string(&audit_path)
            .expect("read audit after")
            .lines()
            .count();
        assert_eq!(after, before);
    }

    #[test]
    fn an_identical_duplicate_observe_keeps_the_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut session = open_session(tmp.path());
        for (id, kind) in [("d1", "criterion"), ("d2", "criterion")] {
            session
                .command(&envelope(json!({
                    "kind": "dimension.register",
                    "dimensionId": id,
                    "dimensionKind": kind,
                })))
                .expect("register dimension");
        }
        for (dimension, member) in [("d1", "x"), ("d2", "y")] {
            session
                .command(&envelope(json!({
                    "kind": "members.sync",
                    "dimensionId": dimension,
                    "members": [ { "id": member } ],
                })))
                .expect("sync member");
        }
        session
            .command(&envelope(json!({
                "kind": "scale.declare",
                "scale": { "id": "s", "levels": [ { "observation": "on", "value": 1 } ] },
            })))
            .expect("declare scale");
        session
            .command(&envelope(json!({
                "kind": "relation.declare",
                "relationId": "r",
                "from": "d1",
                "to": "d2",
                "relationType": "supports",
                "scale": "s",
            })))
            .expect("declare relation");
        let observe = envelope(json!({
            "kind": "observe",
            "relationId": "r",
            "from": "x",
            "to": "y",
            "observation": "on",
            "provenanceClass": "normative",
        }));
        session.command(&observe).expect("first observe");
        let before = session.status()["version"].as_u64();
        let result = session.command(&observe).expect("duplicate observe");
        assert_eq!(result["version"].as_u64(), before);
    }
}
