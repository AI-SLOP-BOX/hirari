use crate::tempo::TempoOrchestrator;

/// Sample clock used by the engine control plane.
///
/// This is the Rust replacement for the unused C++ `EngineClock`. Playhead
/// advancement preserves its sample-rate ratio behavior; project beat
/// conversions use the same tempo-map implementation as the Rust engine.
pub struct EngineClockOrchestrator {
    quantum_playhead: f64,
    nominal_rate: f64,
    effective_rate: f64,
    tempo: TempoOrchestrator,
}

impl Default for EngineClockOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineClockOrchestrator {
    pub fn new() -> Self {
        Self {
            quantum_playhead: 0.0,
            nominal_rate: 48_000.0,
            effective_rate: 48_000.0,
            tempo: TempoOrchestrator::new(),
        }
    }

    /// Advances the playhead by the effective-to-nominal hardware-rate ratio.
    pub fn advance(&mut self, samples: u64) {
        if !self.nominal_rate.is_finite()
            || self.nominal_rate <= 0.0
            || !self.effective_rate.is_finite()
            || self.effective_rate < 0.0
        {
            return;
        }
        self.quantum_playhead += samples as f64 * (self.effective_rate / self.nominal_rate);
    }

    pub fn get_sub_sample_offset(&self) -> f64 {
        self.quantum_playhead - self.quantum_playhead.floor()
    }

    pub fn set_playhead(&mut self, beats: f64) {
        if beats.is_finite() {
            self.quantum_playhead = beats;
        }
    }

    pub fn get_current_beats(&self) -> f64 {
        self.quantum_playhead
    }

    pub fn get_current_sample(&self) -> u64 {
        self.quantum_playhead.floor().max(0.0) as u64
    }

    pub fn set_hardware_rate(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        self.nominal_rate = sample_rate;
        if self.effective_rate == 0.0 {
            self.effective_rate = sample_rate;
        }
    }

    pub fn set_effective_rate(&mut self, sample_rate: f64) {
        if sample_rate.is_finite() && sample_rate >= 0.0 {
            self.effective_rate = sample_rate;
        }
    }

    pub fn samples_to_beats(&self, samples: u64) -> f64 {
        self.tempo.samples_to_beats(samples, self.nominal_rate)
    }

    pub fn beats_to_samples(&self, beats: f64) -> u64 {
        self.tempo.beats_to_samples(beats, self.nominal_rate)
    }

    pub fn get_musical_time(&self, samples: u64) -> MusicalTime {
        let beats = self.samples_to_beats(samples);
        let total_ticks = (beats * MusicalTime::TICKS_PER_BEAT as f64)
            .floor()
            .clamp(0.0, i64::MAX as f64) as i64;
        let ticks_per_bar = MusicalTime::TICKS_PER_BEAT * MusicalTime::BEATS_PER_BAR;
        let ticks_per_sixteenth = MusicalTime::TICKS_PER_BEAT / MusicalTime::SIXTEENTHS_PER_BEAT;
        MusicalTime {
            bar: (total_ticks / ticks_per_bar + 1).min(i32::MAX as i64) as i32,
            beat: ((total_ticks % ticks_per_bar) / MusicalTime::TICKS_PER_BEAT + 1) as i32,
            sixteenth: ((total_ticks % MusicalTime::TICKS_PER_BEAT) / ticks_per_sixteenth + 1)
                as i32,
            tick: (total_ticks % ticks_per_sixteenth) as i32,
            total_ticks,
            total_beats: beats,
        }
    }

    pub fn tempo_map_mut(&mut self) -> &mut TempoOrchestrator {
        &mut self.tempo
    }

    pub fn audit_engine_clock(&self) -> bool {
        self.quantum_playhead.is_finite()
            && self.quantum_playhead >= 0.0
            && self.nominal_rate.is_finite()
            && self.nominal_rate > 0.0
            && self.effective_rate.is_finite()
            && self.effective_rate >= 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicalTime {
    pub bar: i32,
    pub beat: i32,
    pub sixteenth: i32,
    pub tick: i32,
    pub total_ticks: i64,
    pub total_beats: f64,
}

impl MusicalTime {
    pub const TICKS_PER_BEAT: i64 = 960;
    pub const BEATS_PER_BAR: i64 = 4;
    pub const SIXTEENTHS_PER_BEAT: i64 = 4;
}

#[cfg(test)]
mod tests {
    use super::{EngineClockOrchestrator, MusicalTime};

    #[test]
    fn advances_with_effective_rate_and_keeps_fractional_position() {
        let mut clock = EngineClockOrchestrator::new();
        clock.set_effective_rate(48_024.0);
        clock.advance(48_000);
        assert!((clock.get_current_beats() - 48_024.0).abs() < 1.0e-9);
        assert_eq!(clock.get_current_sample(), 48_024);
        assert_eq!(clock.get_sub_sample_offset(), 0.0);
        clock.set_playhead(12.75);
        assert_eq!(clock.get_sub_sample_offset(), 0.75);
    }

    #[test]
    fn invalid_rate_updates_are_ignored_and_playhead_stays_finite() {
        let mut clock = EngineClockOrchestrator::new();
        clock.set_hardware_rate(f64::NAN);
        clock.set_effective_rate(f64::INFINITY);
        clock.advance(128);
        assert_eq!(clock.get_current_beats(), 128.0);
        assert!(clock.audit_engine_clock());
    }

    #[test]
    fn tempo_map_drives_beat_and_bar_conversion() {
        let clock = EngineClockOrchestrator::new();
        assert_eq!(clock.samples_to_beats(96_000), 4.0);
        assert_eq!(clock.beats_to_samples(8.0), 192_000);
        let position = clock.get_musical_time(96_000);
        assert_eq!(position.bar, 2);
        assert_eq!(position.beat, 1);
        assert_eq!(position.sixteenth, 1);
        assert_eq!(position.tick, 0);
        assert_eq!(position.total_ticks, 4 * MusicalTime::TICKS_PER_BEAT);
    }
}
