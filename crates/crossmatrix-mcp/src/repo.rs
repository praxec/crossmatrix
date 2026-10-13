//! Repo-committed model store: state-root resolution, store initialization,
//! and cross-document validation.
//!
//! A `.crossmatrix/` directory lives beside a project and is meant to be
//! committed. It holds canonical-JSON documents for the three ADR-0004 layers
//! plus an append-only audit log.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};

/// Where a resolved store root came from (precedence order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootSource {
    Env,
    Ancestor,
    XdgDataHome,
    Home,
    Cwd,
}

/// Resolve the store root without touching the environment: `env` is injected
/// and `cwd` is provided. Resolution order:
///
/// 1. `CROSSMATRIX_DIR` (relative paths joined onto `cwd`)
/// 2. nearest ancestor of `cwd` (inclusive) containing a `.crossmatrix` directory
/// 3. `$XDG_DATA_HOME/crossmatrix`
/// 4. `$HOME/.local/share/crossmatrix`
/// 5. `cwd/crossmatrix-state`
pub fn resolve_root(env: &dyn Fn(&str) -> Option<String>, cwd: &Path) -> (PathBuf, RootSource) {
    if let Some(dir) = env("CROSSMATRIX_DIR").filter(|v| !v.is_empty()) {
        let path = Path::new(&dir);
        let root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        return (root, RootSource::Env);
    }

    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(".crossmatrix");
        if candidate.is_dir() {
            return (candidate, RootSource::Ancestor);
        }
    }

    if let Some(xdg) = env("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return (Path::new(&xdg).join("crossmatrix"), RootSource::XdgDataHome);
    }

    if let Some(home) = env("HOME").filter(|v| !v.is_empty()) {
        return (
            Path::new(&home)
                .join(".local")
                .join("share")
                .join("crossmatrix"),
            RootSource::Home,
        );
    }

    (cwd.join("crossmatrix-state"), RootSource::Cwd)
}

/// `resolve_root` against the process environment and current directory.
pub fn resolve_root_from_process() -> (PathBuf, RootSource) {
    let env = |key: &str| std::env::var(key).ok();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_root(&env, &cwd)
}

/// Result of `init`: paths are relative to `project_dir`, '/'-separated.
#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    pub root: PathBuf,
    pub created: Vec<String>,
    pub preserved: Vec<String>,
}

const README_MD: &str = "\
# .crossmatrix — repo-committed model store

This directory is crossmatrix's model store and is **meant to be committed**
to version control. The model travels with the repository.

Layout:

- `config/<configId>@<version>.json` — methodology: scales, relation
  declarations, contractions.
- `dimensions/<dimensionSetId>@<version>.json` — members/rosters.
- `state/<stateId>.json` — cells/observations plus a monotonic `revision`.
- `audit/<stateId>.jsonl` — append-only mutation log; union-merged.

All files are canonical JSON (object keys sorted) so diffs stay minimal and
merge-friendly. `audit/*.jsonl` uses `merge=union` (see `.gitattributes`).

Findings are computed and are never stored here.

Validate the store with:

    crossmatrix-mcp check

See the crossmatrix ADRs (0002–0004) for the engine and schema design.
";

