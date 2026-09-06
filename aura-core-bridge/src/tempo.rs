#[derive(Clone, Debug, PartialEq)]
pub struct TempoEvent {
    pub sample_pos: u64,
    pub bpm: f64,
    pub ramp: bool,
    pub world_beats: f64,
}

pub struct TempoOrchestrator {
    pub events: Vec<TempoEvent>,
    pub tap_tempo: TapTempo,
}

#[derive(Default, Debug, Clone)]
pub struct TapTempo {
    taps: Vec<u64>,
}
impl TapTempo {
    pub fn tap(&mut self, timestamp_ms: u64) -> Option<f64> {
        if let Some(&last) = self.taps.last() {
            if timestamp_ms <= last {
                return None;
            }
            let interval = timestamp_ms - last;
            if !(200..=4000).contains(&interval) {
                self.taps.clear();
            }
        }
        self.taps.push(timestamp_ms);
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        if self.taps.len() < 2 {
            return None;
        }
        let mut intervals: Vec<u64> = self
            .taps
            .windows(2)
            .map(|window| window[1] - window[0])
            .collect();
        intervals.sort_unstable();
        let median = intervals[intervals.len() / 2] as f64;
        Some((60_000.0 / median).clamp(20.0, 300.0))
    }
    pub fn clear(&mut self) {
        self.taps.clear();
    }
}

impl Default for TempoOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TempoOrchestrator {
    pub fn new() -> Self {
        Self {
            events: vec![TempoEvent {
                sample_pos: 0,
                bpm: 120.0,
                ramp: false,
                world_beats: 0.0,
            }],
            tap_tempo: TapTempo::default(),
        }
    }

    pub fn upsert_event(&mut self, event: TempoEvent) -> bool {
        if event.bpm.is_finite()
            && (20.0..=999.0).contains(&event.bpm)
            && event.world_beats.is_finite()
            && event.world_beats >= 0.0
        {
            if let Some(old) = self
                .events
                .iter_mut()
                .find(|e| e.sample_pos == event.sample_pos)
            {
                *old = event;
            } else {
                self.events.push(event);
            }
            self.events.sort_by_key(|e| e.sample_pos);
            true
        } else {
            false
        }
    }
    pub fn remove_event(&mut self, sample_pos: u64) -> bool {
        if sample_pos == 0 {
            return false;
        }
        let n = self.events.len();
        self.events.retain(|e| e.sample_pos != sample_pos);
        n != self.events.len()
    }

    /// Moves a tempo-map node during time-warp editing while preserving the
    /// unique, strictly ordered event invariant.
    pub fn move_event(&mut self, from_sample: u64, to_sample: u64) -> bool {
        if from_sample == 0
            || to_sample == 0
            || from_sample == to_sample
            || self
                .events
                .iter()
                .any(|event| event.sample_pos == to_sample)
        {
            return false;
        }
        let Some(index) = self
            .events
            .iter()
            .position(|event| event.sample_pos == from_sample)
        else {
            return false;
        };
        let mut candidate = self.events.clone();
        candidate[index].sample_pos = to_sample;
        candidate.sort_by_key(|event| event.sample_pos);
        if candidate.first().map(|event| event.sample_pos) != Some(0)
            || candidate
                .windows(2)
                .any(|pair| pair[0].sample_pos >= pair[1].sample_pos)
        {
            return false;
        }
        self.events = candidate;
        true
    }

    pub fn set_ramp(&mut self, sample_pos: u64, ramp: bool) -> bool {
        let Some(event) = self
            .events
            .iter_mut()
            .find(|event| event.sample_pos == sample_pos)
        else {
            return false;
        };
        event.ramp = ramp;
        true
    }

    /// Feed a wall-clock tap and apply the resulting tempo to the map origin.
    /// The map remains sorted and its integrated beat positions are recalculated
    /// by the caller once the project sample rate is known.
    pub fn tap_and_set_tempo(&mut self, timestamp_ms: u64) -> Option<f64> {
        let bpm = self.tap_tempo.tap(timestamp_ms)?;
        if let Some(origin) = self.events.iter_mut().find(|event| event.sample_pos == 0) {
            origin.bpm = bpm;
        }
        Some(bpm)
    }

