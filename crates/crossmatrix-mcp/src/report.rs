//! Pure, read-only report/query projections over a loaded
//! [`crossmatrix::Model`] (the ADR-0003 deferred reads).
//!
//! Every function here is a deterministic, side-effect-free transformation of
//! the engine document. There is no I/O and no MCP wiring: the manager layer
//! composes these values into tool responses later. Only the public
//! `crossmatrix` API is used (`dimensions`, `relations`, `scales`, `doc`,
//! `marginalize`, `findings`, `tensions`, `coverage`, `orphans`, `describe`).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crossmatrix::{Axis, Cell, Finding, Member, Model, Relation, SourceRef};

/// One stale source-reference occurrence: what was stored, what the caller
/// supplied, and where in the model the reference lives.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StaleRef {
    pub source_ref: String,
    pub stored: String,
    pub given: String,
    pub location: String,
}

/// The three-way staleness classification of every checkable source reference.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StaleReport {
    pub stale: Vec<StaleRef>,
    pub fresh: Vec<String>,
    pub unchecked: Vec<String>,
}

// ── public queries ──

/// The engine's valence-by-separation tension/conflict findings
/// ([`Model::tensions`]), optionally narrowed to findings with at least one
/// contributor whose label starts with `<relation_id>:` (contributors are
/// formatted `<relationId>:<fromMember>`). Order is preserved.
pub fn conflicts(model: &Model, relation_id: Option<&str>) -> Vec<Finding> {
    model
        .tensions()
        .into_iter()
        .filter(|finding| match relation_id {
            None => true,
            Some(relation_id) => {
                let prefix = format!("{relation_id}:");
                finding
                    .contributors
                    .iter()
                    .any(|contributor| contributor.starts_with(&prefix))
            }
        })
        .collect()
}

/// The merged engine model document as JSON, never including computed findings
/// (a `findings` key is removed when present).
pub fn export_model(model: &Model) -> serde_json::Value {
    let mut value = serde_json::to_value(model.doc()).expect("engine document must serialize");
    if let Some(object) = value.as_object_mut() {
        object.remove("findings");
    }
    value
}

/// Classify every source reference that carries both a non-empty `contentHash`
/// and a `sourceRef` string against a caller-supplied `sourceRef -> hash` map.
///
/// `stale` is per-occurrence (each carries its `location`); `fresh` and
/// `unchecked` are deduplicated by source-ref string. All lists are sorted.
pub fn stale_against(model: &Model, given: &BTreeMap<String, String>) -> StaleReport {
    let mut references: Vec<(&SourceRef, String)> = Vec::new();

    for dim in model.dimensions() {
        for member in &dim.members {
            if let Some(source_ref) = &member.source_ref {
                references.push((
                    source_ref,
                    format!("dimensions/{}/members/{}", dim.id, member.id),
                ));
            }
        }
    }

    for relation in model.relations() {
        for cell in &relation.cells {
            for (observation_index, observation) in cell.observations.iter().enumerate() {
                if let Some(source_ref) = &observation.source_ref {
                    references.push((
                        source_ref,
                        format!(
                            "relations/{}/cells/{}->{}/observations/{}",
                            relation.id, cell.from, cell.to, observation_index
                        ),
                    ));
                }
                for (evidence_index, evidence) in observation.evidence.iter().enumerate() {
                    references.push((
                        &evidence.source_ref,
                        format!(
                            "relations/{}/cells/{}->{}/observations/{}/evidence/{}",
                            relation.id, cell.from, cell.to, observation_index, evidence_index
                        ),
                    ));
                }
            }
        }
    }

    let mut stale: Vec<StaleRef> = Vec::new();
    let mut fresh: BTreeSet<String> = BTreeSet::new();
    let mut unchecked: BTreeSet<String> = BTreeSet::new();

    for (source_ref, location) in references {
        let stored = source_ref
            .content_hash
            .as_deref()
            .filter(|hash| !hash.is_empty());
        let reference = source_ref
            .source_ref
            .as_deref()
            .filter(|value| !value.is_empty());
        let (Some(stored), Some(reference)) = (stored, reference) else {
            continue;
        };

        match given.get(reference) {
            Some(given_hash) if given_hash == stored => {
                fresh.insert(reference.to_string());
            }
            Some(given_hash) => stale.push(StaleRef {
                source_ref: reference.to_string(),
                stored: stored.to_string(),
                given: given_hash.clone(),
                location,
            }),
            None => {
                unchecked.insert(reference.to_string());
            }
        }
    }

    stale.sort_by(|a, b| (&a.source_ref, &a.location).cmp(&(&b.source_ref, &b.location)));

    StaleReport {
        stale,
        fresh: fresh.into_iter().collect(),
        unchecked: unchecked.into_iter().collect(),
    }
}

