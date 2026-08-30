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

fn quarantine_file() -> PathBuf {
    if let Some(path) = std::env::var_os("AURA_PLUGIN_QUARANTINE_FILE") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/Aura/quarantined-plugins.txt")
}

fn quarantine_set() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| {
        let entries = std::fs::read_to_string(quarantine_file())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(str::to_owned)
            .collect();
        Mutex::new(entries)
    })
}

fn persist_quarantine(set: &HashSet<String>) -> std::io::Result<()> {
    let path = quarantine_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut values: Vec<&String> = set.iter().collect();
    values.sort();
    let contents = values
        .into_iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
    {
        let mut file = std::fs::File::create(&temporary)?;
        use std::io::Write;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn favorites_file() -> PathBuf {
    if let Some(path) = std::env::var_os("AURA_PLUGIN_FAVORITES_FILE") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/Aura/favorite-plugins.txt")
}

fn favorite_set() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| {
        let entries = std::fs::read_to_string(favorites_file())
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && line.len() <= 256 && !line.contains('\0'))
            .map(str::to_owned)
            .collect();
        Mutex::new(entries)
    })
}

fn persist_favorites(set: &HashSet<String>) -> std::io::Result<()> {
    let path = favorites_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut values: Vec<&String> = set.iter().collect();
    values.sort();
    let contents = values
        .into_iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Persist a stable catalog ID as a user favorite. Binary identity remains
/// separate: replacing a plugin invalidates its cached state/quarantine
/// identity without unexpectedly removing the user's browser favorite.
pub fn set_favorite(id: &str, favorite: bool) -> std::io::Result<bool> {
    if id.trim().is_empty() || id.len() > 256 || id.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid plugin id",
        ));
    }
    let mut set = favorite_set()
        .lock()
        .map_err(|_| std::io::Error::other("favorite state unavailable"))?;
    let changed = if favorite {
        set.insert(id.to_owned())
    } else {
        set.remove(id)
    };
    if changed {
        if let Err(error) = persist_favorites(&set) {
            // Do not leave the process with a state that was never committed
            // to disk; callers may immediately issue another catalog query.
            if favorite {
                set.remove(id);
            } else {
                set.insert(id.to_owned());
            }
            return Err(error);
        }
    }
    Ok(changed)
}

pub fn is_quarantined_hash(binary_hash: &str) -> bool {
    quarantine_set()
        .lock()
        .map(|set| set.contains(binary_hash))
        .unwrap_or(false)
}

/// Check quarantine by concrete plugin path without trusting the display
/// name or extension.  This is deliberately separate from `is_admitted_path`:
/// project hydration may encounter a plugin that is not in the current scan
/// roots, but a known-bad binary must still never be re-launched.
pub fn is_quarantined_path(path: &str) -> bool {
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    let (hash, bytes, _) = binary_identity(&canonical);
    bytes != 0 && !hash.is_empty() && is_quarantined_hash(&hash)
}

pub fn quarantine_plugin(plugin: &InstalledPlugin) -> Result<(), String> {
    if plugin.binary_hash.len() != 64 {
        return Err("plugin has no valid binary identity".into());
    }
    let mut set = quarantine_set()
        .lock()
        .map_err(|_| "quarantine lock poisoned")?;
    set.insert(plugin.binary_hash.clone());
    persist_quarantine(&set).map_err(|error| error.to_string())
}

pub fn clear_plugin_quarantine(plugin: &InstalledPlugin) -> Result<bool, String> {
    let mut set = quarantine_set()
        .lock()
        .map_err(|_| "quarantine lock poisoned")?;
    let removed = set.remove(&plugin.binary_hash);
    if removed {
        persist_quarantine(&set).map_err(|error| error.to_string())?;
    }
    Ok(removed)
}

pub fn quarantine_path(path: &str) -> Result<InstalledPlugin, String> {
    let requested = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let plugin = scan()
        .into_iter()
        .find(|plugin| std::fs::canonicalize(&plugin.path).ok().as_ref() == Some(&requested))
        .ok_or_else(|| "plugin path is not in the installed catalog".to_owned())?;
    quarantine_plugin(&plugin)?;
    Ok(plugin)
}