/// Create `<project_dir>/.crossmatrix/` with the documented layout. Existing
/// files are never overwritten; a second run reports everything preserved.
pub fn init(project_dir: &Path) -> Result<InitReport, String> {
    let root = project_dir.join(".crossmatrix");
    let mut created = Vec::new();
    let mut preserved = Vec::new();

    let dirs = [
        ".crossmatrix",
        ".crossmatrix/config",
        ".crossmatrix/dimensions",
        ".crossmatrix/state",
        ".crossmatrix/audit",
    ];
    for rel in dirs {
        let abs = project_dir.join(rel);
        if abs.is_dir() {
            preserved.push(rel.to_string());
        } else {
            fs::create_dir_all(&abs).map_err(|e| e.to_string())?;
            created.push(rel.to_string());
        }
    }

    let files: [(&str, &str); 6] = [
        (".crossmatrix/.gitattributes", "audit/*.jsonl merge=union\n"),
        (".crossmatrix/README.md", README_MD),
        (".crossmatrix/config/.gitkeep", ""),
        (".crossmatrix/dimensions/.gitkeep", ""),
        (".crossmatrix/state/.gitkeep", ""),
        (".crossmatrix/audit/.gitkeep", ""),
    ];
    for (rel, content) in files {
        let abs = project_dir.join(rel);
        if abs.exists() {
            preserved.push(rel.to_string());
        } else {
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&abs, content).map_err(|e| e.to_string())?;
            created.push(rel.to_string());
        }
    }

    Ok(InitReport {
        root,
        created,
        preserved,
    })
}

/// A single validation problem, addressed by a root-relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub path: String,
    pub message: String,
}

/// Result of `check`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub documents: usize,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy)]
enum DocKind {
    Versioned,
    State,
}

/// Validate a repo-committed store rooted at `root`.
pub fn check(root: &Path) -> CheckReport {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    if !root.is_dir() {
        diagnostics.push(Diagnostic {
            path: ".".to_string(),
            message: "store root does not exist".to_string(),
        });
        return CheckReport {
            documents: 0,
            diagnostics,
        };
    }

    let config_schema = parse_schema(include_str!("../schema/crossmatrix-config.schema.json"));
    let dims_schema = parse_schema(include_str!("../schema/crossmatrix-dimensions.schema.json"));
    let state_schema = parse_schema(include_str!("../schema/crossmatrix-state.schema.json"));

    let mut documents = 0usize;
    let config_docs = collect_documents(
        root,
        DocSpec {
            subdir: "config",
            schema: &config_schema,
            id_field: "configId",
            version_field: "version",
            kind: DocKind::Versioned,
        },
        &mut diagnostics,
        &mut documents,
    );
    let dims_docs = collect_documents(
        root,
        DocSpec {
            subdir: "dimensions",
            schema: &dims_schema,
            id_field: "dimensionSetId",
            version_field: "version",
            kind: DocKind::Versioned,
        },
        &mut diagnostics,
        &mut documents,
    );
    let state_docs = collect_documents(
        root,
        DocSpec {
            subdir: "state",
            schema: &state_schema,
            id_field: "stateId",
            version_field: "version",
            kind: DocKind::State,
        },
        &mut diagnostics,
        &mut documents,
    );

    let mut config_index: BTreeMap<(String, String), (String, Value)> = BTreeMap::new();
    for (rel, value) in &config_docs {
        if let (Some(id), Some(ver)) = (str_field(value, "configId"), str_field(value, "version")) {
            config_index.insert(
                (id.to_string(), ver.to_string()),
                (rel.clone(), value.clone()),
            );
        }
    }
    let mut dims_index: BTreeMap<(String, String), (String, Value)> = BTreeMap::new();
    for (rel, value) in &dims_docs {
        if let (Some(id), Some(ver)) = (
            str_field(value, "dimensionSetId"),
            str_field(value, "version"),
        ) {
            dims_index.insert(
                (id.to_string(), ver.to_string()),
                (rel.clone(), value.clone()),
            );
        }
    }

    check_configurations(&config_docs, &dims_index, &mut diagnostics);
    check_states(&state_docs, &config_index, &dims_index, &mut diagnostics);
    check_audit(root, &mut diagnostics);

    diagnostics.sort_by(|a, b| (&a.path, &a.message).cmp(&(&b.path, &b.message)));
    CheckReport {
        documents,
        diagnostics,
    }
}

