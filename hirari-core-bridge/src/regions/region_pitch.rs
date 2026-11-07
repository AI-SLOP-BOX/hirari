//! Region F0 analysis driven entirely from Rust.
//!
//! The host owns the audio buffers, the region table and the note store, so it
//! exposes them through the region host ops. Rust reads the region geometry,
//! retains the decoded audio, runs the analysis kernel, asks the host to
//! commit the segments into the region's note table and republish the
//! snapshot. Nothing here runs on the audio thread; the caller is the pitch
//! analysis job, which already holds the track's analysis gate.

use std::os::raw::c_void;
use std::ptr;

use crate::audio_pitch_analysis::hirari_audio_pitch_analyze_region;
use crate::region_edit_host::{
    host_ops, warp_markers_of, HirariRegionEditPayload, HirariRegionState,
    HIRARI_REGION_EDIT_PITCH_COMMIT,
};
use crate::region_warp::HirariWarpMarker;

/// Matches the host's buffer bound for offline analysis.
const MAX_ANALYSIS_SAMPLES: u64 = 16_000_000;
/// Sample rates below this are treated as unset, like the native analysis did.
const MIN_SOURCE_RATE: f64 = 8000.0;
const MAX_CHANNELS: usize = 32;

/// Analyzer tuning retained from the native analyzer's defaults.
const CONFIG_MIN_HZ: f64 = 65.0;
const CONFIG_MAX_HZ: f64 = 1200.0;
const CONFIG_THRESHOLD: f64 = 0.82;
const CONFIG_NOTE_CHANGE_CENTS: f64 = 90.0;
const CONFIG_NOTE_CHANGE_FRAMES: usize = 2;

/// Releases a retained audio handle when analysis ends, on every path.
struct RetainedAudio {
    release: unsafe extern "C" fn(*mut c_void),
    handle: *mut c_void,
}

impl Drop for RetainedAudio {
    fn drop(&mut self) {
        unsafe { (self.release)(self.handle) };
    }
}

fn rate_or(default: f64, candidate: f64) -> f64 {
    if candidate.is_finite() && candidate >= MIN_SOURCE_RATE {
        candidate
    } else {
        default
    }
}

/// Runs the offline VariAudio-style F0 analysis for one region and commits
/// the result into its note table. Returns false when the region changed, the
/// audio is unusable, or the host rejected the commit.
#[no_mangle]
pub unsafe extern "C" fn hirari_region_pitch_analyze(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    sample_rate: f64,
) -> bool {
    if host.is_null() || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    let Some(ops) = host_ops() else {
        return false;
    };
    let (Some(find_region), Some(apply), Some(retain_audio), Some(audio_view), Some(release_audio)) =
        (
            ops.find_region,
            ops.apply_edit,
            ops.retain_audio,
            ops.audio_view,
            ops.release_audio,
        )
    else {
        return false;
    };

    let mut expected = HirariRegionState::default();
    if !find_region(host, track_id, region_id, &mut expected) {
        return false;
    }
    let mut generation = 0u64;
    if let Some(token) = ops.track_token {
        if !token(host, track_id, &mut generation, ptr::null_mut()) {
            return false;
        }
    }

    let source_rate = rate_or(sample_rate, expected.source_sample_rate);
    let timeline_rate = rate_or(sample_rate, expected.timeline_sample_rate);
    let source_span = if expected.source_length > 0 {
        expected.source_length
    } else {
        expected.len
    };
    if expected.len == 0 || source_span == 0 || source_span > MAX_ANALYSIS_SAMPLES {
        return false;
    }

    let handle = retain_audio(host, track_id, region_id);
    if handle.is_null() {
        return false;
    }
    let audio = RetainedAudio {
        release: release_audio,
        handle,
    };

    let mut channels: Vec<*const f32> = vec![ptr::null(); MAX_CHANNELS];
    let mut samples = 0u64;
    let channel_count = audio_view(audio.handle, channels.as_mut_ptr(), channels.len(), &mut samples);
    if channel_count == 0
        || channel_count as usize > channels.len()
        || samples == 0
        || samples > MAX_ANALYSIS_SAMPLES
        || expected.source_offset > samples
        || source_span > samples - expected.source_offset
    {
        return false;
    }

    let frames_per_timeline_frame = {
        let value = expected.source_frames_per_timeline_frame();
        if expected.warp_ratio.is_finite() && value > 0.0 {
            value
        } else {
            1.0
        }
    };
    let markers: Vec<HirariWarpMarker> =
        warp_markers_of(&ops, host, track_id, region_id).unwrap_or_default();

    let analysis = hirari_audio_pitch_analyze_region(
        channels.as_ptr(),
        channel_count as usize,
        expected.source_offset as usize,
        source_span as usize,
        u8::from(expected.reverse),
        source_rate,
        timeline_rate,
        expected.len,
        frames_per_timeline_frame,
        markers.as_ptr(),
        markers.len(),
        CONFIG_MIN_HZ,
        CONFIG_MAX_HZ,
        CONFIG_THRESHOLD,
        CONFIG_NOTE_CHANGE_CENTS,
        CONFIG_NOTE_CHANGE_FRAMES,
    );
    if analysis.is_null() {
        return false;
    }

    let payload = HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_PITCH_COMMIT,
        // The host takes ownership of the analysis result on every path.
        blob: analysis.cast(),
        blob_len: 0,
        state: expected,
        managed: false,
        generation,
    };
    apply(host, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_or_falls_back_below_the_floor() {
        assert_eq!(rate_or(44_100.0, 48_000.0), 48_000.0);
        assert_eq!(rate_or(44_100.0, 0.0), 44_100.0);
        assert_eq!(rate_or(44_100.0, f64::NAN), 44_100.0);
        assert_eq!(rate_or(44_100.0, 7_999.0), 44_100.0);
    }

    #[test]
    fn null_host_and_invalid_rate_are_rejected() {
        assert!(!unsafe { hirari_region_pitch_analyze(ptr::null_mut(), 1, 1, 48_000.0) });
        assert!(!unsafe { hirari_region_pitch_analyze(1usize as *mut c_void, 1, 1, 0.0) });
        assert!(!unsafe { hirari_region_pitch_analyze(1usize as *mut c_void, 1, 1, f64::NAN) });
    }
}
