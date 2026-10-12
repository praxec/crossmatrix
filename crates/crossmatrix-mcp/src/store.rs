//! Persistence layer: filesystem-backed stores for Configuration,
//! Dimensions, and State documents, keyed by stable id@version per ADR-0004.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::schema_types::{CrossMatrixConfiguration, CrossMatrixDimensions, CrossMatrixState};

/// Recursively sort object keys (explicit, independent of serde_json's Map
/// ordering feature). Takes ownership so large documents are re-keyed by move
/// rather than deep-cloned.
fn canonicalize(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, item)| (key, canonicalize(item)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize).collect())
        }
        other => other,
    }
}

/// Write `value` to `path` as canonical JSON — recursively sorted object keys,
/// 2-space indent, exactly one trailing newline — atomically via a temp file in
/// the same directory followed by `fs::rename`.
pub fn write_canonical(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    write_canonical_owned(path, value.clone())
}

/// [`write_canonical`] for a value the caller no longer needs (no deep clone).
pub fn write_canonical_owned(path: &Path, value: serde_json::Value) -> Result<(), String> {
    let canonical = canonicalize(value);
    let mut text = serde_json::to_string_pretty(&canonical).map_err(|e| e.to_string())?;
    text.push('\n');

    let parent = path
        .parent()
        .ok_or_else(|| format!("path '{}' has no parent directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("path '{}' has no file name", path.display()))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        nanos
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

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

    /// Persist a Configuration; written canonically to
    /// `<root>/config/<configId>@<version>.json`.
    pub fn put(&self, config: &CrossMatrixConfiguration) -> Result<(), String> {
        let path = self.config_path(&config.config_id, &config.version);
        let value = serde_json::to_value(config).map_err(|e| e.to_string())?;
        write_canonical_owned(&path, value)
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

    /// Persist a Dimensions doc; written canonically to
    /// `<root>/dimensions/<dimensionSetId>@<version>.json`.
    pub fn put(&self, dims: &CrossMatrixDimensions) -> Result<(), String> {
        let path = self.dims_path(&dims.dimension_set_id, &dims.version);
        let value = serde_json::to_value(dims).map_err(|e| e.to_string())?;
        write_canonical_owned(&path, value)
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

    /// Persist a State doc; written canonically to
    /// `<root>/state/<stateId>@<version>.json`.
    pub fn put(&self, state: &CrossMatrixState) -> Result<(), String> {
        let path = self.state_path(&state.state_id, state.version.as_deref().unwrap_or(""));
        let value = serde_json::to_value(state).map_err(|e| e.to_string())?;
        write_canonical_owned(&path, value)
    }

    /// Retrieve a State doc by id + version.
    pub fn get(&self, state_id: &str, version: &str) -> Result<CrossMatrixState, String> {
        let path = self.state_path(state_id, version);
        let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    /// Path of the unversioned "current" State doc: `<root>/state/<stateId>.json`.
    fn current_path(&self, state_id: &str) -> PathBuf {
        self.state_dir().join(format!("{}.json", state_id))
    }

    /// Persist the unversioned "current" State doc: `<root>/state/<stateId>.json`.
    pub fn put_current(&self, state: &CrossMatrixState) -> Result<(), String> {
        let path = self.current_path(&state.state_id);
        let value = serde_json::to_value(state).map_err(|e| e.to_string())?;
        write_canonical_owned(&path, value)
    }

    /// Read the unversioned "current" State doc: `<root>/state/<stateId>.json`.
    pub fn get_current(&self, state_id: &str) -> Result<CrossMatrixState, String> {
        let path = self.current_path(state_id);
        let json = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    /// Whether the unversioned "current" State doc exists.
    pub fn exists_current(&self, state_id: &str) -> bool {
        self.current_path(state_id).is_file()
    }
}

/// One append-only audit-log record. Serialized with camelCase keys:
/// `ts`, `requestId`, `actor`, `op`, `beforeVersion`, `afterVersion`, `summary`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub ts: String,
    pub request_id: String,
    pub actor: serde_json::Value,
    pub op: String,
    pub before_version: u64,
    pub after_version: u64,
    pub summary: String,
}

/// Append-only JSONL audit log rooted at `<root>/audit/<stateId>.jsonl`.
pub struct AuditLog {
    root: PathBuf,
}

impl AuditLog {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn log_path(&self, state_id: &str) -> PathBuf {
        self.root.join("audit").join(format!("{}.jsonl", state_id))
    }

    /// Append exactly one compact JSON object (sorted keys) plus `\n`.
    pub fn append(&self, state_id: &str, entry: &AuditEntry) -> Result<(), String> {
        let path = self.log_path(state_id);
        let parent = path
            .parent()
            .ok_or_else(|| format!("path '{}' has no parent directory", path.display()))?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let value = serde_json::to_value(entry).map_err(|e| e.to_string())?;
        let compact = serde_json::to_string(&canonicalize(value)).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(compact.as_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read all entries in file order; a missing file yields an empty vec.
    pub fn read(&self, state_id: &str) -> Result<Vec<AuditEntry>, String> {
        let path = self.log_path(state_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            entries.push(serde_json::from_str(line).map_err(|e| e.to_string())?);
        }
        Ok(entries)
    }
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`, computed with only `std`
/// (Howard Hinnant's `civil_from_days`).
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
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

    #[test]
    fn write_canonical_sorts_keys_indents_two_spaces_and_ends_with_one_newline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("out.json");
        write_canonical(&path, &serde_json::json!({"b": 1, "a": {"d": 2, "c": 3}}))
            .expect("write_canonical");
        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "{\n  \"a\": {\n    \"c\": 3,\n    \"d\": 2\n  },\n  \"b\": 1\n}\n"
        );
    }

    #[test]
    fn write_canonical_leaves_no_temp_files_in_the_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("out.json");
        write_canonical(&path, &serde_json::json!({"a": 1})).expect("write_canonical");
        let temp_files: Vec<_> = fs::read_dir(tmp.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(
            temp_files.is_empty(),
            "temp files left behind: {temp_files:?}"
        );
    }

    #[test]
    fn state_store_put_current_get_current_roundtrips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::new(tmp.path().to_path_buf());
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");
        store.put_current(&state).expect("put_current");
        let got = store.get_current(&state.state_id).expect("get_current");
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::to_value(&got).unwrap()
        );
    }

    #[test]
    fn state_store_exists_current_reports_written_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::new(tmp.path().to_path_buf());
        let state: CrossMatrixState =
            serde_json::from_str(include_str!("../../../examples/split/demo.state.json"))
                .expect("deserialize state fixture");
        store.put_current(&state).expect("put_current");
        assert!(store.exists_current(&state.state_id));
    }

    #[test]
    fn state_store_exists_current_is_false_for_unknown_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = StateStore::new(tmp.path().to_path_buf());
        assert!(!store.exists_current("state_missing"));
    }

    #[test]
    fn audit_log_read_missing_file_is_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log = AuditLog::new(tmp.path().to_path_buf());
        assert!(log.read("state_missing").expect("read").is_empty());
    }

    #[test]
    fn audit_log_append_twice_then_read_returns_entries_in_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log = AuditLog::new(tmp.path().to_path_buf());
        let first = sample_entry("req-1", "observe", 0, 1);
        let second = sample_entry("req-2", "observe", 1, 2);
        log.append("state_demo", &first).expect("append first");
        log.append("state_demo", &second).expect("append second");
        assert_eq!(log.read("state_demo").expect("read"), vec![first, second]);
    }

    #[test]
    fn audit_log_appends_one_compact_line_per_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log = AuditLog::new(tmp.path().to_path_buf());
        log.append("state_demo", &sample_entry("req-1", "observe", 0, 1))
            .expect("append");
        let raw = fs::read_to_string(tmp.path().join("audit/state_demo.jsonl")).expect("read");
        assert_eq!(raw.lines().count(), 1);
    }

    #[test]
    fn audit_log_line_has_expected_camel_case_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let log = AuditLog::new(tmp.path().to_path_buf());
        log.append("state_demo", &sample_entry("req-1", "observe", 0, 1))
            .expect("append");
        let raw = fs::read_to_string(tmp.path().join("audit/state_demo.jsonl")).expect("read");
        let value: serde_json::Value =
            serde_json::from_str(raw.trim_end()).expect("parse audit line");
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "actor",
                "afterVersion",
                "beforeVersion",
                "op",
                "requestId",
                "summary",
                "ts",
            ]
        );
    }

    #[test]
    fn now_rfc3339_has_utc_display_shape() {
        let stamp = now_rfc3339();
        assert_eq!(
            (
                &stamp[4..5],
                &stamp[7..8],
                &stamp[10..11],
                &stamp[13..14],
                &stamp[16..17],
                &stamp[19..20],
            ),
            ("-", "-", "T", ":", ":", "Z")
        );
    }

    fn sample_entry(request_id: &str, op: &str, before: u64, after: u64) -> AuditEntry {
        AuditEntry {
            ts: "2024-01-02T03:04:05Z".to_string(),
            request_id: request_id.to_string(),
            actor: serde_json::json!({"actorType": "llm", "persona": "analyst"}),
            op: op.to_string(),
            before_version: before,
            after_version: after,
            summary: "observed".to_string(),
        }
    }
}
