//! crossmatrix — a sparse, N-dimensional, weighted cross-reference engine
//! (generalized House of Quality). Types are GENERATED from the canonical JSON
//! Schema via typify (single source of truth:
//! `knowledge-framework/schemas/crossmatrix.schema.json`).
//!
//! Implemented operations: `marginalize`, `contract`, `findings`
//! (stale-reference), and the traceability queries (`describe`, `slice`,
//! `trace`, `coverage`, `orphans`, `gaps_next`). Valence/tension (conflict)
//! analysis is still to be implemented (see ADR 0002/0003 in
//! `knowledge-framework/adr/`).
//!
//! Invariants the full loader must enforce (ADR 0002): observation-only (no
//! LLM numbers — already structural via the schema), valence-by-separation,
//! evidence-for-inferred, acyclic + scale-compatible contraction chains,
//! source-traced + contentHash staleness, and the precondition that weighted
//! analyses require resolvable member weights.

use std::collections::{HashMap, HashSet};

mod raw {
    #![allow(clippy::all, dead_code)]
    typify::import_types!("schema/crossmatrix.schema.json");
}

pub use raw::{
    Cell, Contraction, CrossMatrixModel, Dimension, Evidence, Finding, FindingKind, Level, Member,
    Observation, ProvenanceClass, Relation, ScaleDef, SourceRef,
};

/// Validation failure at ingestion (fail-fast; recoverable — the caller/LLM
/// retries the offending mutation with the diagnostic).
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cell(s) reference a member not in the relation's dimension: {0:?}")]
    DanglingCell(Vec<String>),
    #[error("unknown observation token(s) in relation '{relation_id}': {tokens:?}")]
    UnknownObservationToken {
        relation_id: String,
        tokens: Vec<String>,
    },
    #[error(
        "inferred observation in relation '{relation_id}' cell {from}->{to} must carry at least one evidence item"
    )]
    UnevidencedInferred {
        relation_id: String,
        from: String,
        to: String,
    },
    #[error("contraction chain is cyclic: {chain_id}")]
    CyclicContraction { chain_id: String },
    #[error("scale mismatch in contraction chain '{chain_id}': {detail}")]
    ScaleMismatch { chain_id: String, detail: String },
    #[error(
        "unweighted member(s) on axis of relation '{relation_id}' prevent weighted analysis: {members:?}"
    )]
    UnweightedMemberForWeightedAnalysis {
        relation_id: String,
        members: Vec<String>,
    },
}

/// A validated cross-matrix model. Construct only via [`Model::load`].
#[derive(Debug, Clone)]
pub struct Model {
    doc: CrossMatrixModel,
    /// dimension id -> set of member ids (built once at load).
    members_by_dim: HashMap<String, HashSet<String>>,
}