pub fn clear_quarantine_path(path: &str) -> Result<InstalledPlugin, String> {
    let requested = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let plugin = scan()
        .into_iter()
        .find(|plugin| std::fs::canonicalize(&plugin.path).ok().as_ref() == Some(&requested))
        .ok_or_else(|| "plugin path is not in the installed catalog".to_owned())?;
    clear_plugin_quarantine(&plugin)?;
    Ok(plugin)
}

fn roots() -> Vec<PathBuf> {
    let mut result = vec![
        PathBuf::from("/Library/Audio/Plug-Ins/CLAP"),
        PathBuf::from("/Library/Audio/Plug-Ins/VST3"),
        PathBuf::from("/Library/Audio/Plug-Ins/Components"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        result.extend([
            home.join("Library/Audio/Plug-Ins/CLAP"),
            home.join("Library/Audio/Plug-Ins/VST3"),
            home.join("Library/Audio/Plug-Ins/Components"),
        ]);
    }
    // Allow installations outside the platform defaults (portable plugin
    // folders, per-project tools, CI fixtures, and user-managed bundles)
    // without recompiling Aura. `split_paths` uses the native path-list
    // separator and therefore works on macOS, Linux, and Windows.
    if let Some(extra) = std::env::var_os("AURA_PLUGIN_PATHS") {
        result.extend(std::env::split_paths(&extra));
    }
    result
}

fn format_for(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "clap" => Some("clap"),
        "vst3" => Some("vst3"),
        "component" => Some("au"),
        _ => None,
    }
}

fn canonical(path: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn collect_candidates(root: &std::path::Path, candidates: &mut Vec<PathBuf>) {
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return;
    };
    if !metadata.is_dir() {
        if format_for(root).is_some() {
            candidates.push(root.to_path_buf());
        }
        return;
    }
    // A plugin bundle is itself a directory. Do not descend into it: its
    // implementation files can carry misleading extensions and scanning
    // them would create duplicate catalog entries. Only recurse through
    // ordinary user-created folders.
    if format_for(root).is_some() {
        candidates.push(root.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        collect_candidates(&entry.path(), candidates);
    }
}

fn binary_identity(path: &std::path::Path) -> (String, u64, u64) {
    let mut files = Vec::new();
    collect_files(path, path, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    let mut bytes_total = 0u64;
    let mut newest = 0u64;
    for (relative, file) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        if let Ok(contents) = std::fs::read(&file) {
            bytes_total = bytes_total.saturating_add(contents.len() as u64);
            hasher.update((contents.len() as u64).to_le_bytes());
            hasher.update(&contents);
        }
        if let Ok(modified) = std::fs::metadata(&file).and_then(|meta| meta.modified()) {
            newest = newest.max(
                modified
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
    }
    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (hash, bytes_total, newest)
}

/// Control-plane fingerprint used when a plugin instance is serialized into
/// a project. Missing plugins intentionally return `None`; an absent binary
/// must be represented as unavailable, not as the hash of an empty file.
pub fn binary_hash_for_path(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    if !path.exists() {
        return None;
    }
    let (hash, bytes, _) = binary_identity(path);
    (bytes > 0).then_some(hash)
}

fn collect_files(
    root: &std::path::Path,
    path: &std::path::Path,
    files: &mut Vec<(String, PathBuf)>,
) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        files.push((relative, path.to_path_buf()));
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_files(root, &entry.path(), files);
    }
}

#[derive(Clone, Copy, Serialize)]
struct WorkerCapabilities {
    clap: bool,
    au: bool,
    vst3: bool,
}

fn worker_capabilities() -> WorkerCapabilities {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("AURA_PLUGIN_HOST_BIN") {
        candidates.push(PathBuf::from(configured));
    }
    candidates.extend([
        PathBuf::from("build-tools/aura-plugin-host-worker"),
        PathBuf::from("/usr/local/bin/aura-plugin-host-worker"),
    ]);
    // Match the native host's packaged-app lookup. Without this, the UI can
    // report every installed plugin as unavailable even though the worker is
    // correctly embedded beside the shipped application executable.
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("aura-plugin-host-worker"));
            candidates.push(parent.join("Contents/MacOS/aura-plugin-host-worker"));
            if let Some(contents) = parent.parent() {
                candidates.push(contents.join("MacOS/aura-plugin-host-worker"));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        let Ok(output) = std::process::Command::new(candidate)
            .arg("--capabilities")
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
            return WorkerCapabilities {
                clap: value
                    .get("clap")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                au: value
                    .get("au")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                vst3: value
                    .get("vst3")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            };
        }
    }
    WorkerCapabilities {
        clap: false,
        au: false,
        vst3: false,
    }
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn scan() -> Vec<InstalledPlugin> {
    // Keep one entry per concrete binary/format.  A previous implementation
    // keyed by the display name, which silently discarded e.g. Vital's VST3
    // when its CLAP was also installed.  The browser needs to show every
    // available format so future plugins require no UI special case.
    let capabilities = worker_capabilities();
    let mut found = BTreeMap::<String, InstalledPlugin>::new();
    for root in roots() {
        let mut candidates = Vec::new();
        collect_candidates(&root, &mut candidates);
        for path in candidates {
            let Some(format) = format_for(&path) else {
                continue;
            };
            let Some(path) = canonical(path) else {
                continue;
            };
            let Some(file_name) = path.file_stem().and_then(|v| v.to_str()) else {
                continue;
            };
            let (binary_hash, binary_bytes, modified_unix_seconds) = binary_identity(&path);
            // A bundle that cannot be read is not an installed/admissible
            // plugin.  Publishing it with the hash of an empty traversal
            // makes a permission error look like a valid cache entry and can
            // later fail much deeper in the sandbox admission path.
            if binary_bytes == 0 || binary_hash.is_empty() {
                continue;
            }
            let name = file_name.to_owned();
            let key = normalized(&name);
            if key.is_empty() {
                continue;
            }
            // Keep every concrete format visible.  Whether it can be
            // instantiated is reported by the worker capability probe below.
            let path_key = path.to_string_lossy().to_ascii_lowercase();
            let id = format!("{key}.{format}.{}", content_fingerprint(&path_key));
            let quarantined = is_quarantined_hash(&binary_hash);
            let candidate = InstalledPlugin {
                id: id.clone(),
                name,
                format: format.to_owned(),
                path: path.to_string_lossy().into_owned(),
                installed: true,
                sandboxed: match format {
                    "clap" => capabilities.clap,
                    "vst3" => capabilities.vst3,
                    _ => capabilities.au,
                },
                capability: if quarantined {
                    "quarantined".to_owned()
                } else {
                    match format {
                        "clap" if capabilities.clap => "sandbox-ready".to_owned(),
                        "vst3" if capabilities.vst3 => "vst3-sandbox-ready".to_owned(),
                        "vst3" => "vst3-sdk-required".to_owned(),
                        "au" if capabilities.au => "au-sandbox-ready".to_owned(),
                        "au" => "au-worker-unavailable".to_owned(),
                        _ => "worker-unavailable".to_owned(),
                    }
                },
                binary_hash,
                binary_bytes,
                modified_unix_seconds,
                quarantined,
                tags: vec![format.to_owned()],
                favorite: false,
            };
            found.entry(id).or_insert(candidate);
        }
    }
    found.into_values().collect()
}

