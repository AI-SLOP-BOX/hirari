use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackTransferBundle {
    pub source_project: String,
    pub tracks: Vec<TrackTransferItem>,
    pub dependencies: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackTransferItem {
    pub track_id: u32,
    pub name: String,
    pub kind: String,
    pub payload: Vec<u8>,
}
impl TrackTransferBundle {
    pub fn validate(&self) -> bool {
        !self.source_project.trim().is_empty()
            && self.source_project.len() <= 256
            && !self.source_project.contains('\0')
            && !self.tracks.is_empty()
            && self.tracks.len() <= 65_536
            && self.tracks.iter().all(|t| {
                t.track_id != 0
                    && !t.name.trim().is_empty()
                    && t.name.len() <= 256
                    && !t.name.contains('\0')
                    && !t.kind.trim().is_empty()
                    && t.kind.len() <= 64
                    && !t.kind.contains('\0')
                    && t.payload.len() <= 64 * 1024 * 1024
            })
            && self
                .tracks
                .iter()
                .enumerate()
                .all(|(i, t)| self.tracks[..i].iter().all(|p| p.track_id != t.track_id))
            && self.dependencies.len() <= 1_000_000
            && self
                .dependencies
                .iter()
                .all(|p| !p.trim().is_empty() && p.len() <= 4096 && !p.contains('\0'))
    }
    pub fn track_ids(&self) -> Vec<u32> {
        let mut ids: Vec<_> = self.tracks.iter().map(|t| t.track_id).collect();
        ids.sort_unstable();
        ids
    }
    pub fn select_tracks(&self, ids: &[u32]) -> Result<Self, String> {
        if ids.is_empty() || ids.contains(&0) || ids.windows(2).any(|w| w[0] == w[1]) {
            return Err("invalid track selection".into());
        }
        let mut tracks = Vec::with_capacity(ids.len());
        for id in ids {
            tracks.push(
                self.tracks
                    .iter()
                    .find(|track| track.track_id == *id)
                    .cloned()
                    .ok_or_else(|| "track not found".to_owned())?,
            );
        }
        let bundle = Self {
            source_project: self.source_project.clone(),
            tracks,
            dependencies: self.dependencies.clone(),
        };
        bundle
            .validate()
            .then_some(bundle)
            .ok_or_else(|| "invalid selected bundle".into())
    }
    pub fn merge(&self, other: &Self) -> Result<Self, String> {
        if self.source_project != other.source_project {
            return Err("source projects differ".into());
        }
        let mut tracks = self.tracks.clone();
        tracks.extend(other.tracks.clone());
        let mut dependencies = self.dependencies.clone();
        dependencies.extend(other.dependencies.clone());
        let bundle = Self {
            source_project: self.source_project.clone(),
            tracks,
            dependencies,
        };
        bundle
            .validate()
            .then_some(bundle)
            .ok_or_else(|| "duplicate or invalid merged tracks".into())
    }
    pub fn remap_track_ids(&self, mapping: &[(u32, u32)]) -> Result<Self, String> {
        let mut out = self.clone();
        for track in &mut out.tracks {
            if let Some((_, replacement)) =
                mapping.iter().find(|(source, _)| *source == track.track_id)
            {
                if *replacement == 0 {
                    return Err("invalid remapped track id".into());
                }
                track.track_id = *replacement;
            }
        }
        out.validate()
            .then_some(out)
            .ok_or_else(|| "duplicate or invalid remapped tracks".into())
    }
    pub fn dependency_paths(&self) -> Vec<String> {
        let mut p = self.dependencies.clone();
        p.sort();
        p.dedup();
        p
    }
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid track transfer bundle".into());
        }
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        let b: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if !b.validate() {
            Err("invalid track transfer bundle".into())
        } else {
            Ok(b)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let b = TrackTransferBundle {
            source_project: "demo".into(),
            tracks: vec![TrackTransferItem {
                track_id: 2,
                name: "Bass".into(),
                kind: "audio".into(),
                payload: vec![1],
            }],
            dependencies: vec!["a".into(), "a".into()],
        };
        assert_eq!(
            TrackTransferBundle::from_json(&b.to_json().unwrap()).unwrap(),
            b
        );
        assert_eq!(b.dependency_paths(), vec!["a"]);
    }
    #[test]
    fn selection_rejects_missing_and_preserves_order() {
        let b = TrackTransferBundle {
            source_project: "demo".into(),
            tracks: vec![
                TrackTransferItem {
                    track_id: 1,
                    name: "A".into(),
                    kind: "audio".into(),
                    payload: vec![],
                },
                TrackTransferItem {
                    track_id: 2,
                    name: "B".into(),
                    kind: "midi".into(),
                    payload: vec![],
                },
            ],
            dependencies: vec![],
        };
        assert_eq!(b.select_tracks(&[2, 1]).unwrap().track_ids(), vec![1, 2]);
        assert!(b.select_tracks(&[3]).is_err());
    }
}
