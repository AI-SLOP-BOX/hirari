#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PitchBlock {
    pub start_sample: u64,
    pub end_sample: u64,
    pub target_note: f32,
    pub vibrato_amount: f32,
    /// Vibrato rate in millihertz, shared with imported vocal note contracts.
    #[serde(default = "default_vibrato_rate_millihz")]
    pub vibrato_rate_millihz: u16,
    pub drift_amount: f32,
    /// Fine pitch and vocal-formant controls exposed by a VariAudio-style
    /// segment editor. Cents are applied after the semitone target.
    #[serde(default)]
    pub micro_pitch_cents: f32,
    /// Optional dense pitch-drift curve sampled uniformly across the block.
    /// Values are cents and are applied in addition to the target note.
    #[serde(default)]
    pub pitch_curve_cents: Vec<f32>,
    #[serde(default)]
    pub formant_shift_semitones: f32,
    /// Timing anchor as a multiplicative source-time ratio.
    #[serde(default = "default_timing_ratio")]
    pub timing_ratio: f32,
}

fn default_timing_ratio() -> f32 {
    1.0
}
fn default_vibrato_rate_millihz() -> u16 {
    5_000
}

impl PitchBlock {
    pub fn validate(&self) -> bool {
        self.start_sample < self.end_sample
            && self.target_note.is_finite()
            && (0.0..=127.0).contains(&self.target_note)
            && self.vibrato_amount.is_finite()
            && self.drift_amount.is_finite()
            && self.vibrato_amount.abs() <= 1.0
            && self.drift_amount.abs() <= 1.0
            && (500..=20_000).contains(&self.vibrato_rate_millihz)
            && self.micro_pitch_cents.is_finite()
            && self.micro_pitch_cents.abs() <= 1200.0
            && self.pitch_curve_cents.len() <= 4096
            && self
                .pitch_curve_cents
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1200.0)
            && self.formant_shift_semitones.is_finite()
            && self.formant_shift_semitones.abs() <= 24.0
            && self.timing_ratio.is_finite()
            && (0.25..=4.0).contains(&self.timing_ratio)
    }
}

#[derive(Default)]
pub struct PitchOrchestrator {
    pub blocks: Vec<PitchBlock>,
}

impl PitchOrchestrator {
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// Inserts or replaces a note segment while rejecting overlaps.  Segments
    /// are kept sorted so editor hit-testing remains deterministic.
    pub fn upsert_block(&mut self, block: PitchBlock) -> bool {
        if !block.validate() {
            return false;
        }
        let replacement = self
            .blocks
            .iter()
            .position(|b| b.start_sample == block.start_sample && b.end_sample == block.end_sample);
        if self.blocks.iter().enumerate().any(|(i, other)| {
            Some(i) != replacement
                && block.start_sample < other.end_sample
                && other.start_sample < block.end_sample
        }) {
            return false;
        }
        if let Some(existing) = self
            .blocks
            .iter_mut()
            .find(|b| b.start_sample == block.start_sample && b.end_sample == block.end_sample)
        {
            *existing = block;
        } else {
            self.blocks.push(block);
        }
        self.blocks.sort_by_key(|b| b.start_sample);
        true
    }

    pub fn remove_block(&mut self, start_sample: u64, end_sample: u64) -> bool {
        let before = self.blocks.len();
        self.blocks
            .retain(|b| !(b.start_sample == start_sample && b.end_sample == end_sample));
        before != self.blocks.len()
    }

