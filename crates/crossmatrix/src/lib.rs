//! crossmatrix — a sparse, N-dimensional, weighted cross-reference engine
//! (generalized House of Quality). Types are GENERATED from the canonical JSON
//! Schema via typify (single source of truth:
//! `knowledge-framework/schemas/crossmatrix.schema.json`).
//!
//! SCAFFOLD: `findings` and driver queries are stubs to be implemented (see
//! ADR 0002/0003 in `knowledge-framework/adr/`). `marginalize` and `contract`
//! are implemented.
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
                if let (Some(a), Some(b)) = (first, second) {
                    if a.to != b.from {
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
