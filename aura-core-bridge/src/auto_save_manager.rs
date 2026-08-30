use crate::generation_gate::GenerationGate;
use crate::persistence::{BackupInfo, PersistenceOrchestrator, RecoveryCandidate};
use anyhow::Result;

pub struct AutoSaveOrchestrator {
    pub project_path: String,
    pub is_running: bool,
    pub is_dirty: bool,
    pub max_backups: u32,
    pub last_backup: Option<BackupInfo>,
    /// Monotonic project-content generation. Async save completions must
    /// match this value before they are allowed to clear dirty state.
    pub dirty_generation: u64,
    pending_snapshot: Option<Vec<u8>>,
    /// Canonical invalidation gate shared by every asynchronous save caller.
    /// `dirty_generation` remains public for ABI/UI compatibility, while the
    /// gate is the actual stale-completion authority.
    publication_gate: GenerationGate,
}

impl Default for AutoSaveOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoSaveOrchestrator {
    pub fn new() -> Self {
        Self {
            project_path: String::new(),
            is_running: false,
            is_dirty: false,
            max_backups: 10,
            last_backup: None,
            dirty_generation: 0,
            pending_snapshot: None,
            publication_gate: GenerationGate::new(),
        }
    }

    /// Starts auto-save for a valid project path.
    ///
    /// An empty path cannot identify a project, so it leaves the manager stopped.
    /// Starting a project also begins a new dirty-state lifecycle.
    pub fn start(&mut self, path: String) {
        if path.trim().is_empty() {
            self.stop();
            self.project_path.clear();
            return;
        }

        self.project_path = path;
        self.is_running = true;
        self.is_dirty = false;
        self.dirty_generation = self.publication_gate.begin();
    }

    /// Stops auto-save without discarding the unsaved-state indicator.
    pub fn stop(&mut self) {
        self.is_running = false;
        self.publication_gate.cancel();
    }

