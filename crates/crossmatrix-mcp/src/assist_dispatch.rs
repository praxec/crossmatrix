//! Shared async dispatch for the optional AI-assist queries (X07/X08).
//!
//! This module is ALWAYS compiled. The concrete backends are feature-gated:
//! `jev` supplies the [`Judge`] used by `assist.suggest` and the uncertainty
//! ranking, `rig` supplies the [`crate::assist_rig::TextModel`] used by
//! `assist.members`, `assist.narrative` and the narrative report.
//!
//! `main.rs` and the integration tests both drive [`query`], so the locking
//! protocol is shared: the model is cloned under the session lock, the lock is
//! dropped before any `await`, and re-acquired only to record.
//!
//! Non-assist kinds return `None` so the caller falls back to the existing
//! synchronous [`crate::queries::query`] path.

use std::sync::Mutex;

use serde_json::{Value, json};

use crate::assist::{AssistUnavailable, UnavailableReason};
use crate::assist_rig::{
    MembersRequest, NarrativeRequest, RigUnavailable, TextModel, narrate, propose_members,
};
use crate::queries;
use crate::session::{Rejection, Session};

#[cfg(feature = "jev")]
use std::collections::BTreeMap;

#[cfg(feature = "jev")]
use crossmatrix::{Member, Model, Relation};

#[cfg(feature = "jev")]
use crate::assist::Usage;
#[cfg(feature = "jev")]
use crate::assist::jev::{JevAnswer, JevQuestion, JevReply, Judge, MAX_QUESTIONS_PER_CALL};

/// Placeholder judge trait for builds without the `jev` feature. It is never
/// implemented because every jev-backed kind returns `FeatureNotBuilt` without
/// inspecting the judge.
#[cfg(not(feature = "jev"))]
pub trait Judge: Send + Sync {}

const SUGGEST_LINKS: [&str; 3] = ["observe", "proposals", "assist.suggest"];
const GAPS_LINKS: [&str; 4] = ["gaps.orphans", "coverage", "trace", "assist.suggest"];
const MEMBERS_LINKS: [&str; 2] = ["member.propose", "members.sync"];
const NARRATIVE_LINKS: [&str; 2] = ["report", "analyze.findings"];

/// Dispatch one request to the assist handlers. Returns `None` when `request`
/// is not an assist kind (the caller then uses the synchronous query path).
pub async fn query(
    session: &Mutex<Session>,
    judge: Option<&dyn Judge>,
    text_model: Option<&dyn TextModel>,
    request: &Value,
) -> Option<Result<Value, Rejection>> {
    let kind = request
        .pointer("/query/kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    match kind {
        "assist.suggest" => Some(suggest(session, judge, request).await),
        "assist.members" => Some(members(session, text_model, request).await),
        "assist.narrative" => Some(narrative(session, text_model, request).await),
        "gaps.next" if is_uncertainty(request) => Some(uncertainty(session, judge, request).await),
        "report" if wants_narrative(request) => {
            Some(report_narrative(session, text_model, request).await)
        }
        _ => None,
    }
}

fn is_uncertainty(request: &Value) -> bool {
    opt_str(request, "rankBy") == Some("uncertainty")
}

fn wants_narrative(request: &Value) -> bool {
    opt_bool(request, "narrative")
}

fn opt_str<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request
        .pointer(&format!("/query/{name}"))
        .and_then(Value::as_str)
}

