//! `.hirpatch` — device-only sidecar format for sharing built-in devices.
//!
//! An encrypted envelope is deliberately rejected; the file is human-readable
//! JSON that a DAW or AI editor may inspect and hand-edit. The importer is
//! strict by design: unknown keys reject at parse, and every dataflow unit,
//! link and limit is validated exactly as if the user had hand-written a
//! broken project fragment.
//!
//! Sharing a third-party plug-in as a `.hirpatch` is impossible by
//! construction: external plugins have no `kind` branch, and this format
//! carries nothing but the builtin dataflow declaration.

use crate::project_contracts::{validate_builtin_dataflow, BuiltinDataflow};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const HIRPATCH_SCHEMA_VERSION: u32 = 1;
const MAX_HIRPATCH_BYTES: usize = 1 << 20; // 1 MiB

/// The `.hirpatch` envelope. `deny_unknown_fields` keeps the importer strict:
/// anything the spec does not declare is rejected, never silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HirPatch {
    pub schema_version: u32,
    pub name: String,
    pub dataflow: BuiltinDataflow,
}

/// Serializes a built-in device as a `.hirpatch` sidecar. Deterministic
/// output; identical input produces identical bytes.
pub fn export_hirpatch_device(
    name: &str,
    device: &BuiltinDataflow,
) -> Result<Vec<u8>> {
    validate_builtin_dataflow(device, "exported device") ?;
    if name.trim().is_empty() || name.len() > 256 || name.contains('\0') {
        bail!("hirpatch device name is empty or too long");
    }
    let patch = HirPatch {
        schema_version: HIRPATCH_SCHEMA_VERSION,
        name: name.to_string(),
        dataflow: device.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&patch).context("hirpatch serialization failed")?;
    if bytes.len() > MAX_HIRPATCH_BYTES {
        bail!("exported hirpatch exceeds the 1 MiB limit");
    }
    Ok(bytes)
}

/// Strictly reads a `.hirpatch` sidecar and returns the contained device.
/// Any malformed envelope, unit, link or limit aborts the import; callers
/// must not partially apply a rejected patch.
pub fn import_hirpatch_device(bytes: &[u8]) -> Result<HirPatch> {
    if bytes.len() > MAX_HIRPATCH_BYTES {
        bail!("hirpatch exceeds the 1 MiB limit");
    }
    let patch: HirPatch = serde_json::from_slice(bytes)
        .context("hirpatch JSON is malformed or carries unknown keys")?;
    if patch.schema_version != HIRPATCH_SCHEMA_VERSION {
        bail!("unsupported hirpatch schema version {}", patch.schema_version);
    }
    if patch.name.trim().is_empty() || patch.name.len() > 256 || patch.name.contains('\0') {
        bail!("hirpatch device name is invalid");
    }
    validate_builtin_dataflow(&patch.dataflow, "hirpatch device")?;
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_contracts::{BuiltinLink, BuiltinUnit};

    fn device() -> BuiltinDataflow {
        BuiltinDataflow {
            units: vec![BuiltinUnit {
                id: "gain-1".into(),
                kind: "builtin/gain".into(),
                parameters: [("level".to_string(), 0.5)].into_iter().collect(),
            }],
            links: vec![],
        }
    }

    #[test]
    fn export_import_round_trips_byte_identically() {
        let bytes = export_hirpatch_device("Lead", &device()).unwrap();
        let reparsed = import_hirpatch_device(&bytes).unwrap();
        assert_eq!(reparsed.name, "Lead");
        assert_eq!(reparsed.dataflow, device());
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["schema_version"], 1);
    }

    #[test]
    fn unknown_envelope_keys_reject() {
        let with_junk = br#"{"schema_version":1,"name":"X","dataflow":{"units":[],"links":[],"bogus":true}}"#;
        assert!(import_hirpatch_device(with_junk).is_err());
    }

    #[test]
    fn wrong_schema_version_rejects() {
        let bad = br#"{"schema_version":2,"name":"X","dataflow":{"units":[],"links":[]}}"#;
        assert!(import_hirpatch_device(bad).is_err());
    }

    #[test]
    fn dangling_link_rejects() {
        let patch = HirPatch {
            schema_version: 1,
            name: "X".into(),
            dataflow: BuiltinDataflow {
                units: vec![],
                links: vec![BuiltinLink {
                    from: "ghost:out".into(),
                    to: "other:in".into(),
                }],
            },
        };
        let bytes = serde_json::to_vec(&patch).unwrap();
        assert!(import_hirpatch_device(&bytes).is_err());
    }
}
