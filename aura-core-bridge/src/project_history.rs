//! Content-addressed project history for non-destructive DAW workflows.
//!
//! Audio files are referenced by the project/asset layer; this store keeps
//! immutable JSON project snapshots and small refs/commit records.  It is
//! intentionally independent from the audio engine so checkout/revert can be
//! hydrated through the same transactional project path as a normal load.

use crate::project::ProjectDocument;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{create_dir_all, read, read_to_string, rename, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const HISTORY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCommit {
    pub schema_version: u32,
    pub commit_id: String,
    pub snapshot_hash: String,
    pub parent: Option<String>,
    pub branch: String,
    pub message: String,
    pub created_unix_seconds: u64,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub project_format_version: u32,
    #[serde(default)]
    pub asset_manifest_hash: String,
    #[serde(default)]
    pub plugin_manifest_hash: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub render_artifact_hash: String,
    #[serde(default)]
    pub render_artifact_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRef {
    pub name: String,
    pub commit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryStatus {
    pub branch: String,
    pub head: Option<String>,
    pub snapshot_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotDiff {
    pub from: String,
    pub to: String,
    pub changed_sections: Vec<String>,
    #[serde(default)]
    pub changes: Vec<SnapshotChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotChange {
    pub section: String,
    pub before_hash: String,
    pub after_hash: String,
    /// Stable entity identity when the changed section is an array of
    /// project entities.  Older history records omit this field and remain
    /// readable through the serde default.
    #[serde(default)]
    pub entity_id: Option<String>,
    /// One of `added`, `removed`, or `changed` for entity-level entries;
    /// section-level entries use `changed`.
    #[serde(default = "default_change_operation")]
    pub operation: String,
    /// Bounded field-level values for machine clients.  Large blobs such as
    /// plugin state are represented by the entity hashes above, never copied
    /// into a diff response.
    #[serde(default)]
    pub fields: Vec<FieldChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

fn default_change_operation() -> String {
    "changed".to_owned()
}

pub struct ProjectHistoryStore {
    root: PathBuf,
    project_root: PathBuf,
}

#[derive(Debug, Serialize)]
struct AssetFingerprint {
    path: String,
    exists: bool,
    #[serde(default)]
    is_symlink: bool,
    bytes: u64,
    content_hash: String,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    frame_count: Option<u64>,
}

struct HistoryLock {
    path: PathBuf,
    nonce: String,
}

impl HistoryLock {
    fn acquire(root: &Path) -> Result<Self> {
        let path = root.join("LOCK");
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                if reclaim_dead_lock(&path) {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|retry| {
                            anyhow::anyhow!("history lock could not be reclaimed: {retry}")
                        })?
                } else {
                    bail!("history transaction is already in progress: {error}");
                }
            }
        };
        let nonce = Uuid::new_v4().to_string();
        let token = format!("pid={} nonce={}\n", std::process::id(), nonce);
        if let Err(error) = file
            .write_all(token.as_bytes())
            .and_then(|_| file.sync_all())
        {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self { path, nonce })
    }
}

impl Drop for HistoryLock {
    fn drop(&mut self) {
        let owns_lock = read_to_string(&self.path)
            .ok()
            .and_then(|contents| {
                contents
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("nonce=").map(str::to_owned))
            })
            .is_some_and(|nonce| nonce == self.nonce);
        if owns_lock {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn reclaim_dead_lock(path: &Path) -> bool {
    let Ok(contents) = read_to_string(path) else {
        return false;
    };
    let Some(pid) = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid=")?.parse::<i32>().ok())
    else {
        return false;
    };
    #[cfg(unix)]
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true);
    #[cfg(not(unix))]
    let alive = true;
    !alive && std::fs::remove_file(path).is_ok()
}

impl ProjectHistoryStore {
    pub fn open(project_path: impl AsRef<Path>) -> Result<Self> {
        let project_path = project_path.as_ref();
        let project_root = if project_path.is_dir() {
            project_path.to_path_buf()
        } else if project_path.is_file() {
            project_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        } else if !project_path.exists() && project_path.extension().is_none() {
            // A new directory project has no filesystem marker yet. Paths
            // with an extension are intentionally rejected here because they
            // are ambiguous; callers must choose open_project_file() or
            // open_project_directory() explicitly.
            project_path.to_path_buf()
        } else {
            bail!(
                "ambiguous project history path {}; use open_project_file or open_project_directory",
                project_path.display()
            )
        };
        Self::open_at_root(project_root)
    }

    pub fn open_project_file(project_file: impl AsRef<Path>) -> Result<Self> {
        let project_file = project_file.as_ref();
        let root = project_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self::open_at_root(root)
    }

    pub fn open_project_directory(project_directory: impl AsRef<Path>) -> Result<Self> {
        Self::open_at_root(project_directory.as_ref().to_path_buf())
    }

    fn open_at_root(project_root: PathBuf) -> Result<Self> {
        let root = project_root.join(".aura").join("history");
        for directory in ["snapshots", "commits", "refs/heads", "refs/tags"] {
            create_dir_all(root.join(directory))?;
        }
        let head = root.join("HEAD");
        if !head.exists() {
            atomic_write(&head, b"main\n")?;
        }
        let identity = root.join("PROJECT_ID");
        if read_to_string(&identity)
            .ok()
            .and_then(|value| value.trim().parse::<Uuid>().ok())
            .is_none()
        {
            atomic_write(&identity, Uuid::new_v4().to_string().as_bytes())?;
        }
        Ok(Self { root, project_root })
    }

    pub fn status(&self) -> Result<HistoryStatus> {
        let branch = self.current_branch()?;
        self.status_for_branch(&branch)
    }

    /// Verify that the on-disk working project is the snapshot currently
    /// referenced by HEAD.  History refs are intentionally cheap to update,
    /// so this explicit check prevents callers from mistaking a moved HEAD
    /// for an already-hydrated engine/project state.
    pub fn verify_working_tree(&self, project_file: impl AsRef<Path>) -> Result<serde_json::Value> {
        let project_file = project_file.as_ref();
        let project_file_text = project_file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let status = self.status()?;
        let expected_project_id = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        let project = ProjectDocument::load(project_file_text).with_context(|| {
            format!("failed to load working project {}", project_file.display())
        })?;
        let project_bytes = serde_json::to_vec(&project)?;
        let working_snapshot_hash = content_hash(&project_bytes);
        let identity_matches = project.project_id == expected_project_id;
        let expected_asset_manifest_hash = status
            .head
            .as_deref()
            .map(|commit| self.read_commit(commit))
            .transpose()?
            .map(|commit| commit.asset_manifest_hash);
        let working_asset_manifest_hash = self.asset_manifest_hash(&project)?;
        let head_matches = status
            .snapshot_hash
            .as_deref()
            .is_some_and(|hash| hash == working_snapshot_hash);
        let asset_manifest_matches = expected_asset_manifest_hash
            .as_deref()
            .is_some_and(|hash| hash == working_asset_manifest_hash);
        Ok(serde_json::json!({
            "ok": identity_matches && head_matches && asset_manifest_matches,
            "branch": status.branch,
            "head": status.head,
            "head_snapshot_hash": status.snapshot_hash,
            "working_snapshot_hash": working_snapshot_hash,
            "project_id": project.project_id,
            "history_project_id": expected_project_id,
            "identity_matches": identity_matches,
            "working_tree_matches_head": head_matches,
            "expected_asset_manifest_hash": expected_asset_manifest_hash,
            "working_asset_manifest_hash": working_asset_manifest_hash,
            "asset_manifest_matches_head": asset_manifest_matches,
        }))
    }

    /// Returns the state a branch would expose without changing HEAD.  This
    /// is deliberately separate from `checkout` so callers can hydrate and
    /// validate a working tree before publishing the ref update.
    pub fn status_for_branch(&self, branch: &str) -> Result<HistoryStatus> {
        validate_ref_name(branch)?;
        let head = self.read_ref("heads", branch)?;
        let snapshot_hash = head
            .as_deref()
            .map(|commit| self.read_commit(commit))
            .transpose()?
            .map(|commit| commit.snapshot_hash);
        Ok(HistoryStatus {
            branch: branch.to_owned(),
            head,
            snapshot_hash,
        })
    }

    pub fn commit(&self, project: &ProjectDocument, message: &str) -> Result<ProjectCommit> {
        if message.trim().is_empty() || message.len() > 512 {
            bail!("commit message must be 1..=512 characters");
        }
        let _lock = HistoryLock::acquire(&self.root)?;
        self.commit_locked(project, message, None)
    }

    /// Commit a project together with a rendered audio artifact. The render
    /// is never copied into history; its content hash and byte size make the
    /// external file independently verifiable and reproducible.
    pub fn commit_with_render_file(
        &self,
        project: &ProjectDocument,
        message: &str,
        render_file: impl AsRef<Path>,
    ) -> Result<ProjectCommit> {
        if message.trim().is_empty() || message.len() > 512 {
            bail!("commit message must be 1..=512 characters");
        }
        let render_file = render_file.as_ref();
        let (bytes, hash, _, _, _) = fingerprint_asset_file(render_file)
            .with_context(|| format!("failed to fingerprint render {}", render_file.display()))?;
        let _lock = HistoryLock::acquire(&self.root)?;
        self.commit_locked(project, message, Some((hash, bytes)))
    }

    /// Commit the on-disk working tree while holding the history lock.
    /// Loading before acquiring the lock permits a concurrent save to be
    /// committed with stale bytes, even though the ref update itself is
    /// serialized.  The CLI and other file-backed adapters should use this
    /// entry point so read, snapshot creation, commit publication, and ref
    /// update share one transaction boundary.
    pub fn commit_file(
        &self,
        project_file: impl AsRef<Path>,
        message: &str,
    ) -> Result<ProjectCommit> {
        if message.trim().is_empty() || message.len() > 512 {
            bail!("commit message must be 1..=512 characters");
        }
        let project_file = project_file.as_ref();
        let project_file_text = project_file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let _lock = HistoryLock::acquire(&self.root)?;
        let project = ProjectDocument::load(project_file_text)
            .with_context(|| format!("failed to load project {}", project_file.display()))?;
        self.commit_locked(&project, message, None)
    }

    fn commit_locked(
        &self,
        project: &ProjectDocument,
        message: &str,
        render: Option<(String, u64)>,
    ) -> Result<ProjectCommit> {
        project.validate()?;
        self.ensure_project_identity(&project.project_id)?;
        let bytes = serde_json::to_vec(project)?;
        let snapshot_hash = content_hash(&bytes);
        let branch = self.current_branch()?;
        let parent = self.read_ref("heads", &branch)?;
        let created = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let project_id = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        let asset_manifest_hash = self.asset_manifest_hash(project)?;
        // Track-local sandbox entries are part of the persisted plugin graph
        // too.  Hash them together with the normalized instance contracts so
        // older projects cannot silently reuse a stale plugin manifest.
        let plugin_manifest_hash = content_hash(&serde_json::to_vec(&(
            &project.plugin_instances,
            &project.tracks,
        ))?);
        let platform = std::env::consts::OS.to_owned();
        atomic_write(
            &self
                .root
                .join("snapshots")
                .join(format!("{snapshot_hash}.json")),
            &bytes,
        )?;
        let mut commit = ProjectCommit {
            schema_version: HISTORY_SCHEMA_VERSION,
            commit_id: String::new(),
            snapshot_hash,
            parent,
            branch: branch.clone(),
            message: message.trim().to_owned(),
            created_unix_seconds: created,
            project_id,
            project_format_version: project.schema_version,
            asset_manifest_hash,
            plugin_manifest_hash,
            platform,
            render_artifact_hash: render
                .as_ref()
                .map(|value| value.0.clone())
                .unwrap_or_default(),
            render_artifact_bytes: render.map(|value| value.1).unwrap_or_default(),
        };
        let material = commit_id_material(&commit);
        let commit_id = content_hash(material.as_bytes());
        commit.commit_id = commit_id.clone();
        atomic_write(
            &self.root.join("commits").join(format!("{commit_id}.json")),
            &serde_json::to_vec_pretty(&commit)?,
        )?;
        atomic_write(
            &self.root.join("refs/heads").join(branch),
            commit_id.as_bytes(),
        )?;
        Ok(commit)
    }

    pub fn create_branch(&self, name: &str, from: Option<&str>) -> Result<ProjectRef> {
        validate_ref_name(name)?;
        let _lock = HistoryLock::acquire(&self.root)?;
        let commit_id = match from {
            Some(id) => {
                self.read_commit(id)?;
                id.to_owned()
            }
            None => self
                .status()?
                .head
                .ok_or_else(|| anyhow::anyhow!("cannot branch without a commit"))?,
        };
        atomic_write(
            &self.root.join("refs/heads").join(name),
            commit_id.as_bytes(),
        )?;
        Ok(ProjectRef {
            name: name.to_owned(),
            commit_id,
        })
    }

    pub fn checkout(&self, branch: &str) -> Result<HistoryStatus> {
        validate_ref_name(branch)?;
        let _lock = HistoryLock::acquire(&self.root)?;
        if self.read_ref("heads", branch)?.is_none() {
            bail!("branch does not exist: {branch}");
        }
        atomic_write(&self.root.join("HEAD"), format!("{branch}\n").as_bytes())?;
        self.status()
    }

    /// Hydrate a branch snapshot and publish HEAD as one history transaction.
    ///
    /// The project file is written first so an invalid snapshot can never move
    /// HEAD.  If publishing HEAD fails after the working tree was replaced,
    /// the original bytes are restored before returning the error.  This keeps
    /// the three observable states (history HEAD, working tree, and the next
    /// engine hydration) from silently diverging at the filesystem boundary.
    pub fn checkout_and_restore(
        &self,
        project_file: impl AsRef<Path>,
        branch: &str,
    ) -> Result<HistoryStatus> {
        validate_ref_name(branch)?;
        let project_file = project_file.as_ref();
        let project_file_text = project_file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let _lock = HistoryLock::acquire(&self.root)?;
        self.validate_working_project_identity(project_file)?;
        let commit_id = self
            .read_ref("heads", branch)?
            .ok_or_else(|| anyhow::anyhow!("branch does not exist: {branch}"))?;
        let target = self.load_commit(&commit_id)?;
        let previous = if project_file.exists() {
            Some(read(project_file).with_context(|| {
                format!(
                    "failed to snapshot current project {}",
                    project_file.display()
                )
            })?)
        } else {
            None
        };

        target.save_atomic(project_file_text).with_context(|| {
            format!(
                "failed to restore history commit into {}",
                project_file.display()
            )
        })?;

        if let Err(head_error) =
            atomic_write(&self.root.join("HEAD"), format!("{branch}\n").as_bytes())
        {
            let rollback = match previous.as_deref() {
                Some(bytes) => atomic_write(project_file, bytes),
                None => remove_published_file(project_file),
            };
            return match rollback {
                Ok(()) => Err(anyhow::anyhow!(
                    "history HEAD publication failed; project restore rolled back: {head_error}"
                )),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "history HEAD publication failed and project rollback failed: {head_error}; rollback: {rollback_error}"
                )),
            };
        }
        self.status()
    }

    /// Restore a historical snapshot as a new commit on the current branch.
    /// This keeps HEAD aligned with the working tree instead of leaving a
    /// silent detached working state after a CLI `revert`.
    pub fn revert_and_restore(
        &self,
        project_file: impl AsRef<Path>,
        commit_id: &str,
    ) -> Result<ProjectCommit> {
        let project_file = project_file.as_ref();
        let project_file_text = project_file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let _lock = HistoryLock::acquire(&self.root)?;
        self.validate_working_project_identity(project_file)?;
        let target = self.load_commit(commit_id)?;
        let previous = if project_file.exists() {
            Some(read(project_file).with_context(|| {
                format!(
                    "failed to snapshot current project {}",
                    project_file.display()
                )
            })?)
        } else {
            None
        };
        target.save_atomic(project_file_text).with_context(|| {
            format!(
                "failed to restore history commit into {}",
                project_file.display()
            )
        })?;
        let message = format!("revert {commit_id}");
        match self.commit_locked(&target, &message, None) {
            Ok(commit) => Ok(commit),
            Err(error) => {
                let rollback = match previous.as_deref() {
                    Some(bytes) => atomic_write(project_file, bytes),
                    None => remove_published_file(project_file),
                };
                match rollback {
                    Ok(()) => Err(anyhow::anyhow!(
                        "revert commit failed; project restore rolled back: {error}"
                    )),
                    Err(rollback_error) => Err(anyhow::anyhow!(
                        "revert commit failed and project rollback failed: {error}; rollback: {rollback_error}"
                    )),
                }
            }
        }
    }

    /// Apply selected sections from a historical snapshot and publish the
    /// result as a new commit.  The pure `cherry_pick` helper remains useful
    /// for previews; this method is the durable CLI path.
    pub fn cherry_pick_and_restore(
        &self,
        project_file: impl AsRef<Path>,
        commit_id: &str,
        sections: &[String],
    ) -> Result<ProjectCommit> {
        let project_file = project_file.as_ref();
        let project_file_text = project_file
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?;
        let _lock = HistoryLock::acquire(&self.root)?;
        self.validate_working_project_identity(project_file)?;
        let current = ProjectDocument::load(project_file_text)?;
        let merged = self.cherry_pick(&current, commit_id, sections)?;
        let previous = read(project_file)?;
        merged.save_atomic(project_file_text)?;
        let message = format!("cherry-pick {commit_id}: {}", sections.join(","));
        match self.commit_locked(&merged, &message, None) {
            Ok(commit) => Ok(commit),
            Err(error) => {
                let rollback = atomic_write(project_file, &previous);
                match rollback {
                    Ok(()) => Err(anyhow::anyhow!(
                        "cherry-pick commit failed; project restore rolled back: {error}"
                    )),
                    Err(rollback_error) => Err(anyhow::anyhow!(
                        "cherry-pick commit failed and project rollback failed: {error}; rollback: {rollback_error}"
                    )),
                }
            }
        }
    }

    pub fn tag(&self, name: &str, commit_id: Option<&str>) -> Result<ProjectRef> {
        validate_ref_name(name)?;
        let _lock = HistoryLock::acquire(&self.root)?;
        let commit_id = match commit_id {
            Some(id) => {
                self.read_commit(id)?;
                id.to_owned()
            }
            None => self
                .status()?
                .head
                .ok_or_else(|| anyhow::anyhow!("cannot tag without a commit"))?,
        };
        atomic_write(
            &self.root.join("refs/tags").join(name),
            commit_id.as_bytes(),
        )?;
        Ok(ProjectRef {
            name: name.to_owned(),
            commit_id,
        })
    }

    pub fn load_commit(&self, commit_id: &str) -> Result<ProjectDocument> {
        let commit = self.read_commit(commit_id)?;
        let project_id = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        if commit.project_id != project_id {
            bail!("commit belongs to a different project history");
        }
        let bytes = read(
            self.root
                .join("snapshots")
                .join(format!("{}.json", commit.snapshot_hash)),
        )?;
        if content_hash(&bytes) != commit.snapshot_hash {
            bail!("snapshot checksum mismatch for commit {commit_id}");
        }
        let project: ProjectDocument =
            serde_json::from_slice(&bytes).context("invalid project snapshot")?;
        project.validate()?;
        Ok(project)
    }

    fn validate_working_project_identity(&self, project_file: &Path) -> Result<()> {
        if !project_file.exists() {
            return Ok(());
        }
        let expected = read_to_string(self.root.join("PROJECT_ID"))?
            .trim()
            .to_owned();
        let current = ProjectDocument::load(
            project_file
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("project path is not valid UTF-8"))?,
        )?;
        if current.project_id != expected {
            bail!(
                "working project identity does not match this history (expected {expected}, found {})",
                current.project_id
            );
        }
        Ok(())
    }

    pub fn log(&self, limit: usize) -> Result<Vec<ProjectCommit>> {
        let mut commits = Vec::new();
        let mut cursor = self.status()?.head;
        for _ in 0..limit.clamp(1, 512) {
            let Some(id) = cursor else {
                break;
            };
            let commit = self.read_commit(&id)?;
            cursor = commit.parent.clone();
            commits.push(commit);
        }
        Ok(commits)
    }

    pub fn diff_commits(&self, from: &str, to: &str) -> Result<SnapshotDiff> {
        let before = serde_json::to_value(self.load_commit(from)?)?;
        let after = serde_json::to_value(self.load_commit(to)?)?;
        let mut changed_sections = Vec::new();
        let mut changes = Vec::new();
        for section in [
            "metadata",
            "sample_rate",
            "tempo_events",
            "time_signature_events",
            "tracks",
            "regions",
            "plugin_instances",
            "midi_learn_mappings",
            "midi_notes",
            "chord_track",
            "macro_mappings",
            "warp_markers",
            "render_targets",
            "openutau_vocals",
            "freeze_artifacts",
            "sidechain_routes",
            "feedback_routes",
            "audio_routes",
        ] {
            if before.get(section) != after.get(section) {
                let before_hash = content_hash(&serde_json::to_vec(&before[section])?);
                let after_hash = content_hash(&serde_json::to_vec(&after[section])?);
                changed_sections.push(section.to_owned());
                changes.push(SnapshotChange {
                    section: section.to_owned(),
                    before_hash,
                    after_hash,
                    entity_id: None,
                    operation: "changed".to_owned(),
                    fields: Vec::new(),
                });
                append_entity_changes(section, &before[section], &after[section], &mut changes);
            }
        }
        Ok(SnapshotDiff {
            from: from.to_owned(),
            to: to.to_owned(),
            changed_sections,
            changes,
        })
    }

    pub fn cherry_pick(
        &self,
        current: &ProjectDocument,
        commit_id: &str,
        sections: &[String],
    ) -> Result<ProjectDocument> {
        if sections.is_empty() || sections.len() > 16 {
            bail!("cherry-pick requires 1..=16 sections");
        }
        let source = self.load_commit(commit_id)?;
        let mut result = current.clone();
        for section in sections {
            match section.as_str() {
                "metadata" => result.metadata = source.metadata.clone(),
                "sample_rate" => result.sample_rate = source.sample_rate,
                "tempo_events" => result.tempo_events = source.tempo_events.clone(),
                "time_signature_events" => {
                    result.time_signature_events = source.time_signature_events.clone()
                }
                "tracks" => result.tracks = source.tracks.clone(),
                "regions" => result.regions = source.regions.clone(),
                "plugin_instances" => result.plugin_instances = source.plugin_instances.clone(),
                "midi_learn_mappings" => {
                    result.midi_learn_mappings = source.midi_learn_mappings.clone()
                }
                "midi_notes" => result.midi_notes = source.midi_notes.clone(),
                "chord_track" => result.chord_track = source.chord_track.clone(),
                "macro_mappings" => result.macro_mappings = source.macro_mappings.clone(),
                "warp_markers" => result.warp_markers = source.warp_markers.clone(),
                "render_targets" => result.render_targets = source.render_targets.clone(),
                "openutau_vocals" => result.openutau_vocals = source.openutau_vocals.clone(),
                "freeze_artifacts" => result.freeze_artifacts = source.freeze_artifacts.clone(),
                "sidechain_routes" => result.sidechain_routes = source.sidechain_routes.clone(),
                "feedback_routes" => result.feedback_routes = source.feedback_routes.clone(),
                "audio_routes" => result.audio_routes = source.audio_routes.clone(),
                other => bail!("unknown cherry-pick section: {other}"),
            }
        }
        result.metadata.tracks_count = result.tracks.len() as u32;
        result.validate()?;
        Ok(result)
    }

    fn current_branch(&self) -> Result<String> {
        let branch = read_to_string(self.root.join("HEAD"))?.trim().to_owned();
        validate_ref_name(&branch)?;
        Ok(branch)
    }

    fn asset_manifest_hash(&self, project: &ProjectDocument) -> Result<String> {
        // Include every external audio file that can affect the audible
        // result. Region paths alone miss OpenUtau source/render pairs.
        let mut paths = project
            .regions
            .iter()
            .map(|region| region.path.clone())
            .collect::<Vec<_>>();
        for vocal in &project.openutau_vocals {
            paths.push(vocal.source_path.clone());
            paths.push(vocal.rendered_audio_path.clone());
        }
        paths.sort();
        paths.dedup();
        let mut manifest = Vec::with_capacity(paths.len());
        for path in paths {
            let requested = Path::new(&path);
            let absolute = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                self.project_root.join(requested)
            };
            let display_path = absolute
                .strip_prefix(&self.project_root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or(path);
            let is_symlink = std::fs::symlink_metadata(&absolute)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false);
            let fingerprint = match fingerprint_asset_file(&absolute) {
                Ok((byte_count, content_hash, sample_rate, channels, frame_count)) => {
                    AssetFingerprint {
                        path: display_path,
                        exists: true,
                        is_symlink,
                        bytes: byte_count,
                        content_hash,
                        sample_rate,
                        channels,
                        frame_count,
                    }
                }
                Err(_) => AssetFingerprint {
                    path: display_path,
                    exists: false,
                    is_symlink,
                    bytes: 0,
                    content_hash: content_hash(&[]),
                    sample_rate: None,
                    channels: None,
                    frame_count: None,
                },
            };
            manifest.push(fingerprint);
        }
        Ok(content_hash(&serde_json::to_vec(&manifest)?))
    }

    fn ensure_project_identity(&self, project_id: &str) -> Result<()> {
        let identity_path = self.root.join("PROJECT_ID");
        let stored = read_to_string(&identity_path)?.trim().to_owned();
        if stored == project_id {
            return Ok(());
        }
        // `open()` creates the marker before the first commit.  Adopt the
        // document's UUID exactly once while the history is still empty;
        // after a commit, replacing a project at the same path fails closed.
        let has_commits = std::fs::read_dir(self.root.join("commits"))
            .map(|entries| entries.flatten().next().is_some())
            .unwrap_or(true);
        if !has_commits && self.read_ref("heads", &self.current_branch()?)?.is_none() {
            atomic_write(&identity_path, project_id.as_bytes())?;
            return Ok(());
        }
        bail!("project UUID does not match this history store")
    }

    fn read_ref(&self, group: &str, name: &str) -> Result<Option<String>> {
        let path = self.root.join("refs").join(group).join(name);
        if !path.exists() {
            return Ok(None);
        }
        let value = read_to_string(path)?.trim().to_owned();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    fn read_commit(&self, id: &str) -> Result<ProjectCommit> {
        validate_commit_id(id)?;
        let commit: ProjectCommit =
            serde_json::from_slice(&read(self.root.join("commits").join(format!("{id}.json")))?)?;
        if commit.schema_version != HISTORY_SCHEMA_VERSION || commit.commit_id != id {
            bail!("invalid commit metadata");
        }
        if commit.project_id.is_empty()
            || commit.project_format_version == 0
            || commit.snapshot_hash.len() != 64
            || commit.asset_manifest_hash.len() != 64
            || commit.plugin_manifest_hash.len() != 64
            || commit.platform.trim().is_empty()
        {
            bail!("incomplete commit metadata");
        }
        let material = commit_id_material(&commit);
        if content_hash(material.as_bytes()) != id {
            bail!("commit checksum mismatch");
        }
        Ok(commit)
    }
}

/// Add bounded, machine-actionable changes for array-backed project entities.
/// The section-level hashes remain the compatibility contract, while these
/// entries let CLI/LLM clients show which track, region, plugin, mapping, or
/// render target changed without diffing an entire project snapshot locally.
fn append_entity_changes(
    section: &str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    changes: &mut Vec<SnapshotChange>,
) {
    let key = match section {
        "tracks" | "regions" => "id",
        "plugin_instances" => "instance_id",
        "midi_learn_mappings" => "mapping_id",
        "midi_notes" => "",
        "macro_mappings" => "mapping_id",
        "warp_markers" => "marker_id",
        "render_targets" => "target_id",
        "openutau_vocals" => "source_path",
        "freeze_artifacts" => "track_id",
        "audio_routes" | "feedback_routes" => "source_destination",
        _ => return,
    };
    let (Some(before_items), Some(after_items)) = (before.as_array(), after.as_array()) else {
        return;
    };

    let mut before_by_id = std::collections::BTreeMap::new();
    let mut after_by_id = std::collections::BTreeMap::new();
    for item in before_items {
        if let Some(id) = entity_id(item, key) {
            before_by_id.insert(id, item);
        }
    }
    for item in after_items {
        if let Some(id) = entity_id(item, key) {
            after_by_id.insert(id, item);
        }
    }

    let mut ids = std::collections::BTreeSet::new();
    ids.extend(before_by_id.keys().cloned());
    ids.extend(after_by_id.keys().cloned());
    for id in ids {
        let before_item = before_by_id.get(&id).copied();
        let after_item = after_by_id.get(&id).copied();
        let (operation, before_hash, after_hash) = match (before_item, after_item) {
            (None, Some(item)) => (
                "added",
                content_hash(&[]),
                content_hash(&serde_json::to_vec(item).unwrap_or_default()),
            ),
            (Some(item), None) => (
                "removed",
                content_hash(&serde_json::to_vec(item).unwrap_or_default()),
                content_hash(&[]),
            ),
            (Some(before_item), Some(after_item)) => {
                let before_bytes = serde_json::to_vec(before_item).unwrap_or_default();
                let after_bytes = serde_json::to_vec(after_item).unwrap_or_default();
                if before_bytes == after_bytes {
                    continue;
                }
                (
                    "changed",
                    content_hash(&before_bytes),
                    content_hash(&after_bytes),
                )
            }
            (None, None) => continue,
        };
        changes.push(SnapshotChange {
            section: section.to_owned(),
            before_hash,
            after_hash,
            entity_id: Some(id),
            operation: operation.to_owned(),
            fields: field_changes(before_item, after_item),
        });
    }
}

fn entity_id(item: &serde_json::Value, key: &str) -> Option<String> {
    if key == "source_destination" {
        let object = item.as_object()?;
        return Some(format!(
            "{}:{}",
            object.get("source_id")?.as_u64()?,
            object.get("destination_id")?.as_u64()?
        ));
    }
    if key.is_empty() {
        let object = item.as_object()?;
        let track = object.get("track_id")?.as_u64()?;
        let pitch = object.get("pitch")?.as_u64()?;
        let velocity = object.get("velocity")?.as_u64()?;
        let start = object.get("start_sample")?.as_u64()?;
        let length = object.get("length_samples")?.as_u64()?;
        return Some(format!("{track}:{pitch}:{velocity}:{start}:{length}"));
    }
    let value = item.get(key)?;
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn field_changes(
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
) -> Vec<FieldChange> {
    let mut names = std::collections::BTreeSet::new();
    for value in [before, after].into_iter().flatten() {
        if let Some(object) = value.as_object() {
            names.extend(object.keys().cloned());
        }
    }
    names
        .into_iter()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "plugin_states"
                    | "plugin_state_hex"
                    | "sandbox_plugin_states"
                    | "sandbox_plugin_state_hex"
                    | "state_blob"
            )
        })
        .filter_map(|field| {
            let before_value = before.and_then(|value| value.get(&field)).cloned();
            let after_value = after.and_then(|value| value.get(&field)).cloned();
            (before_value != after_value).then_some(FieldChange {
                field,
                before: before_value,
                after: after_value,
            })
        })
        .take(32)
        .collect()
}

