//! Region processor orchestrator, split out of region_processing.rs.
use super::*;

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

/// Apply the realtime RegionProcessor gain and fade-in operation to one
/// channel. The C++ owner supplies its already-clamped audio range; all
/// per-sample math and non-finite handling stay in Rust.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_processor_apply_gain_fade(
    samples: *mut f32,
    frames: u32,
    region_relative_pos: u64,
    gain: f32,
    fade_in_samples: u32,
    fade_type: u8,
) -> bool {
    if samples.is_null() || frames == 0 || !gain.is_finite() {
        return false;
    }
    let fade_type = match fade_type {
        0 => FadeType::Linear,
        1 => FadeType::SCurve,
        2 => FadeType::Exponential,
        _ => return false,
    };
    let gain = gain.clamp(0.0, 4.0);
    let samples = unsafe { std::slice::from_raw_parts_mut(samples, frames as usize) };
    for (index, sample) in samples.iter_mut().enumerate() {
        let relative = region_relative_pos.saturating_add(index as u64);
        let envelope = if fade_in_samples > 0 && relative < fade_in_samples as u64 {
            fade_curve(relative as f32 / fade_in_samples as f32, fade_type)
        } else {
            1.0
        };
        let input = if sample.is_finite() { *sample } else { 0.0 };
        let output = input * gain * envelope;
        *sample = if output.is_finite() { output } else { 0.0 };
    }
    true
}

/// Region range-edit operations, as a pure transform.
///
/// The caller passes its current edits by immutable pointer and receives a
/// freshly allocated buffer it must release with
/// [`hirari_free_range_edits`]. Rust never writes into caller storage, so a
/// caller can no longer observe a count returned by Rust and then resize a
/// container whose contents Rust had written through a raw pointer. That
/// pattern is what silently zeroed an appended edit once already.
///
/// `out_count` receives the resulting entry count. A null return means the
/// operation was rejected and the caller's state is unchanged.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_range_edits_apply(
    edits_in: *const NativeRangeEdit,
    count_in: usize,
    region_length: u64,
    op: u32,
    start: u64,
    end: u64,
    gain: f32,
    fade_in: u64,
    fade_out: u64,
    out_count: *mut usize,
) -> *mut NativeRangeEdit {
    const OP_NORMALIZE: u32 = 0;
    const OP_UPSERT: u32 = 1;
    const OP_REMOVE: u32 = 2;
    const OP_REPLACE: u32 = 3;
    if out_count.is_null() || count_in > 4096 {
        return std::ptr::null_mut();
    }
    let source: &[NativeRangeEdit] = if count_in == 0 {
        &[]
    } else if edits_in.is_null() {
        return std::ptr::null_mut();
    } else {
        unsafe { slice::from_raw_parts(edits_in, count_in) }
    };

    let gain_is_valid =
        |value: f32| value.is_finite() && (0.0..=16.0).contains(&value);

    let mut result: Vec<NativeRangeEdit> = match op {
        OP_NORMALIZE => {
            if region_length == 0 {
                Vec::new()
            } else {
                let mut valid: Vec<NativeRangeEdit> = Vec::with_capacity(source.len());
                for mut edit in source.iter().copied() {
                    if edit.start >= edit.end
                        || edit.start >= region_length
                        || !gain_is_valid(edit.gain)
                    {
                        continue;
                    }
                    edit.end = edit.end.min(region_length);
                    let length = edit.end - edit.start;
                    edit.fade_in = edit.fade_in.min(length);
                    edit.fade_out = edit.fade_out.min(length);
                    valid.push(edit);
                }
                valid
            }
        }
        OP_UPSERT => {
            if start >= end || end > region_length || !gain_is_valid(gain) || fade_in > end - start
                || fade_out > end - start
            {
                return std::ptr::null_mut();
            }
            let replacement = NativeRangeEdit {
                start,
                end,
                gain,
                fade_in,
                fade_out,
            };
            let mut next: Vec<NativeRangeEdit> = source.to_vec();
            if let Some(existing) = next
                .iter_mut()
                .find(|edit| edit.start == start && edit.end == end)
            {
                *existing = replacement;
            } else {
                next.push(replacement);
            }
            next
        }
        OP_REMOVE => {
            let before = source.len();
            let next: Vec<NativeRangeEdit> = source
                .iter()
                .copied()
                .filter(|edit| !(edit.start == start && edit.end == end))
                .collect();
            if next.len() == before {
                return std::ptr::null_mut();
            }
            next
        }
        OP_REPLACE => {
            let mut next: Vec<NativeRangeEdit> = source.to_vec();
            let mut rejected = false;
            for edit in next.iter_mut() {
                if edit.start >= edit.end
                    || edit.end > region_length
                    || !gain_is_valid(edit.gain)
                {
                    rejected = true;
                    break;
                }
                let length = edit.end - edit.start;
                edit.fade_in = edit.fade_in.min(length);
                edit.fade_out = edit.fade_out.min(length);
            }
            if rejected {
                return std::ptr::null_mut();
            }
            next
        }
        _ => return std::ptr::null_mut(),
    };
    if result.len() > 4096 {
        return std::ptr::null_mut();
    }
    result.sort_by_key(|edit| edit.start);
    let length = result.len();
    unsafe { *out_count = length };
    let boxed = result.into_boxed_slice();
    Box::into_raw(boxed) as *mut NativeRangeEdit
}

/// Releases a buffer returned by [`hirari_region_range_edits_apply`].
#[no_mangle]
pub unsafe extern "C" fn hirari_free_range_edits(edits: *mut NativeRangeEdit, count: usize) {
    if edits.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(std::slice::from_raw_parts_mut(edits, count)));
    }
}

#[cfg(test)]
#[path = "region_processing_tests.rs"]
mod tests;