/// A deterministic Markdown House of Quality report for one relation. `Err`
/// when the relation id is unknown. Every section is always emitted; empty
/// sections render `_none_`.
pub fn relation_report_markdown(model: &Model, relation_id: &str) -> Result<String, String> {
    let relation = find_relation(model, relation_id)
        .ok_or_else(|| format!("unknown relation: {relation_id}"))?;

    let from_members = active_members(model, &relation.from);
    let to_members = active_members(model, &relation.to);

    let mut out = String::new();

    // Header.
    out.push_str(&format!("# House of Quality — {}\n\n", relation.id));
    out.push_str(&format!(
        "_Model `{}` · `{}` → `{}` · {} · scale `{}`_\n\n",
        model.doc().model_id,
        relation.from,
        relation.to,
        relation.relation_type,
        relation.scale
    ));

    // Weights.
    out.push_str("## Weights\n");
    out.push_str("| Member | Label | Weight token | Weight |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for member in &from_members {
        let token = member.weight_observation.as_deref().unwrap_or("—");
        let weight = resolved_weight(model, &relation.from, member)
            .map(fmt_score)
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            escape_pipes(&member.id),
            member_label(member),
            escape_pipes(token),
            weight
        ));
    }
    out.push('\n');

    // Relationship matrix.
    out.push_str("## Relationship matrix\n");
    let mut header = String::from("|  |");
    for member in &to_members {
        header.push_str(&format!(" {} |", member_label(member)));
    }
    out.push_str(&header);
    out.push('\n');
    out.push_str("| --- |");
    for _ in &to_members {
        out.push_str(" --- |");
    }
    out.push('\n');
    for from_member in &from_members {
        let mut row = format!("| {} |", member_label(from_member));
        for to_member in &to_members {
            let tokens = cell_tokens(relation, &from_member.id, &to_member.id);
            let text = if tokens.is_empty() {
                "·".to_string()
            } else {
                tokens.join(" / ")
            };
            row.push_str(&format!(" {text} |"));
        }
        out.push_str(&row);
        out.push('\n');
    }
    out.push('\n');

    // Correlation roof: active self-relations on the R.to dimension.
    out.push_str("## Correlation roof\n");
    let self_relations: Vec<&Relation> = model
        .relations()
        .iter()
        .filter(|s| !is_deprecated(&s.status) && s.from == relation.to && s.to == relation.to)
        .collect();
    if self_relations.is_empty() {
        out.push_str("_none_\n\n");
    } else {
        for self_relation in self_relations {
            out.push_str(&format!(
                "### {} ({})\n",
                self_relation.id, self_relation.relation_type
            ));
            if self_relation.cells.is_empty() {
                out.push_str("_none_\n");
            } else {
                for cell in &self_relation.cells {
                    let a = label_of_id(model, &cell.from);
                    let b = label_of_id(model, &cell.to);
                    let tokens = observation_tokens(cell);
                    out.push_str(&format!("- {a} × {b}: {}\n", tokens.join(" / ")));
                }
            }
        }
        out.push('\n');
    }

    // Technical importance.
    out.push_str("## Technical importance\n");
    match marginalize_rows(model, relation_id) {
        Ok(rows) => {
            out.push_str("| Rank | Member | Label | Score | Relative |\n");
            out.push_str("| --- | --- | --- | --- | --- |\n");
            let max = rows.first().map(|(_, score)| *score).unwrap_or(0.0);
            for (index, (member, score)) in rows.iter().enumerate() {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    index + 1,
                    escape_pipes(member),
                    label_of_id(model, member),
                    fmt_score(*score),
                    fmt_percent(*score, max)
                ));
            }
        }
        Err(error) => out.push_str(&format!("_unavailable: {error}_\n")),
    }
    out.push('\n');

    // Targets.
    out.push_str("## Targets\n");
    let mut targets =
        String::from("| Member | Label | Target | Unit |\n| --- | --- | --- | --- |\n");
    let mut has_target = false;
    for member in &to_members {
        if let Some(target) = member.attributes.get("target").and_then(scalar_text) {
            has_target = true;
            let unit = member
                .attributes
                .get("unit")
                .and_then(scalar_text)
                .unwrap_or_else(|| "—".to_string());
            targets.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                escape_pipes(&member.id),
                member_label(member),
                escape_pipes(&target),
                escape_pipes(&unit)
            ));
        }
    }
    if has_target {
        out.push_str(&targets);
    } else {
        out.push_str("_none_\n");
    }
    out.push('\n');

    // Coverage.
    out.push_str("## Coverage\n");
    out.push_str(&format!("Coverage policy: {}\n", relation.coverage_policy));
    let coverage = model.coverage();
    for (axis, dimension) in [("from", &relation.from), ("to", &relation.to)] {
        let uncovered = coverage
            .iter()
            .find(|entry| entry.relation == relation.id && entry.axis == axis)
            .map(|entry| entry.uncovered.join(", "))
            .filter(|list| !list.is_empty())
            .unwrap_or_else(|| "none".to_string());
        out.push_str(&format!("Uncovered {dimension}: {uncovered}\n"));
    }
    let orphans: Vec<String> = model
        .orphans()
        .into_iter()
        .filter(|orphan| orphan.dimension == relation.from || orphan.dimension == relation.to)
        .map(|orphan| orphan.member)
        .collect();
    let orphans_text = if orphans.is_empty() {
        "none".to_string()
    } else {
        orphans.join(", ")
    };
    out.push_str(&format!("Orphans: {orphans_text}\n\n"));

    // Findings.
    out.push_str("## Findings\n");
    let all_findings = model.findings();
    let mut scope: BTreeSet<String> = BTreeSet::new();
    for member in &from_members {
        scope.insert(member.id.clone());
    }
    for member in &to_members {
        scope.insert(member.id.clone());
    }
    for finding in &all_findings {
        for contributor in &finding.contributors {
            if let Some((_, member)) = contributor.split_once(':') {
                scope.insert(member.to_string());
            }
        }
    }
    let selected: Vec<&Finding> = all_findings
        .iter()
        .filter(|finding| {
            finding.relation.as_deref() == Some(relation_id)
                || finding.members.iter().any(|member| scope.contains(member))
        })
        .collect();
    if selected.is_empty() {
        out.push_str("_none_\n");
    } else {
        for finding in selected {
            out.push_str(&format!(
                "- **{}** ({}) {} — {}\n",
                finding.kind,
                finding.severity,
                finding.members.join(", "),
                finding.explanation.as_deref().unwrap_or("")
            ));
            if !finding.contributors.is_empty() {
                out.push_str(&format!(
                    "  - contributors: {}\n",
                    finding.contributors.join(", ")
                ));
            }
        }
    }
    out.push('\n');

    Ok(out)
}

