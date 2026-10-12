//! The `crossmatrix.query` read/analysis surface: pure projections over the
//! currently-open [`crossmatrix::Model`], dispatched by query kind.
//!
//! Every response carries non-empty HATEOAS `links`; every error is a
//! [`Rejection`] with links. Nothing here mutates the model.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crossmatrix::{Axis, Model, TraceDirection};

use crate::report;
use crate::session::{Rejection, Session};

/// Query links reused across kinds.
const CONFLICT_LINKS: [&str; 3] = ["analyze.findings", "trace", "report"];
const REPORT_LINKS: [&str; 3] = ["conflicts", "export", "analyze.findings"];
const EXPORT_LINKS: [&str; 3] = ["report", "conflicts", "analyze.findings"];
const STALE_LINKS: [&str; 4] = ["members.sync", "observe", "trace", "slice"];

fn no_model() -> Rejection {
    Rejection::new(
        "no_model",
        "no model loaded",
        None,
        vec!["model.open".to_string()],
    )
}

fn unknown_relation(relation: &str) -> Rejection {
    Rejection::new(
        "unknown_relation",
        format!("unknown relation: {relation}"),
        None,
        vec!["relation.declare".to_string(), "describe".to_string()],
    )
}

fn unknown_member(member: &str) -> Rejection {
    Rejection::new(
        "unknown_member",
        format!("unknown member: {member}"),
        None,
        vec![
            "members.sync".to_string(),
            "member.propose".to_string(),
            "trace".to_string(),
        ],
    )
}

fn analysis_failed(message: impl Into<String>) -> Rejection {
    Rejection::new(
        "analysis_failed",
        message,
        None,
        vec!["describe".to_string(), "model.status".to_string()],
    )
}

fn bad_request(message: impl Into<String>) -> Rejection {
    Rejection::new(
        "bad_request",
        message,
        None,
        vec!["model.status".to_string()],
    )
}

/// A required string parameter under `/query/<name>` (fail-fast, recoverable).
fn str_param<'a>(request: &'a Value, name: &str) -> Result<&'a str, Rejection> {
    request
        .pointer(&format!("/query/{name}"))
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request(format!("query requires string parameter 'query.{name}'")))
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

/// A bounded `usize` parameter: `default` when absent, clamped to `max` when
/// present (so a hostile huge `limit` can never force unbounded work).
fn opt_limit(request: &Value, name: &str, default: usize, max: usize) -> usize {
    request
        .pointer(&format!("/query/{name}"))
        .and_then(Value::as_u64)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX).min(max))
        .unwrap_or(default)
}

/// The opaque numeric-offset cursor, defaulting to offset 0.
fn cursor_offset(request: &Value) -> usize {
    request
        .pointer("/query/cursor")
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn next_cursor(offset: usize, limit: usize, total: usize) -> Option<String> {
    if offset.saturating_add(limit) < total {
        Some((offset + limit).to_string())
    } else {
        None
    }
}

fn want_rollup(request: &Value) -> bool {
    opt_bool(request, "rollup")
}

fn serialize_items<I, T>(items: I) -> Result<Vec<Value>, Rejection>
where
    I: IntoIterator<Item = T>,
    T: serde::Serialize,
{
    items
        .into_iter()
        .map(|item| {
            serde_json::to_value(item)
                .map_err(|error| analysis_failed(format!("serialize: {error}")))
        })
        .collect()
}

fn deprecated<S: serde::Serialize>(status: &Option<S>) -> bool {
    serde_json::to_value(status).is_ok_and(|value| value.as_str() == Some("deprecated"))
}

/// Index every member by its `sourceRef.uri`, falling back to
/// `sourceRef.sourceRef`. A key may resolve to several members (shared source).
fn uri_index(model: &Model) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for dimension in model.dimensions() {
        for member in &dimension.members {
            let Some(source) = &member.source_ref else {
                continue;
            };
            let key = source
                .uri
                .as_deref()
                .or(source.source_ref.as_deref())
                .filter(|value| !value.is_empty());
            if let Some(key) = key {
                index
                    .entry(key.to_string())
                    .or_default()
                    .push(member.id.clone());
            }
        }
    }
    index
}

