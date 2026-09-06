//! Host-side ARA2 protocol contract.
//!
//! This is deliberately SDK-neutral: it gives the Core a deterministic
//! document/analysis/note-segment lifecycle that an ARA2 SDK adapter can bind
//! to later, without pretending that the proprietary SDK is present.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Ara2NoteSegment {
    pub id: String,
    pub start_sample: u64,
    pub end_sample: u64,
    pub pitch: f32,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub formant: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum Ara2AnalysisState {
    Requested,
    Running,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Ara2AnalysisRequest {
    pub id: String,
    pub kind: String,
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Ara2DocumentSnapshot {
    pub plugin_id: String,
    pub region_id: String,
    pub sample_rate: f64,
    pub channels: u32,
    pub sample_count: u64,
    pub analyses: BTreeMap<String, Ara2AnalysisState>,
    pub note_segments: Vec<Ara2NoteSegment>,
}

#[derive(Debug, Default)]
pub struct Ara2ProtocolEndpoint {
    document: Option<Ara2DocumentSnapshot>,
}

impl Ara2ProtocolEndpoint {
    pub fn bind_document(
        &mut self,
        plugin_id: &str,
        region_id: &str,
        sample_rate: f64,
        channels: u32,
        sample_count: u64,
    ) -> Result<(), String> {
        if plugin_id.trim().is_empty() || region_id.trim().is_empty() {
            return Err("plugin_id and region_id are required".into());
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample_rate must be positive".into());
        }
        if channels == 0 || channels > 32 {
            return Err("channels must be between 1 and 32".into());
        }
        self.document = Some(Ara2DocumentSnapshot {
            plugin_id: plugin_id.to_owned(),
            region_id: region_id.to_owned(),
            sample_rate,
            channels,
            sample_count,
            analyses: BTreeMap::new(),
            note_segments: Vec::new(),
        });
        Ok(())
    }

    pub fn unbind_document(&mut self) {
        self.document = None;
    }

    pub fn request_analysis(&mut self, request: Ara2AnalysisRequest) -> Result<(), String> {
        let document = self.document.as_mut().ok_or("ARA2 document is not bound")?;
        if request.id.trim().is_empty() || request.kind.trim().is_empty() {
            return Err("analysis id and kind are required".into());
        }
        if request.start_sample >= request.end_sample || request.end_sample > document.sample_count
        {
            return Err("analysis range is outside the bound region".into());
        }
        document
            .analyses
            .insert(request.id, Ara2AnalysisState::Requested);
        Ok(())
    }

    pub fn set_analysis_state(&mut self, id: &str, state: Ara2AnalysisState) -> Result<(), String> {
        let document = self.document.as_mut().ok_or("ARA2 document is not bound")?;
        if !document.analyses.contains_key(id) {
            return Err("analysis request does not exist".into());
        }
        document.analyses.insert(id.to_owned(), state);
        Ok(())
    }

    pub fn set_note_segments(&mut self, mut segments: Vec<Ara2NoteSegment>) -> Result<(), String> {
        let document = self.document.as_mut().ok_or("ARA2 document is not bound")?;
        segments.sort_by_key(|segment| segment.start_sample);
        for segment in &segments {
            if segment.id.trim().is_empty()
                || segment.start_sample >= segment.end_sample
                || segment.end_sample > document.sample_count
                || !segment.pitch.is_finite()
                || !segment.gain_db.is_finite()
                || !segment.formant.is_finite()
            {
                return Err("invalid ARA2 note segment".into());
            }
        }
        document.note_segments = segments;
        Ok(())
    }

    pub fn snapshot(&self) -> Option<Ara2DocumentSnapshot> {
        self.document.clone()
    }

    pub fn read_range(&self, start_sample: u64, frames: u32) -> Result<(u64, u32), String> {
        let document = self.document.as_ref().ok_or("ARA2 document is not bound")?;
        let end = start_sample
            .checked_add(u64::from(frames))
            .ok_or("read range overflow")?;
        if start_sample >= document.sample_count || end > document.sample_count {
            return Err("read range is outside the bound region".into());
        }
        Ok((start_sample, frames))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_document_analysis_and_note_lifecycle() {
        let mut endpoint = Ara2ProtocolEndpoint::default();
        endpoint
            .bind_document("melodyne", "vox-1", 48_000.0, 2, 48_000)
            .unwrap();
        endpoint
            .request_analysis(Ara2AnalysisRequest {
                id: "pitch".into(),
                kind: "pitch".into(),
                start_sample: 0,
                end_sample: 24_000,
            })
            .unwrap();
        endpoint
            .set_analysis_state("pitch", Ara2AnalysisState::Ready)
            .unwrap();
        endpoint
            .set_note_segments(vec![Ara2NoteSegment {
                id: "n1".into(),
                start_sample: 100,
                end_sample: 1_000,
                pitch: 60.0,
                gain_db: 0.0,
                formant: 1.0,
            }])
            .unwrap();
        assert_eq!(endpoint.read_range(0, 512).unwrap(), (0, 512));
        assert_eq!(
            endpoint.snapshot().unwrap().analyses["pitch"],
            Ara2AnalysisState::Ready
        );
    }
}