    /// INDUSTRIAL: Recalculates integrated beat positions with absolute precision and temporal sovereignty.
    pub fn recalculate_integrated_time(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        self.normalize_events();
        let mut current_beats = 0.0;
        let mut last_samples = 0;
        let mut last_bpm = 120.0;
        let mut last_ramp = false;

        for event in self.events.iter_mut() {
            let step = event.sample_pos.saturating_sub(last_samples);
            if step > 0 {
                let mut avg_bpm = last_bpm;
                if last_ramp {
                    avg_bpm = (last_bpm + event.bpm) * 0.5;
                }
                if last_bpm.is_finite()
                    && last_bpm > 0.0
                    && event.bpm.is_finite()
                    && event.bpm > 0.0
                {
                    current_beats += (step as f64 / sample_rate) * (avg_bpm / 60.0);
                }
            }
            event.world_beats = current_beats;
            last_samples = event.sample_pos;
            last_bpm = event.bpm;
            last_ramp = event.ramp;
        }
    }

    /// INDUSTRIAL: Performs beat-to-sample resolution with absolute precision and temporal sovereignty.
    /// Solves the quadratic trapezoidal equation to determine sample-accurate position inside tempo ramps.
    pub fn beats_to_samples(&self, beats: f64, sample_rate: f64) -> u64 {
        if self.events.is_empty()
            || !beats.is_finite()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
        {
            return 0;
        }

        let idx = match self
            .events
            .binary_search_by(|e| e.world_beats.total_cmp(&beats))
        {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    0
                } else {
                    idx - 1
                }
            }
        };

        let prev = &self.events[idx];
        if !prev.bpm.is_finite() || prev.bpm <= 0.0 {
            return prev.sample_pos;
        }
        let beat_step = beats - prev.world_beats;
        if beat_step <= 0.0 {
            return prev.sample_pos;
        }

        if prev.ramp && idx + 1 < self.events.len() {
            let next = &self.events[idx + 1];
            let duration_samples = next.sample_pos.saturating_sub(prev.sample_pos);
            if duration_samples > 0 {
                let duration_seconds = duration_samples as f64 / sample_rate;
                let alpha = (next.bpm - prev.bpm) / duration_seconds;

                if alpha.abs() > 1e-6 {
                    // Solve quadratic equation: 0.5 * alpha * t^2 + prev.bpm * t - 60.0 * beat_step = 0
                    let a = 0.5 * alpha;
                    let b = prev.bpm;
                    let c = -60.0 * beat_step;
                    let discriminant = b * b - 4.0 * a * c;
                    if discriminant >= 0.0 {
                        let t = (-b + discriminant.sqrt()) / (2.0 * a);
                        safe_sample_offset(prev.sample_pos, t * sample_rate)
                    } else {
                        safe_sample_offset(
                            prev.sample_pos,
                            (beat_step * 60.0 / prev.bpm) * sample_rate,
                        )
                    }
                } else {
                    safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
                }
            } else {
                safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
            }
        } else {
            safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
        }
    }

    /// INDUSTRIAL: Performs sample-to-beat resolution with absolute precision and temporal sovereignty.
    /// Evaluates cumulative trapezoidal beat integration inside tempo ramps.
    pub fn samples_to_beats(&self, samples: u64, sample_rate: f64) -> f64 {
        if self.events.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }

        let idx = match self.events.binary_search_by_key(&samples, |e| e.sample_pos) {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    0
                } else {
                    idx - 1
                }
            }
        };

        let prev = &self.events[idx];
        if !prev.bpm.is_finite() || prev.bpm <= 0.0 {
            return prev.world_beats;
        }
        let sample_step = samples.saturating_sub(prev.sample_pos);
        let t = sample_step as f64 / sample_rate;

        if prev.ramp && idx + 1 < self.events.len() {
            let next = &self.events[idx + 1];
            let duration_samples = next.sample_pos.saturating_sub(prev.sample_pos);
            if duration_samples > 0 {
                let duration_seconds = duration_samples as f64 / sample_rate;
                let alpha = (next.bpm - prev.bpm) / duration_seconds;
                let beats_diff = (prev.bpm * t + 0.5 * alpha * t * t) / 60.0;
                prev.world_beats + beats_diff
            } else {
                prev.world_beats + (t * prev.bpm / 60.0)
            }
        } else {
            prev.world_beats + (t * prev.bpm / 60.0)
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide temporal synchronization graph.
    pub fn audit_tempo(&self) -> bool {
        !self.events.is_empty()
            && self
                .events
                .first()
                .map(|event| event.sample_pos == 0)
                .unwrap_or(false)
            && self.events.windows(2).all(|pair| {
                pair[0].sample_pos < pair[1].sample_pos
                    && pair[0].world_beats.is_finite()
                    && pair[0].bpm.is_finite()
                    && pair[0].bpm > 0.0
            })
            && self.events.iter().all(|event| {
                event.bpm.is_finite() && event.bpm > 0.0 && event.world_beats.is_finite()
            })
    }

    /// Sorts and sanitizes externally edited tempo events before any binary
    /// search. Duplicate sample positions are collapsed deterministically.
    pub fn normalize_events(&mut self) {
        self.events.retain(|event| {
            event.bpm.is_finite() && event.bpm > 0.0 && event.world_beats.is_finite()
        });
        self.events.sort_by_key(|event| event.sample_pos);
        let mut normalized: Vec<TempoEvent> = Vec::with_capacity(self.events.len().max(1));
        for event in self.events.drain(..) {
            if let Some(previous) = normalized.last_mut() {
                if previous.sample_pos == event.sample_pos {
                    *previous = event;
                    continue;
                }
            }
            normalized.push(event);
        }
        if normalized.first().map(|event| event.sample_pos) != Some(0) {
            normalized.insert(
                0,
                TempoEvent {
                    sample_pos: 0,
                    bpm: 120.0,
                    ramp: false,
                    world_beats: 0.0,
                },
            );
        }
        self.events = normalized;
    }
}

