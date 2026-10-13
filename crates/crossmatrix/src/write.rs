//! Pure, typed write API.
//!
//! Every function takes a validated [`Model`] plus typed arguments and returns
//! a **new** validated `Model` (or a `(Model, SyncSummary)` pair). The input is
//! never mutated: each call builds a fresh [`CrossMatrixModel`] document and
//! re-validates it through [`Model::from_doc`], so a successful write is always
//! a loadable model and a rejected write is a [`WriteError`].
//!
//! The module is deliberately domain-neutral (no domain words) and pure (no fs,
//! no network, no dependencies beyond `serde_json`/`thiserror`).

use std::collections::{HashMap, HashSet};

use crate::raw;
use crate::{
    Cell, Contraction, CrossMatrixModel, Dimension, Evidence, Member, Model, Observation, Relation,
    ScaleDef, SourceRef, ValidationError,
};

/// A rejected write. Variants mirror the validation the write API performs
/// before (and the loader performs after) each mutation.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("unknown dimension: {0}")]
    UnknownDimension(String),
    #[error("unknown relation: {0}")]
    UnknownRelation(String),
    #[error("unknown scale: {0}")]
    UnknownScale(String),
    #[error("unknown member '{member}' in dimension '{dimension}'")]
    UnknownMember { dimension: String, member: String },
    #[error("unknown token '{token}' in scale '{scale}'")]
    UnknownToken { scale: String, token: String },
    #[error("unknown relation type: {0}")]
    UnknownRelationType(String),
    #[error("unknown coverage policy: {0}")]
    UnknownCoveragePolicy(String),
    #[error("unknown weight combination: {0}")]
    UnknownWeightCombination(String),
    #[error("duplicate member '{member}' in dimension '{dimension}'")]
    DuplicateMember { dimension: String, member: String },
    #[error("duplicate token '{token}' in scale '{scale}'")]
    DuplicateToken { scale: String, token: String },
    #[error("dimension '{dimension}' has no weightScale")]
    MissingWeightScale { dimension: String },
    #[error("observation-only write rejected: {0}")]
    ObservationOnly(String),
    #[error("measured token '{token}' does not match bucket '{bucket}'")]
    MeasuredMismatch { token: String, bucket: String },
    #[error("relation is deprecated: {0}")]
    RelationDeprecated(String),
    #[error("unknown cell {relation}:{from}->{to}")]
    UnknownCell {
        relation: String,
        from: String,
        to: String,
    },
    #[error("observation index {index} out of range (len {len})")]
    ObservationIndexOutOfRange { index: usize, len: usize },
    #[error("unknown target: {0}")]
    UnknownTarget(String),
    #[error("ambiguous target: {0}")]
    AmbiguousTarget(String),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
}

impl WriteError {
    /// A stable, snake_case code identifying the failure class.
    pub fn code(&self) -> &'static str {
        match self {
            WriteError::UnknownDimension(_) => "unknown_dimension",
            WriteError::UnknownRelation(_) => "unknown_relation",
            WriteError::UnknownScale(_) => "unknown_scale",
            WriteError::UnknownMember { .. } => "unknown_member",
            WriteError::UnknownToken { .. } => "unknown_token",
            WriteError::UnknownRelationType(_) => "unknown_relation_type",
            WriteError::UnknownCoveragePolicy(_) => "unknown_coverage_policy",
            WriteError::UnknownWeightCombination(_) => "unknown_weight_combination",
            WriteError::DuplicateMember { .. } => "duplicate_member",
            WriteError::DuplicateToken { .. } => "duplicate_token",
            WriteError::MissingWeightScale { .. } => "missing_weight_scale",
            WriteError::ObservationOnly(_) => "observation_only",
            WriteError::MeasuredMismatch { .. } => "measured_mismatch",
            WriteError::RelationDeprecated(_) => "relation_deprecated",
            WriteError::UnknownCell { .. } => "unknown_cell",
            WriteError::ObservationIndexOutOfRange { .. } => "observation_index_out_of_range",
            WriteError::UnknownTarget(_) => "unknown_target",
            WriteError::AmbiguousTarget(_) => "ambiguous_target",
            WriteError::Invalid(_) => "invalid",
        }
    }
}

/// A dimension upsert: `Some` metadata fields overwrite; members are untouched.
pub struct DimensionSpec {
    pub id: String,
    pub kind: Option<String>,
    pub label: Option<String>,
    pub system: Option<String>,
    pub weight_scale: Option<String>,
    pub weight_semantics: Option<String>,
    pub order: Option<i64>,
}

