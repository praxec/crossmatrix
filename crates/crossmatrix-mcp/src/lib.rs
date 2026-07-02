//! crossmatrix-mcp library crate — schema types generated from canonical JSON Schemas.

pub mod merge;
pub mod schema_types;
pub mod store;

use crate::store::ConfigStore;
use crate::store::DimensionsStore;
use crate::store::StateStore;

/// Open a model from the three stores: read State → resolve configRef →
/// resolve dimensionSetRef → merge → Model::load.
pub fn open(
    config_store: &ConfigStore,
    dims_store: &DimensionsStore,
    state_store: &StateStore,
    state_id: &str,
    version: &str,
) -> Result<crossmatrix::Model, String> {
    let state = state_store.get(state_id, version)?;
    let config = config_store.get(&state.config_ref.config_id, &state.config_ref.version)?;
    let dimensions = dims_store.get(
        &config.dimension_set_ref.dimension_set_id,
        &config.dimension_set_ref.version,
    )?;
    let model_json = merge::merge(&config, &dimensions, &state)?;
    let model_str = serde_json::to_string(&model_json).map_err(|e| e.to_string())?;
    crossmatrix::Model::load(&model_str).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};

    #[test]
    fn open_from_split_examples_yields_expected_dimensions_count() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let config_store = ConfigStore::new(root.clone());
        let dims_store = DimensionsStore::new(root.clone());
        let state_store = StateStore::new(root);

        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");
        let dims: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        config_store.put(&config).expect("put config");
        dims_store.put(&dims).expect("put dimensions");
        state_store.put(&state).expect("put state");

        let model = open(
            &config_store,
            &dims_store,
            &state_store,
            &state.state_id,
            state.version.as_deref().unwrap_or(""),
        )
        .expect("open must succeed");

        assert_eq!(
            model.dimensions().len(),
            3,
            "opened model must have 3 dimensions from the qfd-fmeca example"
        );
    }
}
