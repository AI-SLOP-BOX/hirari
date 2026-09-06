//! Shared time and parameter contracts for Audio, VFX, and future media cores.
//!
//! The types in this module deliberately contain no DAW or UI concepts. A
//! video frame, audio sample, beat, and timecode are different views of one
//! integer timeline, so cross-domain clients never need to round-trip through
//! floating-point seconds themselves.

use serde::{Deserialize, Serialize};

pub const TIMELINE_API_VERSION: &str = "aura.timeline.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TimelineRate {
    pub sample_rate: f64,
    pub frame_rate: f64,
}

impl TimelineRate {
    pub fn validate(self) -> bool {
        self.sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&self.sample_rate)
            && self.frame_rate.is_finite()
            && (1.0..=240.0).contains(&self.frame_rate)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MasterTick(pub u128);

/// A lossless-enough shared position. `ticks_per_second` is fixed by the
/// production session and should be chosen high enough for sub-frame timing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MasterClock {
    pub tick: MasterTick,
    pub ticks_per_second: u64,
}

impl MasterClock {
    pub fn from_seconds(seconds: f64, ticks_per_second: u64) -> Option<Self> {
        if !seconds.is_finite() || seconds < 0.0 || ticks_per_second == 0 {
            return None;
        }
        Some(Self {
            tick: MasterTick((seconds * ticks_per_second as f64).round() as u128),
            ticks_per_second,
        })
    }

    pub fn seconds(self) -> f64 {
        self.tick.0 as f64 / self.ticks_per_second.max(1) as f64
    }

    pub fn samples(self, sample_rate: f64) -> Option<u64> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return None;
        }
        Some((self.seconds() * sample_rate).round().min(u64::MAX as f64) as u64)
    }

    pub fn frame(self, frame_rate: f64) -> Option<u64> {
        if !frame_rate.is_finite() || frame_rate <= 0.0 {
            return None;
        }
        Some((self.seconds() * frame_rate).floor().min(u64::MAX as f64) as u64)
    }

    pub fn subframe(self, frame_rate: f64) -> Option<f64> {
        if !frame_rate.is_finite() || frame_rate <= 0.0 {
            return None;
        }
        Some((self.seconds() * frame_rate).fract())
    }

    pub fn smpte(self, frame_rate: f64) -> Option<SmpteTimecode> {
        let total_frames = self.seconds() * frame_rate;
        if !frame_rate.is_finite() || frame_rate <= 0.0 || total_frames < 0.0 {
            return None;
        }
        let whole = total_frames.floor() as u64;
        let fps = frame_rate.round() as u64;
        if fps == 0 {
            return None;
        }
        let frames_per_hour = fps * 60 * 60;
        let frames_per_minute = fps * 60;
        Some(SmpteTimecode {
            hours: whole / frames_per_hour,
            minutes: (whole % frames_per_hour) / frames_per_minute,
            seconds: (whole % frames_per_minute) / fps,
            frames: whole % fps,
            subframe: total_frames.fract(),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SmpteTimecode {
    pub hours: u64,
    pub minutes: u64,
    pub seconds: u64,
    pub frames: u64,
    pub subframe: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TempoPoint {
    pub beat: f64,
    pub bpm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TempoMap {
    pub points: Vec<TempoPoint>,
}

impl TempoMap {
    pub fn new(initial_bpm: f64) -> Option<Self> {
        (initial_bpm.is_finite() && (1.0..=999.0).contains(&initial_bpm)).then_some(Self {
            points: vec![TempoPoint {
                beat: 0.0,
                bpm: initial_bpm,
            }],
        })
    }

    pub fn insert(&mut self, point: TempoPoint) -> bool {
        if !point.beat.is_finite()
            || point.beat < 0.0
            || !point.bpm.is_finite()
            || !(1.0..=999.0).contains(&point.bpm)
        {
            return false;
        }
        if let Some(existing) = self.points.iter_mut().find(|item| item.beat == point.beat) {
            *existing = point;
        } else {
            self.points.push(point);
            self.points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        }
        true
    }

    /// Converts a musical beat into seconds using piecewise-constant tempo.
    pub fn beat_to_seconds(&self, beat: f64) -> Option<f64> {
        if !beat.is_finite() || beat < 0.0 || self.points.is_empty() {
            return None;
        }
        let mut seconds = 0.0;
        let mut previous = self.points[0];
        if previous.beat != 0.0 {
            return None;
        }
        for next in self.points.iter().copied().skip(1) {
            if beat <= next.beat {
                return Some(seconds + (beat - previous.beat) * 60.0 / previous.bpm);
            }
            seconds += (next.beat - previous.beat) * 60.0 / previous.bpm;
            previous = next;
        }
        Some(seconds + (beat - previous.beat) * 60.0 / previous.bpm)
    }

    pub fn beat_to_clock(&self, beat: f64, ticks_per_second: u64) -> Option<MasterClock> {
        MasterClock::from_seconds(self.beat_to_seconds(beat)?, ticks_per_second)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterBinding {
    pub source: String,
    pub target: String,
    pub source_unit: String,
    pub target_unit: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BindingValue {
    pub clock: MasterClock,
    pub value: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_clock_maps_to_audio_video_and_smpte_views() {
        let clock = MasterClock::from_seconds(5.25, 1_000_000).unwrap();
        assert_eq!(clock.samples(48_000.0), Some(252_000));
        assert_eq!(clock.frame(24.0), Some(126));
        assert!((clock.subframe(24.0).unwrap() - 0.0).abs() < 1e-9);
        assert_eq!(clock.smpte(24.0).unwrap().seconds, 5);
    }

    #[test]
    fn timeline_rate_rejects_non_audio_sample_rates() {
        assert!(!TimelineRate {
            sample_rate: 1.0,
            frame_rate: 24.0,
        }
        .validate());
        assert!(TimelineRate {
            sample_rate: 48_000.0,
            frame_rate: 24.0,
        }
        .validate());
    }

    #[test]
    fn tempo_changes_are_visible_to_video_clients() {
        let mut map = TempoMap::new(120.0).unwrap();
        assert!(map.insert(TempoPoint {
            beat: 4.0,
            bpm: 60.0
        }));
        assert!((map.beat_to_seconds(6.0).unwrap() - 4.0).abs() < 1e-9);
        assert_eq!(
            map.beat_to_clock(6.0, 1_000_000).unwrap().samples(48_000.0),
            Some(192_000)
        );
    }

    #[test]
    fn bindings_are_domain_neutral() {
        let binding = ParameterBinding {
            source: "audio.synth.cutoff".into(),
            target: "vfx.glow.intensity".into(),
            source_unit: "normalized".into(),
            target_unit: "normalized".into(),
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(json.contains("vfx.glow.intensity"));
    }
}