fn opt_bool(request: &Value, name: &str) -> bool {
    request
        .pointer(&format!("/query/{name}"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn opt_u64(request: &Value, name: &str) -> Option<u64> {
    request
        .pointer(&format!("/query/{name}"))
        .and_then(Value::as_u64)
}

fn bad_request(message: impl Into<String>) -> Rejection {
    Rejection::new(
        "bad_request",
        message,
        None,
        vec!["model.status".to_string()],
    )
}

fn no_model() -> Rejection {
    Rejection::new(
        "no_model",
        "no model loaded",
        None,
        vec!["model.open".to_string()],
    )
}

#[cfg(feature = "jev")]
fn unknown_relation(relation: &str) -> Rejection {
    Rejection::new(
        "unknown_relation",
        format!("unknown relation: {relation}"),
        None,
        vec!["relation.declare".to_string(), "describe".to_string()],
    )
}

fn unknown_dimension(dimension: &str) -> Rejection {
    Rejection::new(
        "unknown_dimension",
        format!("unknown dimension: {dimension}"),
        None,
        vec!["dimension.register".to_string(), "describe".to_string()],
    )
}

/// The unavailable reason for a missing text model: a compiled-but-unconfigured
/// backend reports `no_key`; a build without the feature reports
/// `feature_not_built`.
fn text_unavailable(detail: &str) -> RigUnavailable {
    #[cfg(feature = "rig")]
    {
        RigUnavailable::new("no_key", format!("text model is not configured: {detail}"))
    }
    #[cfg(not(feature = "rig"))]
    {
        let _ = detail;
        RigUnavailable::new("feature_not_built", "the rig feature is not compiled")
    }
}

// ── assist.suggest ──

#[cfg(feature = "jev")]
async fn suggest(
    session: &Mutex<Session>,
    judge: Option<&dyn Judge>,
    request: &Value,
) -> Result<Value, Rejection> {
    match judge {
        Some(judge) => suggest_with_judge(session, judge, request).await,
        None => Ok(AssistUnavailable::new(
            UnavailableReason::NoKey,
            "assist judge is not configured",
        )
        .to_value(&SUGGEST_LINKS)),
    }
}

#[cfg(not(feature = "jev"))]
async fn suggest(
    _session: &Mutex<Session>,
    _judge: Option<&dyn Judge>,
    _request: &Value,
) -> Result<Value, Rejection> {
    Ok(AssistUnavailable::new(
        UnavailableReason::FeatureNotBuilt,
        "the jev feature is not compiled",
    )
    .to_value(&SUGGEST_LINKS))
}

#[cfg(feature = "jev")]
async fn suggest_with_judge(
    session: &Mutex<Session>,
    judge: &dyn Judge,
    request: &Value,
) -> Result<Value, Rejection> {
    let relation_id = opt_str(request, "relationId")
        .ok_or_else(|| bad_request("assist.suggest requires 'relationId'"))?
        .to_string();
    let record = opt_bool(request, "record");
    let max_cells = max_cells(request);

    let plan = {
        let guard = session.lock().expect("session mutex");
        build_plan(&guard, &relation_id, request, max_cells)?
    };

    let mut proposals: Vec<Value> = Vec::new();
    let mut usage = UsageAcc::default();
    let mut model = String::new();
    let mut partial: Option<Value> = None;

    for cell in &plan.cells {
        let question = match build_question(&plan, cell) {
            Ok(question) => question,
            Err(error) => {
                partial = Some(partial_of(&error));
                break;
            }
        };
        let mut questions = BTreeMap::new();
        questions.insert("cell".to_string(), question);
        match judge.ask(state_for(&plan, cell), questions).await {
            Ok(reply) => {
                usage.add(&reply.usage);
                if !reply.model.is_empty() {
                    model = reply.model.clone();
                }
                match choice_of(&reply) {
                    Ok((token, probabilities, confidence)) => proposals.push(json!({
                        "cell": { "from": cell.from, "to": cell.to },
                        "token": token,
                        "probabilities": probabilities,
                        "confidence": confidence,
                    })),
                    Err(error) => {
                        partial = Some(partial_of(&error));
                        break;
                    }
                }
            }
            Err(error) => {
                partial = Some(partial_of(&error));
                break;
            }
        }
    }

    let mut recorded = 0usize;
    if record && !proposals.is_empty() {
        let evidence_model = if model.is_empty() { "jev" } else { &model };
        let mut guard = session.lock().expect("session mutex");
        for proposal in &proposals {
            let command = observe_command(&plan, proposal, evidence_model)?;
            guard.command(&command)?;
            recorded += 1;
        }
    }

    let mut response = json!({
        "ok": true,
        "relationId": plan.relation_id,
        "proposals": proposals,
        "recorded": recorded,
        "usage": usage.to_value(),
        "model": model,
        "links": SUGGEST_LINKS,
    });
    if let Some(partial) = partial {
        response["partial"] = partial;
    }
    Ok(response)
}

// ── gaps.next rankBy=uncertainty ──

#[cfg(feature = "jev")]
async fn uncertainty(
    session: &Mutex<Session>,
    judge: Option<&dyn Judge>,
    request: &Value,
) -> Result<Value, Rejection> {
    let judge = match judge {
        Some(judge) => judge,
        None => {
            return Ok(AssistUnavailable::new(
                UnavailableReason::NoKey,
                "assist judge is not configured",
            )
            .to_value(&GAPS_LINKS));
        }
    };
    let relation_id = opt_str(request, "relationId")
        .ok_or_else(|| bad_request("gaps.next rankBy=uncertainty requires 'relationId'"))?
        .to_string();
    let max_cells = max_cells(request);

    let plan = {
        let guard = session.lock().expect("session mutex");
        build_plan(&guard, &relation_id, request, max_cells)?
    };

    let mut ranked: Vec<Ranked> = Vec::new();
    let mut usage = UsageAcc::default();
    let mut model = String::new();

    for cell in &plan.cells {
        let question = match build_question(&plan, cell) {
            Ok(question) => question,
            Err(_) => break,
        };
        let mut questions = BTreeMap::new();
        questions.insert("cell".to_string(), question);
        match judge.ask(state_for(&plan, cell), questions).await {
            Ok(reply) => {
                usage.add(&reply.usage);
                if !reply.model.is_empty() {
                    model = reply.model.clone();
                }
                if let Ok((token, probabilities, confidence)) = choice_of(&reply) {
                    ranked.push(Ranked {
                        from: cell.from.clone(),
                        to: cell.to.clone(),
                        token,
                        probabilities,
                        uncertainty: 1.0 - confidence,
                    });
                }
            }
            Err(_) => break,
        }
    }

    ranked.sort_by(|a, b| {
        b.uncertainty
            .partial_cmp(&a.uncertainty)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });

    let gaps: Vec<Value> = ranked
        .iter()
        .map(|entry| {
            json!({
                "cell": { "from": entry.from, "to": entry.to },
                "uncertainty": entry.uncertainty,
                "token": entry.token,
                "probabilities": entry.probabilities,
            })
        })
        .collect();

    Ok(json!({
        "gaps": gaps,
        "heuristic": "jev uncertainty (1 - confidence)",
        "usage": usage.to_value(),
        "model": model,
        "links": GAPS_LINKS,
    }))
}

#[cfg(not(feature = "jev"))]
async fn uncertainty(
    _session: &Mutex<Session>,
    _judge: Option<&dyn Judge>,
    _request: &Value,
) -> Result<Value, Rejection> {
    Ok(AssistUnavailable::new(
        UnavailableReason::FeatureNotBuilt,
        "the jev feature is not compiled",
    )
    .to_value(&GAPS_LINKS))
}

// ── assist.members ──

async fn members(
    session: &Mutex<Session>,
    text_model: Option<&dyn TextModel>,
    request: &Value,
) -> Result<Value, Rejection> {
    let Some(text_model) = text_model else {
        return Ok(text_unavailable("assist.members").to_value(&MEMBERS_LINKS));
    };
    let dimension_id = opt_str(request, "dimensionId")
        .ok_or_else(|| bad_request("assist.members requires 'dimensionId'"))?
        .to_string();
    let source_text = opt_str(request, "sourceText")
        .ok_or_else(|| bad_request("assist.members requires 'sourceText'"))?
        .to_string();
    let source_uri = opt_str(request, "sourceUri").map(str::to_string);
    let max_members = opt_u64(request, "maxMembers").unwrap_or(20) as usize;
    let max_tokens = opt_u64(request, "maxTokens").unwrap_or(1024) as u32;

    let (label, existing) = {
        let guard = session.lock().expect("session mutex");
        let model = guard.model().ok_or_else(no_model)?;
        let dimension = model
            .dimensions()
            .iter()
            .find(|dimension| dimension.id == dimension_id)
            .ok_or_else(|| unknown_dimension(&dimension_id))?;
        (
            dimension.label.clone(),
            dimension
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>(),
        )
    };

    let members_request = MembersRequest {
        dimension_id: dimension_id.clone(),
        dimension_label: label,
        existing_member_ids: existing,
        source_text,
        source_uri,
        max_members,
        max_tokens,
    };
    match propose_members(text_model, &members_request).await {
        Ok(result) => {
            let mut response = serde_json::to_value(&result)
                .map_err(|error| bad_request(format!("serialize members: {error}")))?;
            response["ok"] = json!(true);
            response["dimensionId"] = json!(dimension_id);
            response["links"] = json!(MEMBERS_LINKS);
            Ok(response)
        }
        Err(error) => Ok(error.to_value(&MEMBERS_LINKS)),
    }
}

// ── assist.narrative ──

async fn narrative(
    session: &Mutex<Session>,
    text_model: Option<&dyn TextModel>,
    request: &Value,
) -> Result<Value, Rejection> {
    let Some(text_model) = text_model else {
        return Ok(text_unavailable("assist.narrative").to_value(&NARRATIVE_LINKS));
    };
    let relation_id = opt_str(request, "relationId").map(str::to_string);
    let max_words = opt_u64(request, "maxWords").unwrap_or(200) as usize;
    let max_tokens = opt_u64(request, "maxTokens").unwrap_or(1024) as u32;

    let (title, findings, priorities) = narrative_inputs(session, relation_id.as_deref())?;
    let narrative_request = NarrativeRequest {
        title,
        findings,
        priorities,
        max_words,
        max_tokens,
    };
    match narrate(text_model, &narrative_request).await {
        Ok(result) => Ok(json!({
            "ok": true,
            "narrative": result.narrative,
            "usage": result.usage,
            "model": result.model,
            "links": NARRATIVE_LINKS,
        })),
        Err(error) => Ok(error.to_value(&NARRATIVE_LINKS)),
    }
}

// ── report { narrative: true } ──

async fn report_narrative(
    session: &Mutex<Session>,
    text_model: Option<&dyn TextModel>,
    request: &Value,
) -> Result<Value, Rejection> {
    let relation_id = opt_str(request, "relationId").map(str::to_string);
    let mut report = {
        let guard = session.lock().expect("session mutex");
        queries::query(&guard, request)?
    };
    let markdown = report
        .get("markdown")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let Some(text_model) = text_model else {
        report["narrativeUnavailable"] = json!(text_unavailable("report").reason);
        return Ok(report);
    };

    let (title, findings, priorities) = narrative_inputs(session, relation_id.as_deref())?;
    let narrative_request = NarrativeRequest {
        title,
        findings,
        priorities,
        max_words: 200,
        max_tokens: 1024,
    };
    match narrate(text_model, &narrative_request).await {
        Ok(result) => {
            report["markdown"] = json!(format!(
                "{markdown}\n## Narrative\n\n{}\n",
                result.narrative
            ));
            report["usage"] = serde_json::to_value(&result.usage).unwrap_or(Value::Null);
            Ok(report)
        }
        Err(error) => {
            report["narrativeUnavailable"] = json!(error.reason);
            Ok(report)
        }
    }
}

/// Snapshot `(title, findings, priorities)` for a narrative call under the lock.
fn narrative_inputs(
    session: &Mutex<Session>,
    relation_id: Option<&str>,
) -> Result<(String, Value, Option<Value>), Rejection> {
    let guard = session.lock().expect("session mutex");
    let model = guard.model().ok_or_else(no_model)?;
    let findings = serde_json::to_value(model.findings())
        .map_err(|error| bad_request(format!("serialize findings: {error}")))?;
    let priorities = relation_id
        .and_then(|relation| model.marginalize(relation, crossmatrix::Axis::From).ok())
        .map(|pairs| {
            Value::Array(
                pairs
                    .into_iter()
                    .map(|(member, value)| json!({ "member": member, "value": value }))
                    .collect(),
            )
        });
    let title = relation_id
        .map(str::to_string)
        .unwrap_or_else(|| model.doc().model_id.clone());
    Ok((title, findings, priorities))
}

// ── shared jev planning ──

#[cfg(feature = "jev")]
struct MemberState {
    id: String,
    label: String,
    description: Option<String>,
    evidence: Vec<String>,
}

#[cfg(feature = "jev")]
struct DimRef {
    id: String,
    label: String,
}

#[cfg(feature = "jev")]
struct CellTask {
    from: String,
    to: String,
    from_state: MemberState,
    to_state: MemberState,
}

#[cfg(feature = "jev")]
struct SuggestPlan {
    relation_id: String,
    relation_type: String,
    from_dim: DimRef,
    to_dim: DimRef,
    criteria: BTreeMap<String, String>,
    cells: Vec<CellTask>,
    model_id: String,
}

#[cfg(feature = "jev")]
struct Ranked {
    from: String,
    to: String,
    token: String,
    probabilities: BTreeMap<String, f64>,
    uncertainty: f64,
}

#[cfg(feature = "jev")]
fn max_cells(request: &Value) -> usize {
    opt_u64(request, "maxCells")
        .map(|value| (value as usize).clamp(1, MAX_QUESTIONS_PER_CALL))
        .unwrap_or(10)
}

#[cfg(feature = "jev")]
fn build_plan(
    session: &Session,
    relation_id: &str,
    request: &Value,
    max_cells: usize,
) -> Result<SuggestPlan, Rejection> {
    let model = session.model().ok_or_else(no_model)?;
    let relation = model
        .relations()
        .iter()
        .find(|relation| relation.id == relation_id)
        .ok_or_else(|| unknown_relation(relation_id))?;
    let from_dim =
        dimension_ref(model, &relation.from).ok_or_else(|| unknown_dimension(&relation.from))?;
    let to_dim =
        dimension_ref(model, &relation.to).ok_or_else(|| unknown_dimension(&relation.to))?;
    let criteria = scale_criteria(model, &relation.scale)
        .ok_or_else(|| bad_request(format!("unknown scale '{}'", relation.scale)))?;

    let pairs = match parse_cells(request) {
        Some(given) => given.into_iter().take(max_cells).collect(),
        None => derive_candidates(model, relation, max_cells),
    };

    let cells = pairs
        .into_iter()
        .map(|(from, to)| CellTask {
            from_state: member_state(model, &from),
            to_state: member_state(model, &to),
            from,
            to,
        })
        .collect();

    Ok(SuggestPlan {
        relation_id: relation_id.to_string(),
        relation_type: relation.relation_type.to_string(),
        from_dim,
        to_dim,
        criteria,
        cells,
        model_id: model.doc().model_id.clone(),
    })
}

#[cfg(feature = "jev")]
fn parse_cells(request: &Value) -> Option<Vec<(String, String)>> {
    request
        .pointer("/query/cells")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let from = entry.get("from").and_then(Value::as_str)?;
                    let to = entry.get("to").and_then(Value::as_str)?;
                    Some((from.to_string(), to.to_string()))
                })
                .collect()
        })
}