fn check_configurations(
    config_docs: &[(String, Value)],
    dims_index: &BTreeMap<(String, String), (String, Value)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (rel, config) in config_docs {
        let scales = declared_scales(config);
        let dsr = config.get("dimensionSetRef");
        let dsid = dsr.and_then(|d| str_field(d, "dimensionSetId"));
        let dsver = dsr.and_then(|d| str_field(d, "version"));
        let dims_entry = match (dsid, dsver) {
            (Some(id), Some(ver)) => dims_index.get(&(id.to_string(), ver.to_string())),
            _ => None,
        };
        if dims_entry.is_none() {
            diagnostics.push(Diagnostic {
                path: rel.clone(),
                message: format!(
                    "dimensionSetRef: unresolved dimension set '{}@{}'",
                    dsid.unwrap_or(""),
                    dsver.unwrap_or("")
                ),
            });
        }
        let dims_doc = dims_entry.map(|(_, value)| value);

        let relations = array_field(config, "relations");
        let mut relation_ids: BTreeSet<String> = BTreeSet::new();
        for (i, relation) in relations.iter().enumerate() {
            if let Some(id) = str_field(relation, "id") {
                relation_ids.insert(id.to_string());
            }
            for field in ["from", "to"] {
                let unknown_dimension = match (str_field(relation, field), dims_doc) {
                    (Some(dim), Some(dims_doc)) if !dimension_exists(dims_doc, dim) => Some(dim),
                    _ => None,
                };
                if let Some(dim) = unknown_dimension {
                    diagnostics.push(Diagnostic {
                        path: rel.clone(),
                        message: format!("relations[{i}].{field}: unknown dimension '{dim}'"),
                    });
                }
            }
            if let Some(scale) = str_field(relation, "scale").filter(|s| !scales.contains(*s)) {
                diagnostics.push(Diagnostic {
                    path: rel.clone(),
                    message: format!("relations[{i}].scale: undeclared scale '{scale}'"),
                });
            }
        }

        for (i, contraction) in array_field(config, "contractions").iter().enumerate() {
            for (j, link) in array_field(contraction, "chain").iter().enumerate() {
                if let Some(link) = link.as_str().filter(|l| !relation_ids.contains(*l)) {
                    diagnostics.push(Diagnostic {
                        path: rel.clone(),
                        message: format!("contractions[{i}].chain[{j}]: unknown relation '{link}'"),
                    });
                }
            }
        }

        if let (Some((dims_rel, dims_doc)), Some(_)) = (dims_entry, dsid) {
            for (i, dim) in array_field(dims_doc, "dimensions").iter().enumerate() {
                if let Some(weight_scale) =
                    str_field(dim, "weightScale").filter(|s| !scales.contains(*s))
                {
                    diagnostics.push(Diagnostic {
                        path: dims_rel.clone(),
                        message: format!(
                            "dimensions[{i}].weightScale: undeclared scale '{weight_scale}'"
                        ),
                    });
                }
            }
        }
    }
}

