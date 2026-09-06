#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeType {
    Linear,
    SCurve,
    Exponential,
}

pub struct FadeInfo {
    pub duration_samples: u32,
    pub fade_type: FadeType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionEdit {
    pub id: u64,
    pub label: String,
    pub gain_milli_db: i32,
}
#[derive(Clone, Debug, Default)]
pub struct RegionEditHistory {
    edits: Vec<RegionEdit>,
    redo: Vec<RegionEdit>,
    next_id: u64,
}
impl RegionEditHistory {
    pub fn new() -> Self {
        Self {
            edits: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
        }
    }
    pub fn record(&mut self, label: &str, gain_db: f32) -> Option<u64> {
        if label.trim().is_empty()
            || label.len() > 128
            || label.contains('\0')
            || !gain_db.is_finite()
            || !(-120.0..=24.0).contains(&gain_db)
            || self.edits.len() >= 65_536
        {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.edits.push(RegionEdit {
            id,
            label: label.trim().into(),
            gain_milli_db: (gain_db * 1000.0).round() as i32,
        });
        self.redo.clear();
        Some(id)
    }
    pub fn undo(&mut self) -> Option<RegionEdit> {
        let edit = self.edits.pop()?;
        self.redo.push(edit.clone());
        Some(edit)
    }
    pub fn redo(&mut self) -> Option<RegionEdit> {
        let edit = self.redo.pop()?;
        self.edits.push(edit.clone());
        Some(edit)
    }
    pub fn entries(&self) -> &[RegionEdit] {
        &self.edits
    }
    pub fn redo_entries(&self) -> &[RegionEdit] {
        &self.redo
    }
    pub fn latest_action(&self) -> Option<&str> {
        self.edits.last().map(|edit| edit.label.as_str())
    }
    pub fn clear(&mut self) {
        self.edits.clear();
        self.redo.clear();
    }
    pub fn snapshot(&self) -> Vec<RegionEdit> {
        self.edits.clone()
    }
    pub fn audit(&self) -> bool {
        if self.edits.len() > 65_536 || self.redo.len() > 65_536 || self.next_id == 0 {
            return false;
        }
        let all = self
            .edits
            .iter()
            .chain(self.redo.iter())
            .collect::<Vec<_>>();
        all.iter().all(|e| {
            e.id > 0
                && e.id < self.next_id
                && !e.label.trim().is_empty()
                && e.label.len() <= 128
                && !e.label.contains('\0')
                && (-120_000..=24_000).contains(&e.gain_milli_db)
        }) && all
            .iter()
            .enumerate()
            .all(|(i, e)| all[..i].iter().all(|p| p.id != e.id))
            && self.edits.windows(2).all(|w| w[0].id < w[1].id)
    }
}

pub struct RegionProcessorOrchestrator {
    pub gain: f32,
    pub fade_in: FadeInfo,
    pub fade_out: FadeInfo,
    pub length_samples: u64,
}

impl Default for RegionProcessorOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionProcessorOrchestrator {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            fade_in: FadeInfo {
                duration_samples: 0,
                fade_type: FadeType::Linear,
            },
            fade_out: FadeInfo {
                duration_samples: 0,
                fade_type: FadeType::Linear,
            },
            length_samples: 0,
        }
    }

