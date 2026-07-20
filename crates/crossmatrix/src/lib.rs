//! crossmatrix — a sparse, N-dimensional, weighted cross-reference engine
//! (generalized House of Quality). Types are GENERATED from the canonical JSON
//! Schema via typify (single source of truth:
//! `knowledge-framework/schemas/crossmatrix.schema.json`).
//!
//! Implemented operations: `marginalize`, `contract`, `findings`
//! (stale-reference + tension/conflict), `tensions` (valence-by-separation),
//! and the traceability queries (`describe`, `slice`, `trace`, `coverage`,
//! `orphans`, `gaps_next`). See ADR 0002/0003 in `adr/`.
//!
//! Invariants the full loader must enforce (ADR 0002): observation-only (no
//! LLM numbers — already structural via the schema), valence-by-separation,
//! evidence-for-inferred, acyclic + scale-compatible contraction chains,
//! source-traced + contentHash staleness, and the precondition that weighted
//! analyses require resolvable member weights.

use std::collections::{BTreeSet, HashMap, HashSet};

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

/// A sparse contraction frontier: `(source, current endpoint)` -> the
/// accumulated value plus the set of intermediate members traversed to reach it
/// (the lineage that populates a [`Finding`]'s `path`).
type ContractionFrontier = HashMap<(String, String), (f64, BTreeSet<String>)>;

