use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MediaTagEntry {
    pub path: String,
    pub tags: Vec<String>,
    pub favorite: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaTagIndex {
    pub entries: Vec<MediaTagEntry>,
}

impl MediaTagIndex {
    pub fn upsert(&mut self, path: &str, tags: &[String], favorite: bool) -> bool {
        if path.trim().is_empty()
            || path.len() > 4096
            || path.contains('\0')
            || tags.len() > 128
            || tags.iter().any(|t| t.len() > 128 || t.contains('\0'))
        {
            return false;
        }
        let mut normalized: Vec<String> = tags
            .iter()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        normalized.sort();
        normalized.dedup();
        if let Some(entry) = self.entries.iter_mut().find(|e| e.path == path) {
            entry.tags = normalized;
            entry.favorite = favorite;
        } else {
            self.entries.push(MediaTagEntry {
                path: path.to_owned(),
                tags: normalized,
                favorite,
            });
        }
        true
    }
    pub fn search(&self, query: &str, favorites_only: bool) -> Vec<MediaTagEntry> {
        let q = query.trim().to_ascii_lowercase();
        let mut results: Vec<_> = self
            .entries
            .iter()
            .filter(|e| {
                (!favorites_only || e.favorite)
                    && (q.is_empty()
                        || e.path.to_ascii_lowercase().contains(&q)
                        || e.tags.iter().any(|t| t.contains(&q)))
            })
            .cloned()
            .collect();
        results.sort_by_key(|entry| (!entry.favorite, entry.path.to_ascii_lowercase()));
        results
    }
    pub fn remove(&mut self, path: &str) -> bool {
        let n = self.entries.len();
        self.entries.retain(|e| e.path != path);
        n != self.entries.len()
    }
    pub fn validate(&self) -> bool {
        self.entries.len() <= 1_000_000
            && self.entries.iter().all(|e| {
                !e.path.trim().is_empty()
                    && e.path.len() <= 4096
                    && !e.path.contains('\0')
                    && e.tags.len() <= 128
                    && e.tags
                        .iter()
                        .all(|t| !t.trim().is_empty() && t.len() <= 128 && !t.contains('\0'))
                    && e.tags
                        .iter()
                        .enumerate()
                        .all(|(i, t)| e.tags[..i].iter().all(|p| p != t))
            })
            && self
                .entries
                .iter()
                .enumerate()
                .all(|(i, e)| self.entries[..i].iter().all(|p| p.path != e.path))
    }
}

#[cfg(test)]
mod media_tag_tests {
    use super::*;
    #[test]
    fn tags_search_and_favorites() {
        let mut i = MediaTagIndex::default();
        assert!(i.upsert("kick.wav", &["Drum".into()], true));
        assert_eq!(i.search("drum", true).len(), 1);
        assert!(i.remove("kick.wav"));
    }
}

pub struct AssetInfo {
    pub original_path: String,
    pub consolidated_path: String,
    pub size: u64,
}

pub fn tempo_sync_ratio(source_bpm: f64, project_bpm: f64) -> Option<f64> {
    if !source_bpm.is_finite()
        || !project_bpm.is_finite()
        || source_bpm <= 0.0
        || project_bpm <= 0.0
    {
        return None;
    }
    Some((project_bpm / source_bpm).clamp(0.03125, 32.0))
}
pub fn tempo_sync_length(samples: u64, source_bpm: f64, project_bpm: f64) -> Option<u64> {
    let ratio = tempo_sync_ratio(source_bpm, project_bpm)?;
    let value = (samples as f64 / ratio).round();
    if !value.is_finite() || value < 0.0 || value > u64::MAX as f64 {
        return None;
    }
    Some(value as u64)
}

pub struct AssetOrchestrator {
    pub assets: Vec<AssetInfo>,
}

impl Default for AssetOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetOrchestrator {
    pub fn duplicate_assets_by_content(&self) -> Vec<Vec<String>> {
        let mut groups = std::collections::HashMap::<[u8; 32], Vec<String>>::new();
        for asset in &self.assets {
            if let Ok(bytes) = fs::read(&asset.consolidated_path) {
                let hash: [u8; 32] = Sha256::digest(&bytes).into();
                groups
                    .entry(hash)
                    .or_default()
                    .push(asset.consolidated_path.clone());
            }
        }
        let mut result: Vec<_> = groups
            .into_values()
            .filter(|paths| paths.len() > 1)
            .map(|mut paths| {
                paths.sort();
                paths
            })
            .collect();
        result.sort_by(|a, b| a[0].cmp(&b[0]));
        result
    }
    /// Reports duplicate references and files in a media pool that are not
    /// referenced by the current timeline.
    pub fn analyze_media_usage(&self, timeline_data: &[u8], media_dir: &str) -> Option<Value> {
        let timeline: Value = serde_json::from_slice(timeline_data).ok()?;
        let references = referenced_paths(&timeline);
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for path in &references {
            *counts.entry(path.clone()).or_default() += 1;
        }
        let mut duplicates: Vec<Value> = counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(path, count)| serde_json::json!({"path":path,"references":count}))
            .collect();
        duplicates.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        let mut unused = Vec::new();
        let dir = Path::new(media_dir);
        if dir.is_dir() {
            for entry in fs::read_dir(dir).ok()?.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let value = path.to_string_lossy().to_string();
                if !references
                    .iter()
                    .any(|reference| resolve_source(dir, reference) == path || reference == &value)
                {
                    unused.push(value);
                }
            }
        }
        unused.sort();
        Some(
            serde_json::json!({"ok":true,"referenced_count":references.len(),"duplicates":duplicates,"unused_files":unused}),
        )
    }
    pub fn missing_references(
        &self,
        timeline_data: &[u8],
        project_dir: &str,
    ) -> Option<Vec<String>> {
        let timeline: Value = serde_json::from_slice(timeline_data).ok()?;
        let root = Path::new(project_dir);
        if !root.is_dir() {
            return None;
        }
        Some(
            referenced_paths(&timeline)
                .into_iter()
                .filter(|reference| !resolve_source(root, reference).is_file())
                .collect(),
        )
    }
    pub fn new() -> Self {
        Self { assets: Vec::new() }
    }

    /// Collects the file references in timeline JSON into `<project_dir>/Assets`.
    ///
    /// The public boolean API is retained for compatibility: malformed timeline
    /// data, missing files, unsafe destinations, and copy/stat failures return
    /// `false` and leave the existing asset list unchanged.
    pub fn consolidate_assets(&mut self, project_dir: &str, timeline_data: &[u8]) -> bool {
        let project_dir = Path::new(project_dir);
        if project_dir.as_os_str().is_empty() || !project_dir.is_dir() {
            return false;
        }

        let timeline: Value = match serde_json::from_slice(timeline_data) {
            Ok(value) => value,
            Err(_) => return false,
        };
        let references = referenced_paths(&timeline);
        let asset_dir = project_dir.join("Assets");
        if fs::create_dir_all(&asset_dir).is_err() || !asset_dir.is_dir() {
            return false;
        }

        let mut planned = Vec::with_capacity(references.len());
        let mut used_destinations = HashSet::new();
        for original_path in references {
            let source = resolve_source(project_dir, &original_path);
            // Do not capture symlinks into a project. A symlink can change its
            // target after collection and would make the project depend on an
            // uncontrolled external path.
            let source_metadata = match fs::symlink_metadata(&source) {
                Ok(metadata) => metadata,
                Err(_) => return false,
            };
            if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
                return false;
            }

            let destination = match destination_for(&asset_dir, &source, &mut used_destinations) {
                Some(path) => path,
                None => return false,
            };
            planned.push((original_path, source, destination));
        }

        let mut created = Vec::new();
        let mut consolidated = Vec::with_capacity(planned.len());
        for (original_path, source, destination) in planned {
            let already_consolidated = same_file(&source, &destination);
            if !already_consolidated {
                if !copy_atomic(&source, &destination) {
                    for path in created {
                        let _ = fs::remove_file(path);
                    }
                    return false;
                }
                created.push(destination.clone());
            }

            let size = match fs::metadata(&destination) {
                Ok(metadata) if metadata.is_file() => metadata.len(),
                _ => {
                    for path in created {
                        let _ = fs::remove_file(path);
                    }
                    return false;
                }
            };
            consolidated.push(AssetInfo {
                original_path,
                consolidated_path: destination.to_string_lossy().into_owned(),
                size,
            });
        }

        self.assets = consolidated;
        true
    }

    pub fn audit_assets(&self) -> bool {
        self.assets.iter().all(|asset| {
            fs::metadata(&asset.consolidated_path)
                .map(|metadata| metadata.is_file() && metadata.len() == asset.size)
                .unwrap_or(false)
        })
    }

    /// Consolidates assets and returns a rewritten project document whose
    /// references point inside the project. The compatibility boolean API
    /// above intentionally does not mutate caller-owned JSON; integrations
    /// that persist a project should use this transactional form.
    pub fn consolidate_assets_and_rewrite_json(
        &mut self,
        project_dir: &str,
        timeline_data: &[u8],
    ) -> Option<Vec<u8>> {
        if !self.consolidate_assets(project_dir, timeline_data) {
            return None;
        }
        let mut timeline: Value = serde_json::from_slice(timeline_data).ok()?;
        let project_dir = Path::new(project_dir);
        let mut replacements = std::collections::HashMap::new();
        for asset in &self.assets {
            let destination = Path::new(&asset.consolidated_path);
            let relative = destination
                .strip_prefix(project_dir)
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| asset.consolidated_path.clone());
            replacements.insert(asset.original_path.clone(), relative);
        }
        rewrite_references(&mut timeline, &replacements, false);
        serde_json::to_vec(&timeline).ok()
    }
}

