//! Installed third-party instruments exposed through Aura's plugin rack.
//!
//! The catalog is deliberately control-plane only: discovery and alias
//! resolution happen off the audio thread, while instantiation is delegated
//! to the existing sandbox host.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub format: String,
    pub path: String,
    pub installed: bool,
    pub sandboxed: bool,
    pub capability: String,
    /// Content identity used to invalidate stale project/plugin caches.
    pub binary_hash: String,
    pub binary_bytes: u64,
    pub modified_unix_seconds: u64,
    /// Persisted by binary hash, so replacing/upgrading a plugin naturally
    /// creates a new admission identity instead of inheriting old quarantine.
    pub quarantined: bool,
    /// Stable browser metadata used by CLI/UI filtering.
    pub tags: Vec<String>,
    pub favorite: bool,
}

/// Host-facing contract for the two common third-party synths.  This is kept
/// separate from `InstalledPlugin` so discovery remains format-agnostic while
/// the instrument lane can still expose a stable MIDI/state interface.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstrumentProfile {
    pub id: String,
    pub aliases: Vec<String>,
    pub formats: Vec<String>,
    pub midi_input: bool,
    pub midi_channels: u8,
    pub parameter_automation: bool,
    pub state_save_restore: bool,
    pub sidechain_input: bool,
    /// This is a capability template, not proof that a binary is installed
    /// or that the current host successfully opened it.
    pub verification_status: String,
}

/// Returns the complete identity for state/UI caches. A plugin upgrade or a
/// schema bump must produce a different key even when the host has not yet
/// rescanned the binary contents.
pub fn plugin_cache_identity(
    plugin_id: &str,
    plugin_version: &str,
    binary_hash: &str,
    state_schema_version: u32,
    gui_state_schema_version: u32,
) -> Option<String> {
    if plugin_id.trim().is_empty()
        || plugin_version.trim().is_empty()
        || binary_hash.len() != 64
        || !binary_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || state_schema_version == 0
        || gui_state_schema_version == 0
        || plugin_id.contains('\0')
        || plugin_version.contains('\0')
    {
        return None;
    }
    Some(format!(
        "{plugin_id}@{plugin_version}#bin-{binary_hash}:state-{state_schema_version}:gui-{gui_state_schema_version}"
    ))
}

pub fn instrument_profiles() -> Vec<InstrumentProfile> {
    vec![
        InstrumentProfile {
            id: "vital".into(),
            aliases: vec!["Vital".into(), "Vital Synth".into()],
            formats: vec!["clap".into(), "vst3".into(), "au".into()],
            midi_input: true,
            midi_channels: 16,
            parameter_automation: true,
            state_save_restore: true,
            sidechain_input: true,
            verification_status: "template_unverified".into(),
        },
        InstrumentProfile {
            id: "surge_xt".into(),
            aliases: vec!["Surge XT".into(), "Surge".into()],
            formats: vec!["clap".into(), "vst3".into(), "au".into()],
            midi_input: true,
            midi_channels: 16,
            parameter_automation: true,
            state_save_restore: true,
            sidechain_input: true,
            verification_status: "template_unverified".into(),
        },
    ]
}

pub fn recommended_plugins() -> serde_json::Value {
    serde_json::json!([
        {"id":"aura.internal.compressor","use":"vocal","reason":"smooth level control"},
        {"id":"aura.internal.deesser","use":"vocal","reason":"reduce sibilance"},
        {"id":"aura.internal.reverb","use":"space","reason":"add room or plate-like ambience"},
        {"id":"aura.internal.delay","use":"space","reason":"tempo-synced echoes"},
        {"id":"aura.internal.limiter","use":"master","reason":"protect the output from clipping"}
    ])
}

#[cfg(test)]
mod cache_identity_tests {
    use super::plugin_cache_identity;

    #[test]
    fn plugin_version_and_schema_changes_invalidate_identity() {
        let hash = "a".repeat(64);
        let v1 = plugin_cache_identity("vital", "1.5.5", &hash, 1, 1).unwrap();
        let v2 = plugin_cache_identity("vital", "1.5.6", &hash, 1, 1).unwrap();
        let gui = plugin_cache_identity("vital", "1.5.5", &hash, 1, 2).unwrap();
        assert_ne!(v1, v2);
        assert_ne!(v1, gui);
        assert!(plugin_cache_identity("vital", "1", "bad", 1, 1).is_none());
    }
}

include!("plugin_catalog_scan.rs");

include!("plugin_catalog_collections.rs");

fn valid_collection_token(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 256
        && !value.contains('\0')
        && !value.contains('/')
        && !value.contains('\\')
}

fn valid_collection_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.contains('\0')
}

fn valid_folder(folder: &[String]) -> bool {
    folder.len() <= 16
        && folder
            .iter()
            .all(|part| valid_collection_name(part) && part != "." && part != "..")
}

pub fn json() -> String {
    let capabilities = worker_capabilities();
    let favorites = favorite_set()
        .lock()
        .map(|set| set.clone())
        .unwrap_or_default();
    let mut plugins = vec![
        InstalledPlugin {
            id: "aura.internal.limiter".into(),
            name: "Aura Limiter".into(),
            format: "internal".into(),
            path: "Aura/Limiter".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamics".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.compressor".into(),
            name: "Aura Compressor".into(),
            format: "internal".into(),
            path: "Aura/Compressor".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamics · assistant-ready".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "internal".into()],
            favorite: false,
        },
    ];
    plugins.extend(scan());
    for plugin in &mut plugins {
        plugin.favorite = favorites.contains(&plugin.id);
    }
    serde_json::json!({
        "ok": true,
        "plugins": plugins,
        "instrument_profiles": instrument_profiles(),
        "recommended": recommended_plugins(),
        "worker_capabilities": capabilities,
        "openutau": crate::openutau::status(),
    })
    .to_string()
}

include!("plugin_catalog_tests.rs");