/// Hash an asset without loading the entire recording into memory. WAV
/// metadata is intentionally best-effort: a valid file may contain a large
/// unknown chunk before `fmt`/`data`, so the content hash remains authoritative
/// even when the bounded metadata probe cannot reach those chunks.
type AssetFingerprintData = (u64, String, Option<u32>, Option<u16>, Option<u64>);

fn fingerprint_asset_file(path: &Path) -> std::io::Result<AssetFingerprintData> {
    const HASH_CHUNK_BYTES: usize = 1024 * 1024;
    let mut file = File::open(path)?;
    let byte_count = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut probe = Vec::with_capacity(HASH_CHUNK_BYTES.min(byte_count as usize));
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    loop {
        let read_bytes = file.read(&mut buffer)?;
        if read_bytes == 0 {
            break;
        }
        hasher.update(&buffer[..read_bytes]);
        if probe.len() < HASH_CHUNK_BYTES {
            let remaining = HASH_CHUNK_BYTES - probe.len();
            probe.extend_from_slice(&buffer[..read_bytes.min(remaining)]);
        }
    }
    let digest = hasher.finalize();
    let hash = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let (sample_rate, channels, frame_count) = wav_metadata(&probe);
    Ok((byte_count, hash, sample_rate, channels, frame_count))
}