/// An umbrella Markdown report: a header + summary, then each active
/// non-self relation's report with headings demoted one level.
pub fn aspect_report_markdown(model: &Model, title: Option<&str>) -> String {
    let title = title.unwrap_or("Crossmatrix report");
    let doc = model.doc();
    let cells: usize = doc
        .relations
        .iter()
        .map(|relation| relation.cells.len())
        .sum();

    let mut out = String::new();
    out.push_str(&format!("# {title} — {}\n\n", doc.model_id));
    out.push_str(&format!(
        "{} dimensions · {} relations · {cells} cells\n\n",
        doc.dimensions.len(),
        doc.relations.len()
    ));

    for relation in doc
        .relations
        .iter()
        .filter(|relation| !is_deprecated(&relation.status) && relation.from != relation.to)
    {
        if let Ok(section) = relation_report_markdown(model, &relation.id) {
            out.push_str(&demote_headings(&section));
            out.push('\n');
        }
    }

    out
}

// ── helpers ──

fn find_relation<'a>(model: &'a Model, relation_id: &str) -> Option<&'a Relation> {
    model
        .relations()
        .iter()
        .find(|relation| relation.id == relation_id)
}

fn active_members<'a>(model: &'a Model, dimension_id: &str) -> Vec<&'a Member> {
    model
        .dimensions()
        .iter()
        .find(|dimension| dimension.id == dimension_id)
        .map(|dimension| {
            dimension
                .members
                .iter()
                .filter(|member| !is_deprecated(&member.status))
                .collect()
        })
        .unwrap_or_default()
}