/// A relation upsert. An existing relation keeps its cells and status.
pub struct RelationSpec {
    pub id: String,
    pub from: String,
    pub to: String,
    pub relation_type: String,
    pub scale: String,
    pub coverage_policy: Option<String>,
}

/// A contraction upsert.
pub struct ContractionSpec {
    pub id: String,
    pub chain: Vec<String>,
    pub label: Option<String>,
    pub weight_combination: Option<String>,
    pub weight_by_source_members: Option<bool>,
}

/// One roster entry for [`sync_members`] / one proposal for [`propose_member`].
pub struct MemberSpec {
    pub id: String,
    pub label: Option<String>,
    pub weight_observation: Option<String>,
    pub weight: Option<f64>,
    pub source_ref: Option<SourceRef>,
    pub attributes: Option<serde_json::Map<String, serde_json::Value>>,
}

/// What [`sync_members`] changed, in roster/declared order.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct SyncSummary {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub deprecated: Vec<String>,
    pub reactivated: Vec<String>,
}

impl SyncSummary {
    /// True when the sync changed nothing (no adds, updates, deprecations, or
    /// reactivations); `unchanged` entries do not count as a change.
    pub fn is_noop(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.deprecated.is_empty()
            && self.reactivated.is_empty()
    }
}

/// The kind of element [`deprecate`] targets when the id alone is ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Member,
    Relation,
}

#[derive(Clone, Copy)]
enum ResolvedTarget {
    Member,
    Relation,
}

fn order_to_u64(order: i64) -> u64 {
    order.max(0) as u64
}

fn relation_type_from_str(value: &str) -> Option<raw::RelationType> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

fn coverage_policy_from_str(value: &str) -> Option<raw::RelationCoveragePolicy> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

fn weight_combination_from_str(value: &str) -> Option<raw::ContractionWeightCombination> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

fn is_deprecated(status: Option<&raw::LifecycleStatus>) -> bool {
    matches!(status, Some(raw::LifecycleStatus::Deprecated))
}

/// `(dimension index, member index)` of the first member with this id.
fn member_location(doc: &CrossMatrixModel, id: &str) -> Option<(usize, usize)> {
    doc.dimensions
        .iter()
        .enumerate()
        .find_map(|(di, d)| d.members.iter().position(|m| m.id == id).map(|mi| (di, mi)))
}

fn member_owner<'a>(doc: &'a CrossMatrixModel, id: &str) -> Option<&'a str> {
    doc.dimensions.iter().find_map(|d| {
        d.members
            .iter()
            .any(|m| m.id == id)
            .then_some(d.id.as_str())
    })
}

fn observation_json_eq(a: &Observation, b: &Observation) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn source_ref_json_eq(a: &Option<SourceRef>, b: &Option<SourceRef>) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

fn member_content_differs(m: &Member, spec: &MemberSpec) -> bool {
    m.label != spec.label
        || m.weight_observation != spec.weight_observation
        || m.weight != spec.weight
        || !source_ref_json_eq(&m.source_ref, &spec.source_ref)
        || m.attributes != spec.attributes.clone().unwrap_or_default()
}

/// The shared weight/weightObservation rules for [`sync_members`] and
/// [`propose_member`].
fn validate_member_spec(
    dimension_id: &str,
    dim_weight_scale: Option<&str>,
    scales: &[ScaleDef],
    spec: &MemberSpec,
) -> Result<(), WriteError> {
    if spec.weight.is_some() {
        let deterministic = spec
            .source_ref
            .as_ref()
            .and_then(|sr| sr.system.as_deref())
            .is_some_and(|s| !s.is_empty());
        if !deterministic {
            return Err(WriteError::ObservationOnly(spec.id.clone()));
        }
    }
    if let Some(token) = &spec.weight_observation {
        let ws = dim_weight_scale.ok_or_else(|| WriteError::MissingWeightScale {
            dimension: dimension_id.to_string(),
        })?;
        let known = scales
            .iter()
            .find(|s| s.id == ws)
            .is_some_and(|s| s.levels.iter().any(|l| &l.observation == token));
        if !known {
            return Err(WriteError::UnknownToken {
                scale: ws.to_string(),
                token: token.clone(),
            });
        }
    }
    Ok(())
}