fn content_fingerprint(value: &str) -> String {
    // This is only a stable catalog key, not an integrity hash.  Binary
    // identity is supplied by the native admission layer when instantiated.
    // Include the full canonical path so same-named plugins remain selectable.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn resolve(alias: &str) -> Option<InstalledPlugin> {
    let (name_alias, requested_format) = alias
        .split_once('@')
        .or_else(|| alias.split_once(':'))
        .map_or((alias, None), |(name, format)| {
            (name, Some(format.to_ascii_lowercase()))
        });
    let requested = normalized(name_alias);
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
        InstalledPlugin {
            id: "aura.internal.gate".into(),
            name: "Aura Gate".into(),
            format: "internal".into(),
            path: "Aura/Gate".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime noise gate".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "gate".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.saturation".into(),
            name: "Aura Saturation".into(),
            format: "internal".into(),
            path: "Aura/Saturation".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime tube saturation".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["color".into(), "saturation".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.transient".into(),
            name: "Aura Transient Shaper".into(),
            format: "internal".into(),
            path: "Aura/Transient".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime transient shaping".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "transient".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.deesser".into(),
            name: "Aura De-Esser".into(),
            format: "internal".into(),
            path: "Aura/DeEsser".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime sibilance reduction".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["dynamics".into(), "de-esser".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.delay".into(),
            name: "Aura Delay".into(),
            format: "internal".into(),
            path: "Aura/Delay".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime tempo-synced delay".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["delay".into(), "time-based".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.reverb".into(),
            name: "Aura Reverb".into(),
            format: "internal".into(),
            path: "Aura/Reverb".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime algorithmic reverb".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["reverb".into(), "time-based".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.dynamiceq".into(),
            name: "Aura Dynamic EQ".into(),
            format: "internal".into(),
            path: "Aura/DynamicEQ".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime dynamic equalization".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["eq".into(), "dynamic".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.midside".into(),
            name: "Aura Mid/Side".into(),
            format: "internal".into(),
            path: "Aura/MidSide".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime mid-side processing".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["mid-side".into(), "stereo".into(), "internal".into()],
            favorite: false,
        },
        InstalledPlugin {
            id: "aura.internal.width".into(),
            name: "Aura Stereo Width".into(),
            format: "internal".into(),
            path: "Aura/Width".into(),
            installed: true,
            sandboxed: false,
            capability: "native realtime stereo width".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            modified_unix_seconds: 0,
            quarantined: false,
            tags: vec!["stereo".into(), "width".into(), "internal".into()],
            favorite: false,
        },
    ];
    plugins.extend(scan());

    // An explicit catalog id is an unambiguous request and must win even
    // when the same plugin is installed in several formats.  Name-based
    // lookup is intentionally deterministic instead of depending on the
    // BTreeMap/hash suffix ordering used by discovery.
    if let Some(plugin) = plugins
        .iter()
        .find(|plugin| plugin.id == alias || normalized(&plugin.id) == normalized(alias))
    {
        return Some(plugin.clone());
    }
    const FORMAT_PRIORITY: [&str; 3] = ["clap", "vst3", "au"];
    plugins.sort_by_key(|plugin| {
        let format_rank = FORMAT_PRIORITY
            .iter()
            .position(|format| *format == plugin.format)
            .unwrap_or(FORMAT_PRIORITY.len());
        (format_rank, plugin.path.to_ascii_lowercase())
    });
    plugins.into_iter().find(|plugin| {
        normalized(&plugin.name) == requested
            && requested_format
                .as_deref()
                .is_none_or(|format| format == plugin.format)
    })
}