fn member_by_id<'a>(model: &'a Model, member_id: &str) -> Option<&'a Member> {
    model
        .dimensions()
        .iter()
        .flat_map(|dimension| dimension.members.iter())
        .find(|member| member.id == member_id)
}

fn resolved_weight(model: &Model, dimension_id: &str, member: &Member) -> Option<f64> {
    if let Some(weight) = member.weight {
        return Some(weight);
    }
    let dimension = model
        .dimensions()
        .iter()
        .find(|dimension| dimension.id == dimension_id)?;
    let weight_scale = dimension.weight_scale.as_deref()?;
    let scale = model
        .scales()
        .iter()
        .find(|scale| scale.id == weight_scale)?;
    let observation = member.weight_observation.as_deref()?;
    scale
        .levels
        .iter()
        .find(|level| level.observation == observation)
        .map(|level| level.value)
}

fn marginalize_rows(model: &Model, relation_id: &str) -> Result<Vec<(String, f64)>, String> {
    // `marginalize` sorts descending but leaves equal scores in HashMap order;
    // re-sort with a member-id tiebreak so the report is fully deterministic.
    let mut rows = model
        .marginalize(relation_id, Axis::From)
        .map_err(|error| error.to_string())?;
    rows.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(rows)
}

fn cell_tokens(relation: &Relation, from: &str, to: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for cell in relation
        .cells
        .iter()
        .filter(|cell| cell.from == from && cell.to == to)
    {
        for token in observation_tokens(cell) {
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
    }
    tokens
}

fn observation_tokens(cell: &Cell) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for observation in &cell.observations {
        if !tokens.contains(&observation.observation) {
            tokens.push(observation.observation.clone());
        }
    }
    tokens
}

/// The status is a generated enum that is not re-exported at the crate root,
/// so it is inspected through its serialized form.
fn is_deprecated<S: serde::Serialize>(status: &Option<S>) -> bool {
    serde_json::to_value(status).is_ok_and(|value| value.as_str() == Some("deprecated"))
}

fn member_label(member: &Member) -> String {
    escape_pipes(member.label.as_deref().unwrap_or(member.id.as_str()))
}

fn label_of_id(model: &Model, member_id: &str) -> String {
    member_by_id(model, member_id)
        .map(member_label)
        .unwrap_or_else(|| escape_pipes(member_id))
}

fn escape_pipes(text: &str) -> String {
    text.replace('|', "\\|")
}

fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Format a score with up to two decimals, trimming trailing zeros.
fn fmt_score(value: f64) -> String {
    let mut text = format!("{value:.2}");
    if let Some(dot) = text.find('.') {
        while text.len() > dot + 1 && text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

fn fmt_percent(score: f64, max: f64) -> String {
    if max <= 0.0 {
        return "0%".to_string();
    }
    format!("{:.0}%", score / max * 100.0)
}

fn demote_headings(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            if line.starts_with('#') {
                format!("#{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