fn safe_sample_offset(base: u64, offset: f64) -> u64 {
    if !offset.is_finite() || offset <= 0.0 {
        return base;
    }
    base.saturating_add(offset.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::{TempoEvent, TempoOrchestrator};

    #[test]
    fn normalizes_unsorted_duplicate_and_invalid_events() {
        let mut tempo = TempoOrchestrator {
            events: vec![
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 90.0,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 0,
                    bpm: f64::NAN,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 100.0,
                    ramp: true,
                    world_beats: 0.0,
                },
            ],
            tap_tempo: super::TapTempo::default(),
        };
        tempo.recalculate_integrated_time(44_100.0);
        assert!(tempo.audit_tempo());
        assert_eq!(tempo.events.len(), 2);
        assert_eq!(tempo.events[0].sample_pos, 0);
        assert_eq!(tempo.events[1].sample_pos, 44_100);
        assert_eq!(tempo.events[1].bpm, 100.0);
    }

    #[test]
    fn tap_updates_origin_tempo() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(0).is_none());
        assert_eq!(tempo.tap_and_set_tempo(500), Some(120.0));
        assert_eq!(tempo.events[0].bpm, 120.0);
    }

    #[test]
    fn non_monotonic_taps_are_ignored() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(1000).is_none());
        assert!(tempo.tap_and_set_tempo(900).is_none());
        assert_eq!(tempo.tap_and_set_tempo(1500), Some(120.0));
    }

    #[test]
    fn time_warp_moves_nodes_without_collisions() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 48_000,
            bpm: 100.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 96_000,
            bpm: 110.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.move_event(96_000, 72_000));
        assert!(!tempo.move_event(72_000, 48_000));
        assert!(tempo.set_ramp(72_000, true));
        assert!(tempo.audit_tempo());
    }
}
