//! Projection of shared production time into a VFX timeline.
//!
//! This is intentionally a pure Project/VFX module: it consumes the stable
//! timeline/event contracts and produces visual cues, without depending on the
//! Audio Engine implementation.

use crate::production_events::ProductionEvent;
use crate::production_timeline::{MasterClock, TempoMap};
use serde::{Deserialize, Serialize};

pub const VFX_TIMELINE_API_VERSION: &str = "aura.vfx-timeline.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VfxCue {
    pub id: String,
    pub clock: MasterClock,
    pub kind: VfxCueKind,
    pub intensity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VfxCueKind {
    Beat,
    Marker,
    AudioEvent,
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BeatGridSpec {
    pub start_beat: f64,
    pub end_beat: f64,
    pub subdivision: u32,
    pub ticks_per_second: u64,
}

impl BeatGridSpec {
    pub fn validate(self) -> bool {
        self.start_beat.is_finite()
            && self.end_beat.is_finite()
            && self.start_beat >= 0.0
            && self.end_beat > self.start_beat
            && self.end_beat - self.start_beat <= 1_000_000.0
            && (1..=64).contains(&self.subdivision)
            && self.ticks_per_second > 0
    }
}

pub fn beat_grid(map: &TempoMap, spec: BeatGridSpec) -> Option<Vec<VfxCue>> {
    if !spec.validate() {
        return None;
    }
    let mut cues = Vec::new();
    let mut index = 0u64;
    let step = 1.0 / spec.subdivision as f64;
    let mut beat = spec.start_beat;
    while beat < spec.end_beat {
        let clock = map.beat_to_clock(beat, spec.ticks_per_second)?;
        cues.push(VfxCue {
            id: format!("beat-{index}"),
            clock,
            kind: VfxCueKind::Beat,
            intensity: if (beat.fract()).abs() < f64::EPSILON {
                1.0
            } else {
                0.5
            },
        });
        index = index.checked_add(1)?;
        beat += step;
        if cues.len() > 4_000_000 {
            return None;
        }
    }
    Some(cues)
}

pub fn cue_from_event(event: &ProductionEvent, clock: MasterClock) -> Option<VfxCue> {
    let (id, kind, intensity) = match event {
        ProductionEvent::MarkerChanged { marker_id } => {
            (format!("marker-{marker_id}"), VfxCueKind::Marker, 1.0)
        }
        ProductionEvent::TrackGainChanged { track_id, gain } => (
            format!("track-gain-{track_id}"),
            VfxCueKind::AudioEvent,
            f64::from(gain.abs()).clamp(0.0, 1.0),
        ),
        ProductionEvent::AutomationChanged { target } => (
            format!("automation-{}", target.replace(['/', ' ', '\0'], "_")),
            VfxCueKind::AudioEvent,
            1.0,
        ),
        ProductionEvent::PlayheadMoved { .. }
        | ProductionEvent::TempoMapChanged
        | ProductionEvent::TrackAdded { .. }
        | ProductionEvent::TrackRemoved { .. }
        | ProductionEvent::RegionMoved { .. }
        | ProductionEvent::PluginAdded { .. }
        | ProductionEvent::TransportStarted
        | ProductionEvent::TransportStopped => return None,
        ProductionEvent::Custom { domain, name, .. } => (
            format!("{domain}-{name}").replace(['/', ' ', '\0'], "_"),
            VfxCueKind::Custom,
            1.0,
        ),
    };
    Some(VfxCue {
        id,
        clock,
        kind,
        intensity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::production_timeline::{MasterTick, TempoPoint};

    #[test]
    fn tempo_map_projects_to_a_video_beat_grid() {
        let map = TempoMap::new(120.0).unwrap();
        let cues = beat_grid(
            &map,
            BeatGridSpec {
                start_beat: 0.0,
                end_beat: 2.0,
                subdivision: 2,
                ticks_per_second: 1_000,
            },
        )
        .unwrap();
        assert_eq!(cues.len(), 4);
        assert_eq!(cues[0].clock.tick, MasterTick(0));
        assert_eq!(cues[2].clock.tick, MasterTick(1_000));
    }

    #[test]
    fn tempo_changes_shift_later_visual_cues() {
        let mut map = TempoMap::new(120.0).unwrap();
        map.insert(TempoPoint {
            beat: 2.0,
            bpm: 60.0,
        });
        let cues = beat_grid(
            &map,
            BeatGridSpec {
                start_beat: 0.0,
                end_beat: 4.0,
                subdivision: 1,
                ticks_per_second: 1_000,
            },
        )
        .unwrap();
        assert_eq!(cues[2].clock.tick, MasterTick(1_000));
        assert_eq!(cues[3].clock.tick, MasterTick(2_000));
    }

    #[test]
    fn audio_automation_event_becomes_visual_cue() {
        let event = ProductionEvent::AutomationChanged {
            target: "audio/synth cutoff".into(),
        };
        let cue = cue_from_event(
            &event,
            MasterClock {
                tick: MasterTick(12),
                ticks_per_second: 1_000,
            },
        )
        .unwrap();
        assert_eq!(cue.kind, VfxCueKind::AudioEvent);
        assert_eq!(cue.id, "automation-audio_synth_cutoff");
    }
}
