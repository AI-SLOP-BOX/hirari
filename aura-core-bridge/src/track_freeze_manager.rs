use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreezeArtifact {
    pub track_id: u32,
    pub project_generation: u64,
    pub audio_generation: u64,
    pub total_samples: u64,
    pub sample_rate: u32,
    pub path: PathBuf,
    pub content_checksum: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeState {
    NotFrozen,
    Planned,
    Ready,
}

/// Control-plane freeze registry. The canonical renderer produces the file;
/// this registry owns its identity, publication, and invalidation rules.
#[derive(Debug, Default)]
pub struct FreezeOrchestrator {
    artifacts: HashMap<u32, FreezeArtifact>,
    planned: HashMap<u32, (u64, u32)>,
}

impl FreezeOrchestrator {
    pub fn new() -> Self { Self::default() }

    /// Compatibility planning API. It deliberately does not claim that audio
    /// was rendered until publish_rendered_artifact succeeds.
    pub fn freeze_track(&mut self, track_id: u32, total_samples: u64, sample_rate: u32) {
        if track_id == 0 || total_samples == 0 || sample_rate == 0 { return; }
        self.invalidate_track(track_id);
        self.planned.insert(track_id, (total_samples, sample_rate));
    }

    pub fn publish_rendered_artifact(
        &mut self, track_id: u32, path: impl AsRef<Path>, project_generation: u64,
        audio_generation: u64, total_samples: u64, sample_rate: u32,
    ) -> Result<(), String> {
        if track_id == 0 || project_generation == 0 || audio_generation == 0
            || total_samples == 0 || sample_rate == 0 {
            return Err("invalid freeze artifact metadata".into());
        }
        let path = path.as_ref();
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("freeze artifact is unavailable: {error}"))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("freeze artifact must be a non-empty regular file".into());
        }
        let bytes = std::fs::read(path)
            .map_err(|error| format!("freeze artifact cannot be read: {error}"))?;
        let content_checksum = crate::persistence::PersistenceOrchestrator::calculate_checksum(&bytes);
        self.artifacts.insert(track_id, FreezeArtifact {
            track_id, project_generation, audio_generation, total_samples, sample_rate,
            path: path.to_path_buf(), content_checksum,
        });
        self.planned.remove(&track_id);
        Ok(())
    }

    pub fn unfreeze_track(&mut self, track_id: u32) { self.invalidate_track(track_id); }

    pub fn invalidate_track(&mut self, track_id: u32) {
        self.artifacts.remove(&track_id);
        self.planned.remove(&track_id);
    }

    pub fn invalidate_stale(&mut self, project_generation: u64, audio_generation: u64) {
        self.artifacts.retain(|_, artifact| {
            artifact.project_generation == project_generation
                && artifact.audio_generation == audio_generation
        });
    }

    pub fn state(&self, track_id: u32) -> FreezeState {
        if self.artifacts.contains_key(&track_id) { FreezeState::Ready }
        else if self.planned.contains_key(&track_id) { FreezeState::Planned }
        else { FreezeState::NotFrozen }
    }

    pub fn artifact(&self, track_id: u32) -> Option<&FreezeArtifact> { self.artifacts.get(&track_id) }

    pub fn frozen_tracks(&self) -> impl Iterator<Item = u32> + '_ {
        self.artifacts.keys().copied()
    }

    pub fn audit_track_freeze_manager(&self) -> bool {
        self.artifacts.iter().all(|(track_id, artifact)| {
            *track_id != 0 && artifact.track_id == *track_id
                && artifact.project_generation != 0 && artifact.audio_generation != 0
                && artifact.total_samples != 0 && artifact.sample_rate != 0
                && artifact.path.is_file()
                && std::fs::read(&artifact.path)
                    .map(|bytes| crate::persistence::PersistenceOrchestrator::calculate_checksum(&bytes)
                        == artifact.content_checksum).unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_does_not_claim_a_rendered_freeze() {
        let mut manager = FreezeOrchestrator::new();
        manager.freeze_track(7, 48_000, 48_000);
        assert_eq!(manager.state(7), FreezeState::Planned);
        assert!(manager.artifact(7).is_none());
        assert!(manager.audit_track_freeze_manager());
    }

    #[test]
    fn publishes_and_detects_mutation() {
        let path = std::env::temp_dir().join(format!("aura-freeze-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"RIFF-aura-freeze-test").unwrap();
        let mut manager = FreezeOrchestrator::new();
        manager.publish_rendered_artifact(2, &path, 3, 4, 1024, 48_000).unwrap();
        assert_eq!(manager.state(2), FreezeState::Ready);
        assert!(manager.audit_track_freeze_manager());
        std::fs::write(&path, b"mutated").unwrap();
        assert!(!manager.audit_track_freeze_manager());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn generation_change_invalidates_old_artifacts() {
        let path = std::env::temp_dir().join(format!("aura-freeze-generation-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"freeze").unwrap();
        let mut manager = FreezeOrchestrator::new();
        manager.publish_rendered_artifact(1, &path, 10, 20, 1, 48_000).unwrap();
        manager.invalidate_stale(11, 20);
        assert_eq!(manager.state(1), FreezeState::NotFrozen);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_artifact_is_not_published() {
        let mut manager = FreezeOrchestrator::new();
        assert!(manager.publish_rendered_artifact(1, "/missing/freeze.wav", 1, 1, 1, 48_000).is_err());
        assert_eq!(manager.state(1), FreezeState::NotFrozen);
    }
}