fn check_states(
    state_docs: &[(String, Value)],
    config_index: &BTreeMap<(String, String), (String, Value)>,
    dims_index: &BTreeMap<(String, String), (String, Value)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (rel, state) in state_docs {
        let config_ref = state.get("configRef");
        let cid = config_ref.and_then(|c| str_field(c, "configId"));
        let cver = config_ref.and_then(|c| str_field(c, "version"));
        let config_entry = match (cid, cver) {
            (Some(id), Some(ver)) => config_index.get(&(id.to_string(), ver.to_string())),
            _ => None,
        };
        let config = match config_entry {
            Some((_, value)) => value,
            None => {
                diagnostics.push(Diagnostic {
                    path: rel.clone(),
                    message: format!(
                        "configRef: unresolved configuration '{}@{}'",
                        cid.unwrap_or(""),
                        cver.unwrap_or("")
                    ),
                });
                continue;
            }
        };

        let dsr = config.get("dimensionSetRef");
        let dsid = dsr.and_then(|d| str_field(d, "dimensionSetId"));
        let dsver = dsr.and_then(|d| str_field(d, "version"));
        let dims_doc = match (dsid, dsver) {
            (Some(id), Some(ver)) => dims_index
                .get(&(id.to_string(), ver.to_string()))
                .map(|(_, value)| value),
            _ => None,
        };

        let relations = array_field(config, "relations");
        let cells = array_field(state, "cells");
        for (i, cell) in cells.iter().enumerate() {
            let relation_id = str_field(cell, "relationId");
            let relation = relation_id
                .and_then(|rid| relations.iter().find(|r| str_field(r, "id") == Some(rid)));
            let relation = match relation {
                Some(relation) => relation,
                None => {
                    diagnostics.push(Diagnostic {
                        path: rel.clone(),
                        message: format!(
                            "cells[{i}].relationId: unknown relation '{}'",
                            relation_id.unwrap_or("")
                        ),
                    });
                    continue;
                }
            };

            if let Some(dims_doc) = dims_doc {
                for field in ["from", "to"] {
                    let unknown_member = match (str_field(cell, field), str_field(relation, field))
                    {
                        (Some(member), Some(dim))
                            if !dimension_has_member(dims_doc, dim, member) =>
                        {
                            Some((member, dim))
                        }
                        _ => None,
                    };
                    if let Some((member, dim)) = unknown_member {
                        diagnostics.push(Diagnostic {
                            path: rel.clone(),
                            message: format!(
                                "cells[{i}].{field}: unknown member '{member}' in dimension '{dim}'"
                            ),
                        });
                    }
                }
            }

            let scoped_levels = str_field(relation, "scale")
                .and_then(|scale| scale_levels(config, scale).map(|levels| (scale, levels)));
            if let Some((scale, levels)) = scoped_levels {
                for (j, observation) in array_field(cell, "observations").iter().enumerate() {
                    let unknown_token = str_field(observation, "observation")
                        .filter(|token| !levels.iter().any(|level| level.as_str() == *token));
                    if let Some(token) = unknown_token {
                        diagnostics.push(Diagnostic {
                            path: rel.clone(),
                            message: format!(
                                "cells[{i}].observations[{j}].observation: unknown token '{token}' in scale '{scale}'"
                            ),
                        });
                    }
                }
            }
        }

        if let Some(dims_doc) = dims_doc {
            match merge_and_load(config, dims_doc, state) {
                Ok(()) => {}
                Err(error) => diagnostics.push(Diagnostic {
                    path: rel.clone(),
                    message: format!("merge/load: {error}"),
                }),
            }
        }
    }
}

fn check_audit(root: &Path, diagnostics: &mut Vec<Diagnostic>) {
    let audit_dir = root.join("audit");
    let entries = match fs::read_dir(&audit_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().map(|ext| ext == "jsonl").unwrap_or(false)
        })
        .collect();
    files.sort();
    for path in files {
        let rel = rel_path(root, &path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if serde_json::from_str::<Value>(line).is_err() {
                diagnostics.push(Diagnostic {
                    path: format!("{rel}:{}", index + 1),
                    message: "invalid JSON".to_string(),
                });
            }
        }
    }
}

struct DocSpec<'a> {
    subdir: &'a str,
    schema: &'a Value,
    id_field: &'a str,
    version_field: &'a str,
    kind: DocKind,
}