/// One drained [`ContractionFrontier`] entry, ready to rank.
type ContractionEntry = ((String, String), (f64, BTreeSet<String>));

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

        // Build initial sparse matrix: (from, to) → (max observation value, the
        // set of intermediate members traversed to get there — empty at hop 1).
        let mut current: ContractionFrontier = HashMap::new();
        for cell in &first_rel.cells {
            let max_val = cell
                .observations
                .iter()
                .filter_map(|o| scale_values.get(o.observation.as_str()))
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
                .unwrap_or(0.0);
            if max_val > 0.0 {
                current.insert(
                    (cell.from.clone(), cell.to.clone()),
                    (max_val, BTreeSet::new()),
                );
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

            let mut next: ContractionFrontier = HashMap::new();
            for ((from, mid), (val_a, seen)) in &current {
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
                        let entry = next.entry(key).or_insert((0.0, BTreeSet::new()));
                        entry.0 += combined;
                        // Paths that merge into the same endpoints are summed, so
                        // the lineage is the UNION of every intermediate traversed.
                        entry.1.extend(seen.iter().cloned());
                        entry.1.insert(mid.clone());
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

            let mut weighted: ContractionFrontier = HashMap::new();
            for ((from, to), (val, seen)) in current.drain() {
                let w = member_weights.get(from.as_str()).copied().unwrap_or(1.0);
                weighted.insert((from, to), (val * w, seen));
            }
            current = weighted;
        }

        // Sort by value descending (relative rank), id-tiebroken for determinism.
        let mut entries: Vec<ContractionEntry> = current.into_iter().collect();
        entries.sort_by(|a, b| {
            b.1.0
                .partial_cmp(&a.1.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        // A contraction produces a RANKING, so entries are `weighted_priority`
        // at `info` — a high rank is the *good* case and must not read as an
        // alarm. An entry is promoted to `critical_exposure`/`warn` only when it
        // actually warrants attention: when the path it rides on touches a member
        // that tension analysis reports as being in conflict. High priority
        // resting on contested ground is the case worth surfacing.
        let contested: HashSet<String> = self
            .tensions()
            .iter()
            .filter(|f| matches!(f.kind, FindingKind::Conflict))
            .flat_map(|f| f.members.iter().cloned())
            .collect();

        entries
            .into_iter()
            .map(|((from, to), (value, seen))| {
                let mut path: Vec<String> = Vec::with_capacity(seen.len() + 2);
                path.push(from.clone());
                path.extend(seen.iter().cloned());
                path.push(to.clone());

                let hit: Vec<&String> = path.iter().filter(|m| contested.contains(*m)).collect();

                let (kind, severity, explanation) = if hit.is_empty() {
                    (
                        "weighted_priority",
                        "info",
                        format!("contracted priority: {value:.2}"),
                    )
                } else {
                    (
                        "critical_exposure",
                        "warn",
                        format!(
                            "contracted priority {value:.2}, but the path rests on contested \
                             member(s): {}",
                            hit.iter()
                                .map(|m| m.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                };

                serde_json::from_value(serde_json::json!({
                    "kind": kind,
                    "severity": severity,
                    "members": [from, to],
                    "path": path,
                    "relation": contraction_id,
                    "net": value,
                    "explanation": explanation,
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

        findings.extend(self.tensions());

        findings
    }

    /// Valence-by-separation tension analysis (ADR 0002). For each member, the
    /// **inbound** supporting and opposing forces are accumulated *separately*
    /// and never folded into one signed scale, so strong support and strong
    /// opposition cannot cancel into a misleading zero.
    ///
    /// `net` is `support - oppose` (relative rank, not a probability);
    /// `tension` is `min(support, oppose)` — precisely the magnitude a bipolar
    /// net-zero would have hidden.
    ///
    /// Contributions are normalised to each relation's own scale maximum before
    /// being combined: a `9` on a 0..9 scale and a `9` on a 0..100 scale are not
    /// commensurable, and a member is routinely acted on through relations that
    /// use different scales. They are deliberately NOT importance-weighted —
    /// this answers "what forces act on this member", not "what should I fix
    /// first" (that is [`Model::gaps_next`]).
    ///
    /// Emits `conflict` (severity `block`) when opposition meets or exceeds
    /// support, and `tension` (severity `warn`) when both are present but
    /// support still dominates. Relation types carrying neither valence
    /// (`derives_from`, `constrains`, `dependsOn`) are structural and skipped;
    /// deprecated members and relations are excluded.
    pub fn tensions(&self) -> Vec<Finding> {
        #[derive(Default)]
        struct Acc {
            support: f64,
            oppose: f64,
            /// (label, normalised magnitude, is_oppose)
            drivers: Vec<(String, f64, bool)>,
        }

        let deprecated: HashSet<&str> = self
            .doc
            .dimensions
            .iter()
            .flat_map(|d| d.members.iter())
            .filter(|m| !member_active(m))
            .map(|m| m.id.as_str())
            .collect();

        let mut acc: HashMap<&str, Acc> = HashMap::new();

        for rel in self.doc.relations.iter().filter(|r| relation_active(r)) {
            let Some(oppose) = relation_opposes(&rel.relation_type) else {
                continue; // structural relation: no valence to separate
            };
            let (scale_map, scale_max) = self.scale_table(&rel.scale);
            if scale_max <= 0.0 {
                continue; // degenerate or unknown scale: nothing commensurable
            }

            for cell in &rel.cells {
                if deprecated.contains(cell.from.as_str()) || deprecated.contains(cell.to.as_str())
                {
                    continue;
                }
                // Same multi-observation convention as `marginalize`: strongest wins.
                let raw = cell
                    .observations
                    .iter()
                    .filter_map(|o| scale_map.get(o.observation.as_str()))
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .copied()
                    .unwrap_or(0.0);
                if raw <= 0.0 {
                    continue; // an explicit "no effect" is not a force
                }

                let norm = raw / scale_max;
                let entry = acc.entry(cell.to.as_str()).or_default();
                if oppose {
                    entry.oppose += norm;
                } else {
                    entry.support += norm;
                }
                entry
                    .drivers
                    .push((format!("{}:{}", rel.id, cell.from), norm, oppose));
            }
        }

        let mut out: Vec<(f64, String, Finding)> = Vec::new();

        for (member, a) in acc {
            if a.oppose <= 0.0 {
                continue; // nothing opposing: not a tension, by definition
            }

            let tension = a.support.min(a.oppose);
            let net = a.support - a.oppose;
            // Opposition winning (or tying) is a conflict; opposition present but
            // outweighed is a tension the user should still see.
            let (kind, severity) = if net <= 0.0 {
                ("conflict", "block")
            } else {
                ("tension", "warn")
            };

            // Minimal sensitivity: the drivers that actually move the number,
            // opposing side first, each ranked by its own share.
            let mut drivers = a.drivers;
            drivers.sort_by(|x, y| {
                y.2.cmp(&x.2)
                    .then_with(|| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal))
                    .then_with(|| x.0.cmp(&y.0))
            });
            let contributors: Vec<String> =
                drivers.into_iter().map(|(label, _, _)| label).collect();

            let explanation = format!(
                "{member}: support {:.2} vs opposition {:.2} (net {net:+.2}, tension {tension:.2}) \
                 — normalised across {} contributing cell(s)",
                a.support,
                a.oppose,
                contributors.len()
            );

            let finding: Finding = serde_json::from_value(serde_json::json!({
                "kind": kind,
                "severity": severity,
                "members": [member],
                "path": [member],
                "contributors": contributors,
                "net": net,
                "tension": tension,
                "explanation": explanation,
            }))
            .expect("Finding json must deserialize");

            out.push((tension, member.to_string(), finding));
        }

        // Deterministic: strongest hidden tension first, then member id.
        out.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        out.into_iter().map(|(_, _, f)| f).collect()
    }

    /// A scale's `observation -> value` table plus its maximum level value
    /// (the normalisation denominator). Empty/`0.0` when the scale is unknown.
    fn scale_table(&self, scale_id: &str) -> (HashMap<&str, f64>, f64) {
        let Some(scale) = self.doc.scales.iter().find(|s| s.id == scale_id) else {
            return (HashMap::new(), 0.0);
        };
        let map: HashMap<&str, f64> = scale
            .levels
            .iter()
            .map(|l| (l.observation.as_str(), l.value))
            .collect();
        let max = scale
            .levels
            .iter()
            .map(|l| l.value)
            .fold(f64::NEG_INFINITY, f64::max);
        (map, if max.is_finite() { max } else { 0.0 })
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

/// The valence of a relation type: `Some(false)` supporting, `Some(true)`
/// opposing, `None` structural (carries no valence, so it takes no part in
/// tension analysis). Kept exhaustive on purpose — a new `RelationType` in the
/// schema must fail to compile here rather than silently default to structural.
fn relation_opposes(rt: &raw::RelationType) -> Option<bool> {
    use raw::RelationType as R;
    match rt {
        R::Satisfies | R::Supports | R::Implements | R::Verifies | R::Covers | R::Mitigates => {
            Some(false)
        }
        R::Undermines | R::Opposes | R::Blocks | R::Exposes => Some(true),
        // `constrains` bounds a member rather than arguing against it, and
        // `derives_from`/`dependsOn` are pure structure (the latter routes to
        // cyclic/SCC analysis, not valence).
        R::DerivesFrom | R::Constrains | R::DependsOn => None,
    }
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

    // ── valence-by-separation / tension analysis ──

    /// Arrange: `x` is acted on by one supporting and one opposing relation,
    /// each on its own scale, so the cross-scale normalisation is exercised too.
    /// `sup`/`opp` are observation tokens; "none" means no cell on that side.
    fn valenced(sup: &str, opp: &str) -> Model {
        // "absent" = no cell at all; "none" = a real cell holding the zero token.
        let cell = |tok: &str| match tok {
            "absent" => String::new(),
            t => format!(r#"{{"from":"a","to":"x","observations":[{{"observation":"{t}"}}]}}"#),
        };
        let json = format!(
            r#"{{"schemaVersion":"0.2.0","modelId":"valenced",
              "dimensions":[
                {{"id":"d1","order":0,"members":[{{"id":"a"}}]}},
                {{"id":"d2","order":1,"members":[{{"id":"x"}}]}}
              ],
              "scales":[
                {{"id":"s9","levels":[
                  {{"observation":"none","value":0,"order":0}},
                  {{"observation":"weak","value":1,"order":1}},
                  {{"observation":"strong","value":9,"order":2}}]}},
                {{"id":"s100","levels":[
                  {{"observation":"none","value":0,"order":0}},
                  {{"observation":"weak","value":10,"order":1}},
                  {{"observation":"strong","value":100,"order":2}}]}}
              ],
              "relations":[
                {{"id":"r_sup","from":"d1","to":"d2","relationType":"supports",
                  "scale":"s9","cells":[{}]}},
                {{"id":"r_opp","from":"d1","to":"d2","relationType":"undermines",
                  "scale":"s100","cells":[{}]}}
              ]}}"#,
            cell(sup),
            cell(opp)
        );
        Model::load(&json).expect("valenced fixture must load")
    }

    #[test]
    fn support_without_opposition_is_not_a_tension() {
        assert!(valenced("strong", "absent").tensions().is_empty());
    }

    #[test]
    fn equal_support_and_opposition_do_not_cancel_to_nothing() {
        // The core invariant: bipolar netting would hide this entirely.
        assert_eq!(valenced("strong", "strong").tensions().len(), 1);
    }

    #[test]
    fn equal_support_and_opposition_report_the_hidden_magnitude() {
        let t = &valenced("strong", "strong").tensions()[0];
        assert_eq!(t.tension, Some(1.0));
    }

    #[test]
    fn opposition_meeting_support_is_a_conflict_not_a_tension() {
        let t = &valenced("strong", "strong").tensions()[0];
        assert!(matches!(t.kind, FindingKind::Conflict));
    }

    #[test]
    fn outweighed_opposition_is_a_tension() {
        let t = &valenced("strong", "weak").tensions()[0];
        assert!(matches!(t.kind, FindingKind::Tension));
    }

    #[test]
    fn conflict_is_severity_block() {
        let t = &valenced("weak", "strong").tensions()[0];
        assert!(matches!(t.severity, raw::FindingSeverity::Block));
    }

    #[test]
    fn contributions_are_normalised_across_differing_scale_maxima() {
        // support "strong" = 9/9 = 1.0; opposition "weak" = 10/100 = 0.1.
        // Un-normalised these would read 9 vs 10 and invert the verdict.
        let t = &valenced("strong", "weak").tensions()[0];
        assert_eq!(t.net, Some(0.9));
    }

    #[test]
    fn opposing_drivers_are_listed_first_for_minimal_sensitivity() {
        let t = &valenced("strong", "strong").tensions()[0];
        assert_eq!(t.contributors.first().map(String::as_str), Some("r_opp:a"));
    }

    #[test]
    fn an_explicit_zero_observation_is_not_a_force() {
        // A recorded "no effect" is data, not opposition.
        assert!(valenced("strong", "none").tensions().is_empty());
    }

    #[test]
    fn structural_relations_carry_no_valence() {
        assert_eq!(relation_opposes(&raw::RelationType::DependsOn), None);
    }

    // ── contraction result semantics ──

    /// Arrange: a two-hop chain `a -> m -> z`. When `contested` is set, `m` is
    /// also strongly undermined, so tension analysis reports it in conflict and
    /// the chain's ranking rides on contested ground.
    fn chained(contested: bool) -> Model {
        let opp = if contested {
            r#",{"id":"r_opp","from":"d1","to":"d2","relationType":"undermines","scale":"s9",
                 "cells":[{"from":"a","to":"m","observations":[{"observation":"strong"}]}]}"#
        } else {
            ""
        };
        let json = format!(
            r#"{{"schemaVersion":"0.2.0","modelId":"chained",
              "dimensions":[
                {{"id":"d1","order":0,"members":[{{"id":"a"}}]}},
                {{"id":"d2","order":1,"members":[{{"id":"m"}}]}},
                {{"id":"d3","order":2,"members":[{{"id":"z"}}]}}
              ],
              "scales":[{{"id":"s9","levels":[
                {{"observation":"none","value":0,"order":0}},
                {{"observation":"strong","value":9,"order":1}}]}}],
              "relations":[
                {{"id":"r1","from":"d1","to":"d2","relationType":"supports","scale":"s9",
                  "cells":[{{"from":"a","to":"m","observations":[{{"observation":"strong"}}]}}]}},
                {{"id":"r2","from":"d2","to":"d3","relationType":"supports","scale":"s9",
                  "cells":[{{"from":"m","to":"z","observations":[{{"observation":"strong"}}]}}]}}
                {opp}
              ],
              "contractions":[{{"id":"c1","chain":["r1","r2"],"weightCombination":"min"}}]}}"#
        );
        Model::load(&json).expect("chained fixture must load")
    }

    #[test]
    fn a_clean_ranking_is_priority_not_an_alarm() {
        let f = &chained(false).contract("c1")[0];
        assert!(matches!(f.kind, FindingKind::WeightedPriority));
    }

    #[test]
    fn a_clean_ranking_is_severity_info() {
        let f = &chained(false).contract("c1")[0];
        assert!(matches!(f.severity, raw::FindingSeverity::Info));
    }

    #[test]
    fn a_ranking_resting_on_a_contested_member_is_a_critical_exposure() {
        let f = &chained(true).contract("c1")[0];
        assert!(matches!(f.kind, FindingKind::CriticalExposure));
    }

    #[test]
    fn contraction_path_records_the_traversed_intermediate() {
        let f = &chained(false).contract("c1")[0];
        assert_eq!(f.path, vec!["a", "m", "z"]);
    }

    #[test]
    fn tensions_are_reported_through_findings() {
        assert!(
            valenced("strong", "strong")
                .findings()
                .iter()
                .any(|f| matches!(f.kind, FindingKind::Conflict))
        );
    }
}