impl Model {
    /// Parse + validate (full invariant set per ADR 0002/0003).
    pub fn load(json: &str) -> Result<Self, ValidationError> {
        let doc: CrossMatrixModel = serde_json::from_str(json)?;

        let mut members_by_dim: HashMap<String, HashSet<String>> = HashMap::new();
        for d in &doc.dimensions {
            members_by_dim.insert(
                d.id.clone(),
                d.members.iter().map(|m| m.id.clone()).collect(),
            );
        }

        // Build scale -> valid observation token sets
        let mut scale_tokens: HashMap<String, HashSet<String>> = HashMap::new();
        for s in &doc.scales {
            let tokens: HashSet<String> = s.levels.iter().map(|l| l.observation.clone()).collect();
            scale_tokens.insert(s.id.clone(), tokens);
        }

        // Build relation id -> relation for contraction validation
        let mut rel_by_id: HashMap<String, &Relation> = HashMap::new();
        for r in &doc.relations {
            rel_by_id.insert(r.id.clone(), r);
        }

        // Validate contractions: acyclic + scale-compatible
        for ctr in &doc.contractions {
            // Cycle check: a chain is cyclic if any relation id appears more than once
            let mut seen: HashSet<&str> = HashSet::new();
            for rid in &ctr.chain {
                if !seen.insert(rid.as_str()) {
                    return Err(ValidationError::CyclicContraction {
                        chain_id: ctr.id.clone(),
                    });
                }
            }
            // Scale compatibility: consecutive relations must share an axis dimension
            // and their scales must be the same (for now — compose later)
            for pair in ctr.chain.windows(2) {
                let first = rel_by_id.get(&pair[0]);
                let second = rel_by_id.get(&pair[1]);
                if let (Some(a), Some(b)) = (first, second)
                    && a.to != b.from
                {
                    return Err(ValidationError::ScaleMismatch {
                        chain_id: ctr.id.clone(),
                        detail: format!(
                            "relation '{}' to='{}' but next relation '{}' from='{}' — shared axis must match",
                            a.id, a.to, b.id, b.from
                        ),
                    });
                }
            }
        }

        // Integrity: every cell's from/to resolves to a member of the relation's
        // from/to dimension; every observation token is a declared scale level;
        // every inferred observation carries evidence.
        let mut dangling = Vec::new();
        let mut unknown_tokens: Vec<(String, String)> = Vec::new();
        let mut unevidenced: Vec<(String, String, String)> = Vec::new();
        for r in &doc.relations {
            let from_set = members_by_dim.get(&r.from);
            let to_set = members_by_dim.get(&r.to);
            let valid_tokens = scale_tokens.get(&r.scale);
            for c in &r.cells {
                let from_ok = from_set.map(|s| s.contains(&c.from)).unwrap_or(false);
                let to_ok = to_set.map(|s| s.contains(&c.to)).unwrap_or(false);
                if !from_ok || !to_ok {
                    dangling.push(format!("{}:{}->{}", r.id, c.from, c.to));
                }
                if let Some(vt) = valid_tokens {
                    for obs in &c.observations {
                        if !vt.contains(&obs.observation) {
                            unknown_tokens.push((r.id.clone(), obs.observation.clone()));
                        }
                        if obs.provenance_class == Some(ProvenanceClass::Inferred)
                            && obs.evidence.is_empty()
                        {
                            unevidenced.push((r.id.clone(), c.from.clone(), c.to.clone()));
                        }
                    }
                }
            }
        }
        if !dangling.is_empty() {
            return Err(ValidationError::DanglingCell(dangling));
        }
        if !unknown_tokens.is_empty() {
            let (rel_id, tokens): (String, Vec<String>) = {
                let rel_id = unknown_tokens[0].0.clone();
                let tokens: Vec<String> = unknown_tokens.iter().map(|(_, t)| t.clone()).collect();
                (rel_id, tokens)
            };
            return Err(ValidationError::UnknownObservationToken {
                relation_id: rel_id,
                tokens,
            });
        }
        if !unevidenced.is_empty() {
            let (rel_id, from, to) = unevidenced[0].clone();
            return Err(ValidationError::UnevidencedInferred {
                relation_id: rel_id,
                from,
                to,
            });
        }

        Ok(Model {
            doc,
            members_by_dim,
        })
    }