fn collect_documents(
    root: &Path,
    spec: DocSpec<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    documents: &mut usize,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let validator = match jsonschema::validator_for(spec.schema) {
        Ok(validator) => Some(validator),
        Err(error) => {
            diagnostics.push(Diagnostic {
                path: spec.subdir.to_string(),
                message: format!("embedded schema is invalid: {error}"),
            });
            None
        }
    };

    let entries = match fs::read_dir(root.join(spec.subdir)) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().map(|ext| ext == "json").unwrap_or(false))
        .collect();
    files.sort();

    for path in files {
        let rel = rel_path(root, &path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                diagnostics.push(Diagnostic {
                    path: rel,
                    message: format!("read failed: {error}"),
                });
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                diagnostics.push(Diagnostic {
                    path: rel,
                    message: format!("invalid JSON: {error}"),
                });
                continue;
            }
        };
        *documents += 1;

        if let Some(validator) = &validator {
            for error in validator.iter_errors(&value) {
                diagnostics.push(Diagnostic {
                    path: rel.clone(),
                    message: format!("{}: {}", error.instance_path(), error),
                });
            }
        }

        if let Some(id) = str_field(&value, spec.id_field) {
            let expected = match str_field(&value, spec.version_field) {
                Some(version) => format!("{id}@{version}"),
                None => id.to_string(),
            };
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let matches = match spec.kind {
                DocKind::Versioned => stem == expected,
                DocKind::State => stem == id || stem == expected,
            };
            if !matches {
                diagnostics.push(Diagnostic {
                    path: rel.clone(),
                    message: format!(
                        "filename '{stem}' does not match document id/version '{expected}'"
                    ),
                });
            }
        }
        out.push((rel, value));
    }
    out
}

fn merge_and_load(config: &Value, dims: &Value, state: &Value) -> Result<(), String> {
    let config: CrossMatrixConfiguration =
        serde_json::from_value(config.clone()).map_err(|e| format!("configuration: {e}"))?;
    let dims: CrossMatrixDimensions =
        serde_json::from_value(dims.clone()).map_err(|e| format!("dimensions: {e}"))?;
    let state: CrossMatrixState =
        serde_json::from_value(state.clone()).map_err(|e| format!("state: {e}"))?;
    let model = crate::merge::merge(&config, &dims, &state)?;
    let text = serde_json::to_string(&model).map_err(|e| e.to_string())?;
    crossmatrix::Model::load(&text).map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("embedded schema is valid JSON")
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn str_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn array_field(value: &Value, field: &str) -> Vec<Value> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| items.to_vec())
        .unwrap_or_default()
}

fn declared_scales(config: &Value) -> BTreeSet<String> {
    array_field(config, "scales")
        .iter()
        .filter_map(|scale| str_field(scale, "id").map(String::from))
        .collect()
}

fn dimension_exists(dims: &Value, dimension_id: &str) -> bool {
    array_field(dims, "dimensions")
        .iter()
        .any(|dim| str_field(dim, "id") == Some(dimension_id))
}

fn dimension_has_member(dims: &Value, dimension_id: &str, member_id: &str) -> bool {
    array_field(dims, "dimensions")
        .iter()
        .find(|dim| str_field(dim, "id") == Some(dimension_id))
        .map(|dim| {
            array_field(dim, "members")
                .iter()
                .any(|member| str_field(member, "id") == Some(member_id))
        })
        .unwrap_or(false)
}