fn member_from_spec(spec: &MemberSpec) -> Member {
    Member {
        id: spec.id.clone(),
        label: spec.label.clone(),
        weight_observation: spec.weight_observation.clone(),
        weight: spec.weight,
        source_ref: spec.source_ref.clone(),
        attributes: spec.attributes.clone().unwrap_or_default(),
        status: None,
        superseded_by: None,
    }
}

/// Upsert a dimension by id. New dimensions are appended with empty members;
/// existing dimensions keep their members and only have `Some` metadata
/// overwritten. `weight_scale` must name a declared scale.
pub fn register_dimension(model: &Model, spec: &DimensionSpec) -> Result<Model, WriteError> {
    if let Some(ws) = &spec.weight_scale
        && !model.scales().iter().any(|s| &s.id == ws)
    {
        return Err(WriteError::UnknownScale(ws.clone()));
    }

    let mut doc = CrossMatrixModel::clone(model.doc());
    if let Some(d) = doc.dimensions.iter_mut().find(|d| d.id == spec.id) {
        if let Some(kind) = &spec.kind {
            d.kind = Some(kind.clone());
        }
        if let Some(label) = &spec.label {
            d.label = Some(label.clone());
        }
        if let Some(system) = &spec.system {
            d.system = Some(system.clone());
        }
        if let Some(ws) = &spec.weight_scale {
            d.weight_scale = Some(ws.clone());
        }
        if let Some(semantics) = &spec.weight_semantics {
            d.weight_semantics = Some(semantics.clone());
        }
        if let Some(order) = spec.order {
            d.order = order_to_u64(order);
        }
    } else {
        let order = match spec.order {
            Some(order) => order_to_u64(order),
            None => doc
                .dimensions
                .iter()
                .map(|d| d.order)
                .max()
                .map(|m| m + 1)
                .unwrap_or(0),
        };
        doc.dimensions.push(Dimension {
            id: spec.id.clone(),
            kind: spec.kind.clone(),
            label: spec.label.clone(),
            members: Vec::new(),
            order,
            system: spec.system.clone(),
            weight_scale: spec.weight_scale.clone(),
            weight_semantics: spec.weight_semantics.clone(),
        });
    }
    Ok(Model::from_doc(doc)?)
}

/// Upsert a scale by id (replacing the whole [`ScaleDef`]). Duplicate level
/// tokens are rejected; a replacement that orphans a token already used by a
/// cell fails loader validation.
pub fn declare_scale(model: &Model, scale: ScaleDef) -> Result<Model, WriteError> {
    {
        let mut seen: HashSet<&str> = HashSet::new();
        for level in &scale.levels {
            if !seen.insert(level.observation.as_str()) {
                return Err(WriteError::DuplicateToken {
                    scale: scale.id.clone(),
                    token: level.observation.clone(),
                });
            }
        }
    }

    let mut doc = CrossMatrixModel::clone(model.doc());
    match doc.scales.iter_mut().find(|s| s.id == scale.id) {
        Some(existing) => *existing = scale,
        None => doc.scales.push(scale),
    }
    Ok(Model::from_doc(doc)?)
}

/// Upsert a relation by id. An existing relation keeps its cells and status.
pub fn declare_relation(model: &Model, spec: &RelationSpec) -> Result<Model, WriteError> {
    let doc = model.doc();
    if !doc.dimensions.iter().any(|d| d.id == spec.from) {
        return Err(WriteError::UnknownDimension(spec.from.clone()));
    }
    if !doc.dimensions.iter().any(|d| d.id == spec.to) {
        return Err(WriteError::UnknownDimension(spec.to.clone()));
    }
    if !doc.scales.iter().any(|s| s.id == spec.scale) {
        return Err(WriteError::UnknownScale(spec.scale.clone()));
    }
    let relation_type = relation_type_from_str(&spec.relation_type)
        .ok_or_else(|| WriteError::UnknownRelationType(spec.relation_type.clone()))?;
    let coverage_policy = match &spec.coverage_policy {
        Some(policy) => coverage_policy_from_str(policy)
            .ok_or_else(|| WriteError::UnknownCoveragePolicy(policy.clone()))?,
        None => raw::RelationCoveragePolicy::None,
    };

    let mut next = CrossMatrixModel::clone(doc);
    match next.relations.iter_mut().find(|r| r.id == spec.id) {
        Some(r) => {
            r.from = spec.from.clone();
            r.to = spec.to.clone();
            r.relation_type = relation_type;
            r.scale = spec.scale.clone();
            r.coverage_policy = coverage_policy;
        }
        None => next.relations.push(Relation {
            id: spec.id.clone(),
            from: spec.from.clone(),
            to: spec.to.clone(),
            relation_type,
            scale: spec.scale.clone(),
            coverage_policy,
            status: None,
            superseded_by: None,
            cells: Vec::new(),
        }),
    }
    Ok(Model::from_doc(next)?)
}