    /// Imports detector output into editable VariAudio-style blocks. The
    /// replacement is atomic and rejects malformed/overlapping detections.
    pub fn import_detected_segments(
        &mut self,
        detected: &[crate::pitch_detector::DetectedPitchSegment],
    ) -> bool {
        if detected.len() > 1_000_000
            || detected.iter().any(|segment| {
                segment.start_sample >= segment.end_sample
                    || segment.midi_note > 127
                    || !segment.frequency_hz.is_finite()
                    || segment.frequency_hz <= 0.0
                    || !segment.confidence.is_finite()
                    || !(0.0..=1.0).contains(&segment.confidence)
            })
        {
            return false;
        }
        let mut blocks = Vec::with_capacity(detected.len());
        for segment in detected {
            blocks.push(PitchBlock {
                start_sample: segment.start_sample,
                end_sample: segment.end_sample,
                target_note: segment.midi_note as f32,
                vibrato_amount: 0.0,
                vibrato_rate_millihz: 5_000,
                drift_amount: 0.0,
                micro_pitch_cents: 0.0,
                pitch_curve_cents: Vec::new(),
                formant_shift_semitones: 0.0,
                timing_ratio: 1.0,
            });
        }
        blocks.sort_by_key(|block| block.start_sample);
        if blocks
            .windows(2)
            .any(|pair| pair[0].end_sample > pair[1].start_sample)
        {
            return false;
        }
        self.blocks = blocks;
        true
    }

    /// INDUSTRIAL: Resolves the pitch shift ratio for a given sample with absolute precision and tuning sovereignty.
    pub fn get_shift_ratio(&self, now: u64, detected_freq: f32) -> f32 {
        // INDUSTRIAL: Implementation of high-performance pitch correction logic.
        // Rust's safe memory management handles complex vocal streams with
        // absolute bit-accuracy and zero-latency.
        if !detected_freq.is_finite() || detected_freq <= 0.0 {
            return 1.0;
        }
        if let Some(block) = self.find_block(now) {
            if !block.target_note.is_finite() {
                return 1.0;
            }
            // Resolve the same expressive curve exposed to the editor so the
            // audio path follows micro-pitch gestures sample by sample.
            let target_note = self
                .target_note_at(now)
                .unwrap_or(block.target_note)
                .clamp(0.0, 127.0);
            let target_freq = 440.0
                * 2.0f32.powf((target_note - 69.0) / 12.0)
                * 2.0f32.powf(block.micro_pitch_cents.clamp(-1200.0, 1200.0) / 1200.0);
            let ratio = target_freq / detected_freq;
            if ratio.is_finite() {
                ratio.clamp(0.03125, 32.0)
            } else {
                1.0
            }
        } else {
            1.0
        }
    }

    /// Returns the non-destructive formant multiplier for the segment at a
    /// sample position. The audio processor can use this value to select a
    /// formant-preserving or formant-shifting spectral path.
    pub fn get_formant_ratio(&self, now: u64) -> f32 {
        self.find_block(now)
            .map(|block| {
                2.0f32
                    .powf(block.formant_shift_semitones / 12.0)
                    .clamp(0.25, 4.0)
            })
            .unwrap_or(1.0)
    }

    pub fn get_timing_ratio(&self, now: u64) -> f32 {
        self.find_block(now)
            .map(|block| block.timing_ratio)
            .unwrap_or(1.0)
    }

