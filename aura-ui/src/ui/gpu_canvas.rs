//! Backend-neutral plot frames for the future wgpu canvas.
//!
//! Slint owns controls and layout. This module owns the bounded, finite data
//! contract consumed by a high-rate waveform/spectrum renderer, so moving the
//! draw step to wgpu does not change the audio or UI state APIs.

use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlotFrame {
    pub revision: u64,
    pub waveform_min_max: Vec<[f32; 2]>,
    pub spectrum: Vec<f32>,
    pub meters: Vec<f32>,
    /// Normalized MIDI note rectangles: x, width, y, velocity.
    pub piano_notes: Vec<[f32; 4]>,
}

#[derive(Clone, Default)]
pub struct PlotFrameStore(Arc<RwLock<PlotFrame>>);

impl PlotFrameStore {
    pub fn snapshot(&self) -> PlotFrame {
        self.0.read().map(|frame| frame.clone()).unwrap_or_default()
    }

    pub fn publish_waveform(&self, samples: &[f32], width: usize) {
        let peaks = waveform_lod(samples, width);
        if let Ok(mut frame) = self.0.write() {
            frame.revision = frame.revision.wrapping_add(1);
            frame.waveform_min_max = peaks;
        }
    }

    pub fn publish_spectrum(&self, values: &[f32]) {
        if let Ok(mut frame) = self.0.write() {
            frame.revision = frame.revision.wrapping_add(1);
            frame.spectrum = finite_normalized(values);
        }
    }

    pub fn publish_meters(&self, values: &[f32]) {
        if let Ok(mut frame) = self.0.write() {
            frame.revision = frame.revision.wrapping_add(1);
            frame.meters = finite_normalized(values);
        }
    }

    pub fn publish_piano_notes(&self, notes: &[[f32; 4]]) {
        if let Ok(mut frame) = self.0.write() {
            let filtered: Vec<[f32; 4]> = notes.iter().copied().filter(|note| {
                note.iter().all(|value| value.is_finite())
                    && note[0] >= 0.0
                    && note[1] > 0.0
                    && note[2] >= 0.0
                    && note[2] <= 1.0
            }).collect();
            if frame.piano_notes == filtered {
                return;
            }
            frame.revision = frame.revision.wrapping_add(1);
            frame.piano_notes = filtered;
        }
    }
}

pub fn shared_store() -> PlotFrameStore {
    PlotFrameStore::default()
}

pub fn waveform_lod(samples: &[f32], width: usize) -> Vec<[f32; 2]> {
    if samples.is_empty() || width == 0 {
        return Vec::new();
    }
    let buckets = width.min(samples.len());
    (0..buckets)
        .map(|bucket| {
            let start = bucket * samples.len() / buckets;
            let end = ((bucket + 1) * samples.len() / buckets).max(start + 1);
            let mut min = 0.0f32;
            let mut max = 0.0f32;
            let mut seen = false;
            for &value in &samples[start..end.min(samples.len())] {
                if value.is_finite() {
                    min = if seen { min.min(value) } else { value };
                    max = if seen { max.max(value) } else { value };
                    seen = true;
                }
            }
            if seen { [min.clamp(-1.0, 1.0), max.clamp(-1.0, 1.0)] } else { [0.0, 0.0] }
        })
        .collect()
}

fn finite_normalized(values: &[f32]) -> Vec<f32> {
    values.iter().map(|value| value.clamp(0.0, 1.0)).filter(|value| value.is_finite()).collect()
}

#[cfg(test)]
mod tests {
    use super::{waveform_lod, PlotFrameStore};

    #[test]
    fn lod_preserves_extrema_per_pixel_bucket() {
        assert_eq!(waveform_lod(&[-0.2, 0.8, -0.5, 0.1], 2), vec![[-0.2, 0.8], [-0.5, 0.1]]);
    }

    #[test]
    fn lod_is_bounded_and_finite() {
        let result = waveform_lod(&[f32::NAN, 2.0, -2.0], 64);
        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|pair| pair.iter().all(|value| value.is_finite() && value.abs() <= 1.0)));
    }

    #[test]
    fn store_publishes_separate_plot_streams() {
        let store = PlotFrameStore::default();
        store.publish_waveform(&[-1.0, 0.5, 1.0], 2);
        store.publish_spectrum(&[-1.0, 0.5, 2.0, f32::NAN]);
        let frame = store.snapshot();
        assert_eq!(frame.waveform_min_max.len(), 2);
        assert_eq!(frame.spectrum, vec![0.0, 0.5, 1.0]);
        assert!(frame.revision >= 2);
    }

    #[test]
    fn piano_notes_are_filtered_and_deduplicated() {
        let store = PlotFrameStore::default();
        let notes = [[0.1, 0.2, 0.5, 0.8], [f32::NAN, 0.1, 0.2, 0.3]];
        store.publish_piano_notes(&notes);
        let first = store.snapshot();
        store.publish_piano_notes(&notes);
        let second = store.snapshot();
        assert_eq!(first.piano_notes.len(), 1);
        assert_eq!(first.revision, second.revision);
    }

    #[test]
    fn analysis_streams_do_not_require_waveform_data() {
        let store = PlotFrameStore::default();
        store.publish_spectrum(&[0.2, 0.6]);
        store.publish_meters(&[0.4, 0.8]);
        let frame = store.snapshot();
        assert!(frame.waveform_min_max.is_empty());
        assert_eq!(frame.spectrum, vec![0.2, 0.6]);
        assert_eq!(frame.meters, vec![0.4, 0.8]);
    }
}