/// Upsert a contraction by id. Every chain relation must exist; acyclic and
/// axis compatibility are enforced by the loader.
pub fn declare_contraction(model: &Model, spec: &ContractionSpec) -> Result<Model, WriteError> {
    let doc = model.doc();
    for rid in &spec.chain {
        if !doc.relations.iter().any(|r| &r.id == rid) {
            return Err(WriteError::UnknownRelation(rid.clone()));
        }
    }
    let weight_combination = match &spec.weight_combination {
        Some(combination) => weight_combination_from_str(combination)
            .ok_or_else(|| WriteError::UnknownWeightCombination(combination.clone()))?,
        None => raw::ContractionWeightCombination::Product,
    };

    let mut next = CrossMatrixModel::clone(doc);
    match next.contractions.iter_mut().find(|c| c.id == spec.id) {
        Some(c) => {
            c.chain = spec.chain.clone();
            if let Some(label) = &spec.label {
                c.label = Some(label.clone());
            }
            if spec.weight_combination.is_some() {
                c.weight_combination = weight_combination;
            }
            if let Some(weight_by_source) = spec.weight_by_source_members {
                c.weight_by_source_members = weight_by_source;
            }
        }
        None => next.contractions.push(Contraction {
            id: spec.id.clone(),
            label: spec.label.clone(),
            chain: spec.chain.clone(),
            weight_combination,
            weight_by_source_members: spec.weight_by_source_members.unwrap_or(true),
        }),
    }
    Ok(Model::from_doc(next)?)
}

/// Make the roster authoritative for one dimension. Members absent from the
/// roster are deprecated (never deleted); re-listed members are reactivated.
pub fn sync_members(
    model: &Model,
    dimension_id: &str,
    roster: &[MemberSpec],
) -> Result<(Model, SyncSummary), WriteError> {
    let doc = model.doc();
    let dim_index = doc
        .dimensions
        .iter()
        .position(|d| d.id == dimension_id)
        .ok_or_else(|| WriteError::UnknownDimension(dimension_id.to_string()))?;
    let dim_weight_scale = doc.dimensions[dim_index].weight_scale.clone();

    // Index every member once: owner lookup is O(1) instead of a linear scan
    // per roster entry, keeping the whole sync O(total members + cells).
    let mut owner_by_member: HashMap<&str, &str> = HashMap::new();
    for d in &doc.dimensions {
        for m in &d.members {
            owner_by_member
                .entry(m.id.as_str())
                .or_insert(d.id.as_str());
        }
    }

    for spec in roster {
        if let Some(owner) = owner_by_member.get(spec.id.as_str())
            && *owner != dimension_id
        {
            return Err(WriteError::DuplicateMember {
                dimension: dimension_id.to_string(),
                member: spec.id.clone(),
            });
        }
        validate_member_spec(dimension_id, dim_weight_scale.as_deref(), &doc.scales, spec)?;
    }

    let mut next = CrossMatrixModel::clone(doc);
    let mut summary = SyncSummary::default();
    let roster_ids: HashSet<&str> = roster.iter().map(|s| s.id.as_str()).collect();

    // Deprecate members absent from the roster (declared order).
    {
        let dim = &mut next.dimensions[dim_index];
        for member in dim.members.iter_mut() {
            if !roster_ids.contains(member.id.as_str()) && !is_deprecated(member.status.as_ref()) {
                member.status = Some(raw::LifecycleStatus::Deprecated);
                summary.deprecated.push(member.id.clone());
            }
        }
    }

    // Member id -> position within the target dimension, maintained as we add,
    // so upsert never does a per-entry linear `position` scan.
    let mut index_by_id: HashMap<String, usize> = next.dimensions[dim_index]
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| (member.id.clone(), index))
        .collect();

    // Upsert roster entries (roster order).
    for spec in roster {
        let existing = index_by_id.get(spec.id.as_str()).copied();
        match existing {
            Some(index) => {
                let was_deprecated =
                    is_deprecated(next.dimensions[dim_index].members[index].status.as_ref());
                let changed =
                    member_content_differs(&next.dimensions[dim_index].members[index], spec);
                {
                    let member = &mut next.dimensions[dim_index].members[index];
                    member.label = spec.label.clone();
                    member.weight_observation = spec.weight_observation.clone();
                    member.weight = spec.weight;
                    member.source_ref = spec.source_ref.clone();
                    member.attributes = spec.attributes.clone().unwrap_or_default();
                    if was_deprecated {
                        member.status = None;
                        member.superseded_by = None;
                    }
                }
                if was_deprecated {
                    summary.reactivated.push(spec.id.clone());
                } else if changed {
                    summary.updated.push(spec.id.clone());
                } else {
                    summary.unchanged.push(spec.id.clone());
                }
            }
            None => {
                let new_index = next.dimensions[dim_index].members.len();
                next.dimensions[dim_index]
                    .members
                    .push(member_from_spec(spec));
                index_by_id.insert(spec.id.clone(), new_index);
                summary.added.push(spec.id.clone());
            }
        }
    }

    Ok((Model::from_doc(next)?, summary))
}