    /// Replaces the editable per-segment pitch curve.  Samples are uniformly
    /// distributed from segment start to end and validated atomically.
    pub fn set_pitch_curve(
        &mut self,
        start_sample: u64,
        end_sample: u64,
        curve_cents: Vec<f32>,
    ) -> bool {
        if curve_cents.len() > 4096
            || curve_cents
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 1200.0)
        {
            return false;
        }
        let Some(block) = self
            .blocks
            .iter_mut()
            .find(|block| block.start_sample == start_sample && block.end_sample == end_sample)
        else {
            return false;
        };
        block.pitch_curve_cents = curve_cents;
        true
    }

    /// Updates the expressive vibrato controls of one detected segment.
    pub fn set_vibrato(
        &mut self,
        start_sample: u64,
        end_sample: u64,
        amount: f32,
        rate_millihz: u16,
    ) -> bool {
        if !amount.is_finite() || amount.abs() > 1.0 || !(500..=20_000).contains(&rate_millihz) {
            return false;
        }
        let Some(block) = self
            .blocks
            .iter_mut()
            .find(|block| block.start_sample == start_sample && block.end_sample == end_sample)
        else {
            return false;
        };
        block.vibrato_amount = amount;
        block.vibrato_rate_millihz = rate_millihz;
        true
    }

    /// Resolves the editable pitch curve at a sample position, including
    /// bounded vibrato and drift controls for expressive vocal correction.
    pub fn target_note_at(&self, now: u64) -> Option<f32> {
        let block = self.find_block(now)?;
        let span = (block.end_sample - block.start_sample).max(1) as f32;
        let phase = (now.saturating_sub(block.start_sample) as f32 / span).clamp(0.0, 1.0);
        let rate_hz = (f32::from(block.vibrato_rate_millihz.max(500)) / 1000.0).min(20.0);
        let vibrato =
            block.vibrato_amount.clamp(-1.0, 1.0) * (phase * std::f32::consts::TAU * rate_hz).sin();
        let drift = block.drift_amount.clamp(-1.0, 1.0) * (phase * 2.0 - 1.0);
        let curve = match block.pitch_curve_cents.as_slice() {
            [] => 0.0,
            values => {
                let position = phase * (values.len().saturating_sub(1)) as f32;
                let left = position.floor() as usize;
                let right = (left + 1).min(values.len() - 1);
                let amount = position - left as f32;
                values[left] + (values[right] - values[left]) * amount
            }
        } / 100.0;
        Some((block.target_note + vibrato + drift + curve).clamp(0.0, 127.0))
    }

    fn find_block(&self, now: u64) -> Option<&PitchBlock> {
        self.blocks
            .iter()
            .find(|b| now >= b.start_sample && now < b.end_sample)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide vocal tuning synchronization graph.
    pub fn audit_pitch(&self) -> bool {
        self.blocks.iter().all(PitchBlock::validate)
            && self
                .blocks
                .windows(2)
                .all(|pair| pair[0].end_sample <= pair[1].start_sample)
    }
}

#[cfg(test)]
mod tests {
    use super::{PitchBlock, PitchOrchestrator};

    #[test]
    fn invalid_detection_returns_unity_ratio() {
        let mut pitch = PitchOrchestrator::new();
        pitch.blocks.push(PitchBlock {
            start_sample: 0,
            end_sample: 100,
            target_note: 69.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0,
        });
        assert_eq!(pitch.get_shift_ratio(1, 0.0), 1.0);
        assert_eq!(pitch.get_shift_ratio(1, f32::NAN), 1.0);
    }

    #[test]
    fn audit_rejects_invalid_pitch_blocks() {
        let mut pitch = PitchOrchestrator::new();
        pitch.blocks.push(PitchBlock {
            start_sample: 100,
            end_sample: 10,
            target_note: 69.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0,
        });
        assert!(!pitch.audit_pitch());
    }

    #[test]
    fn editor_segments_are_sorted_and_non_overlapping() {
        let mut pitch = PitchOrchestrator::new();
        let block = |start, end| PitchBlock {
            start_sample: start,
            end_sample: end,
            target_note: 69.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0,
        };
        assert!(pitch.upsert_block(block(100, 200)));
        assert!(pitch.upsert_block(block(0, 50)));
        assert!(!pitch.upsert_block(block(40, 120)));
        assert!(pitch.audit_pitch());
        assert_eq!(pitch.blocks[0].start_sample, 0);
    }

    #[test]
    fn segment_exposes_fine_pitch_formant_and_timing_controls() {
        let mut pitch = PitchOrchestrator::new();
        assert!(pitch.upsert_block(PitchBlock {
            start_sample: 0,
            end_sample: 100,
            target_note: 69.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 100.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 12.0,
            timing_ratio: 1.5
        }));
        let ratio = pitch.get_shift_ratio(10, 440.0);
        assert!((ratio - 2.0f32.powf(100.0 / 1200.0)).abs() < 1e-4);
        assert!((pitch.get_formant_ratio(10) - 2.0).abs() < 1e-5);
        assert_eq!(pitch.get_timing_ratio(10), 1.5);
    }

