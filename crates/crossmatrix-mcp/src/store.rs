//! Persistence layer: filesystem-backed stores for Configuration,
//! Dimensions, and State documents, keyed by stable id@version per ADR-0004.

use std::fs;
use std::path::PathBuf;

use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};

/// Filesystem-backed store for Configuration documents.
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    fn config_path(&self, config_id: &str, version: &str) -> PathBuf {
        self.config_dir()
            .join(format!("{}@{}.json", config_id, version))
    }

    /// Persist a Configuration; written to `<root>/config/<configId>@<version>.json`.
    pub fn put(&self, config: &CrossMatrixConfiguration) -> Result<(), String> {
        let dir = self.config_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = self.config_path(&config.config_id, &config.version);
        let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Retrieve a Configuration by id + version.
    pub fn get(&self, config_id: &str, version: &str) -> Result<CrossMatrixConfiguration, String> {
        let path = self.config_path(config_id, version);
        let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

/// Filesystem-backed store for Dimensions documents.
pub struct DimensionsStore {
    root: PathBuf,
}

impl DimensionsStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn dims_dir(&self) -> PathBuf {
        self.root.join("dimensions")
    }

    fn dims_path(&self, dims_id: &str, version: &str) -> PathBuf {
        self.dims_dir()
            .join(format!("{}@{}.json", dims_id, version))
    }

    /// Persist a Dimensions doc; written to `<root>/dimensions/<dimensionSetId>@<version>.json`.
    pub fn put(&self, dims: &CrossMatrixDimensions) -> Result<(), String> {
        let dir = self.dims_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = self.dims_path(&dims.dimension_set_id, &dims.version);
        let json = serde_json::to_string_pretty(dims).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Retrieve a Dimensions doc by id + version.
    pub fn get(&self, dims_id: &str, version: &str) -> Result<CrossMatrixDimensions, String> {
        let path = self.dims_path(dims_id, version);
        let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

/// Filesystem-backed store for State documents.
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    fn state_path(&self, state_id: &str, version: &str) -> PathBuf {
        self.state_dir()
            .join(format!("{}@{}.json", state_id, version))
    }

    /// Persist a State doc; written to `<root>/state/<stateId>@<version>.json`.
    pub fn put(&self, state: &CrossMatrixState) -> Result<(), String> {
        let dir = self.state_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = self.state_path(&state.state_id, state.version.as_deref().unwrap_or(""));
        let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Retrieve a State doc by id + version.
    pub fn get(&self, state_id: &str, version: &str) -> Result<CrossMatrixState, String> {
        let path = self.state_path(state_id, version);
        let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_store_put_get_roundtrips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(tmp.path().to_path_buf());

        let config: CrossMatrixConfiguration =
            serde_json::from_str(include_str!("../../../examples/split/hoq.config.json"))
                .expect("deserialize config fixture");

        store.put(&config).expect("put must succeed");
        let got = store
            .get(&config.config_id, &config.version)
            .expect("get must succeed");

        assert_eq!(
            serde_json::to_value(&config).unwrap(),
            serde_json::to_value(&got).unwrap(),
            "round-tripped config must equal original"
        );
    }

    #[test]
    fn dimensions_store_put_get_roundtrips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = DimensionsStore::new(tmp.path().to_path_buf());

        let dims: CrossMatrixDimensions = serde_json::from_str(include_str!(
            "../../../examples/split/qfd-fmeca.dimensions.json"
        ))
        .expect("deserialize dimensions fixture");

        store.put(&dims).expect("put must succeed");
        let got = store
            .get(&dims.dimension_set_id, &dims.version)
            .expect("get must succeed");

        assert_eq!(
            serde_json::to_value(&dims).unwrap(),
            serde_json::to_value(&got).unwrap(),
            "round-tripped dimensions must equal original"
        );
    }

    #[test]
    fn state_store_put_get_roundtrips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::new(tmp.path().to_path_buf());

        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");

        store.put(&state).expect("put must succeed");
        let got = store
            .get(&state.state_id, state.version.as_deref().unwrap_or(""))
            .expect("get must succeed");

        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::to_value(&got).unwrap(),
            "round-tripped state must equal original"
        );
    }
}