/// Propose one new active member. Errors if the id already exists in any
/// dimension.
pub fn propose_member(
    model: &Model,
    dimension_id: &str,
    spec: &MemberSpec,
) -> Result<Model, WriteError> {
    let doc = model.doc();
    let dim_index = doc
        .dimensions
        .iter()
        .position(|d| d.id == dimension_id)
        .ok_or_else(|| WriteError::UnknownDimension(dimension_id.to_string()))?;
    if member_owner(doc, &spec.id).is_some() {
        return Err(WriteError::DuplicateMember {
            dimension: dimension_id.to_string(),
            member: spec.id.clone(),
        });
    }
    validate_member_spec(
        dimension_id,
        doc.dimensions[dim_index].weight_scale.as_deref(),
        &doc.scales,
        spec,
    )?;

    let mut next = CrossMatrixModel::clone(doc);
    next.dimensions[dim_index]
        .members
        .push(member_from_spec(spec));
    Ok(Model::from_doc(next)?)
}

/// Append one observation to a cell (creating the cell when absent). An
/// observation exactly equal to one already on the cell is a no-op.
pub fn observe(
    model: &Model,
    relation_id: &str,
    from: &str,
    to: &str,
    observation: Observation,
) -> Result<Model, WriteError> {
    let doc = model.doc();
    let rel_index = doc
        .relations
        .iter()
        .position(|r| r.id == relation_id)
        .ok_or_else(|| WriteError::UnknownRelation(relation_id.to_string()))?;
    if is_deprecated(doc.relations[rel_index].status.as_ref()) {
        return Err(WriteError::RelationDeprecated(relation_id.to_string()));
    }
    let relation = &doc.relations[rel_index];

    let from_known = doc
        .dimensions
        .iter()
        .find(|d| d.id == relation.from)
        .is_some_and(|d| d.members.iter().any(|m| m.id == from));
    if !from_known {
        return Err(WriteError::UnknownMember {
            dimension: relation.from.clone(),
            member: from.to_string(),
        });
    }
    let to_known = doc
        .dimensions
        .iter()
        .find(|d| d.id == relation.to)
        .is_some_and(|d| d.members.iter().any(|m| m.id == to));
    if !to_known {
        return Err(WriteError::UnknownMember {
            dimension: relation.to.clone(),
            member: to.to_string(),
        });
    }

    let scale = doc.scales.iter().find(|s| s.id == relation.scale);
    let token_known = scale.is_some_and(|s| {
        s.levels
            .iter()
            .any(|l| l.observation == observation.observation)
    });
    if !token_known {
        return Err(WriteError::UnknownToken {
            scale: relation.scale.clone(),
            token: observation.observation.clone(),
        });
    }

    if let Some(value) = observation.measured_value {
        let observed = matches!(
            observation.provenance_class,
            Some(crate::ProvenanceClass::Observed)
        );
        if !observed || observation.source_ref.is_none() {
            return Err(WriteError::ObservationOnly(observation.observation.clone()));
        }
        if let Some(scale) = scale
            && let Some(level) = scale.levels.iter().find(|l| {
                l.range.as_ref().is_some_and(|r| {
                    let min = r.min.unwrap_or(f64::NEG_INFINITY);
                    let max = r.max.unwrap_or(f64::INFINITY);
                    value >= min && value <= max
                })
            })
            && level.observation != observation.observation
        {
            return Err(WriteError::MeasuredMismatch {
                token: observation.observation.clone(),
                bucket: level.observation.clone(),
            });
        }
    }

    let mut next = CrossMatrixModel::clone(doc);
    let relation = &mut next.relations[rel_index];
    match relation
        .cells
        .iter()
        .position(|c| c.from == from && c.to == to)
    {
        Some(index) => {
            let cell = &mut relation.cells[index];
            let duplicate = cell
                .observations
                .iter()
                .any(|existing| observation_json_eq(existing, &observation));
            if !duplicate {
                cell.observations.push(observation);
            }
        }
        None => relation.cells.push(Cell {
            from: from.to_string(),
            to: to.to_string(),
            observations: vec![observation],
        }),
    }
    Ok(Model::from_doc(next)?)
}

