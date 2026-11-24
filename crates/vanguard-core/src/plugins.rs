//! Loader and validator for community database driver plugin manifests (`plugins/*.json`).

use crate::DbError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED_TRANSPORTS: &[&str] = &["tcp", "file", "http", "in_process"];
const ALLOWED_CAPABILITIES: &[&str] = &[
    "query",
    "schema_inspect",
    "streaming_export",
    "transactions",
    "explain",
    "wal_inspect",
    "parquet_scan",
    "columnar_batch",
];

/// Community database driver manifest loaded from `plugins/*.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverPluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: String,
    pub wire_transport: String,
    pub default_port: Option<u16>,
    pub author: String,
    pub license: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub streaming_export: bool,
    pub zero_c_deps: bool,
    pub min_core_version: String,
}

/// Validation report for a single plugin manifest file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginValidationReport {
    pub file: String,
    pub plugin_id: Option<String>,
    pub valid: bool,
    pub errors: Vec<String>,
}

/// Validates a single parsed `DriverPluginManifest` against db-vanguard schema constraints.
#[must_use]
pub fn validate_manifest(manifest: &DriverPluginManifest) -> Vec<String> {
    let mut errors = Vec::new();

    if !is_kebab_case(&manifest.id) {
        errors.push(format!(
            "id '{}' must be lowercase kebab-case starting with an ASCII letter",
            manifest.id
        ));
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 80 {
        errors.push("name must be 1..=80 characters".to_string());
    }
    if !is_semver(&manifest.version) {
        errors.push(format!(
            "version '{}' must be valid semver (MAJOR.MINOR.PATCH)",
            manifest.version
        ));
    }
    if !is_semver(&manifest.min_core_version) {
        errors.push(format!(
            "min_core_version '{}' must be valid semver (MAJOR.MINOR.PATCH)",
            manifest.min_core_version
        ));
    }
    if manifest.protocol.trim().is_empty() {
        errors.push("protocol must not be empty".to_string());
    }
    if !ALLOWED_TRANSPORTS.contains(&manifest.wire_transport.as_str()) {
        errors.push(format!(
            "wire_transport '{}' must be one of {:?}",
            manifest.wire_transport, ALLOWED_TRANSPORTS
        ));
    }
    if matches!(manifest.wire_transport.as_str(), "tcp" | "http") && manifest.default_port.is_none()
    {
        errors.push(format!(
            "wire_transport '{}' requires a non-null default_port",
            manifest.wire_transport
        ));
    }
    if manifest.author.trim().is_empty() {
        errors.push("author must not be empty".to_string());
    }
    if manifest.license.trim().is_empty() {
        errors.push("license must not be empty".to_string());
    }
    if manifest.description.trim().len() < 16 {
        errors.push("description must be at least 16 characters".to_string());
    }
    if manifest.capabilities.is_empty() {
        errors.push("capabilities must contain at least one entry".to_string());
    } else {
        for cap in &manifest.capabilities {
            if !ALLOWED_CAPABILITIES.contains(&cap.as_str()) {
                errors.push(format!(
                    "unsupported capability '{cap}'; allowed: {ALLOWED_CAPABILITIES:?}"
                ));
            }
        }
    }

    errors
}

/// Scans `dir` for `*.json` manifests (excluding `schema.json`), parses, and validates all entries.
pub fn load_plugins_from_dir(
    dir: &Path,
) -> Result<(Vec<DriverPluginManifest>, Vec<PluginValidationReport>), DbError> {
    if !dir.exists() {
        return Err(DbError::NotFound(format!(
            "plugins directory '{}' does not exist",
            dir.display()
        )));
    }

    let mut json_files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name != "schema.json")
        })
        .collect();
    json_files.sort();

    let mut manifests = Vec::new();
    let mut reports = Vec::new();
    let mut seen_ids = HashSet::new();

    for path in json_files {
        let file_label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.json")
            .to_string();

        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => {
                reports.push(PluginValidationReport {
                    file: file_label,
                    plugin_id: None,
                    valid: false,
                    errors: vec![format!("failed to read file: {err}")],
                });
                continue;
            }
        };

        match serde_json::from_str::<DriverPluginManifest>(&content) {
            Ok(manifest) => {
                let mut errors = validate_manifest(&manifest);
                if !seen_ids.insert(manifest.id.clone()) {
                    errors.push(format!("duplicate plugin id '{}'", manifest.id));
                }
                let valid = errors.is_empty();
                let pid = manifest.id.clone();
                if valid {
                    manifests.push(manifest);
                }
                reports.push(PluginValidationReport {
                    file: file_label,
                    plugin_id: Some(pid),
                    valid,
                    errors,
                });
            }
            Err(err) => {
                reports.push(PluginValidationReport {
                    file: file_label,
                    plugin_id: None,
                    valid: false,
                    errors: vec![format!("JSON schema mismatch: {err}")],
                });
            }
        }
    }

    Ok((manifests, reports))
}

fn is_kebab_case(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_lowercase() || *bytes.last().unwrap() == b'-' {
        return false;
    }
    let mut prev_dash = false;
    for &b in bytes {
        if b == b'-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() {
            prev_dash = false;
        } else {
            return false;
        }
    }
    true
}

fn is_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_well_formed_manifest_and_rejects_broken_fields() {
        let good = DriverPluginManifest {
            id: "postgres-wire-v3".to_string(),
            name: "PostgreSQL Wire v3".to_string(),
            version: "1.0.0".to_string(),
            protocol: "postgresql-v3".to_string(),
            wire_transport: "tcp".to_string(),
            default_port: Some(5432),
            author: "xity4uu-a11y".to_string(),
            license: "MIT".to_string(),
            description: "Pure-Rust PostgreSQL v3 wire protocol driver with streaming exports."
                .to_string(),
            capabilities: vec!["query".to_string(), "streaming_export".to_string()],
            streaming_export: true,
            zero_c_deps: true,
            min_core_version: "1.0.0".to_string(),
        };
        assert!(validate_manifest(&good).is_empty());

        let mut bad = good;
        bad.id = "Bad_ID!".to_string();
        bad.version = "1.0".to_string();
        bad.default_port = None;
        bad.capabilities = vec!["unknown_cap".to_string()];
        let errs = validate_manifest(&bad);
        assert!(errs.len() >= 4);
    }

    #[test]
    fn loads_and_validates_workspace_plugins_directory() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        if dir.exists() {
            let (manifests, reports) = load_plugins_from_dir(&dir).unwrap();
            assert_eq!(reports.len(), 5, "expected 5 community plugin manifests");
            for rep in &reports {
                assert!(
                    rep.valid,
                    "plugin '{}' failed validation: {:?}",
                    rep.file, rep.errors
                );
            }
            assert_eq!(manifests.len(), 5);
        }
    }
}