    /// Marks the current project as having changes that need saving.
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
        self.dirty_generation = self.publication_gate.begin();
    }

    /// Marks a versioned save attempt as pending.
    ///
    /// This method intentionally does not clear `is_dirty`: without the
    /// serialized project bytes there is no successful disk write to prove.
    /// Callers must use `save_snapshot` to publish an atomic backup and clear
    /// the dirty flag.
    pub fn queue_snapshot(&mut self, data: Vec<u8>) {
        self.pending_snapshot = Some(data);
        self.mark_dirty();
    }

    pub fn perform_versioned_save(&mut self, _index: u32) -> Result<Option<BackupInfo>> {
        if !self.is_running || self.project_path.trim().is_empty() {
            return Ok(None);
        }
        let Some(data) = self.pending_snapshot.take() else {
            return Ok(None);
        };
        self.save_snapshot(&data).map(Some)
    }

    pub fn save_snapshot(&mut self, data: &[u8]) -> Result<BackupInfo> {
        let generation = self.dirty_generation;
        self.save_snapshot_for_generation(data, generation)
    }

    /// Publishes a snapshot only if it was produced from the current dirty
    /// generation. A stale async completion must not clear a newer edit.
    pub fn save_snapshot_for_generation(
        &mut self,
        data: &[u8],
        generation: u64,
    ) -> Result<BackupInfo> {
        if !self.is_running || self.project_path.trim().is_empty() {
            anyhow::bail!("auto-save is not running for a project");
        }
        if generation != self.dirty_generation || !self.publication_gate.accepts(generation) {
            anyhow::bail!(
                "stale auto-save generation: expected {}, current {}",
                generation,
                self.dirty_generation
            );
        }
        let mut persistence = PersistenceOrchestrator::new(self.max_backups);
        let info = persistence.atomic_save_with_backups(&self.project_path, data)?;
        // The write is atomic, but the completion can still be stale if the
        // project was edited while the filesystem operation was in flight.
        // Do not clear the dirty marker or publish the backup as the current
        // snapshot in that case; the next autosave must capture the newer
        // generation.
        if generation != self.dirty_generation || !self.publication_gate.accepts(generation) {
            anyhow::bail!(
                "stale auto-save completion after publish: saved {}, current {}",
                generation,
                self.dirty_generation
            );
        }
        self.last_backup = Some(info.clone());
        self.is_dirty = false;
        Ok(info)
    }

    pub fn recovery_candidates(&self) -> Result<Vec<RecoveryCandidate>> {
        if self.project_path.trim().is_empty() {
            anyhow::bail!("project path is empty");
        }
        PersistenceOrchestrator::recovery_candidates(&self.project_path)
    }

    /// Returns only recovery snapshots that are readable and non-empty, so a
    /// crash-recovery UI never offers a truncated/corrupt candidate.
    pub fn validated_recovery_candidates(&self) -> Result<Vec<RecoveryCandidate>> {
        Ok(self.recovery_candidates()?.into_iter().filter(|candidate| {
            std::fs::metadata(&candidate.path).map(|m| m.is_file() && m.len() > 0 && m.len() <= 512 * 1024 * 1024).unwrap_or(false)
                && std::fs::read(&candidate.path).map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()).unwrap_or(false)
        }).collect())
    }

    /// Restores a validated recovery generation atomically over the project.
    /// The caller must explicitly select the candidate path surfaced by
    /// `validated_recovery_candidates`; corrupt or unrelated files are rejected.
    pub fn restore_recovery_candidate(&mut self, candidate: &RecoveryCandidate) -> Result<()> {
        if self.project_path.trim().is_empty() || candidate.path.as_os_str().is_empty() {
            anyhow::bail!("recovery project or candidate path is empty");
        }
        let candidates = self.validated_recovery_candidates()?;
        if !candidates.iter().any(|entry| entry.path == candidate.path && entry.checksum == candidate.checksum) {
            anyhow::bail!("recovery candidate is not validated");
        }
        let destination = std::path::Path::new(&self.project_path);
        let temp = destination.with_extension("aura-recovery.tmp");
        std::fs::copy(&candidate.path, &temp)?;
        if let Err(error) = std::fs::rename(&temp, destination) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }
        self.is_dirty = false;
        self.pending_snapshot = None;
        self.dirty_generation = self.publication_gate.begin();
        Ok(())
    }

    /// Reports whether the manager is in a valid operational state.
    pub fn audit_auto_save_manager(&self) -> bool {
        self.max_backups > 0
            && self.max_backups <= 10_000
            && (!self.is_running || self.dirty_generation > 0)
            && self.pending_snapshot.as_ref().map(|data| data.len() <= 512 * 1024 * 1024).unwrap_or(true)
            && (!self.is_running || !self.project_path.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::AutoSaveOrchestrator;

    #[test]
    fn start_and_save_manage_dirty_state() {
        let mut manager = AutoSaveOrchestrator::new();
        manager.start("project.logicx".to_owned());
        assert!(manager.is_running);
        manager.mark_dirty();
        let _ = manager.perform_versioned_save(0);
        assert!(manager.is_dirty);
    }

    #[test]
    fn empty_path_does_not_start() {
        let mut manager = AutoSaveOrchestrator::new();
        manager.start("  ".to_owned());
        assert!(!manager.is_running);
        assert!(manager.project_path.is_empty());
    }

    #[test]
    fn stopping_preserves_dirty_state() {
        let mut manager = AutoSaveOrchestrator::new();
        manager.start("project.logicx".to_owned());
        manager.mark_dirty();
        manager.stop();
        let _ = manager.perform_versioned_save(0);
        assert!(manager.is_dirty);
    }

    #[test]
    fn queued_snapshot_is_written_by_versioned_save() {
        let path = std::env::temp_dir().join(format!(
            "aura-autosave-queued-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut manager = AutoSaveOrchestrator::new();
        manager.start(path.to_string_lossy().into_owned());
        manager.queue_snapshot(br#"{"queued":true}"#.to_vec());
        assert!(manager.perform_versioned_save(1).unwrap().is_some());
        assert!(!manager.is_dirty);
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"queued":true}"#);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn snapshot_creates_recovery_candidate_and_clears_dirty_state() {
        let path = std::env::temp_dir().join(format!(
            "aura-autosave-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut manager = AutoSaveOrchestrator::new();
        manager.start(path_string.clone());
        manager.mark_dirty();
        manager.save_snapshot(br#"{"version":1}"#).unwrap();
        assert!(!manager.is_dirty);
        assert!(manager.last_backup.is_some());

        manager.mark_dirty();
        manager.save_snapshot(br#"{"version":2}"#).unwrap();
        let candidates = manager.recovery_candidates().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            std::fs::read(&candidates[0].path).unwrap(),
            br#"{"version":1}"#
        );

        let _ = std::fs::remove_file(&path);
        for candidate in candidates {
            let _ = std::fs::remove_file(candidate.path);
        }
    }

    #[test]
    fn stale_snapshot_generation_cannot_clear_newer_edits() {
        let path = std::env::temp_dir().join(format!(
            "aura-autosave-generation-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_string = path.to_string_lossy().into_owned();
        let mut manager = AutoSaveOrchestrator::new();
        manager.start(path_string.clone());
        manager.mark_dirty();
        let old_generation = manager.dirty_generation;
        manager.mark_dirty();
        assert!(manager
            .save_snapshot_for_generation(br#"{"version":1}"#, old_generation)
            .is_err());
        assert!(manager.is_dirty);
        let _ = std::fs::remove_file(path);
    }
}