    pub fn dimensions(&self) -> &[Dimension] {
        &self.doc.dimensions
    }
    pub fn relations(&self) -> &[Relation] {
        &self.doc.relations
    }
    pub fn contraction_ids(&self) -> Vec<&str> {
        self.doc
            .contractions
            .iter()
            .map(|c| c.id.as_str())
            .collect()
    }
    pub fn member_count(&self, dimension_id: &str) -> usize {
        self.members_by_dim
            .get(dimension_id)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    // ── analytical operations ──

    /// Weighted rollup over an axis (fan-in/fan-out). For each surviving member,
    /// sum(weight_of_rolled_member × max_observation_value) over all cells.
    pub fn marginalize(
        &self,
        relation_id: &str,
        axis: Axis,
    ) -> Result<Vec<(String, f64)>, ValidationError> {
        let rel = self
            .doc
            .relations
            .iter()
            .find(|r| r.id == relation_id)
            .ok_or_else(|| ValidationError::UnweightedMemberForWeightedAnalysis {
                relation_id: relation_id.to_string(),
                members: vec![],
            })?;

        let rolled_dim_id = match axis {
            Axis::From => &rel.from,
            Axis::To => &rel.to,
        };

        // Resolve the relation's scale tokens → values.
        let scale_map: HashMap<&str, f64> = self
            .doc
            .scales
            .iter()
            .find(|s| s.id == rel.scale)
            .map(|s| {
                s.levels
                    .iter()
                    .map(|l| (l.observation.as_str(), l.value))
                    .collect()
            })
            .unwrap_or_default();

        // Resolve member weights on the rolled-up dimension.
        let rolled_dim = self.doc.dimensions.iter().find(|d| d.id == *rolled_dim_id);

        let weight_scale_map: HashMap<&str, f64> = rolled_dim
            .and_then(|d| d.weight_scale.as_deref())
            .and_then(|ws_id| self.doc.scales.iter().find(|s| s.id == ws_id))
            .map(|s| {
                s.levels
                    .iter()
                    .map(|l| (l.observation.as_str(), l.value))
                    .collect()
            })
            .unwrap_or_default();

        let mut unweighted = Vec::new();
        let mut member_weights: HashMap<&str, f64> = HashMap::new();

        if let Some(dim) = rolled_dim {
            for m in &dim.members {
                if let Some(w) = m.weight {
                    member_weights.insert(m.id.as_str(), w);
                } else if let Some(ref wo) = m.weight_observation {
                    if let Some(&val) = weight_scale_map.get(wo.as_str()) {
                        member_weights.insert(m.id.as_str(), val);
                    } else {
                        unweighted.push(m.id.clone());
                    }
                } else {
                    unweighted.push(m.id.clone());
                }
            }
        }

        if !unweighted.is_empty() {
            return Err(ValidationError::UnweightedMemberForWeightedAnalysis {
                relation_id: relation_id.to_string(),
                members: unweighted,
            });
        }

        // Compute: per surviving-axis member, accumulate weight × max_obs.
        let mut acc: HashMap<String, f64> = HashMap::new();
        for cell in &rel.cells {
            let rolled_member = match axis {
                Axis::From => &cell.from,
                Axis::To => &cell.to,
            };
            let surviving_member = match axis {
                Axis::From => &cell.to,
                Axis::To => &cell.from,
            };

            let weight = member_weights
                .get(rolled_member.as_str())
                .copied()
                .unwrap_or(0.0);

            let max_obs = cell
                .observations
                .iter()
                .filter_map(|o| scale_map.get(o.observation.as_str()))
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
                .unwrap_or(0.0);

            *acc.entry(surviving_member.clone()).or_insert(0.0) += weight * max_obs;
        }

        let mut pairs: Vec<(String, f64)> = acc.into_iter().collect();
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(pairs)
    }
    /// Multilinear cascade over a contraction chain. Per-valence sparse matmul
    /// (A[i,j] × B[j,k]) using the deterministic observation→value scale table.
    /// Output is relative RANK within confidence bands, not a magnitude.
    pub fn contract(&self, contraction_id: &str) -> Vec<Finding> {
        let ctr = match self
            .doc
            .contractions
            .iter()
            .find(|c| c.id == contraction_id)
        {
            Some(c) => c,
            None => return Vec::new(),
        };
        if ctr.chain.is_empty() {
            return Vec::new();
        }

        // Index relations + scales by id.
        let rel_by_id: HashMap<&str, &Relation> = self
            .doc
            .relations
            .iter()
            .map(|r| (r.id.as_str(), r))
            .collect();
        let scale_by_id: HashMap<&str, &ScaleDef> =
            self.doc.scales.iter().map(|s| (s.id.as_str(), s)).collect();

        // Resolve the first relation's scale values.
        let first_rel = match rel_by_id.get(ctr.chain[0].as_str()) {
            Some(r) => r,
            None => return Vec::new(),
        };
        let mut scale_values: HashMap<&str, f64> = scale_by_id
            .get(first_rel.scale.as_str())
            .map(|s| {
                s.levels
                    .iter()
                    .map(|l| (l.observation.as_str(), l.value))
                    .collect()
            })
            .unwrap_or_default();

        // Build initial sparse matrix: (from, to) → max observation value.
        let mut current: HashMap<(String, String), f64> = HashMap::new();
        for cell in &first_rel.cells {
            let max_val = cell
                .observations
                .iter()
                .filter_map(|o| scale_values.get(o.observation.as_str()))
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
                .unwrap_or(0.0);
            if max_val > 0.0 {
                current.insert((cell.from.clone(), cell.to.clone()), max_val);
            }
        }

        // Cascade through remaining relations.
        for rid in &ctr.chain[1..] {
            let rel = match rel_by_id.get(rid.as_str()) {
                Some(r) => r,
                None => return Vec::new(),
            };
            scale_values = scale_by_id
                .get(rel.scale.as_str())
                .map(|s| {
                    s.levels
                        .iter()
                        .map(|l| (l.observation.as_str(), l.value))
                        .collect()
                })
                .unwrap_or_default();

            let mut next: HashMap<(String, String), f64> = HashMap::new();
            for ((from, mid), val_a) in &current {
                for cell in &rel.cells {
                    if &cell.from == mid {
                        let max_val_b = cell
                            .observations
                            .iter()
                            .filter_map(|o| scale_values.get(o.observation.as_str()))
                            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                            .copied()
                            .unwrap_or(0.0);

                        let combined = if matches!(
                            ctr.weight_combination,
                            raw::ContractionWeightCombination::Min
                        ) {
                            val_a.min(max_val_b)
                        } else {
                            val_a * max_val_b
                        };

                        let key = (from.clone(), cell.to.clone());
                        let entry = next.entry(key).or_insert(0.0);
                        *entry += combined;
                    }
                }
            }
            current = next;
        }

        // If weightBySourceMembers, multiply each entry by the source member's weight.
        if ctr.weight_by_source_members {
            // Resolve weights for the source dimension (from-dim of the first relation).
            let source_dim_id = &first_rel.from;
            let source_dim = self.doc.dimensions.iter().find(|d| &d.id == source_dim_id);
            let weight_scale_map: HashMap<&str, f64> = source_dim
                .and_then(|d| d.weight_scale.as_deref())
                .and_then(|ws_id| self.doc.scales.iter().find(|s| s.id == ws_id))
                .map(|s| {
                    s.levels
                        .iter()
                        .map(|l| (l.observation.as_str(), l.value))
                        .collect()
                })
                .unwrap_or_default();
            let member_weights: HashMap<&str, f64> = source_dim
                .map(|dim| {
                    dim.members
                        .iter()
                        .map(|m| {
                            let w = m.weight.unwrap_or_else(|| {
                                m.weight_observation
                                    .as_deref()
                                    .and_then(|wo| weight_scale_map.get(wo))
                                    .copied()
                                    .unwrap_or(1.0)
                            });
                            (m.id.as_str(), w)
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut weighted: HashMap<(String, String), f64> = HashMap::new();
            for ((from, to), val) in current.drain() {
                let w = member_weights.get(from.as_str()).copied().unwrap_or(1.0);
                weighted.insert((from, to), val * w);
            }
            current = weighted;
        }

        // Sort by value descending (relative rank).
        let mut entries: Vec<((String, String), f64)> = current.into_iter().collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        entries
            .into_iter()
            .map(|((from, to), value)| {
                serde_json::from_value(serde_json::json!({
                    "kind": "critical_exposure",
                    "severity": "warn",
                    "members": [from, to],
                    "relation": contraction_id,
                    "net": value,
                    "explanation": format!("contracted exposure: {:.2}", value)
                }))
                .expect("Finding json must deserialize")
            })
            .collect()
    }
    /// Deterministic issue-detection (orphan/critical_exposure/tension/...).
    pub fn findings(&self) -> Vec<Finding> {
        let mut findings = Vec::new();

        // ── stale_reference: every SourceRef with a contentHash is re-checked ──
        for dim in &self.doc.dimensions {
            for member in &dim.members {
                if let Some(ref sr) = member.source_ref {
                    self.check_source_ref_staleness(sr, &mut findings);
                }
            }
        }
        for rel in &self.doc.relations {
            for cell in &rel.cells {
                for obs in &cell.observations {
                    if let Some(ref sr) = obs.source_ref {
                        self.check_source_ref_staleness(sr, &mut findings);
                    }
                    for ev in &obs.evidence {
                        self.check_source_ref_staleness(&ev.source_ref, &mut findings);
                    }
                }
            }
        }

        findings
    }

    // ── traceability queries (Vee / QFD House-of-Quality) ──

    /// The dimension owning a member id, if any.
    pub fn dimension_of(&self, member_id: &str) -> Option<&str> {
        self.members_by_dim
            .iter()
            .find_map(|(dim, members)| members.contains(member_id).then_some(dim.as_str()))
    }

    /// The core traceability walk: every member related to `member_id` across
    /// all (non-deprecated) relations, one hop per matching cell. Direction is
    /// relative to the traced member: `Out` = it sits on the relation's `from`
    /// axis, `In` = on its `to` axis.
    pub fn trace(&self, member_id: &str) -> Vec<TraceHop> {
        let mut hops = Vec::new();
        for rel in self.doc.relations.iter().filter(|r| relation_active(r)) {
            for cell in &rel.cells {
                let observations: Vec<String> = cell
                    .observations
                    .iter()
                    .map(|o| o.observation.clone())
                    .collect();
                if cell.from == member_id {
                    hops.push(TraceHop {
                        relation: rel.id.clone(),
                        relation_type: rel.relation_type.to_string(),
                        direction: TraceDirection::Out,
                        member: cell.to.clone(),
                        dimension: rel.to.clone(),
                        observations: observations.clone(),
                    });
                }
                if cell.to == member_id {
                    hops.push(TraceHop {
                        relation: rel.id.clone(),
                        relation_type: rel.relation_type.to_string(),
                        direction: TraceDirection::In,
                        member: cell.from.clone(),
                        dimension: rel.from.clone(),
                        observations,
                    });
                }
            }
        }
        hops.sort_by(|a, b| (&a.relation, &a.member).cmp(&(&b.relation, &b.member)));
        hops
    }

    /// Per-relation, per-axis coverage: which members of the axis dimension
    /// have ≥1 cell on that relation, and which don't. `required` reflects the
    /// relation's `coveragePolicy` (the gate that turns an uncovered member
    /// from a report into a violation). Deprecated members/relations are
    /// excluded from analyses (LifecycleStatus semantics).
    pub fn coverage(&self) -> Vec<AxisCoverage> {
        let mut out = Vec::new();
        for rel in self.doc.relations.iter().filter(|r| relation_active(r)) {
            let policy = rel.coverage_policy;
            let from_required = matches!(
                policy,
                raw::RelationCoveragePolicy::EveryFrom | raw::RelationCoveragePolicy::Both
            );
            let to_required = matches!(
                policy,
                raw::RelationCoveragePolicy::EveryTo | raw::RelationCoveragePolicy::Both
            );
            let from_used: HashSet<&str> = rel.cells.iter().map(|c| c.from.as_str()).collect();
            let to_used: HashSet<&str> = rel.cells.iter().map(|c| c.to.as_str()).collect();
            for (axis, dim_id, used, required) in [
                ("from", &rel.from, &from_used, from_required),
                ("to", &rel.to, &to_used, to_required),
            ] {
                let Some(dim) = self.doc.dimensions.iter().find(|d| &d.id == dim_id) else {
                    continue; // relation onto an undeclared dimension: nothing to evaluate
                };
                let (covered, uncovered): (Vec<_>, Vec<_>) = dim
                    .members
                    .iter()
                    .filter(|m| member_active(m))
                    .map(|m| m.id.clone())
                    .partition(|id| used.contains(id.as_str()));
                out.push(AxisCoverage {
                    relation: rel.id.clone(),
                    axis: axis.to_string(),
                    dimension: dim.id.clone(),
                    required,
                    covered,
                    uncovered,
                });
            }
        }
        out
    }

    /// Traceability gaps: members that appear in NO cell of any
    /// (non-deprecated) relation, in declared dimension/member order.
    pub fn orphans(&self) -> Vec<Orphan> {
        let mut used: HashSet<&str> = HashSet::new();
        for rel in self.doc.relations.iter().filter(|r| relation_active(r)) {
            for cell in &rel.cells {
                used.insert(cell.from.as_str());
                used.insert(cell.to.as_str());
            }
        }
        self.doc
            .dimensions
            .iter()
            .flat_map(|dim| {
                dim.members
                    .iter()
                    .filter(|m| member_active(m) && !used.contains(m.id.as_str()))
                    .map(|m| Orphan {
                        dimension: dim.id.clone(),
                        member: m.id.clone(),
                    })
            })
            .collect()
    }

    /// The highest-priority gaps to close next. Heuristic: the orphans
    /// ([`Model::orphans`]) ordered by resolved importance weight descending
    /// (explicit `weight`, else `weightObservation` via the dimension's
    /// `weightScale`); unweighted members last, then declared order.
    pub fn gaps_next(&self) -> Vec<Gap> {
        let mut gaps: Vec<Gap> = self
            .orphans()
            .into_iter()
            .map(|o| {
                let weight = self
                    .doc
                    .dimensions
                    .iter()
                    .find(|d| d.id == o.dimension)
                    .and_then(|dim| {
                        let m = dim.members.iter().find(|m| m.id == o.member)?;
                        self.resolved_weight(dim, m)
                    });
                Gap {
                    dimension: o.dimension,
                    member: o.member,
                    weight,
                }
            })
            .collect();
        gaps.sort_by(|a, b| {
            let wa = a.weight.unwrap_or(f64::NEG_INFINITY);
            let wb = b.weight.unwrap_or(f64::NEG_INFINITY);
            wb.partial_cmp(&wa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| (&a.dimension, &a.member).cmp(&(&b.dimension, &b.member)))
        });
        gaps
    }

    /// Structural summary: dimensions, relations (with cell counts + coverage
    /// policy), scales, and contraction chains.
    pub fn describe(&self) -> ModelDescription {
        ModelDescription {
            model_id: self.doc.model_id.clone(),
            dimensions: self
                .doc
                .dimensions
                .iter()
                .map(|d| DimensionSummary {
                    id: d.id.clone(),
                    label: d.label.clone(),
                    kind: d.kind.as_ref().map(|k| k.to_string()),
                    members: d.members.len(),
                })
                .collect(),
            relations: self
                .doc
                .relations
                .iter()
                .map(|r| RelationSummary {
                    id: r.id.clone(),
                    from: r.from.clone(),
                    to: r.to.clone(),
                    relation_type: r.relation_type.to_string(),
                    scale: r.scale.clone(),
                    coverage_policy: r.coverage_policy.to_string(),
                    cells: r.cells.len(),
                })
                .collect(),
            scales: self.doc.scales.iter().map(|s| s.id.clone()).collect(),
            contractions: self.doc.contractions.iter().map(|c| c.id.clone()).collect(),
        }
    }

    /// The cells of one relation, optionally filtered by `from`/`to` member.
    /// `None` when the relation id is unknown (fail-fast at the caller).
    pub fn slice(
        &self,
        relation_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Option<Vec<&Cell>> {
        let rel = self.doc.relations.iter().find(|r| r.id == relation_id)?;
        Some(
            rel.cells
                .iter()
                .filter(|c| from.is_none_or(|f| c.from == f) && to.is_none_or(|t| c.to == t))
                .collect(),
        )
    }

    /// Resolve a member's importance weight: explicit `weight`, else
    /// `weightObservation` looked up in the dimension's `weightScale`.
    fn resolved_weight(&self, dim: &Dimension, m: &Member) -> Option<f64> {
        if let Some(w) = m.weight {
            return Some(w);
        }
        let ws_id = dim.weight_scale.as_deref()?;
        let scale = self.doc.scales.iter().find(|s| s.id == ws_id)?;
        let wo = m.weight_observation.as_deref()?;
        scale
            .levels
            .iter()
            .find(|l| l.observation == wo)
            .map(|l| l.value)
    }

    fn check_source_ref_staleness(&self, sr: &SourceRef, findings: &mut Vec<Finding>) {
        let content_hash = match &sr.content_hash {
            Some(h) if !h.is_empty() => h,
            _ => return,
        };

        let path = match &sr.source_ref {
            Some(p) => p.as_str(),
            None => {
                let f: Finding = serde_json::from_value(serde_json::json!({
                    "kind": "stale_reference",
                    "severity": "warn",
                    "explanation": "SourceRef missing sourceRef; cannot resolve"
                }))
                .expect("Finding json must deserialize");
                findings.push(f);
                return;
            }
        };

        // Attempt to read the referenced content; stale if missing or hash-mismatched.
        match std::fs::read_to_string(path) {
            Ok(content) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                content.hash(&mut hasher);
                let computed = format!("sip13:{}", hasher.finish());
                if &computed != content_hash {
                    let f: Finding = serde_json::from_value(serde_json::json!({
                        "kind": "stale_reference",
                        "severity": "warn",
                        "path": [path],
                        "explanation": format!(
                            "content hash mismatch for {}: expected {}, computed {}",
                            path, content_hash, computed
                        )
                    }))
                    .expect("Finding json must deserialize");
                    findings.push(f);
                }
            }
            Err(_) => {
                let f: Finding = serde_json::from_value(serde_json::json!({
                    "kind": "stale_reference",
                    "severity": "warn",
                    "path": [path],
                    "explanation": format!(
                        "cannot read referenced content: {}",
                        path
                    )
                }))
                .expect("Finding json must deserialize");
                findings.push(f);
            }
        }
    }
}

/// Which axis of a relation to roll up.
#[derive(Debug, Clone, Copy)]
pub enum Axis {
    From,
    To,
}

/// Lifecycle filter: deprecated elements are excluded from analyses.
fn member_active(m: &Member) -> bool {
    !matches!(m.status, Some(raw::LifecycleStatus::Deprecated))
}

fn relation_active(r: &Relation) -> bool {
    !matches!(r.status, Some(raw::LifecycleStatus::Deprecated))
}

/// Direction of a [`TraceHop`] relative to the traced member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceDirection {
    /// The traced member is on the relation's `from` axis.
    Out,
    /// The traced member is on the relation's `to` axis.
    In,
}

/// One hop of a traceability walk: a related member reached through a cell.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TraceHop {
    pub relation: String,
    #[serde(rename = "relationType")]
    pub relation_type: String,
    pub direction: TraceDirection,
    /// The related member on the other side of the cell.
    pub member: String,
    /// The dimension that member belongs to.
    pub dimension: String,
    /// Observation tokens recorded on the connecting cell.
    pub observations: Vec<String>,
}

/// Coverage of one axis of one relation: which members of the axis dimension
/// have ≥1 cell, and which don't.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AxisCoverage {
    pub relation: String,
    /// `"from"` or `"to"`.
    pub axis: String,
    pub dimension: String,
    /// Whether the relation's `coveragePolicy` makes uncovered members a violation.
    pub required: bool,
    pub covered: Vec<String>,
    pub uncovered: Vec<String>,
}

/// A member with no relationship at all — a traceability gap.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Orphan {
    pub dimension: String,
    pub member: String,
}

/// An orphan ranked for gap-closing priority (see [`Model::gaps_next`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Gap {
    pub dimension: String,
    pub member: String,
    /// Resolved importance weight; `None` when the member is unweighted.
    pub weight: Option<f64>,
}