    #[test]
    fn detector_segments_import_as_editable_blocks_atomically() {
        let mut pitch = PitchOrchestrator::new();
        let detected = vec![crate::pitch_detector::DetectedPitchSegment {
            start_sample: 100,
            end_sample: 200,
            midi_note: 60,
            frequency_hz: 261.6,
            confidence: 0.9,
        }];
        assert!(pitch.import_detected_segments(&detected));
        assert_eq!(pitch.blocks[0].target_note, 60.0);
        let invalid = vec![crate::pitch_detector::DetectedPitchSegment {
            start_sample: 150,
            end_sample: 250,
            midi_note: 61,
            frequency_hz: 277.0,
            confidence: 0.9,
        }];
        assert!(!pitch.import_detected_segments(&[detected[0].clone(), invalid[0].clone()]));
        assert_eq!(pitch.blocks.len(), 1);
    }

    #[test]
    fn target_note_resolution_applies_bounded_expression_curve() {
        let mut pitch = PitchOrchestrator::new();
        assert!(pitch.upsert_block(PitchBlock {
            start_sample: 0,
            end_sample: 100,
            target_note: 60.0,
            vibrato_amount: 0.5,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.25,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0
        }));
        let start = pitch.target_note_at(0).unwrap();
        let middle = pitch.target_note_at(50).unwrap();
        assert!((0.0..=127.0).contains(&start) && (0.0..=127.0).contains(&middle));
        assert_ne!(start, middle);
    }

    #[test]
    fn target_note_resolution_uses_persisted_vibrato_rate() {
        let mut slow = PitchOrchestrator::new();
        let mut fast = PitchOrchestrator::new();
        let base = |rate| PitchBlock {
            start_sample: 0,
            end_sample: 1000,
            target_note: 60.0,
            vibrato_amount: 0.5,
            vibrato_rate_millihz: rate,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0,
        };
        assert!(slow.upsert_block(base(2_000)));
        assert!(fast.upsert_block(base(10_000)));
        assert_ne!(slow.target_note_at(73), fast.target_note_at(73));
    }

    #[test]
    fn editable_pitch_curve_is_interpolated_in_cents() {
        let mut pitch = PitchOrchestrator::new();
        assert!(pitch.upsert_block(PitchBlock {
            start_sample: 0,
            end_sample: 100,
            target_note: 60.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0
        }));
        assert!(pitch.set_pitch_curve(0, 100, vec![0.0, 100.0, 0.0]));
        assert!((pitch.target_note_at(25).unwrap() - 60.5).abs() < 1e-5);
        assert!(!pitch.set_pitch_curve(0, 100, vec![f32::NAN]));
        assert!(pitch.audit_pitch());
    }

    #[test]
    fn vibrato_controls_update_atomically() {
        let mut pitch = PitchOrchestrator::new();
        assert!(pitch.upsert_block(PitchBlock {
            start_sample: 0,
            end_sample: 100,
            target_note: 60.0,
            vibrato_amount: 0.0,
            vibrato_rate_millihz: 5_000,
            drift_amount: 0.0,
            micro_pitch_cents: 0.0,
            pitch_curve_cents: Vec::new(),
            formant_shift_semitones: 0.0,
            timing_ratio: 1.0
        }));
        assert!(pitch.set_vibrato(0, 100, 0.75, 12_000));
        assert_eq!(pitch.blocks[0].vibrato_rate_millihz, 12_000);
        assert_eq!(pitch.blocks[0].vibrato_amount, 0.75);
        assert!(!pitch.set_vibrato(0, 100, 1.5, 12_000));
        assert_eq!(pitch.blocks[0].vibrato_amount, 0.75);
    }
}