#[cfg(feature = "jev")]
fn derive_candidates(
    model: &Model,
    relation: &Relation,
    max_cells: usize,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for gap in model.gaps_next() {
        let pairs: Vec<(String, String)> = if gap.dimension == relation.from {
            active_members(model, &relation.to)
                .into_iter()
                .filter(|member| !relation.cells.iter().any(|cell| cell.to == *member))
                .map(|member| (gap.member.clone(), member))
                .collect()
        } else if gap.dimension == relation.to {
            active_members(model, &relation.from)
                .into_iter()
                .filter(|member| !relation.cells.iter().any(|cell| cell.from == *member))
                .map(|member| (member, gap.member.clone()))
                .collect()
        } else {
            Vec::new()
        };
        for pair in pairs {
            if out.len() >= max_cells {
                return out;
            }
            if !out.contains(&pair) {
                out.push(pair);
            }
        }
    }
    out
}

#[cfg(feature = "jev")]
fn active_members(model: &Model, dimension_id: &str) -> Vec<String> {
    model
        .dimensions()
        .iter()
        .find(|dimension| dimension.id == dimension_id)
        .map(|dimension| {
            dimension
                .members
                .iter()
                .filter(|member| !is_deprecated(&member.status))
                .map(|member| member.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(feature = "jev")]
fn is_deprecated<S: serde::Serialize>(status: &Option<S>) -> bool {
    serde_json::to_value(status).is_ok_and(|value| value.as_str() == Some("deprecated"))
}

#[cfg(feature = "jev")]
fn is_proposed<S: serde::Serialize>(status: &Option<S>) -> bool {
    serde_json::to_value(status).is_ok_and(|value| value.as_str() == Some("proposed"))
}

#[cfg(feature = "jev")]
fn dimension_ref(model: &Model, dimension_id: &str) -> Option<DimRef> {
    model
        .dimensions()
        .iter()
        .find(|dimension| dimension.id == dimension_id)
        .map(|dimension| DimRef {
            id: dimension.id.clone(),
            label: dimension
                .label
                .clone()
                .unwrap_or_else(|| dimension.id.clone()),
        })
}

#[cfg(feature = "jev")]
fn scale_criteria(model: &Model, scale_id: &str) -> Option<BTreeMap<String, String>> {
    model
        .scales()
        .iter()
        .find(|scale| scale.id == scale_id)
        .map(|scale| {
            scale
                .levels
                .iter()
                .map(|level| {
                    let description = level
                        .description
                        .clone()
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| level.observation.clone());
                    (level.observation.clone(), description)
                })
                .collect()
        })
}

#[cfg(feature = "jev")]
fn member_state(model: &Model, member_id: &str) -> MemberState {
    let member = find_member(model, member_id);
    MemberState {
        id: member_id.to_string(),
        label: member
            .and_then(|member| member.label.clone())
            .unwrap_or_else(|| member_id.to_string()),
        description: member
            .and_then(|member| member.attributes.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string),
        evidence: member_evidence(model, member_id),
    }
}

#[cfg(feature = "jev")]
fn find_member<'a>(model: &'a Model, member_id: &str) -> Option<&'a Member> {
    model
        .dimensions()
        .iter()
        .flat_map(|dimension| dimension.members.iter())
        .find(|member| member.id == member_id)
}