fn commit_id_material(commit: &ProjectCommit) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        commit.snapshot_hash,
        commit.parent.as_deref().unwrap_or(""),
        commit.branch,
        commit.message,
        commit.created_unix_seconds,
        commit.project_id,
        commit.project_format_version,
        commit.asset_manifest_hash,
        commit.plugin_manifest_hash,
        commit.platform,
        commit.render_artifact_hash,
        commit.render_artifact_bytes,
    )
}

fn validate_ref_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.len() > 128
        || name != name.trim()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || name.starts_with('.')
    {
        bail!("invalid history ref name");
    }
    Ok(())
}

fn validate_commit_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid commit id");
    }
    Ok(())
}

fn wav_metadata(bytes: &[u8]) -> (Option<u32>, Option<u16>, Option<u64>) {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return (None, None, None);
    }
    let mut cursor = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut block_align = None;
    let mut data_bytes = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        let end = match cursor.checked_add(size) {
            Some(end) if end <= bytes.len() => end,
            _ => break,
        };
        if id == b"fmt " && size >= 16 {
            channels = Some(u16::from_le_bytes(
                bytes[cursor + 2..cursor + 4].try_into().unwrap(),
            ));
            sample_rate = Some(u32::from_le_bytes(
                bytes[cursor + 4..cursor + 8].try_into().unwrap(),
            ));
            block_align = Some(u16::from_le_bytes(
                bytes[cursor + 12..cursor + 14].try_into().unwrap(),
            ));
        } else if id == b"data" {
            data_bytes = Some(size as u64);
        }
        cursor = end + (size & 1);
    }
    let frames = data_bytes
        .zip(block_align)
        .filter(|(_, align)| *align > 0)
        .map(|(data, align)| data / u64::from(align));
    (sample_rate, channels, frames)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("entry"),
        std::process::id(),
        unique_nonce()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    if let Err(error) = rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn remove_published_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unique_nonce() -> u128 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    nanos ^ ((std::process::id() as u128) << 64) ^ sequence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectDocument;
    use crate::project_contracts::OpenUtauVocalContract;

    #[test]
    fn commit_branch_checkout_and_load_are_content_addressed() {
        let root = std::env::temp_dir().join(format!("aura-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectHistoryStore::open(&root).unwrap();
        let project = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, "[]").unwrap();
        let first = store.commit(&project, "initial").unwrap();
        let branch = store.create_branch("experiment", None).unwrap();
        assert_eq!(branch.commit_id, first.commit_id);
        assert_eq!(store.checkout("experiment").unwrap().branch, "experiment");
        assert_eq!(store.load_commit(&first.commit_id).unwrap(), project);
        let _tag = store.tag("v1", None).unwrap();
        assert_eq!(
            store.status().unwrap().snapshot_hash,
            Some(first.snapshot_hash)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_projects_keep_history_next_to_the_file() {
        let parent = std::env::temp_dir().join(format!("aura-history-file-{}", unique_nonce()));
        std::fs::create_dir_all(&parent).unwrap();
        let project_file = parent.join("Song.aura");
        std::fs::write(&project_file, b"placeholder").unwrap();
        let store = ProjectHistoryStore::open(&project_file).unwrap();
        assert!(parent.join(".aura/history").is_dir());
        assert!(!project_file.join(".aura/history").exists());
        let _ = std::fs::remove_dir_all(parent);
        drop(store);
    }

    #[test]
    fn commit_file_reads_working_tree_inside_history_transaction() {
        let root =
            std::env::temp_dir().join(format!("aura-history-commit-file-{}", unique_nonce()));
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let project = ProjectDocument::from_layout_json("Demo", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let commit = store.commit_file(&project_file, "initial").unwrap();
        assert_eq!(store.load_commit(&commit.commit_id).unwrap(), project);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extension_bearing_missing_paths_require_explicit_project_mode() {
        let root = std::env::temp_dir().join(format!("aura-history-ambiguous-{}", unique_nonce()));
        let missing = root.join("Song.aura");
        assert!(ProjectHistoryStore::open(&missing).is_err());
        let store = ProjectHistoryStore::open_project_directory(&missing).unwrap();
        assert!(missing.join(".aura/history").is_dir());
        let _ = std::fs::remove_dir_all(root);
        drop(store);
    }

    #[test]
    fn commit_and_ref_names_are_strictly_validated() {
        assert!(validate_commit_id(&"a".repeat(64)).is_ok());
        assert!(validate_commit_id("short").is_err());
        assert!(validate_commit_id(&"g".repeat(64)).is_err());
        assert!(validate_ref_name("main-2").is_ok());
        assert!(validate_ref_name("../escape").is_err());
        assert!(validate_ref_name("bad name").is_err());
    }

    #[test]
    fn asset_fingerprint_hashes_large_files_without_changing_content_identity() {
        let path = std::env::temp_dir().join(format!("aura-history-asset-{}", unique_nonce()));
        let bytes = vec![0x5au8; 1024 * 1024 + 17];
        std::fs::write(&path, &bytes).unwrap();
        let (size, hash, _, _, _) = fingerprint_asset_file(&path).unwrap();
        assert_eq!(size, bytes.len() as u64);
        assert_eq!(hash, content_hash(&bytes));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn openutau_source_and_rendered_assets_participate_in_commit_identity() {
        let root = std::env::temp_dir().join(format!("aura-history-openutau-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("voice.ustx");
        let rendered = root.join("voice.wav");
        std::fs::write(&source, b"note C4").unwrap();
        std::fs::write(&rendered, b"render-a").unwrap();

        let store = ProjectHistoryStore::open_project_directory(&root).unwrap();
        let mut project =
            ProjectDocument::from_layout_json("Vocal", 120.0, 48_000.0, "[]").unwrap();
        project.openutau_vocals.push(OpenUtauVocalContract {
            source_path: source.to_string_lossy().into_owned(),
            rendered_audio_path: rendered.to_string_lossy().into_owned(),
            singer: "test-singer".into(),
            source_generation: 1,
            source_hash: String::new(),
            rendered_audio_hash: String::new(),
            rendered_audio_bytes: 0,
            rendered_sample_rate: None,
            rendered_channels: None,
            rendered_frames: None,
            source_note_count: 1,
            source_singers: vec!["test-singer".into()],
            tuning: Default::default(),
        });
        let project_file = root.join("Song.aura");
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&project, "vocal-a").unwrap();

        let verified = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(verified["ok"], true);
        assert_eq!(verified["asset_manifest_matches_head"], true);

        std::fs::write(&source, b"note D4").unwrap();
        let changed_asset = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(changed_asset["ok"], false);
        assert_eq!(changed_asset["working_tree_matches_head"], true);
        assert_eq!(changed_asset["asset_manifest_matches_head"], false);
        let second = store.commit(&project, "vocal-b").unwrap();
        assert_ne!(first.asset_manifest_hash, second.asset_manifest_hash);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn status_for_branch_does_not_publish_a_checkout() {
        let root = std::env::temp_dir().join(format!("aura-history-preview-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        let store = ProjectHistoryStore::open(&root).unwrap();
        let project = ProjectDocument::from_layout_json("Preview", 120.0, 48_000.0, "[]").unwrap();
        let initial = store.commit(&project, "initial").unwrap();
        store.create_branch("experiment", None).unwrap();

        let preview = store.status_for_branch("experiment").unwrap();
        assert_eq!(preview.branch, "experiment");
        assert_eq!(preview.head, Some(initial.commit_id));
        assert_eq!(store.status().unwrap().branch, "main");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn verify_working_tree_detects_unsaved_or_wrong_head_state() {
        let root = std::env::temp_dir().join(format!("aura-history-verify-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let project = ProjectDocument::from_layout_json("Verify", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        store.commit(&project, "initial").unwrap();

        let verified = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(verified["ok"], true);
        assert_eq!(verified["working_tree_matches_head"], true);

        let mut unsaved = project.clone();
        unsaved.metadata.name = "Unsaved edit".into();
        unsaved.save_atomic(project_file.to_str().unwrap()).unwrap();
        let diverged = store.verify_working_tree(&project_file).unwrap();
        assert_eq!(diverged["ok"], false);
        assert_eq!(diverged["identity_matches"], true);
        assert_eq!(diverged["working_tree_matches_head"], false);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn diff_reports_entity_level_track_and_plugin_changes() {
        let root = std::env::temp_dir().join(format!("aura-history-diff-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let layout = r#"[{"id":7,"name":"Vocal","plugin_types":[0],"plugin_state_hex":["00"],"regions":[]}]"#;
        let first = ProjectDocument::from_layout_json("Diff", 120.0, 48_000.0, layout).unwrap();
        let first_commit = store.commit(&first, "initial").unwrap();

        let mut second = first.clone();
        second.tracks[0].name = "Lead Vocal".to_owned();
        second.tracks[0].volume = 0.75;
        second.tracks[0].track_delay_samples = 2400;
        second.plugin_instances[0].bypassed = true;
        second
            .freeze_artifacts
            .push(crate::project_contracts::FreezeArtifactContract {
                track_id: 7,
                project_generation: 2,
                audio_generation: 3,
                total_samples: 48_000,
                sample_rate: 48_000,
                path: "freeze/vocal.wav".to_owned(),
                content_checksum: 1,
            });
        let second_commit = store.commit(&second, "vocal edit").unwrap();
        let diff = store
            .diff_commits(&first_commit.commit_id, &second_commit.commit_id)
            .unwrap();

        assert!(diff.changed_sections.contains(&"tracks".to_owned()));
        assert!(diff
            .changed_sections
            .contains(&"plugin_instances".to_owned()));
        assert!(diff
            .changed_sections
            .contains(&"freeze_artifacts".to_owned()));
        assert!(diff.changes.iter().any(|change| {
            change.section == "tracks"
                && change.entity_id.as_deref() == Some("7")
                && change.operation == "changed"
                && change.fields.iter().any(|field| {
                    field.field == "track_delay_samples"
                        && field.before == Some(serde_json::json!(0))
                        && field.after == Some(serde_json::json!(2400))
                })
        }));
        assert!(diff.changes.iter().any(|change| {
            change.section == "plugin_instances"
                && change.entity_id.as_deref() == Some("track:7:slot:0")
                && change.operation == "changed"
        }));
        assert!(diff.changes.iter().any(|change| {
            change.section == "freeze_artifacts"
                && change.entity_id.as_deref() == Some("7")
                && change.operation == "added"
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checkout_and_restore_publishes_head_only_after_project_restore() {
        let root = std::env::temp_dir().join(format!("aura-history-restore-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        let initial = ProjectDocument::from_layout_json("Initial", 120.0, 48_000.0, "[]").unwrap();
        initial.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&initial, "initial").unwrap();

        let mut changed = initial.clone();
        changed.metadata.name = "Changed".into();
        changed.metadata.bpm = 128.0;
        changed.save_atomic(project_file.to_str().unwrap()).unwrap();
        let second = store.commit(&changed, "changed").unwrap();
        assert_ne!(first.commit_id, second.commit_id);

        store.checkout_and_restore(&project_file, "main").unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            changed
        );

        store
            .create_branch("initial-state", Some(&first.commit_id))
            .unwrap();
        let status = store
            .checkout_and_restore(&project_file, "initial-state")
            .unwrap();
        assert_eq!(status.branch, "initial-state");
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_rejects_a_different_project_at_the_same_path() {
        let root = std::env::temp_dir().join(format!("aura-history-identity-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let project = ProjectDocument::from_layout_json("Original", 120.0, 48_000.0, "[]").unwrap();
        project.save_atomic(project_file.to_str().unwrap()).unwrap();
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();
        store.commit(&project, "initial").unwrap();

        let replacement =
            ProjectDocument::from_layout_json("Replacement", 120.0, 48_000.0, "[]").unwrap();
        replacement
            .save_atomic(project_file.to_str().unwrap())
            .unwrap();
        let before = std::fs::read(&project_file).unwrap();
        assert!(store.checkout_and_restore(&project_file, "main").is_err());
        assert_eq!(std::fs::read(&project_file).unwrap(), before);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revert_and_cherry_pick_publish_new_commits_with_matching_working_tree() {
        let root = std::env::temp_dir().join(format!("aura-history-rewrite-{}", unique_nonce()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project_file = root.join("Song.aura");
        let store = ProjectHistoryStore::open_project_file(&project_file).unwrap();

        let initial = ProjectDocument::from_layout_json("Initial", 120.0, 48_000.0, "[]").unwrap();
        initial.save_atomic(project_file.to_str().unwrap()).unwrap();
        let first = store.commit(&initial, "initial").unwrap();
        let mut changed = initial.clone();
        changed.metadata.name = "Changed".into();
        changed.metadata.bpm = 128.0;
        changed.openutau_vocals.push(OpenUtauVocalContract {
            source_path: "voice.ustx".into(),
            rendered_audio_path: "voice.wav".into(),
            singer: "Teto".into(),
            source_generation: 1,
            source_hash: "source".into(),
            rendered_audio_hash: "rendered".into(),
            rendered_audio_bytes: 4,
            rendered_sample_rate: Some(48_000),
            rendered_channels: Some(2),
            rendered_frames: Some(2),
            source_note_count: 1,
            source_singers: vec!["Teto".into()],
            tuning: Default::default(),
        });
        changed.chord_track.push(crate::harmonic::ChordEvent {
            tick: 0,
            root: 60,
            intervals: vec![0, 4, 7],
            name: "C".into(),
        });
        changed.save_atomic(project_file.to_str().unwrap()).unwrap();
        let second = store.commit(&changed, "changed").unwrap();

        let reverted = store
            .revert_and_restore(&project_file, &first.commit_id)
            .unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );
        assert_eq!(
            store.status().unwrap().head,
            Some(reverted.commit_id.clone())
        );
        assert_eq!(reverted.parent, Some(second.commit_id.clone()));

        let merged = store
            .cherry_pick_and_restore(&project_file, &reverted.commit_id, &["metadata".into()])
            .unwrap();
        assert_eq!(
            ProjectDocument::load(project_file.to_str().unwrap()).unwrap(),
            initial
        );
        assert_eq!(store.status().unwrap().head, Some(merged.commit_id));

        let openutau_only = store
            .cherry_pick(&initial, &second.commit_id, &["openutau_vocals".into()])
            .unwrap();
        assert_eq!(openutau_only.openutau_vocals, changed.openutau_vocals);
        let chord_only = store
            .cherry_pick(&initial, &second.commit_id, &["chord_track".into()])
            .unwrap();
        assert_eq!(chord_only.chord_track, changed.chord_track);

        let _ = std::fs::remove_dir_all(root);
    }
}