fn scale_levels(config: &Value, scale_id: &str) -> Option<Vec<String>> {
    array_field(config, "scales")
        .iter()
        .find(|scale| str_field(scale, "id") == Some(scale_id))
        .map(|scale| {
            array_field(scale, "levels")
                .iter()
                .filter_map(|level| str_field(level, "observation").map(String::from))
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ConfigStore, DimensionsStore, StateStore};

    const CONFIG_JSON: &str = include_str!("../../../examples/split/hoq.config.json");
    const DIMS_JSON: &str = include_str!("../../../examples/split/qfd-fmeca.dimensions.json");
    const STATE_JSON: &str = include_str!("../../../examples/split/demo.state.json");

    const CONFIG_REL: &str = "config/cfg_hoq_qfd_fmeca@1.0.0.json";
    const DIMS_REL: &str = "dimensions/dimset_qfd_fmeca@1.0.0.json";
    const STATE_REL: &str = "state/state_demo.json";

    fn env_with<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    fn write_doc(root: &Path, rel: &str, json: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("create doc dir");
        fs::write(path, json).expect("write doc");
    }

    fn seed_valid_store(root: &Path) {
        write_doc(root, CONFIG_REL, CONFIG_JSON);
        write_doc(root, DIMS_REL, DIMS_JSON);
        write_doc(root, STATE_REL, STATE_JSON);
    }

    fn mutate(root: &Path, rel: &str, edit: impl FnOnce(&mut Value)) {
        let path = root.join(rel);
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("read doc");
        edit(&mut value);
        fs::write(path, serde_json::to_string_pretty(&value).unwrap()).expect("write doc");
    }

    fn diagnostic_matches(report: &CheckReport, path: &str, fragment: &str) -> bool {
        report
            .diagnostics
            .iter()
            .any(|d| d.path == path && d.message.contains(fragment))
    }

    #[test]
    fn resolve_root_prefers_crossmatrix_dir_env_over_ancestor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let cwd = project.join("nested");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(project.join(".crossmatrix")).unwrap();
        let env = env_with(&[("CROSSMATRIX_DIR", "model-store")]);
        assert_eq!(
            resolve_root(&env, &cwd),
            (cwd.join("model-store"), RootSource::Env)
        );
    }

    #[test]
    fn resolve_root_uses_nearest_ancestor_crossmatrix_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");
        let cwd = project.join("nested/deeper");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(project.join(".crossmatrix")).unwrap();
        let env = env_with(&[]);
        assert_eq!(
            resolve_root(&env, &cwd),
            (project.join(".crossmatrix"), RootSource::Ancestor)
        );
    }

    #[test]
    fn resolve_root_uses_xdg_data_home_when_no_env_or_ancestor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("project");
        fs::create_dir_all(&cwd).unwrap();
        let env = env_with(&[("XDG_DATA_HOME", "/data/home")]);
        assert_eq!(
            resolve_root(&env, &cwd),
            (
                PathBuf::from("/data/home/crossmatrix"),
                RootSource::XdgDataHome
            )
        );
    }

    #[test]
    fn resolve_root_uses_home_local_share_when_no_xdg() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("project");
        fs::create_dir_all(&cwd).unwrap();
        let env = env_with(&[("HOME", "/home/analyst")]);
        assert_eq!(
            resolve_root(&env, &cwd),
            (
                PathBuf::from("/home/analyst/.local/share/crossmatrix"),
                RootSource::Home
            )
        );
    }

    #[test]
    fn resolve_root_falls_back_to_cwd_when_nothing_is_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("project");
        fs::create_dir_all(&cwd).unwrap();
        let env = env_with(&[]);
        assert_eq!(
            resolve_root(&env, &cwd),
            (cwd.join("crossmatrix-state"), RootSource::Cwd)
        );
    }

    #[test]
    fn init_creates_the_documented_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let report = init(tmp.path()).expect("init");
        let expected: Vec<String> = [
            ".crossmatrix",
            ".crossmatrix/config",
            ".crossmatrix/dimensions",
            ".crossmatrix/state",
            ".crossmatrix/audit",
            ".crossmatrix/.gitattributes",
            ".crossmatrix/README.md",
            ".crossmatrix/config/.gitkeep",
            ".crossmatrix/dimensions/.gitkeep",
            ".crossmatrix/state/.gitkeep",
            ".crossmatrix/audit/.gitkeep",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(report.created, expected);
    }

    #[test]
    fn init_second_run_reports_everything_preserved_and_creates_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = init(tmp.path()).expect("first init");
        let second = init(tmp.path()).expect("second init");
        assert_eq!(
            (second.created, second.preserved),
            (Vec::<String>::new(), first.created)
        );
    }

    #[test]
    fn init_gitattributes_requests_union_merge_for_audit_logs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init(tmp.path()).expect("init");
        assert_eq!(
            fs::read_to_string(tmp.path().join(".crossmatrix/.gitattributes")).unwrap(),
            "audit/*.jsonl merge=union\n"
        );
    }

    #[test]
    fn init_does_not_overwrite_existing_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init(tmp.path()).expect("first init");
        let readme = tmp.path().join(".crossmatrix/README.md");
        fs::write(&readme, "hand-edited\n").unwrap();
        init(tmp.path()).expect("second init");
        assert_eq!(fs::read_to_string(&readme).unwrap(), "hand-edited\n");
    }

    #[test]
    fn check_accepts_a_valid_store_saved_through_the_stores() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let config: CrossMatrixConfiguration =
            serde_json::from_str(CONFIG_JSON).expect("config fixture");
        let dims: CrossMatrixDimensions =
            serde_json::from_str(DIMS_JSON).expect("dimensions fixture");
        let state: CrossMatrixState = serde_json::from_str(STATE_JSON).expect("state fixture");
        ConfigStore::new(root.clone())
            .put(&config)
            .expect("put config");
        DimensionsStore::new(root.clone())
            .put(&dims)
            .expect("put dimensions");
        StateStore::new(root.clone())
            .put_current(&state)
            .expect("put current state");
        assert!(check(&root).diagnostics.is_empty());
    }

    #[test]
    fn check_reports_unresolved_config_ref() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), STATE_REL, |state| {
            state["configRef"]["version"] = serde_json::json!("9.9.9");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            STATE_REL,
            "configRef: unresolved configuration"
        ));
    }

    #[test]
    fn check_reports_unresolved_dimension_set_ref() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), CONFIG_REL, |config| {
            config["dimensionSetRef"]["version"] = serde_json::json!("9.9.9");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            CONFIG_REL,
            "dimensionSetRef: unresolved dimension set"
        ));
    }

    #[test]
    fn check_reports_unknown_relation_dimension() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), CONFIG_REL, |config| {
            config["relations"][0]["from"] = serde_json::json!("dim_missing");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            CONFIG_REL,
            "relations[0].from: unknown dimension 'dim_missing'"
        ));
    }

    #[test]
    fn check_reports_unknown_cell_relation_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), STATE_REL, |state| {
            state["cells"][0]["relationId"] = serde_json::json!("rel_missing");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            STATE_REL,
            "cells[0].relationId: unknown relation 'rel_missing'"
        ));
    }

    #[test]
    fn check_reports_unknown_cell_member() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), STATE_REL, |state| {
            state["cells"][3]["to"] = serde_json::json!("char_missing");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            STATE_REL,
            "cells[3].to: unknown member 'char_missing' in dimension 'dim_char'"
        ));
    }

    #[test]
    fn check_reports_unknown_observation_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), STATE_REL, |state| {
            state["cells"][2]["observations"][0]["observation"] = serde_json::json!("bogus");
        });
        assert!(diagnostic_matches(
            &check(tmp.path()),
            STATE_REL,
            "cells[2].observations[0].observation: unknown token 'bogus' in scale 'qfd'"
        ));
    }

    #[test]
    fn check_reports_filename_id_mismatch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        fs::remove_file(tmp.path().join(STATE_REL)).unwrap();
        let mismatched = "state/wrong_name.json";
        write_doc(tmp.path(), mismatched, STATE_JSON);
        assert!(diagnostic_matches(
            &check(tmp.path()),
            mismatched,
            "does not match document id/version"
        ));
    }

    #[test]
    fn check_reports_schema_violation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        mutate(tmp.path(), CONFIG_REL, |config| {
            config["aspect"] = serde_json::json!(123);
        });
        assert!(diagnostic_matches(&check(tmp.path()), CONFIG_REL, "aspect"));
    }

    #[test]
    fn check_reports_malformed_audit_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        seed_valid_store(tmp.path());
        write_doc(tmp.path(), "audit/state_demo.jsonl", "this is not json\n");
        assert!(diagnostic_matches(
            &check(tmp.path()),
            "audit/state_demo.jsonl:1",
            "invalid JSON"
        ));
    }

    #[test]
    fn check_reports_a_missing_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            check(&tmp.path().join("does-not-exist")).diagnostics.len(),
            1
        );
    }
}