#[cfg(feature = "jev")]
fn member_evidence(model: &Model, member_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for relation in model.relations() {
        if is_deprecated(&relation.status) {
            continue;
        }
        for cell in &relation.cells {
            if cell.from != member_id && cell.to != member_id {
                continue;
            }
            for observation in &cell.observations {
                if is_proposed(&observation.status) {
                    continue;
                }
                if let Some(rationale) = &observation.rationale
                    && !rationale.is_empty()
                {
                    out.push(rationale.clone());
                }
                for evidence in &observation.evidence {
                    if let Some(excerpt) = &evidence.excerpt
                        && !excerpt.is_empty()
                    {
                        out.push(excerpt.clone());
                    }
                }
                if out.len() >= 3 {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(feature = "jev")]
fn build_question(plan: &SuggestPlan, cell: &CellTask) -> Result<JevQuestion, AssistUnavailable> {
    let instructions = format!(
        "Under the '{}' relation, rate how strongly '{}' ({}) relates to '{}' ({}). \
         Choose exactly one token.",
        plan.relation_type,
        cell.from_state.label,
        cell.from_state.id,
        cell.to_state.label,
        cell.to_state.id
    );
    JevQuestion::choice(instructions, plan.criteria.clone())
}

#[cfg(feature = "jev")]
fn state_for(plan: &SuggestPlan, cell: &CellTask) -> Value {
    json!({
        "relation": {
            "id": plan.relation_id,
            "relationType": plan.relation_type,
            "from": { "id": plan.from_dim.id, "label": plan.from_dim.label },
            "to": { "id": plan.to_dim.id, "label": plan.to_dim.label },
        },
        "from": member_state_value(&cell.from_state),
        "to": member_state_value(&cell.to_state),
    })
}

#[cfg(feature = "jev")]
fn member_state_value(state: &MemberState) -> Value {
    let mut value = json!({
        "id": state.id,
        "label": state.label,
        "evidence": state.evidence,
    });
    if let Some(description) = &state.description {
        value["attributes"] = json!({ "description": description });
    }
    value
}

#[cfg(feature = "jev")]
fn choice_of(reply: &JevReply) -> Result<(String, BTreeMap<String, f64>, f64), AssistUnavailable> {
    match reply.answers.get("cell") {
        Some(JevAnswer::Choice {
            choice,
            probabilities,
            confidence,
        }) => Ok((choice.clone(), probabilities.clone(), *confidence)),
        _ => Err(AssistUnavailable::new(
            UnavailableReason::BadResponse,
            "judge did not return a choice answer",
        )),
    }
}

#[cfg(feature = "jev")]
fn observe_command(plan: &SuggestPlan, proposal: &Value, model: &str) -> Result<Value, Rejection> {
    let from = proposal
        .pointer("/cell/from")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("proposal missing cell.from"))?;
    let to = proposal
        .pointer("/cell/to")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("proposal missing cell.to"))?;
    let token = proposal
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("proposal missing token"))?;
    let confidence = proposal
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let probability = proposal
        .pointer(&format!("/probabilities/{token}"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let request_id = format!("jev-{}-{}-{}-{}", plan.relation_id, from, to, token);
    Ok(json!({
        "schemaVersion": "0.2.0",
        "requestId": request_id,
        "modelId": plan.model_id,
        "actor": { "actorType": "classifier", "persona": "jev" },
        "op": {
            "kind": "observe",
            "relationId": plan.relation_id,
            "from": from,
            "to": to,
            "observation": token,
            "status": "proposed",
            "provenanceClass": "inferred",
            "rationale": "Jev proposal",
            "evidence": [{
                "sourceRef": { "system": "jev", "sourceRef": model },
                "excerpt": format!("p({token})={probability}; confidence={confidence}"),
            }],
        }
    }))
}

#[cfg(feature = "jev")]
fn partial_of(error: &AssistUnavailable) -> Value {
    json!({ "reason": error.reason, "detail": error.detail })
}

#[cfg(feature = "jev")]
#[derive(Default)]
struct UsageAcc {
    input: u64,
    output: u64,
    cost: Option<f64>,
}

#[cfg(feature = "jev")]
impl UsageAcc {
    fn add(&mut self, usage: &Usage) {
        self.input += usage.input_tokens;
        self.output += usage.output_tokens;
        self.cost = match (self.cost, usage.cost_usd) {
            (Some(acc), Some(value)) => Some(acc + value),
            (Some(acc), None) => Some(acc),
            (None, Some(value)) => Some(value),
            (None, None) => None,
        };
    }

    fn to_value(&self) -> Value {
        json!({
            "inputTokens": self.input,
            "outputTokens": self.output,
            "costUsd": self.cost,
        })
    }
}