/// Attach evidence to an existing cell's observation (the last one by default).
pub fn attach_evidence(
    model: &Model,
    relation_id: &str,
    from: &str,
    to: &str,
    observation_index: Option<usize>,
    evidence: Evidence,
) -> Result<Model, WriteError> {
    let doc = model.doc();
    let unknown_cell = || WriteError::UnknownCell {
        relation: relation_id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
    };
    let rel_index = doc
        .relations
        .iter()
        .position(|r| r.id == relation_id)
        .ok_or_else(unknown_cell)?;
    let cell_index = doc.relations[rel_index]
        .cells
        .iter()
        .position(|c| c.from == from && c.to == to)
        .ok_or_else(unknown_cell)?;
    let len = doc.relations[rel_index].cells[cell_index]
        .observations
        .len();
    let index = match observation_index {
        Some(index) => index,
        None => {
            if len == 0 {
                return Err(WriteError::ObservationIndexOutOfRange { index: 0, len });
            }
            len - 1
        }
    };
    if index >= len {
        return Err(WriteError::ObservationIndexOutOfRange { index, len });
    }

    let mut next = CrossMatrixModel::clone(doc);
    next.relations[rel_index].cells[cell_index].observations[index]
        .evidence
        .push(evidence);
    Ok(Model::from_doc(next)?)
}

/// Deprecate a member or relation, optionally recording a same-kind successor.
pub fn deprecate(
    model: &Model,
    target_id: &str,
    kind: Option<TargetKind>,
    superseded_by: Option<&str>,
) -> Result<Model, WriteError> {
    let doc = model.doc();
    let member = member_location(doc, target_id);
    let relation = doc.relations.iter().position(|r| r.id == target_id);
    let resolved = match kind {
        Some(TargetKind::Member) => {
            if member.is_none() {
                return Err(WriteError::UnknownTarget(target_id.to_string()));
            }
            ResolvedTarget::Member
        }
        Some(TargetKind::Relation) => {
            if relation.is_none() {
                return Err(WriteError::UnknownTarget(target_id.to_string()));
            }
            ResolvedTarget::Relation
        }
        None => match (member.is_some(), relation.is_some()) {
            (true, true) => return Err(WriteError::AmbiguousTarget(target_id.to_string())),
            (true, false) => ResolvedTarget::Member,
            (false, true) => ResolvedTarget::Relation,
            (false, false) => return Err(WriteError::UnknownTarget(target_id.to_string())),
        },
    };

    if let Some(successor) = superseded_by {
        let exists = match resolved {
            ResolvedTarget::Member => member_location(doc, successor).is_some(),
            ResolvedTarget::Relation => doc.relations.iter().any(|r| r.id == successor),
        };
        if !exists {
            return Err(WriteError::UnknownTarget(successor.to_string()));
        }
    }

    let mut next = CrossMatrixModel::clone(doc);
    match resolved {
        ResolvedTarget::Member => {
            let (dim_index, member_index) = member.expect("resolved member exists");
            let member = &mut next.dimensions[dim_index].members[member_index];
            member.status = Some(raw::LifecycleStatus::Deprecated);
            if let Some(successor) = superseded_by {
                member.superseded_by = Some(successor.to_string());
            }
        }
        ResolvedTarget::Relation => {
            let rel_index = relation.expect("resolved relation exists");
            let relation = &mut next.relations[rel_index];
            relation.status = Some(raw::LifecycleStatus::Deprecated);
            if let Some(successor) = superseded_by {
                relation.superseded_by = Some(successor.to_string());
            }
        }
    }
    Ok(Model::from_doc(next)?)
}