/// Return whether a concrete path belongs to a plugin discovered by the
/// installed-plugin scanner.  This is used by the command permission layer:
/// project-write clients may select system/user plugin bundles, but may not
/// turn an arbitrary executable path into a plugin load request.
pub fn is_admitted_path(path: &str) -> bool {
    let Ok(requested) = std::fs::canonicalize(path) else {
        return false;
    };
    scan().into_iter().any(|plugin| {
        plugin.installed
            && plugin.sandboxed
            && !plugin.quarantined
            && std::fs::canonicalize(&plugin.path).is_ok_and(|candidate| candidate == requested)
    })
}

pub const DEFAULT_PLUGIN_COLLECTION_ID: &str = "default";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginBlockReason {
    Unsupported32Bit,
    ScanCrash,
    LoadFailure(String),
    InvalidBinary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInspection {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: String,
    pub format: String,
    pub version: String,
    pub path: String,
    pub bitness: u8,
    pub supports_f64: bool,
    pub asio_guard: bool,
    pub sidechain_inputs: u16,
    pub latency_samples: u32,
    pub hidden: bool,
    pub block_reason: Option<PluginBlockReason>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManagerRegistry {
    pub plugins: Vec<PluginInspection>,
}

impl PluginManagerRegistry {
    /// Commit one isolated scan result. A failed or unsupported binary remains
    /// visible on the blocklist instead of disappearing from diagnostics.
    pub fn record_scan(&mut self, mut plugin: PluginInspection, scan_succeeded: bool) -> bool {
        plugin.id = plugin.id.trim().to_owned();
        plugin.name = plugin.name.trim().to_owned();
        if plugin.bitness == 32 {
            plugin.block_reason = Some(PluginBlockReason::Unsupported32Bit);
        } else if !scan_succeeded && plugin.block_reason.is_none() {
            plugin.block_reason = Some(PluginBlockReason::ScanCrash);
        } else if scan_succeeded {
            plugin.block_reason = None;
        }
        if !plugin.validate() {
            return false;
        }
        if let Some(existing) = self.plugins.iter_mut().find(|item| item.id == plugin.id) {
            *existing = plugin;
        } else if self.plugins.len() < 65_536 {
            self.plugins.push(plugin);
        } else {
            return false;
        }
        self.plugins.sort_by_key(|item| item.id.clone());
        true
    }

    pub fn set_hidden(&mut self, id: &str, hidden: bool) -> bool {
        let Some(plugin) = self.plugins.iter_mut().find(|plugin| plugin.id == id) else {
            return false;
        };
        plugin.hidden = hidden;
        true
    }

    /// Reactivation is accepted only after an isolated rescan succeeds.
    /// Cubase-compatible 32-bit entries can never be reactivated.
    pub fn reactivate(&mut self, id: &str, rescan_succeeded: bool) -> bool {
        let Some(plugin) = self.plugins.iter_mut().find(|plugin| plugin.id == id) else {
            return false;
        };
        if plugin.bitness != 64 || !rescan_succeeded {
            return false;
        }
        plugin.block_reason = None;
        true
    }

    pub fn available(
        &self,
        used_in_project: Option<&BTreeSet<String>>,
        require_f64: bool,
    ) -> Vec<&PluginInspection> {
        let mut result = self
            .plugins
            .iter()
            .filter(|plugin| {
                !plugin.hidden
                    && plugin.block_reason.is_none()
                    && (!require_f64 || plugin.supports_f64)
                    && used_in_project.is_none_or(|ids| ids.contains(&plugin.id))
            })
            .collect::<Vec<_>>();
        result.sort_by_key(|plugin| {
            (
                plugin.vendor.to_ascii_lowercase(),
                plugin.name.to_ascii_lowercase(),
            )
        });
        result
    }

    pub fn blocklist(&self) -> Vec<&PluginInspection> {
        let mut result = self
            .plugins
            .iter()
            .filter(|plugin| plugin.block_reason.is_some())
            .collect::<Vec<_>>();
        result.sort_by_key(|plugin| plugin.name.to_ascii_lowercase());
        result
    }

    pub fn diagnostic_report(&self, system: &str) -> Result<String, String> {
        if !self.validate()
            || system.trim().is_empty()
            || system.len() > 1024
            || system.contains('\0')
        {
            return Err("invalid plug-in report data".into());
        }
        let mut report = format!(
            "Aura Plug-in Report\nSystem: {}\nPlug-ins: {}\n",
            system.trim(),
            self.plugins.len()
        );
        for plugin in &self.plugins {
            report.push_str(&format!(
                "{} | {} | {} | {}-bit | latency={} | sidechains={} | hidden={} | status={:?}\n",
                plugin.name,
                plugin.vendor,
                plugin.format,
                plugin.bitness,
                plugin.latency_samples,
                plugin.sidechain_inputs,
                plugin.hidden,
                plugin.block_reason
            ));
        }
        Ok(report)
    }

    pub fn validate(&self) -> bool {
        self.plugins.len() <= 65_536
            && self.plugins.iter().all(PluginInspection::validate)
            && self.plugins.windows(2).all(|pair| pair[0].id < pair[1].id)
    }
}

impl PluginInspection {
    fn validate(&self) -> bool {
        let text = |value: &str, max: usize| {
            !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
        };
        text(&self.id, 256)
            && text(&self.name, 256)
            && text(&self.vendor, 256)
            && text(&self.category, 128)
            && text(&self.format, 32)
            && text(&self.version, 128)
            && text(&self.path, 4096)
            && matches!(self.bitness, 32 | 64)
            && self
                .block_reason
                .as_ref()
                .is_none_or(|reason| match reason {
                    PluginBlockReason::LoadFailure(message) => text(message, 1024),
                    _ => true,
                })
            && (self.bitness != 32
                || self.block_reason == Some(PluginBlockReason::Unsupported32Bit))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollectionEntry {
    pub plugin_id: String,
    pub folder: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollection {
    pub id: String,
    pub name: String,
    pub entries: Vec<PluginCollectionEntry>,
    pub immutable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollectionManager {
    pub collections: Vec<PluginCollection>,
    pub active_id: String,
    pub available_plugin_ids: BTreeSet<String>,
    next_id: u64,
}

impl PluginCollectionManager {
    pub fn new<I, S>(available_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let available_plugin_ids: BTreeSet<_> = available_ids
            .into_iter()
            .map(Into::into)
            .filter(|id| valid_collection_token(id))
            .collect();
        let entries = available_plugin_ids
            .iter()
            .map(|plugin_id| PluginCollectionEntry {
                plugin_id: plugin_id.clone(),
                folder: Vec::new(),
            })
            .collect();
        Self {
            collections: vec![PluginCollection {
                id: DEFAULT_PLUGIN_COLLECTION_ID.into(),
                name: "Default".into(),
                entries,
                immutable: true,
            }],
            active_id: DEFAULT_PLUGIN_COLLECTION_ID.into(),
            available_plugin_ids,
            next_id: 1,
        }
    }

    /// A full rescan recreates Default while preserving unavailable references
    /// in user collections so projects can recover when a plug-in is restored.
    pub fn rescan<I, S>(&mut self, available_ids: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.available_plugin_ids = available_ids
            .into_iter()
            .map(Into::into)
            .filter(|id| valid_collection_token(id))
            .collect();
        if let Some(default) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == DEFAULT_PLUGIN_COLLECTION_ID)
        {
            default.entries = self
                .available_plugin_ids
                .iter()
                .map(|plugin_id| PluginCollectionEntry {
                    plugin_id: plugin_id.clone(),
                    folder: Vec::new(),
                })
                .collect();
            default.name = "Default".into();
            default.immutable = true;
        }
    }

    pub fn create(&mut self, name: &str, include_all: bool) -> Option<String> {
        let name = name.trim();
        if !valid_collection_name(name)
            || self.collections.len() >= 256
            || self
                .collections
                .iter()
                .any(|collection| collection.name.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let id = format!("user-{}", self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        let entries = if include_all {
            self.available_plugin_ids
                .iter()
                .map(|plugin_id| PluginCollectionEntry {
                    plugin_id: plugin_id.clone(),
                    folder: Vec::new(),
                })
                .collect()
        } else {
            Vec::new()
        };
        self.collections.push(PluginCollection {
            id: id.clone(),
            name: name.into(),
            entries,
            immutable: false,
        });
        Some(id)
    }

    pub fn copy_collection(&mut self, source_id: &str, name: &str) -> Option<String> {
        let entries = self
            .collections
            .iter()
            .find(|collection| collection.id == source_id)?
            .entries
            .clone();
        let id = self.create(name, false)?;
        self.collections
            .iter_mut()
            .find(|collection| collection.id == id)?
            .entries = entries;
        Some(id)
    }

    pub fn activate(&mut self, id: &str) -> bool {
        if !self
            .collections
            .iter()
            .any(|collection| collection.id == id)
        {
            return false;
        }
        self.active_id = id.into();
        true
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if !valid_collection_name(name)
            || self
                .collections
                .iter()
                .any(|collection| collection.id != id && collection.name.eq_ignore_ascii_case(name))
        {
            return false;
        }
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id && !collection.immutable)
        else {
            return false;
        };
        collection.name = name.into();
        true
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let Some(index) = self
            .collections
            .iter()
            .position(|collection| collection.id == id && !collection.immutable)
        else {
            return false;
        };
        self.collections.remove(index);
        if self.active_id == id {
            self.active_id = DEFAULT_PLUGIN_COLLECTION_ID.into();
        }
        true
    }

    pub fn add_plugin(&mut self, collection_id: &str, plugin_id: &str, folder: &[String]) -> bool {
        if !self.available_plugin_ids.contains(plugin_id) || !valid_folder(folder) {
            return false;
        }
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == collection_id && !collection.immutable)
        else {
            return false;
        };
        if collection.entries.len() >= 65_536
            || collection
                .entries
                .iter()
                .any(|entry| entry.plugin_id == plugin_id)
        {
            return false;
        }
        collection.entries.push(PluginCollectionEntry {
            plugin_id: plugin_id.into(),
            folder: folder.to_vec(),
        });
        true
    }

    pub fn remove_plugin(&mut self, collection_id: &str, plugin_id: &str) -> bool {
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == collection_id && !collection.immutable)
        else {
            return false;
        };
        let before = collection.entries.len();
        collection
            .entries
            .retain(|entry| entry.plugin_id != plugin_id);
        before != collection.entries.len()
    }

    pub fn remove_unavailable_from_user_collections(&mut self) -> usize {
        let mut removed = 0;
        for collection in self
            .collections
            .iter_mut()
            .filter(|collection| !collection.immutable)
        {
            let before = collection.entries.len();
            collection
                .entries
                .retain(|entry| self.available_plugin_ids.contains(&entry.plugin_id));
            removed += before - collection.entries.len();
        }
        removed
    }

    pub fn active_entries(&self, include_unavailable: bool) -> Vec<&PluginCollectionEntry> {
        let Some(collection) = self
            .collections
            .iter()
            .find(|collection| collection.id == self.active_id)
        else {
            return Vec::new();
        };
        collection
            .entries
            .iter()
            .filter(|entry| {
                include_unavailable || self.available_plugin_ids.contains(&entry.plugin_id)
            })
            .collect()
    }

    pub fn validate(&self) -> bool {
        self.next_id > 0
            && self.collections.len() <= 256
            && self
                .collections
                .iter()
                .any(|collection| collection.id == self.active_id)
            && self
                .collections
                .iter()
                .filter(|collection| {
                    collection.id == DEFAULT_PLUGIN_COLLECTION_ID
                        && collection.immutable
                        && collection.name == "Default"
                })
                .count()
                == 1
            && self
                .collections
                .iter()
                .enumerate()
                .all(|(index, collection)| {
                    valid_collection_token(&collection.id)
                        && valid_collection_name(&collection.name)
                        && collection.entries.len() <= 65_536
                        && self.collections[..index].iter().all(|previous| {
                            previous.id != collection.id
                                && !previous.name.eq_ignore_ascii_case(&collection.name)
                        })
                        && collection
                            .entries
                            .iter()
                            .enumerate()
                            .all(|(entry_index, entry)| {
                                valid_collection_token(&entry.plugin_id)
                                    && valid_folder(&entry.folder)
                                    && collection.entries[..entry_index]
                                        .iter()
                                        .all(|previous| previous.plugin_id != entry.plugin_id)
                            })
                })
            && self
                .collections
                .iter()
                .find(|collection| collection.id == DEFAULT_PLUGIN_COLLECTION_ID)
                .is_some_and(|default| {
                    default
                        .entries
                        .iter()
                        .map(|entry| &entry.plugin_id)
                        .eq(self.available_plugin_ids.iter())
                })
    }
}

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

#[cfg(test)]
mod tests {
    use super::{
        binary_identity, content_fingerprint, instrument_profiles, normalized, recommended_plugins,
    };

    #[test]
    fn aliases_ignore_spaces_and_punctuation() {
        assert_eq!(normalized("Surge XT"), "surgext");
        assert_eq!(normalized("Vital.vst3"), "vitalvst3");
    }

    #[test]
    fn catalog_ids_include_format_and_binary_identity() {
        let a = content_fingerprint("/plugins/vital.clap");
        let b = content_fingerprint("/plugins/vital.vst3");
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn third_party_instrument_profiles_expose_midi_automation_and_state_contract() {
        let profiles = instrument_profiles();
        assert_eq!(profiles.len(), 2);
        for profile in profiles {
            assert!(profile.midi_input);
            assert_eq!(profile.midi_channels, 16);
            assert!(profile.parameter_automation);
            assert!(profile.state_save_restore);
            assert!(profile.formats.contains(&"clap".to_owned()));
        }
    }

    #[test]
    fn recommendations_are_stable_and_point_to_catalog_entries() {
        let recommendations = recommended_plugins();
        let entries = recommendations.as_array().unwrap();
        assert!(entries.len() >= 5);
        assert!(entries.iter().all(|entry| entry["id"].as_str().is_some()
            && entry["use"].as_str().is_some()
            && entry["reason"].as_str().is_some()));
    }

    #[test]
    fn binary_identity_changes_when_plugin_contents_change() {
        let root =
            std::env::temp_dir().join(format!("aura-plugin-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
        let binary = root.join("Contents/MacOS/plugin");
        std::fs::write(&binary, b"version-a").unwrap();
        let first = binary_identity(&root);
        std::fs::write(&binary, b"version-b-updated").unwrap();
        let second = binary_identity(&root);
        assert_ne!(first.0, second.0);
        assert_ne!(first.1, second.1);
        assert!(first.0.len() == 64 && second.0.len() == 64);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod collection_tests {
    use super::{PluginCollectionManager, DEFAULT_PLUGIN_COLLECTION_ID};

    #[test]
    fn default_collection_is_immutable_and_rebuilt_on_rescan() {
        let mut manager = PluginCollectionManager::new(["plug.b", "plug.a"]);
        assert!(manager.validate());
        assert_eq!(
            manager
                .active_entries(false)
                .iter()
                .map(|entry| entry.plugin_id.as_str())
                .collect::<Vec<_>>(),
            vec!["plug.a", "plug.b"]
        );
        assert!(!manager.rename(DEFAULT_PLUGIN_COLLECTION_ID, "Other"));
        assert!(!manager.delete(DEFAULT_PLUGIN_COLLECTION_ID));
        manager.rescan(["plug.c", "plug.a"]);
        assert_eq!(
            manager
                .active_entries(false)
                .iter()
                .map(|entry| entry.plugin_id.as_str())
                .collect::<Vec<_>>(),
            vec!["plug.a", "plug.c"]
        );
        assert!(manager.validate());
    }

    #[test]
    fn user_collection_supports_folders_copy_activation_and_unavailable_cleanup() {
        let mut manager = PluginCollectionManager::new(["synth", "eq", "compressor"]);
        let favorites = manager.create("Favorites", false).unwrap();
        assert!(manager.add_plugin(&favorites, "synth", &["Instruments".into()]));
        assert!(manager.add_plugin(&favorites, "eq", &["Mix".into(), "EQ".into()]));
        assert!(!manager.add_plugin(&favorites, "missing", &[]));
        assert!(manager.activate(&favorites));
        assert_eq!(manager.active_entries(false).len(), 2);

        let copied = manager.copy_collection(&favorites, "Studio A").unwrap();
        assert!(manager.activate(&copied));
        manager.rescan(["synth", "compressor"]);
        assert_eq!(manager.active_entries(true).len(), 2);
        assert_eq!(manager.active_entries(false).len(), 1);
        assert_eq!(manager.remove_unavailable_from_user_collections(), 2);
        assert_eq!(manager.active_entries(true).len(), 1);
        assert!(manager.validate());
    }

    #[test]
    fn deleting_active_user_collection_falls_back_to_default() {
        let mut manager = PluginCollectionManager::new(["limiter"]);
        let id = manager.create("Mastering", true).unwrap();
        assert!(manager.activate(&id));
        assert!(manager.delete(&id));
        assert_eq!(manager.active_id, DEFAULT_PLUGIN_COLLECTION_ID);
        assert!(manager.validate());
    }
}

#[cfg(test)]
mod manager_registry_tests {
    use super::*;

    fn plugin(id: &str, bitness: u8) -> PluginInspection {
        PluginInspection {
            id: id.into(),
            name: id.into(),
            vendor: "Vendor".into(),
            category: "Fx".into(),
            format: "VST3".into(),
            version: "1.0".into(),
            path: format!("/plugins/{id}.vst3"),
            bitness,
            supports_f64: true,
            asio_guard: true,
            sidechain_inputs: 1,
            latency_samples: 64,
            hidden: false,
            block_reason: None,
        }
    }

    #[test]
    fn failed_scan_enters_blocklist_and_successful_rescan_reactivates_64_bit() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("unstable", 64), false));
        assert_eq!(registry.blocklist().len(), 1);
        assert!(!registry.reactivate("unstable", false));
        assert!(registry.reactivate("unstable", true));
        assert_eq!(registry.available(None, false).len(), 1);
        assert!(registry.validate());
    }

    #[test]
    fn unsupported_32_bit_plugin_cannot_be_reactivated() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("legacy", 32), true));
        assert_eq!(
            registry.plugins[0].block_reason,
            Some(PluginBlockReason::Unsupported32Bit)
        );
        assert!(!registry.reactivate("legacy", true));
    }

    #[test]
    fn hidden_and_project_filters_affect_browser_but_not_report() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("eq", 64), true));
        assert!(registry.record_scan(plugin("compressor", 64), true));
        assert!(registry.set_hidden("eq", true));
        let used = BTreeSet::from(["compressor".to_owned()]);
        assert_eq!(registry.available(Some(&used), true)[0].id, "compressor");
        let report = registry.diagnostic_report("macOS test host").unwrap();
        assert!(report.contains("eq | Vendor"));
        assert!(report.contains("compressor | Vendor"));
    }
}