/// Structural summary of a model (see [`Model::describe`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModelDescription {
    #[serde(rename = "modelId")]
    pub model_id: String,
    pub dimensions: Vec<DimensionSummary>,
    pub relations: Vec<RelationSummary>,
    pub scales: Vec<String>,
    pub contractions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DimensionSummary {
    pub id: String,
    pub label: Option<String>,
    pub kind: Option<String>,
    /// Member count.
    pub members: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RelationSummary {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "relationType")]
    pub relation_type: String,
    pub scale: String,
    #[serde(rename = "coveragePolicy")]
    pub coverage_policy: String,
    /// Cell count.
    pub cells: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arrange: the worked example, loaded + integrity-checked. One assertion
    /// per test (TDD discipline) builds on this shared Arrange step.
    fn load_example() -> Model {
        let json = include_str!("../tests/fixtures/qfd-fmeca-3axis.json");
        Model::load(json).expect("example must load + pass integrity")
    }

    #[test]
    fn example_has_at_least_three_dimensions() {
        assert!(load_example().dimensions().len() >= 3);
    }

    #[test]
    fn example_has_at_least_three_relations() {
        assert!(load_example().relations().len() >= 3);
    }

    #[test]
    fn example_req_dimension_has_at_least_three_members() {
        assert!(load_example().member_count("dim_req") >= 3);
    }

    #[test]
    fn rejects_a_dangling_cell() {
        let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
          "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                        {"id":"d2","order":1,"members":[{"id":"x"}]}],
          "relations":[{"id":"r","from":"d1","to":"d2","relationType":"supports","scale":"qfd",
            "cells":[{"from":"a","to":"NOPE","observations":[{"observation":"strong"}]}]}]}"#;
        assert!(matches!(
            Model::load(json),
            Err(ValidationError::DanglingCell(_))
        ));
    }

    #[test]
    fn rejects_inferred_observation_without_evidence() {
        let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
          "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                        {"id":"d2","order":1,"members":[{"id":"x"}]}],
          "scales":[{"id":"qfd","levels":[
            {"observation":"none","value":0,"order":0},
            {"observation":"strong","value":9,"order":3}
          ]}],
          "relations":[{"id":"r","from":"d1","to":"d2","relationType":"supports","scale":"qfd",
            "cells":[{"from":"a","to":"x","observations":[
              {"observation":"strong","provenanceClass":"inferred"}
            ]}]}]}"#;
        assert!(matches!(
            Model::load(json),
            Err(ValidationError::UnevidencedInferred { .. })
        ));
    }

    #[test]
    fn rejects_unknown_observation_token() {
        let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
          "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                        {"id":"d2","order":1,"members":[{"id":"x"}]}],
          "scales":[{"id":"qfd","levels":[
            {"observation":"none","value":0,"order":0},
            {"observation":"strong","value":9,"order":3}
          ]}],
          "relations":[{"id":"r","from":"d1","to":"d2","relationType":"supports","scale":"qfd",
            "cells":[{"from":"a","to":"x","observations":[{"observation":"imaginary"}]}]}]}"#;
        assert!(matches!(
            Model::load(json),
            Err(ValidationError::UnknownObservationToken { .. })
        ));
    }

    #[test]
    fn rejects_cyclic_contraction_chain() {
        let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
          "dimensions":[{"id":"d1","order":0,"members":[{"id":"a"}]},
                        {"id":"d2","order":1,"members":[{"id":"x"}]}],
          "scales":[{"id":"qfd","levels":[
            {"observation":"none","value":0,"order":0},
            {"observation":"strong","value":9,"order":3}
          ]}],
          "relations":[
            {"id":"r1","from":"d1","to":"d2","relationType":"supports","scale":"qfd","cells":[]},
            {"id":"r2","from":"d2","to":"d1","relationType":"supports","scale":"qfd","cells":[]}
          ],
          "contractions":[
            {"id":"c1","chain":["r1","r2","r1"]}
          ]}"#;
        assert!(matches!(
            Model::load(json),
            Err(ValidationError::CyclicContraction { .. })
        ));
    }

    #[test]
    fn marginalize_on_example_returns_per_target_values() {
        let model = load_example();
        let result = model
            .marginalize("rel_req_char", Axis::From)
            .expect("marginalize on valid example must succeed");
        assert!(
            !result.is_empty(),
            "marginalize must return at least one (member, value) pair"
        );
    }

    #[test]
    fn rejects_marginalize_on_axis_with_unweighted_member() {
        let json = r#"{"schemaVersion":"0.2.0","modelId":"t",
          "dimensions":[
            {"id":"d1","order":0,"members":[{"id":"a","weightObservation":"high"}]},
            {"id":"d2","order":1,"members":[{"id":"x"}]}
          ],
          "scales":[
            {"id":"qfd","levels":[
              {"observation":"none","value":0,"order":0},
              {"observation":"strong","value":9,"order":3}
            ]},
            {"id":"importance","levels":[
              {"observation":"high","value":9,"order":2}
            ]}
          ],
          "relations":[
            {"id":"r1","from":"d1","to":"d2","relationType":"supports","scale":"qfd",
              "cells":[
                {"from":"a","to":"x","observations":[{"observation":"strong"}]}
              ]
            }
          ]
        }"#;
        let model = Model::load(json).expect("load");
        let result = model.marginalize("r1", Axis::To);
        assert!(matches!(
            result,
            Err(ValidationError::UnweightedMemberForWeightedAnalysis { .. })
        ));
    }

    #[test]
    fn contract_on_example_returns_non_empty_entries() {
        let model = load_example();
        let result = model.contract("ctr_req_failure_exposure");
        assert!(
            !result.is_empty(),
            "contract must return at least one entry for the example contraction chain"
        );
    }

    // ── traceability queries ──

    /// Minimal 2-axis fixture: d1{a,b} × d2{x}, one cell a→x. `b` is the orphan.
    fn tiny(cells: &str) -> Model {
        let json = format!(
            r#"{{"schemaVersion":"0.2.0","modelId":"tiny",
              "dimensions":[
                {{"id":"d1","order":0,"weightScale":"importance","members":[
                  {{"id":"a","weightObservation":"low"}},
                  {{"id":"b","weightObservation":"high"}}]}},
                {{"id":"d2","order":1,"members":[{{"id":"x"}}]}}
              ],
              "scales":[
                {{"id":"qfd","levels":[
                  {{"observation":"none","value":0,"order":0}},
                  {{"observation":"strong","value":9,"order":3}}]}},
                {{"id":"importance","levels":[
                  {{"observation":"low","value":1,"order":0}},
                  {{"observation":"high","value":9,"order":1}}]}}
              ],
              "relations":[{{"id":"r1","from":"d1","to":"d2","relationType":"supports",
                "scale":"qfd","coveragePolicy":"every_from","cells":[{cells}]}}]}}"#
        );
        Model::load(&json).expect("tiny fixture must load")
    }

    #[test]
    fn trace_returns_related_members_on_the_other_axis() {
        let hops = load_example().trace("req_secure_payment");
        let related: Vec<&str> = hops.iter().map(|h| h.member.as_str()).collect();
        assert_eq!(related, vec!["char_encryption", "char_idempotent_api"]);
    }

    #[test]
    fn trace_reports_inbound_direction_for_target_members() {
        let hops = load_example().trace("fail_data_breach");
        assert!(
            !hops.is_empty() && hops.iter().all(|h| h.direction == TraceDirection::In),
            "a pure target member must trace as inbound hops"
        );
    }

    #[test]
    fn coverage_flags_uncovered_member() {
        let model = tiny(r#"{"from":"a","to":"x","observations":[{"observation":"strong"}]}"#);
        let axis = model
            .coverage()
            .into_iter()
            .find(|c| c.axis == "from")
            .expect("from-axis coverage must exist");
        assert_eq!(axis.uncovered, vec!["b".to_string()]);
    }

    #[test]
    fn coverage_required_follows_coverage_policy() {
        let model = tiny(r#"{"from":"a","to":"x","observations":[{"observation":"strong"}]}"#);
        let required: Vec<bool> = model.coverage().iter().map(|c| c.required).collect();
        // coveragePolicy every_from: from axis gated, to axis not.
        assert_eq!(required, vec![true, false]);
    }

    #[test]
    fn orphans_returns_exactly_the_unrelated_members() {
        let model = tiny(r#"{"from":"a","to":"x","observations":[{"observation":"strong"}]}"#);
        assert_eq!(
            model.orphans(),
            vec![Orphan {
                dimension: "d1".into(),
                member: "b".into()
            }]
        );
    }

    #[test]
    fn fully_covered_matrix_has_empty_orphans() {
        let model = tiny(
            r#"{"from":"a","to":"x","observations":[{"observation":"strong"}]},
               {"from":"b","to":"x","observations":[{"observation":"strong"}]}"#,
        );
        assert!(model.orphans().is_empty());
    }

    #[test]
    fn gaps_next_orders_uncovered_by_weight_descending_unweighted_last() {
        let model = tiny(""); // no cells: everything is a gap
        let gaps = model.gaps_next();
        let order: Vec<&str> = gaps.iter().map(|g| g.member.as_str()).collect();
        // b (high=9) before a (low=1) before x (unweighted).
        assert_eq!(order, vec!["b", "a", "x"]);
    }

    #[test]
    fn describe_summarizes_dimensions_relations_and_counts() {
        let d = load_example().describe();
        assert!(
            d.dimensions.len() == 3
                && d.relations
                    .iter()
                    .any(|r| r.id == "rel_req_char" && r.cells == 4),
            "describe must report 3 dimensions and rel_req_char with 4 cells"
        );
    }

    #[test]
    fn slice_filters_cells_by_from_member() {
        let model = load_example();
        let cells = model
            .slice("rel_req_char", Some("req_secure_payment"), None)
            .expect("relation must exist");
        assert_eq!(cells.len(), 2);
    }

    #[test]
    fn slice_on_unknown_relation_is_none() {
        assert!(load_example().slice("rel_nope", None, None).is_none());
    }

    #[test]
    fn stale_reference_is_reported_in_findings() {
        let model = load_example();
        let findings = model.findings();
        assert!(
            findings
                .iter()
                .any(|f| matches!(f.kind, FindingKind::StaleReference)),
            "expected at least one stale_reference finding from sources with contentHashes"
        );
    }
}
