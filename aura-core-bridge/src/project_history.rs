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

include!("project_history_lock.rs");

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

}
include!("project_history_operations.rs");

include!("project_history_helpers.rs");

#[cfg(test)]
#[path = "project_history_tests.rs"]
mod project_history_tests;
