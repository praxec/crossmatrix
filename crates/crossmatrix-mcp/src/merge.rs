//! Merge: assemble a Configuration + Dimensions + State into the engine model
//! JSON that `crossmatrix::Model::load` consumes.

use std::collections::HashMap;

use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};

/// Merge the three ADR-0004 split layers into a single `serde_json::Value`
/// suitable for `crossmatrix::Model::load`.
pub fn merge(
    config: &CrossMatrixConfiguration,
    dimensions: &CrossMatrixDimensions,
    state: &CrossMatrixState,
) -> Result<serde_json::Value, String> {
    // Fail if State.configRef.version doesn't exactly match Configuration.version (ADR-0004).
    if state.config_ref.version != config.version {
        return Err(format!(
            "configRef version mismatch: state expects '{}', configuration is '{}'",
            state.config_ref.version, config.version
        ));
    }

    // Serialize config + dimensions once so we can extract sub-objects.
    let config_val = serde_json::to_value(config).map_err(|e| format!("serialize config: {e}"))?;
    let dims_val =
        serde_json::to_value(dimensions).map_err(|e| format!("serialize dimensions: {e}"))?;

    // Gather known relation ids for dangling-ref detection.
    let known_relation_ids: Vec<&str> = config.relations.iter().map(|r| r.id.as_str()).collect();

    // Group state cells by relationId for constant-time lookup.
    let state_cells: Vec<serde_json::Value> = state
        .cells
        .iter()
        .map(|c| serde_json::to_value(c).map_err(|e| format!("serialize state cell: {e}")))
        .collect::<Result<_, _>>()?;
    let mut cells_by_rel: HashMap<&str, Vec<&serde_json::Value>> = HashMap::new();
    for sc in &state_cells {
        let rid = sc["relationId"].as_str().unwrap_or("");
        // Reject cells referencing unknown relationIds.
        if !known_relation_ids.contains(&rid) {
            return Err(format!(
                "state cell references unknown relationId '{}'",
                rid
            ));
        }
        cells_by_rel.entry(rid).or_default().push(sc);
    }

    // Build engine relations: only the fields the engine schema expects.
    let mut relations: Vec<serde_json::Value> = Vec::new();
    for rel in &config.relations {
        let cells: Vec<serde_json::Value> = cells_by_rel
            .get(rel.id.as_str())
            .map(|v| {
                v.iter()
                    .map(|sc| {
                        serde_json::json!({
                            "from": sc["from"],
                            "to": sc["to"],
                            "observations": sc["observations"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut r = serde_json::json!({
            "id": rel.id,
            "from": rel.from,
            "to": rel.to,
            "relationType": rel.relation_type,
            "scale": rel.scale,
            "cells": cells,
        });
        r["coveragePolicy"] = serde_json::json!(rel.coverage_policy);
        if let Some(ref s) = rel.status {
            r["status"] = serde_json::json!(s);
        }
        if let Some(ref sb) = rel.superseded_by {
            r["supersededBy"] = serde_json::json!(sb);
        }
        relations.push(r);
    }

    let model = serde_json::json!({
        "schemaVersion": config_val["schemaVersion"],
        "modelId": state.state_id,
        "scales": config_val.get("scales").cloned().unwrap_or_else(|| serde_json::json!([])),
        "dimensions": dims_val["dimensions"],
        "relations": relations,
        "contractions": config_val.get("contractions").cloned().unwrap_or_else(|| serde_json::json!([])),
    });

    Ok(model)
}

/// Inverse of [`merge`]: decompose a validated engine model back into the three
/// ADR-0004 layers. The provided `prev_*` documents supply the identity/schema
/// fields (and any State fields the engine model does not carry), so a model
/// loaded from a store round-trips through `merge(split(m))`.
pub fn split(
    model: &crossmatrix::CrossMatrixModel,
    prev_config: &CrossMatrixConfiguration,
    prev_dims: &CrossMatrixDimensions,
    prev_state: &CrossMatrixState,
) -> Result<
    (
        CrossMatrixConfiguration,
        CrossMatrixDimensions,
        CrossMatrixState,
    ),
    String,
> {
    // Configuration: keep the methodology identity from prev_config; take the
    // declared scales/contractions from the model; relations lose their cells
    // (State owns those) but keep the previous declaration's `label` by id.
    let mut config_val =
        serde_json::to_value(prev_config).map_err(|e| format!("serialize config: {e}"))?;
    let mut relations: Vec<serde_json::Value> = Vec::new();
    for rel in &model.relations {
        let mut rel_val =
            serde_json::to_value(rel).map_err(|e| format!("serialize relation: {e}"))?;
        let prev_label = prev_config
            .relations
            .iter()
            .find(|decl| decl.id == rel.id)
            .and_then(|decl| decl.label.clone());
        let obj = rel_val
            .as_object_mut()
            .ok_or_else(|| "relation did not serialize to an object".to_string())?;
        obj.remove("cells");
        if let Some(label) = prev_label {
            obj.insert("label".to_string(), serde_json::Value::String(label));
        }
        relations.push(rel_val);
    }
    config_val["scales"] =
        serde_json::to_value(&model.scales).map_err(|e| format!("serialize scales: {e}"))?;
    config_val["relations"] = serde_json::Value::Array(relations);
    config_val["contractions"] = serde_json::to_value(&model.contractions)
        .map_err(|e| format!("serialize contractions: {e}"))?;

    // Dimensions: keep the entity-set identity from prev_dims; take the axes
    // and rosters from the model.
    let mut dims_val =
        serde_json::to_value(prev_dims).map_err(|e| format!("serialize dimensions: {e}"))?;
    dims_val["dimensions"] = serde_json::to_value(&model.dimensions)
        .map_err(|e| format!("serialize model dimensions: {e}"))?;

    // State: keep everything from prev_state except the cells, which are the
    // model's cells flattened in relation order and tagged with `relationId`.
    let mut state_val =
        serde_json::to_value(prev_state).map_err(|e| format!("serialize state: {e}"))?;
    let mut cells: Vec<serde_json::Value> = Vec::new();
    for rel in &model.relations {
        for cell in &rel.cells {
            cells.push(serde_json::json!({
                "relationId": rel.id,
                "from": cell.from,
                "to": cell.to,
                "observations": cell.observations,
            }));
        }
    }
    state_val["cells"] = serde_json::Value::Array(cells);

    let config =
        serde_json::from_value(config_val).map_err(|e| format!("build configuration: {e}"))?;
    let dims = serde_json::from_value(dims_val).map_err(|e| format!("build dimensions: {e}"))?;
    let state = serde_json::from_value(state_val).map_err(|e| format!("build state: {e}"))?;
    Ok((config, dims, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_split_examples_yields_loadable_model() {
        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dimensions: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        let merged = merge(&config, &dimensions, &state).expect("merge must succeed");

        let model_result = crossmatrix::Model::load(&merged.to_string());
        assert!(model_result.is_ok(), "merged model must load via core");
    }

    #[test]
    fn merge_rejects_config_version_mismatch() {
        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dimensions: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");

        // State whose configRef.version (2.0.0) differs from config.version (1.0.0).
        let state_json = r#"{
            "schemaVersion": "0.2.0",
            "stateId": "state_demo",
            "configRef": {
                "configId": "cfg_hoq_qfd_fmeca",
                "version": "2.0.0"
            },
            "cells": []
        }"#;
        let state: CrossMatrixState = serde_json::from_str(state_json).expect("deserialize state");

        let result = merge(&config, &dimensions, &state);
        assert!(result.is_err(), "merge must reject config version mismatch");
    }

    #[test]
    fn split_then_merge_round_trips_an_engine_model() {
        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dimensions: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        let merged = merge(&config, &dimensions, &state).expect("merge must succeed");
        let doc: crossmatrix::CrossMatrixModel =
            serde_json::from_value(merged).expect("merged model must deserialize");
        let (config2, dims2, state2) =
            split(&doc, &config, &dimensions, &state).expect("split must succeed");
        let round = merge(&config2, &dims2, &state2).expect("merge round must succeed");
        let mut expected = serde_json::to_value(&doc).expect("serialize model");
        if let Some(obj) = expected.as_object_mut() {
            obj.remove("findings");
        }
        assert_eq!(round, expected);
    }

    #[test]
    fn merge_rejects_cell_with_unknown_relation_id() {
        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dimensions: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");

        // State whose cell references a relationId not declared in config.relations.
        let state_json = r#"{
            "schemaVersion": "0.2.0",
            "stateId": "state_demo",
            "version": "1.0.0",
            "configRef": {
                "configId": "cfg_hoq_qfd_fmeca",
                "version": "1.0.0"
            },
            "cells": [
                {
                    "relationId": "rel_nonexistent",
                    "from": "req_fast_checkout",
                    "to": "char_response_latency",
                    "observations": [
                        {
                            "observation": "strong",
                            "provenanceClass": "normative"
                        }
                    ]
                }
            ]
        }"#;
        let state: CrossMatrixState = serde_json::from_str(state_json).expect("deserialize state");

        let result = merge(&config, &dimensions, &state);
        assert!(
            result.is_err(),
            "merge must reject a cell with an unknown relationId"
        );
    }
}