/// Resolve `trace`'s target to every member it names: an explicit `uri` param
/// wins; otherwise `member` is a member id or a known uri/sourceRef.
fn resolve_trace_members(model: &Model, request: &Value) -> Result<Vec<String>, Rejection> {
    if let Some(uri) = opt_str(request, "uri") {
        return uri_index(model)
            .get(uri)
            .cloned()
            .ok_or_else(|| unknown_member(uri));
    }
    let member = str_param(request, "member")?;
    if model.dimension_of(member).is_some() {
        return Ok(vec![member.to_string()]);
    }
    uri_index(model)
        .get(member)
        .cloned()
        .ok_or_else(|| unknown_member(member))
}

/// Resolve one `slice` axis selector (member id or uri) to its member ids.
fn resolve_selector(model: &Model, selector: &str) -> Result<Vec<String>, Rejection> {
    if model.dimension_of(selector).is_some() {
        return Ok(vec![selector.to_string()]);
    }
    uri_index(model)
        .get(selector)
        .cloned()
        .ok_or_else(|| unknown_member(selector))
}

/// Dispatch one `crossmatrix.query` request. `request` is the whole tool request
/// (`{query: {kind, ...}}`).
pub fn query(session: &Session, request: &Value) -> Result<Value, Rejection> {
    let kind = request
        .pointer("/query/kind")
        .and_then(Value::as_str)
        .unwrap_or("");

    match kind {
        "model.status" => Ok(session.status()),

        "analyze.contract" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let mut findings: Vec<Value> = Vec::new();
                for contraction in model.contraction_ids() {
                    findings.extend(serialize_items(model.contract(contraction))?);
                }
                let mut response = json!({
                    "findings": findings,
                    "links": ["analyze.findings", "analyze.marginalize", "validate", "describe"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "analyze.findings" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let findings = serialize_items(model.findings())?;
                let mut response = json!({
                    "findings": findings,
                    "links": ["analyze.contract", "validate", "describe"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "validate" => {
            let validated = session.model().is_some();
            Ok(json!({
                "validated": validated,
                "links": if validated {
                    vec!["analyze.contract", "analyze.findings", "describe"]
                } else {
                    vec!["open"]
                }
            }))
        }

        "describe" => with_model(session, false, |model| {
            let description = serde_json::to_value(model.describe())
                .map_err(|e| analysis_failed(format!("serialize description: {e}")))?;
            Ok(json!({
                "describe": description,
                "links": ["slice", "trace", "coverage", "gaps.orphans", "gaps.next", "validate"],
            }))
        }),

        "slice" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let relation = str_param(request, "relationId")?;
                let from = opt_str(request, "from")
                    .map(|v| resolve_selector(model, v))
                    .transpose()?;
                let to = opt_str(request, "to")
                    .map(|v| resolve_selector(model, v))
                    .transpose()?;
                let cells = model
                    .slice(relation, None, None)
                    .ok_or_else(|| unknown_relation(relation))?;
                let filtered: Vec<&crossmatrix::Cell> = cells
                    .into_iter()
                    .filter(|cell| {
                        from.as_ref().is_none_or(|set| set.contains(&cell.from))
                            && to.as_ref().is_none_or(|set| set.contains(&cell.to))
                    })
                    .collect();
                let total = filtered.len();
                let limit = opt_limit(request, "limit", 500, 5000);
                let offset = cursor_offset(request);
                let page = serialize_items(filtered.into_iter().skip(offset).take(limit))?;
                let mut response = json!({
                    "relation": relation,
                    "cells": page,
                    "total": total,
                    "limit": limit,
                    "nextCursor": next_cursor(offset, limit, total),
                    "links": ["describe", "trace", "explain"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "trace" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let members = resolve_trace_members(model, request)?;
                let relation_filter = opt_str(request, "relationId");
                let direction = match opt_str(request, "direction").unwrap_or("both") {
                    "in" => Some(TraceDirection::In),
                    "out" => Some(TraceDirection::Out),
                    "both" => None,
                    other => {
                        return Err(bad_request(format!(
                            "query.direction must be in|out|both, got '{other}'"
                        )));
                    }
                };
                let limit = opt_limit(request, "limit", 200, 2000);
                let offset = cursor_offset(request);

                // Collect (relation, member, direction, hop) for a fully
                // deterministic ordering independent of declaration order.
                let mut hops: Vec<(String, String, String, Value)> = Vec::new();
                for member in &members {
                    for hop in model.trace(member) {
                        if relation_filter.is_some_and(|filter| hop.relation != filter) {
                            continue;
                        }
                        if direction.is_some_and(|dir| hop.direction != dir) {
                            continue;
                        }
                        let direction_text = serde_json::to_value(hop.direction)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default();
                        hops.push((
                            hop.relation.clone(),
                            hop.member.clone(),
                            direction_text,
                            serde_json::to_value(&hop)
                                .map_err(|e| analysis_failed(format!("serialize hop: {e}")))?,
                        ));
                    }
                }
                hops.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
                let total = hops.len();
                let page: Vec<Value> = hops
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .map(|(_, _, _, value)| value)
                    .collect();
                let dimension = members
                    .iter()
                    .find_map(|member| model.dimension_of(member))
                    .unwrap_or("")
                    .to_string();
                let mut response = json!({
                    "member": members.first().cloned().unwrap_or_default(),
                    "dimension": dimension,
                    "hops": page,
                    "members": members,
                    "total": total,
                    "limit": limit,
                    "nextCursor": next_cursor(offset, limit, total),
                    "links": ["explain", "coverage", "gaps.orphans", "describe"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "explain" => with_model(session, false, |model| {
            let relation = str_param(request, "relationId")?;
            let from = str_param(request, "from")?;
            let to = str_param(request, "to")?;
            let cells = model
                .slice(relation, Some(from), Some(to))
                .ok_or_else(|| unknown_relation(relation))?;
            Ok(json!({
                "relation": relation,
                "from": from,
                "to": to,
                "cells": cells,
                "links": ["trace", "slice", "describe"],
            }))
        }),

        "coverage" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let axes = model.coverage();
                let uncovered: usize = axes.iter().map(|a| a.uncovered.len()).sum();
                let violations = axes
                    .iter()
                    .filter(|a| a.required && !a.uncovered.is_empty())
                    .count();
                let axis_count = axes.len();
                let mut response = json!({
                    "axes": axes,
                    "summary": { "axes": axis_count, "uncovered": uncovered, "violations": violations },
                    "links": ["gaps.orphans", "gaps.next", "trace", "describe"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "gaps.orphans" => with_model(session, false, |model| {
            Ok(json!({
                "orphans": model.orphans(),
                "links": ["gaps.next", "coverage", "trace"],
            }))
        }),

        "gaps.next" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let mut response = json!({
                    "gaps": model.gaps_next(),
                    "heuristic": "uncovered members ordered by resolved importance weight (unweighted last)",
                    "links": ["gaps.orphans", "coverage", "trace"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "analyze.marginalize" => {
            let rollup = want_rollup(request);
            with_model(session, rollup, |model| {
                let relation = str_param(request, "relationId")?;
                let axis = match str_param(request, "axis")? {
                    "from" => Axis::From,
                    "to" => Axis::To,
                    other => {
                        return Err(bad_request(format!(
                            "query.axis must be 'from' or 'to', got '{other}'"
                        )));
                    }
                };
                let pairs = model
                    .marginalize(relation, axis)
                    .map_err(|e| analysis_failed(format!("marginalize failed: {e}")))?;
                let findings: Vec<Value> = pairs
                    .into_iter()
                    .map(|(member, value)| json!({ "member": member, "value": value }))
                    .collect();
                let mut response = json!({
                    "findings": findings,
                    "links": ["analyze.contract", "analyze.findings", "validate", "describe"],
                });
                if rollup {
                    response["rollup"] = json!(true);
                }
                Ok(response)
            })
        }

        "conflicts" => with_model(session, false, |model| {
            let findings =
                serialize_items(report::conflicts(model, opt_str(request, "relationId")))?;
            Ok(json!({
                "findings": findings,
                "links": CONFLICT_LINKS,
            }))
        }),

        "report" => with_model(session, false, |model| {
            let markdown = report_markdown(session, model, request)?;
            Ok(json!({
                "format": "markdown",
                "markdown": markdown,
                "links": REPORT_LINKS,
            }))
        }),

        "export" => with_model(session, false, |model| {
            match opt_str(request, "format").unwrap_or("model") {
                "model" => Ok(json!({
                    "format": "model",
                    "model": report::export_model(model),
                    "links": EXPORT_LINKS,
                })),
                "report" => {
                    let markdown = report_markdown(session, model, request)?;
                    Ok(json!({
                        "format": "markdown",
                        "markdown": markdown,
                        "links": EXPORT_LINKS,
                    }))
                }
                "findings" => Ok(json!({
                    "format": "findings",
                    "findings": serialize_items(model.findings())?,
                    "links": EXPORT_LINKS,
                })),
                other => Err(bad_request(format!(
                    "query.format must be model|report|findings, got '{other}'"
                ))),
            }
        }),

        "stale" => stale_query(session, request),

        _ => Ok(json!({
            "ok": true,
            "query": kind,
            "note": "query not supported in this build (conflicts needs valence/tension analysis — ADR-0002; export deferred)",
            "links": ["describe", "trace", "coverage", "gaps.orphans", "validate"]
        })),
    }
}

/// Run `f` over the open model, optionally over the derived `rolled_up()` model.
fn with_model<T>(
    session: &Session,
    rollup: bool,
    f: impl FnOnce(&Model) -> Result<T, Rejection>,
) -> Result<T, Rejection> {
    let base = session.model().ok_or_else(no_model)?;
    if rollup {
        let rolled = base.rolled_up();
        f(&rolled)
    } else {
        f(base)
    }
}

/// The `report`/`export report` markdown body: a relation report when
/// `relationId` is given, else the aspect report with a resolved title.
fn report_markdown(session: &Session, model: &Model, request: &Value) -> Result<String, Rejection> {
    if let Some(relation) = opt_str(request, "relationId") {
        return report::relation_report_markdown(model, relation)
            .map_err(|_| unknown_relation(relation));
    }
    let title = opt_str(request, "title")
        .map(str::to_string)
        .or_else(|| session.config_default_title())
        .unwrap_or_else(|| "Crossmatrix report".to_string());
    Ok(report::aspect_report_markdown(model, Some(&title)))
}

// ── stale ──

/// How a member's current hash was determined.
enum Current {
    /// A current hash was resolved; `None` means the source is known-missing.
    Known(Option<String>),
    /// No way to know the current hash (no entry, no opt-in read, or no key).
    Unknown,
    /// A `file://` source was refused (outside the project directory).
    Skipped(String),
}

fn is_file_uri(uri: &str) -> bool {
    uri.starts_with("file://")
}

/// The project directory that `readFiles` is allowed to read from: the parent
/// of a `.crossmatrix` store root, else the process cwd.
fn project_dir(root: &Path) -> PathBuf {
    if root.file_name().and_then(|name| name.to_str()) == Some(".crossmatrix") {
        root.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
}

/// Canonicalize `path` even when it (or an ancestor) does not exist, by
/// canonicalizing the nearest existing ancestor and re-appending the rest.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut result = canonical;
            for component in suffix.iter().rev() {
                result.push(component);
            }
            return result;
        }
        match current.file_name() {
            Some(name) => {
                suffix.push(name.to_os_string());
                if !current.pop() {
                    return path.to_path_buf();
                }
            }
            None => return path.to_path_buf(),
        }
    }
}

/// Resolve a `file://` uri to an on-disk path, refusing anything that escapes
/// the (canonical) project directory.
fn resolve_file_uri(uri: &str, project_canonical: &Path) -> Result<PathBuf, String> {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        project_canonical.join(raw)
    };
    let resolved = canonicalize_lenient(&candidate);
    if !resolved.starts_with(project_canonical) {
        return Err("outside_project_dir".to_string());
    }
    Ok(candidate)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256:");
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn read_file_hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

fn observation_tokens(cell: &crossmatrix::Cell) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for observation in &cell.observations {
        if !tokens.contains(&observation.observation) {
            tokens.push(observation.observation.clone());
        }
    }
    tokens
}

fn stale_legacy(model: &Model) -> Result<Value, Rejection> {
    let stale = serialize_items(
        model
            .findings()
            .into_iter()
            .filter(|finding| matches!(finding.kind, crossmatrix::FindingKind::StaleReference)),
    )?;
    Ok(json!({
        "findings": stale,
        "links": ["analyze.findings", "describe", "validate"],
    }))
}

fn stale_query(session: &Session, request: &Value) -> Result<Value, Rejection> {
    let model = session.model().ok_or_else(no_model)?;

    let hashes_value = request.pointer("/query/hashes");
    let read_files = opt_bool(request, "readFiles");
    let has_hashes = hashes_value.and_then(Value::as_object).is_some();

    if !has_hashes && !read_files {
        return stale_legacy(model);
    }

    let hashes: HashMap<String, Option<String>> = hashes_value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().map(str::to_string)))
                .collect()
        })
        .unwrap_or_default();

    let dimension_filter = opt_str(request, "dimensionId");
    let project_canonical = project_dir(session.root())
        .canonicalize()
        .unwrap_or_else(|_| project_dir(session.root()));

    let mut changed: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut checked = 0usize;
    let mut unchecked = 0usize;

    for dimension in model.dimensions() {
        if dimension_filter.is_some_and(|filter| dimension.id != filter) {
            continue;
        }
        for member in &dimension.members {
            if deprecated(&member.status) {
                continue;
            }
            let source = member.source_ref.as_ref();
            let uri = source.and_then(|s| s.uri.clone());
            let key = uri
                .clone()
                .or_else(|| source.and_then(|s| s.source_ref.clone()));
            let stored = source
                .and_then(|s| s.hash.clone().or_else(|| s.content_hash.clone()))
                .filter(|hash| !hash.is_empty());

            let current = match &key {
                Some(key) if hashes.contains_key(key) => {
                    Current::Known(hashes.get(key).cloned().flatten())
                }
                Some(_) if read_files => match &uri {
                    Some(uri) if is_file_uri(uri) => {
                        match resolve_file_uri(uri, &project_canonical) {
                            Ok(path) => Current::Known(read_file_hash(&path)),
                            Err(reason) => Current::Skipped(reason),
                        }
                    }
                    _ => Current::Unknown,
                },
                _ => Current::Unknown,
            };

            match current {
                Current::Known(current) => match &stored {
                    Some(stored) => {
                        checked += 1;
                        if Some(stored) != current.as_ref() {
                            changed.push(json!({
                                "dimension": dimension.id,
                                "member": member.id,
                                "key": key,
                                "stored": stored,
                                "current": current,
                            }));
                        }
                    }
                    None => unchecked += 1,
                },
                Current::Skipped(reason) => {
                    unchecked += 1;
                    skipped.push(json!({
                        "member": member.id,
                        "key": key,
                        "uri": uri,
                        "reason": reason,
                    }));
                }
                Current::Unknown => unchecked += 1,
            }
        }
    }

    let total_changed = changed.len();
    let limit = opt_limit(request, "limit", 100, 1000);
    let offset = cursor_offset(request);
    let page: Vec<Value> = changed.into_iter().skip(offset).take(limit).collect();

    let page_members: HashSet<&str> = page
        .iter()
        .filter_map(|entry| entry.get("member").and_then(Value::as_str))
        .collect();

    let cell_limit = opt_limit(request, "cellLimit", 500, 5000);
    let mut affected: Vec<Value> = Vec::new();
    let mut cells_truncated = false;
    'relations: for relation in model.relations() {
        if deprecated(&relation.status) {
            continue;
        }
        for cell in &relation.cells {
            if !page_members.contains(cell.from.as_str())
                && !page_members.contains(cell.to.as_str())
            {
                continue;
            }
            if affected.len() >= cell_limit {
                cells_truncated = true;
                break 'relations;
            }
            affected.push(json!({
                "relationId": relation.id,
                "from": cell.from,
                "to": cell.to,
                "tokens": observation_tokens(cell),
            }));
        }
    }

    Ok(json!({
        "changed": page,
        "affectedCells": affected,
        "counts": { "changed": total_changed, "checked": checked, "unchecked": unchecked },
        "limit": limit,
        "nextCursor": next_cursor(offset, limit, total_changed),
        "cellsTruncated": cells_truncated,
        "skipped": skipped,
        "links": STALE_LINKS,
    }))
}