    /// Applies gain and both edge fades without allocating on the processing path.
    pub fn process_signal(&self, buffer: &mut [f32], offset: usize, size: usize, rel_pos: u64) {
        let end = offset.saturating_add(size).min(buffer.len());
        if offset >= end || !self.gain.is_finite() {
            return;
        }

        let gain = self.gain.clamp(0.0, 4.0);
        for (index, sample) in buffer[offset..end].iter_mut().enumerate() {
            let relative = rel_pos.saturating_add(index as u64);
            let mut fade = 1.0f32;

            if self.fade_in.duration_samples > 0 && relative < self.fade_in.duration_samples as u64
            {
                let t = relative as f32 / self.fade_in.duration_samples as f32;
                fade *= fade_curve(t, self.fade_in.fade_type);
            }

            if self.fade_out.duration_samples > 0
                && self.length_samples > 0
                && relative < self.length_samples
                && relative
                    >= self
                        .length_samples
                        .saturating_sub(self.fade_out.duration_samples as u64)
            {
                let remaining = self.length_samples.saturating_sub(relative);
                let t = (remaining as f32 / self.fade_out.duration_samples as f32).clamp(0.0, 1.0);
                fade *= fade_curve(t, self.fade_out.fade_type);
            }

            let input = if sample.is_finite() { *sample } else { 0.0 };
            let output = input * gain * fade.clamp(0.0, 1.0);
            *sample = if output.is_finite() { output } else { 0.0 };
        }
    }

    /// Applies a reversible region operation while recording its metadata in
    /// the event-processing history. Audio is changed only after validation.
    pub fn process_with_history(
        &self,
        buffer: &mut [f32],
        offset: usize,
        size: usize,
        rel_pos: u64,
        label: &str,
        history: &mut RegionEditHistory,
    ) -> Option<u64> {
        if !self.audit_signal() || label.trim().is_empty() {
            return None;
        }
        let gain_db = 20.0 * self.gain.max(f32::MIN_POSITIVE).log10();
        let id = history.record(label, gain_db)?;
        self.process_signal(buffer, offset, size, rel_pos);
        Some(id)
    }

    pub fn set_gain_db(&mut self, gain_db: f32) -> bool {
        if !gain_db.is_finite() || !(-120.0..=24.0).contains(&gain_db) {
            return false;
        }
        self.gain = 10.0f32.powf(gain_db / 20.0).clamp(0.0, 4.0);
        true
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide signal synchronization graph.
    pub fn audit_signal(&self) -> bool {
        let valid_fade = |duration: u32| {
            if self.length_samples == 0 {
                duration == 0
            } else {
                (duration as u64) <= self.length_samples
            }
        };
        self.gain.is_finite()
            && (0.0..=4.0).contains(&self.gain)
            && valid_fade(self.fade_in.duration_samples)
            && valid_fade(self.fade_out.duration_samples)
    }
}

fn fade_curve(value: f32, fade_type: FadeType) -> f32 {
    let t = value.clamp(0.0, 1.0);
    match fade_type {
        FadeType::Linear => t,
        FadeType::SCurve => t * t * (3.0 - 2.0 * t),
        FadeType::Exponential => t * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_gain_and_edge_fades() {
        let processor = RegionProcessorOrchestrator {
            gain: 2.0,
            fade_in: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            fade_out: FadeInfo {
                duration_samples: 2,
                fade_type: FadeType::Linear,
            },
            length_samples: 4,
        };
        let mut buffer = [1.0; 4];
        processor.process_signal(&mut buffer, 0, 4, 0);
        assert_eq!(buffer, [0.0, 1.0, 2.0, 1.0]);
    }

    #[test]
    fn clamps_invalid_range_without_panicking() {
        let processor = RegionProcessorOrchestrator::new();
        let mut buffer = [f32::NAN, 1.0, 2.0];
        processor.process_signal(&mut buffer, 2, 100, 0);
        assert!(buffer[0].is_nan());
        assert_eq!(&buffer[1..], &[1.0, 2.0]);
    }

    #[test]
    fn processing_records_event_history_after_validation() {
        let mut processor = RegionProcessorOrchestrator::new();
        assert!(processor.set_gain_db(6.0));
        let mut history = RegionEditHistory::new();
        let mut buffer = [1.0; 2];
        let id = processor
            .process_with_history(&mut buffer, 0, 2, 0, "clip gain", &mut history)
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(
            history.entries().last().map(|entry| entry.label.as_str()),
            Some("clip gain")
        );
        assert!(buffer[0] > 1.0);
    }
}
