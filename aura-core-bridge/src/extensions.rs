//! Safe extension discovery for the Aura plug-in ecosystem.
//!
//! Extensions are manifest-only at this boundary.  Discovery never executes
//! code, follows symlinks, or grants filesystem access; a later host can add a
//! sandboxed runtime behind this stable contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "default_execution")]
    pub execution: String,
    /// Declared scripting runtime. The host never executes an undeclared
    /// language, keeping Lua/Python/JavaScript support explicit and auditable.
    #[serde(default = "default_runtime_language")]
    pub runtime_language: String,
    /// Relative executable used only by trusted extensions with the explicit
    /// `process_spawn` permission. Sandboxed extensions never execute code.
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub commands: Vec<ExtensionCommand>,
    /// Declarative UI contributions.  The host can render these without
    /// loading extension code, which keeps discovery safe while allowing an
    /// extension to add a panel or command-palette entry immediately.
    #[serde(default)]
    pub contributions: ExtensionContributions,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtensionContributions {
    #[serde(default)]
    pub panels: Vec<ExtensionPanel>,
    #[serde(default)]
    pub menus: Vec<ExtensionMenuItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtensionPanel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtensionMenuItem {
    pub id: String,
    pub title: String,
    pub command_id: String,
    #[serde(default)]
    pub menu: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExtensionCommand {
    pub id: String,
    pub title: String,
    #[serde(default = "default_command_kind")]
    pub kind: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: serde_json::Value,
}

fn default_command_kind() -> String {
    "read_only".into()
}

fn default_execution() -> String {
    "sandboxed".into()
}

fn default_runtime_language() -> String { "none".into() }

fn default_input_schema() -> serde_json::Value {
    serde_json::json!({"type": "object", "additionalProperties": false})
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredExtension {
    pub manifest: ExtensionManifest,
    pub root: PathBuf,
    /// Project-local activation state.  Discovery remains manifest-only, but
    /// disabled extensions must not silently reappear in command/UI registries.
    pub enabled: bool,
}

/// Built-in marketplace index metadata. Network fetching is intentionally
/// outside the audio process; clients can use these records to present a
/// compatible, auditable install target before supplying a local package.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MarketplaceListing {
    pub extension_id: String,
    pub version: String,
    pub channel: String,
    pub package_format: String,
    pub signature_required: bool,
    pub permissions_review_required: bool,
}

pub fn marketplace_catalog() -> Vec<MarketplaceListing> {
    vec![MarketplaceListing {
        extension_id: "aura.example.effects".into(),
        version: "0.1.0".into(),
        channel: "stable".into(),
        package_format: "manifest-directory".into(),
        signature_required: true,
        permissions_review_required: true,
    }]
}

/// Search the local marketplace index without network access or installation.
/// Empty filters match all listings; channel matching is case-insensitive.
pub fn marketplace_search(query: &str, channel: Option<&str>) -> Vec<MarketplaceListing> {
    let needle = query.trim().to_ascii_lowercase();
    let requested_channel = channel.map(|value| value.trim().to_ascii_lowercase());
    marketplace_catalog().into_iter().filter(|listing| {
        let query_matches = needle.is_empty()
            || listing.extension_id.to_ascii_lowercase().contains(&needle)
            || listing.version.to_ascii_lowercase().contains(&needle);
        let channel_matches = requested_channel.as_ref()
            .is_none_or(|value| value == &listing.channel.to_ascii_lowercase());
        query_matches && channel_matches
    }).collect()
}

fn activation_path(root: &Path) -> PathBuf {
    root.join(".aura").join("extensions.json")
}

fn activation_state(root: &Path) -> std::collections::HashMap<String, bool> {
    let path = activation_path(root);
    let Ok(metadata) = fs::symlink_metadata(&path) else { return std::collections::HashMap::new() };
    if metadata.file_type().is_symlink() || !metadata.is_file() { return std::collections::HashMap::new() }
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Persist activation state without executing or loading extension code.
/// The file is project-local and written with a unique temporary name so two
/// independent UI/CLI clients cannot accidentally truncate one another's
/// settings.
pub fn set_enabled(root: impl AsRef<Path>, extension_id: &str, enabled: bool) -> Result<(), String> {
    if !valid_token(extension_id) { return Err("extension id is invalid".into()); }
    let root = root.as_ref();
    let (discovered, _) = discover(root);
    if !discovered.iter().any(|extension| extension.manifest.id == extension_id) {
        return Err("extension is not installed under this root".into());
    }
    let mut state = activation_state(root);
    state.insert(extension_id.to_owned(), enabled);
    fs::create_dir_all(root.join(".aura")).map_err(|e| e.to_string())?;
    let path = activation_path(root);
    let temp = path.with_extension(format!("tmp-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
    {
        use std::io::Write;
        let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&temp, &path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::File::open(root.join(".aura")).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Install a manifest-based extension directory into a project-local root.
/// Only regular files are copied; symlinks and path traversal are rejected.
/// The destination is published with a final rename so discovery never sees a
/// partially copied extension.
pub fn install_from_directory(source: impl AsRef<Path>, root: impl AsRef<Path>) -> Result<String, String> {
    let source = source.as_ref();
    let root = root.as_ref();
    let source_meta = fs::symlink_metadata(source).map_err(|error| format!("invalid extension source: {error}"))?;
    if !source_meta.is_dir() || source_meta.file_type().is_symlink() {
        return Err("extension source must be a regular directory".into());
    }
    let manifest_path = source.join("manifest.json");
    let manifest_meta = fs::symlink_metadata(&manifest_path).map_err(|error| format!("manifest is missing: {error}"))?;
    if !manifest_meta.is_file() || manifest_meta.file_type().is_symlink() {
        return Err("extension manifest must be a regular file".into());
    }
    let manifest: ExtensionManifest = serde_json::from_str(&fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid extension manifest: {error}"))?;
    validate_manifest(&manifest)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let destination = root.join(&manifest.id);
    if destination.exists() {
        return Err("an extension with this id is already installed".into());
    }
    let temporary = root.join(format!(".{}.install-{}-{}", manifest.id, std::process::id(), uuid::Uuid::new_v4()));
    let mut budget = CopyBudget { files: 0, bytes: 0 };
    if let Err(error) = copy_extension_tree(source, &temporary, &mut budget) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(format!("cannot publish extension: {error}"));
    }
    #[cfg(unix)]
    fs::File::open(root).and_then(|file| file.sync_all()).map_err(|error| error.to_string())?;
    Ok(manifest.id)
}

struct CopyBudget { files: u64, bytes: u64 }

fn copy_extension_tree(source: &Path, destination: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    const MAX_FILES: u64 = 4096;
    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            return Err(format!("extension contains a symlink: {}", entry.path().display()));
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_extension_tree(&entry.path(), &target, budget)?;
        } else if file_type.is_file() {
            budget.files = budget.files.saturating_add(1);
            let size = entry.metadata().map_err(|error| error.to_string())?.len();
            budget.bytes = budget.bytes.saturating_add(size);
            if budget.files > MAX_FILES || budget.bytes > MAX_BYTES {
                return Err("extension exceeds the 4096-file or 256 MiB install limit".into());
            }
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        } else {
            return Err(format!("extension contains unsupported file: {}", entry.path().display()));
        }
    }
    Ok(())
}

/// Return the effective activation state for an installed extension.  The
/// distinction between `None` (not installed) and `Some(false)` (installed
/// but disabled) is needed by transaction rollback.
pub fn enabled_state(root: impl AsRef<Path>, extension_id: &str) -> Option<bool> {
    discover(root)
        .0
        .into_iter()
        .find(|extension| extension.manifest.id == extension_id)
        .map(|extension| extension.enabled)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExtensionCommandRegistration {
    pub extension_id: String,
    pub command_id: String,
    pub qualified_id: String,
    pub title: String,
    pub kind: String,
    pub execution: String,
    pub permissions: Vec<String>,
    pub input_schema: serde_json::Value,
    pub root: PathBuf,
    pub entrypoint: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExtensionUiRegistration {
    pub extension_id: String,
    pub kind: String,
    pub id: String,
    pub title: String,
    pub location: String,
    pub command_id: Option<String>,
    pub icon: Option<String>,
}

struct ExtensionRunAudit {
    path: PathBuf,
    extension_id: String,
    command_id: String,
    run_id: String,
    payload_hash: String,
    started_unix_seconds: u64,
    status: String,
}

impl ExtensionRunAudit {
    fn new(root: &Path, extension_id: &str, command_id: &str, run_id: &str, payload: &serde_json::Value) -> Self {
        Self {
            path: root.join(".aura").join("extension-runs.jsonl"),
            extension_id: extension_id.to_owned(), command_id: command_id.to_owned(), run_id: run_id.to_owned(),
            payload_hash: hash_json(payload),
            started_unix_seconds: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |value| value.as_secs()),
            status: "aborted".to_owned(),
        }
    }

    fn finish(&mut self, status: &str) { self.status = status.to_owned(); }
}

impl Drop for ExtensionRunAudit {
    fn drop(&mut self) {
        let Ok(parent) = self.path.parent().map(Path::to_path_buf).ok_or(()) else { return; };
        if fs::create_dir_all(parent).is_err() { return; }
        let record = serde_json::json!({
            "run_id": self.run_id, "extension_id": self.extension_id, "command_id": self.command_id,
            "payload_sha256": self.payload_hash, "started_unix_seconds": self.started_unix_seconds,
            "status": self.status,
        });
        let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&self.path) else { return; };
        let _ = writeln!(file, "{}", record);
        let _ = file.sync_all();
    }
}

fn hash_json(value: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Read only the bounded recent audit trail. Invalid or oversized records are
/// ignored so a damaged log can never prevent extension discovery.
pub fn recent_run_history(root: impl AsRef<Path>) -> Vec<serde_json::Value> {
    let path = root.as_ref().join(".aura").join("extension-runs.jsonl");
    let Ok(contents) = fs::read_to_string(path) else { return Vec::new(); };
    contents.lines().rev().take(64).filter_map(|line| {
        let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
        value.is_object().then_some(value)
    }).collect()
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn validate_manifest(manifest: &ExtensionManifest) -> Result<(), String> {
    if !valid_token(&manifest.id) {
        return Err("extension id is invalid".into());
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 256 {
        return Err("extension name is invalid".into());
    }
    if !matches!(manifest.execution.as_str(), "sandboxed" | "trusted") {
        return Err("extension execution mode is invalid".into());
    }
    if !matches!(manifest.runtime_language.as_str(), "none" | "lua" | "python" | "javascript") {
        return Err("extension runtime language is unsupported".into());
    }
    if manifest.runtime_language != "none" && manifest.entrypoint.is_none() {
        return Err("scripted extension requires an entrypoint".into());
    }
    if let Some(entrypoint) = &manifest.entrypoint {
        let path = Path::new(entrypoint);
        if manifest.execution != "trusted"
            || !manifest.permissions.iter().any(|permission| permission == "process_spawn")
            || entrypoint.len() > 512
            || path.is_absolute()
            || path.components().any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))
        {
            return Err("extension entrypoint requires trusted process_spawn permission and a safe relative path".into());
        }
    }
    if !valid_token(&manifest.version) {
        return Err("extension version is invalid".into());
    }
    if manifest.permissions.iter().any(|permission| {
        !matches!(
            permission.as_str(),
            "project_read"
                | "project_write"
                | "audio_process"
                | "filesystem_external"
                | "network"
                | "process_spawn"
        )
    }) {
        return Err("extension requests an unsupported permission".into());
    }
    if manifest.execution == "sandboxed"
        && manifest
            .permissions
            .iter()
            .any(|permission| matches!(permission.as_str(), "filesystem_external" | "network" | "process_spawn"))
    {
        return Err("sandboxed extensions cannot request external side effects".into());
    }
    if manifest.commands.iter().any(|command| {
        !valid_token(&command.id)
            || command.title.trim().is_empty()
            || !matches!(command.kind.as_str(), "read_only" | "reversible" | "external_side_effect")
            || !command.input_schema.is_object()
    }) {
        return Err("extension declares an invalid command".into());
    }
    if manifest.contributions.panels.len() > 64 || manifest.contributions.menus.len() > 256 {
        return Err("extension declares too many UI contributions".into());
    }
    if manifest.contributions.panels.iter().any(|panel| {
        !valid_token(&panel.id)
            || panel.title.trim().is_empty()
            || panel.title.len() > 256
            || (!panel.location.is_empty()
                && !matches!(panel.location.as_str(), "left" | "right" | "bottom" | "floating"))
    }) {
        return Err("extension declares an invalid panel contribution".into());
    }
    if manifest.contributions.menus.iter().any(|item| {
        !valid_token(&item.id)
            || item.title.trim().is_empty()
            || item.title.len() > 256
            || !valid_token(&item.command_id)
            || (!item.menu.is_empty()
                && !matches!(item.menu.as_str(), "file" | "edit" | "view" | "track" | "plugin" | "help"))
    }) {
        return Err("extension declares an invalid menu contribution".into());
    }
    if manifest.execution == "sandboxed"
        && manifest.commands.iter().any(|command| command.kind == "external_side_effect")
    {
        return Err("sandboxed extensions cannot declare external-side-effect commands".into());
    }
    Ok(())
}

/// Discover manifest-only extensions beneath an explicit project-owned root.
/// Invalid entries are ignored and returned in `errors`; valid entries are
/// sorted by id for deterministic CLI/UI output.
pub fn discover(root: impl AsRef<Path>) -> (Vec<DiscoveredExtension>, Vec<String>) {
    let root = root.as_ref();
    let activation = activation_state(root);
    let mut found = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return (found, errors) };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let Ok(manifest_type) = fs::symlink_metadata(&manifest_path) else { continue };
        if !manifest_type.is_file() || manifest_type.file_type().is_symlink() {
            errors.push(format!("{}: manifest must be a regular file", path.display()));
            continue;
        }
        let result = fs::read_to_string(&manifest_path)
            .map_err(|error| error.to_string())
            .and_then(|contents| serde_json::from_str::<ExtensionManifest>(&contents).map_err(|error| error.to_string()))
            .and_then(|manifest| validate_manifest(&manifest).map(|()| manifest));
        match result {
            Ok(manifest) => {
                let enabled = activation.get(&manifest.id).copied().unwrap_or(true);
                found.push(DiscoveredExtension { manifest, root: path, enabled });
            }
            Err(error) => errors.push(format!("{}: {error}", manifest_path.display())),
        }
    }
    found.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    (found, errors)
}

/// Turn discovered manifests into a deterministic command registry without
/// executing extension code. Qualified IDs prevent two extensions from
/// claiming the same command, while reserved core names remain protected.
pub fn command_registry(root: impl AsRef<Path>) -> (Vec<ExtensionCommandRegistration>, Vec<String>) {
    let (extensions, mut errors) = discover(root);
    let reserved = ["project.inspect", "control.inspect", "plugin_catalog", "history.commit"];
    let mut ids = std::collections::HashSet::new();
    let mut registrations = Vec::new();
    for extension in extensions {
        if !extension.enabled { continue; }
        for command in extension.manifest.commands {
            let qualified_id = format!("{}.{}", extension.manifest.id, command.id);
            if reserved.contains(&qualified_id.as_str()) || !ids.insert(qualified_id.clone()) {
                errors.push(format!("duplicate or reserved extension command: {qualified_id}"));
                continue;
            }
            registrations.push(ExtensionCommandRegistration {
                extension_id: extension.manifest.id.clone(),
                command_id: command.id,
                qualified_id,
                title: command.title,
                kind: command.kind,
                execution: extension.manifest.execution.clone(),
                permissions: extension.manifest.permissions.clone(),
                input_schema: command.input_schema,
                root: extension.root.clone(),
                entrypoint: extension.manifest.entrypoint.clone(),
                enabled: true,
            });
        }
    }
    registrations.sort_by(|left, right| left.qualified_id.cmp(&right.qualified_id));
    (registrations, errors)
}

/// Complete installed-command catalog for UI management. Disabled commands
/// remain visible here so users can re-enable them; execution still uses the
/// filtered command registry above.
pub fn command_catalog(root: impl AsRef<Path>) -> (Vec<ExtensionCommandRegistration>, Vec<String>) {
    let (extensions, mut errors) = discover(root);
    let mut ids = std::collections::HashSet::new();
    let mut registrations = Vec::new();
    for extension in extensions {
        for command in extension.manifest.commands {
            let qualified_id = format!("{}.{}", extension.manifest.id, command.id);
            if !ids.insert(qualified_id.clone()) {
                errors.push(format!("duplicate extension command: {qualified_id}"));
                continue;
            }
            registrations.push(ExtensionCommandRegistration {
                extension_id: extension.manifest.id.clone(), command_id: command.id,
                qualified_id, title: command.title, kind: command.kind,
                execution: extension.manifest.execution.clone(), permissions: extension.manifest.permissions.clone(),
                input_schema: command.input_schema, root: extension.root.clone(),
                entrypoint: extension.manifest.entrypoint.clone(), enabled: extension.enabled,
            });
        }
    }
    registrations.sort_by(|left, right| left.qualified_id.cmp(&right.qualified_id));
    (registrations, errors)
}

include!("extensions_execution.rs");

include!("extensions_tests.rs");