fn rewrite_references(
    value: &mut Value,
    replacements: &std::collections::HashMap<String, String>,
    asset_context: bool,
) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let reference = is_reference_key(key);
                rewrite_references(
                    child,
                    replacements,
                    asset_context || is_asset_container(key),
                );
                if reference {
                    if let Value::String(path) = child {
                        if let Some(replacement) = replacements.get(path) {
                            *path = replacement.clone();
                        }
                    }
                }
            }
        }
        Value::Array(array) => {
            for child in array.iter_mut() {
                if asset_context {
                    if let Value::String(path) = child {
                        if let Some(replacement) = replacements.get(path) {
                            *path = replacement.clone();
                        }
                    }
                }
                rewrite_references(child, replacements, asset_context);
            }
        }
        _ => {}
    }
}

fn referenced_paths(value: &Value) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    collect_references(value, false, &mut result, &mut seen);
    result
}

fn collect_references(
    value: &Value,
    asset_context: bool,
    result: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let key_is_reference = is_reference_key(key);
                collect_references(
                    child,
                    asset_context || is_asset_container(key),
                    result,
                    seen,
                );
                if key_is_reference {
                    if let Value::String(path) = child {
                        add_reference(path, result, seen);
                    }
                }
            }
        }
        Value::Array(array) => {
            for child in array {
                if asset_context {
                    if let Value::String(path) = child {
                        add_reference(path, result, seen);
                    }
                }
                collect_references(child, asset_context, result, seen);
            }
        }
        _ => {}
    }
}

