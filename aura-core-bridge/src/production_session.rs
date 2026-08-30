//! Optional cross-media session sidecar for Project/VFX workflows.
//!
//! The main `.aura` document remains backward compatible. This sidecar stores
//! shared timeline, VFX bindings, cues, and production revision metadata so
//! older Audio projects can be opened without knowing about VFX features.

use crate::production_timeline::{ParameterBinding, TempoMap, TimelineRate};
use crate::vfx_bindings::VfxBindingGraph;
use crate::vfx_timeline_bridge::VfxCue;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PRODUCTION_SESSION_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProductionRevision {
    pub revision: u64,
    pub message: String,
    pub author: String,
    pub timestamp_unix: i64,
    pub audio_snapshot: Option<String>,
    pub visual_snapshot: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProductionSession {
    pub schema_version: u32,
    pub project_id: String,
    pub rate: TimelineRate,
    pub tempo_map: TempoMap,
    #[serde(default)]
    pub bindings: VfxBindingGraph,
    #[serde(default)]
    pub cues: Vec<VfxCue>,
    #[serde(default)]
    pub revisions: Vec<ProductionRevision>,
    #[serde(default)]
    pub external_bindings: Vec<ParameterBinding>,
}

impl ProductionSession {
    pub fn new(
        project_id: impl Into<String>,
        rate: TimelineRate,
        initial_bpm: f64,
    ) -> Option<Self> {
        if !rate.validate() {
            return None;
        }
        Some(Self {
            schema_version: PRODUCTION_SESSION_VERSION,
            project_id: project_id.into(),
            rate,
            tempo_map: TempoMap::new(initial_bpm)?,
            bindings: VfxBindingGraph::default(),
            cues: Vec::new(),
            revisions: Vec::new(),
            external_bindings: Vec::new(),
        })
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PRODUCTION_SESSION_VERSION {
            anyhow::bail!(
                "unsupported production session schema {}",
                self.schema_version
            );
        }
        if self.project_id.trim().is_empty() || self.project_id.len() > 256 {
            anyhow::bail!("production project id is invalid");
        }
        if !self.rate.validate()
            || self.tempo_map.points.is_empty()
            || self.tempo_map.points[0].beat != 0.0
        {
            anyhow::bail!("production timeline is invalid");
        }
        if self.cues.len() > 4_000_000
            || self.revisions.len() > 1_000_000
            || self.external_bindings.len() > 65_536
        {
            anyhow::bail!("production session exceeds bounds");
        }
        if !self.bindings.curves.iter().all(|curve| curve.validate()) {
            anyhow::bail!("production binding graph is invalid");
        }
        Ok(())
    }

    pub fn sidecar_path(project_path: impl AsRef<Path>) -> PathBuf {
        let path = project_path.as_ref();
        PathBuf::from(format!("{}.production.json", path.to_string_lossy()))
    }

    pub fn save_sidecar(&self, project_path: impl AsRef<Path>) -> Result<PathBuf> {
        self.validate()?;
        let path = Self::sidecar_path(project_path);
        let temp = path.with_extension("production.json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write production sidecar {}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("publish production sidecar {}", path.display()))?;
        Ok(path)
    }

    pub fn load_sidecar(project_path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = Self::sidecar_path(project_path);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read production sidecar {}", path.display()))?;
        let session: Self = serde_json::from_slice(&bytes).context("decode production sidecar")?;
        session.validate()?;
        Ok(Some(session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_round_trip_preserves_cross_media_state() {
        let root =
            std::env::temp_dir().join(format!("aura-production-session-{}", std::process::id()));
        let project = root.join("song.aura");
        std::fs::create_dir_all(&root).unwrap();
        let mut session = ProductionSession::new(
            "song",
            TimelineRate {
                sample_rate: 48_000.0,
                frame_rate: 23.976,
            },
            120.0,
        )
        .unwrap();
        session.revisions.push(ProductionRevision {
            revision: 1,
            message: "initial audio+visual lock".into(),
            author: "test".into(),
            timestamp_unix: 0,
            audio_snapshot: Some("audio-hash".into()),
            visual_snapshot: Some("vfx-hash".into()),
        });
        let path = session.save_sidecar(&project).unwrap();
        let restored = ProductionSession::load_sidecar(&project).unwrap().unwrap();
        assert_eq!(restored, session);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn missing_sidecar_is_backward_compatible() {
        let project =
            std::env::temp_dir().join(format!("aura-no-production-{}.aura", std::process::id()));
        assert!(ProductionSession::load_sidecar(project).unwrap().is_none());
    }
}
