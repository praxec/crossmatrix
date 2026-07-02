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
        "scales": config_val["scales"],
        "dimensions": dims_val["dimensions"],
        "relations": relations,
        "contractions": config_val["contractions"],
    });

    Ok(model)
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
