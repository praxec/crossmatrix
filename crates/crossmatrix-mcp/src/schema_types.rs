//! Schema types generated from the three ADR-0004 split schemas:
//! Configuration, Dimensions, State.
//!
//! Generated via `typify::import_types!` — do not hand-author.

mod config {
    #![allow(clippy::all, dead_code, unused_imports)]
    typify::import_types!("schema/crossmatrix-config.schema.json");
}

mod dimensions {
    #![allow(clippy::all, dead_code, unused_imports)]
    typify::import_types!("schema/crossmatrix-dimensions.schema.json");
}

mod state {
    #![allow(clippy::all, dead_code, unused_imports)]
    typify::import_types!("schema/crossmatrix-state.schema.json");
}

pub use config::CrossMatrixConfiguration;
pub use dimensions::CrossMatrixDimensions;
pub use state::CrossMatrixState;

#[cfg(test)]
mod tests {
    use super::config::CrossMatrixConfiguration;
    use super::dimensions::CrossMatrixDimensions;
    use super::state::CrossMatrixState;

    #[test]
    fn deserialize_hoq_config_example() {
        let json = include_str!("../../../examples/split/hoq.config.json");
        let result: Result<CrossMatrixConfiguration, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "hoq.config.json must deserialize");
    }

    #[test]
    fn deserialize_qfd_fmeca_dimensions_example() {
        let json = include_str!("../../../examples/split/qfd-fmeca.dimensions.json");
        let result: Result<CrossMatrixDimensions, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "qfd-fmeca.dimensions.json must deserialize");
    }

    #[test]
    fn deserialize_demo_state_example() {
        let json = include_str!("../../../examples/split/demo.state.json");
        let result: Result<CrossMatrixState, _> = serde_json::from_str(json);
        assert!(result.is_ok(), "demo.state.json must deserialize");
    }
}