fn is_reference_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['_', '-'], "");
    key == "path"
        || key.ends_with("path")
        || matches!(
            key.as_str(),
            "file" | "filepath" | "source" | "asset" | "media"
        )
}

fn is_asset_container(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("asset") || key.contains("region") || key.contains("sample") || key == "files"
}

fn add_reference(path: &str, result: &mut Vec<String>, seen: &mut HashSet<String>) {
    let path = path.trim();
    if !path.is_empty() && seen.insert(path.to_owned()) {
        result.push(path.to_owned());
    }
}

fn resolve_source(project_dir: &Path, reference: &str) -> PathBuf {
    let path = Path::new(reference);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_dir.join(path)
    }
}

fn destination_for(
    asset_dir: &Path,
    source: &Path,
    used_destinations: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let name = source.file_name()?.to_str()?;
    let candidate = asset_dir.join(name);
    let destination =
        if !used_destinations.contains(&candidate) && destination_is_usable(&candidate, source) {
            candidate
        } else {
            let stem = source.file_stem()?.to_str()?;
            let extension = source.extension().and_then(|value| value.to_str());
            let fingerprint = file_fingerprint(source)?;
            let filename = match extension {
                Some(extension) => format!("{stem}-{fingerprint:016x}.{extension}"),
                None => format!("{stem}-{fingerprint:016x}"),
            };
            let hashed = asset_dir.join(filename);
            if !used_destinations.contains(&hashed) && destination_is_usable(&hashed, source) {
                hashed
            } else {
                return None;
            }
        };
    used_destinations.insert(destination.clone());
    Some(destination)
}

fn same_file(left: &Path, right: &Path) -> bool {
    left.canonicalize().ok() == right.canonicalize().ok()
}

fn destination_is_usable(destination: &Path, source: &Path) -> bool {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && (same_file(source, destination) || same_bytes(source, destination))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn file_fingerprint(path: &Path) -> Option<u64> {
    let bytes = fs::read(path).ok()?;
    Some(crate::persistence::PersistenceOrchestrator::calculate_checksum(&bytes))
}

fn same_bytes(left: &Path, right: &Path) -> bool {
    match (fs::read(left), fs::read(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn copy_atomic(source: &Path, destination: &Path) -> bool {
    let temporary = destination.with_extension(format!(
        "asset-tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    let result = File::open(source)
        .and_then(|mut input| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .and_then(|mut output| {
                    io::copy(&mut input, &mut output)?;
                    output.sync_all()
                })
        })
        .and_then(|_| fs::rename(&temporary, destination))
        .and_then(|_| sync_parent_directory(destination));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return false;
    }
    true
}

fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::AssetOrchestrator;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project() -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aura-asset-test-{}-{id}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn consolidates_referenced_assets_and_records_size() {
        let project = temp_project();
        let source = project.join("aura-asset-source.wav");
        fs::write(&source, b"audio bytes").unwrap();
        let timeline = format!(r#"{{"regions":[{{"file_path":"{}"}}]}}"#, source.display());

        let mut orchestrator = AssetOrchestrator::new();
        assert!(orchestrator.consolidate_assets(project.to_str().unwrap(), timeline.as_bytes()));
        assert_eq!(orchestrator.assets.len(), 1);
        assert_eq!(orchestrator.assets[0].size, 11);
        assert_eq!(
            fs::read(&orchestrator.assets[0].consolidated_path).unwrap(),
            b"audio bytes"
        );
        assert!(orchestrator.audit_assets());

        let _ = fs::remove_file(source);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn rejects_missing_references_and_keeps_previous_state() {
        let project = temp_project();
        let mut orchestrator = AssetOrchestrator::new();
        assert!(!orchestrator.consolidate_assets(
            project.to_str().unwrap(),
            br#"{"asset_path":"missing.wav"}"#,
        ));
        assert!(orchestrator.assets.is_empty());
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn same_basenames_use_content_fingerprints_and_leave_no_asset_temp_files() {
        let project = temp_project();
        let first_dir = project.join("one");
        let second_dir = project.join("two");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        let first = first_dir.join("take.wav");
        let second = second_dir.join("take.wav");
        fs::write(&first, b"first audio").unwrap();
        fs::write(&second, b"second audio").unwrap();
        let timeline = serde_json::json!({
            "regions": [
                {"file_path": "one/take.wav"},
                {"file_path": "two/take.wav"}
            ]
        });

        let mut orchestrator = AssetOrchestrator::new();
        assert!(orchestrator
            .consolidate_assets(project.to_str().unwrap(), timeline.to_string().as_bytes()));
        assert_eq!(orchestrator.assets.len(), 2);
        assert_ne!(
            orchestrator.assets[0].consolidated_path,
            orchestrator.assets[1].consolidated_path
        );
        assert!(fs::read_dir(project.join("Assets"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("asset-tmp-")));
        assert!(orchestrator.audit_assets());
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn rewrite_api_removes_external_asset_reference() {
        let project = temp_project();
        let source = project.join("outside").join("vocal.wav");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"audio bytes").unwrap();
        let timeline = serde_json::json!({
            "regions": [{"file_path": source.to_string_lossy()}]
        });
        let mut orchestrator = AssetOrchestrator::new();
        let rewritten = orchestrator
            .consolidate_assets_and_rewrite_json(
                project.to_str().unwrap(),
                timeline.to_string().as_bytes(),
            )
            .expect("rewritten project must be returned");
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        let path = value["regions"][0]["file_path"].as_str().unwrap();
        assert!(path.starts_with("Assets/"));
        assert!(project.join(path).is_file());
        let _ = fs::remove_dir_all(project);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_assets_before_copying() {
        let project = temp_project();
        let source = project.join("real.wav");
        let link = project.join("linked.wav");
        fs::write(&source, b"audio bytes").unwrap();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let timeline = serde_json::json!({"asset_path": "linked.wav"});

        let mut orchestrator = AssetOrchestrator::new();
        assert!(!orchestrator
            .consolidate_assets(project.to_str().unwrap(), timeline.to_string().as_bytes()));
        assert!(orchestrator.assets.is_empty());
        assert!(!project.join("Assets").join("linked.wav").exists());
        let _ = fs::remove_dir_all(project);
    }
}
