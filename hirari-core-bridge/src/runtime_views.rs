//! Rust-side aggregation for telemetry and planar audio captured at native
//! device boundaries. The host owns device negotiation and block capture.

const MAX_AUDIO_INPUT_CHANNELS: usize = 32;
const MAX_AUDIO_INPUT_FRAMES: usize = 8192;
const MAX_TRACKS: usize = 256;

pub fn runtime_health_vector(
    peaks_l: &[f32],
    peaks_r: &[f32],
    sample_count: u32,
    dsp_load: f32,
    correlation: f32,
    active_voices: u32,
    telemetry_version: u64,
    device_running: bool,
    playing: bool,
    playhead: f32,
) -> Vec<f32> {
    let count = (sample_count as usize)
        .min(MAX_TRACKS)
        .min(peaks_l.len())
        .min(peaks_r.len());
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    for index in 0..count {
        if peaks_l[index].is_finite() {
            peak_l = peak_l.max(peaks_l[index].abs());
        }
        if peaks_r[index].is_finite() {
            peak_r = peak_r.max(peaks_r[index].abs());
        }
    }
    vec![
        finite_or_zero(dsp_load),
        peak_l,
        peak_r,
        finite_or_zero(correlation),
        active_voices as f32,
        count as f32,
        (telemetry_version & 0x00ff_ffff) as f32,
        if device_running { 1.0 } else { 0.0 },
        if playing { 1.0 } else { 0.0 },
        finite_or_zero(playhead),
    ]
}

pub fn interleave_planar_audio_input(
    planar: &[f32],
    channels: u32,
    frames: u32,
    channel_stride: u32,
) -> Vec<f32> {
    let channels = channels as usize;
    let frames = frames as usize;
    let stride = channel_stride as usize;
    if channels == 0
        || channels > MAX_AUDIO_INPUT_CHANNELS
        || frames == 0
        || frames > MAX_AUDIO_INPUT_FRAMES
        || stride < frames
        || channels
            .checked_mul(stride)
            .is_none_or(|required| required > planar.len())
    {
        return Vec::new();
    }

    let mut interleaved = Vec::with_capacity(channels * frames);
    for frame in 0..frames {
        for channel in 0..channels {
            interleaved.push(planar[channel * stride + frame]);
        }
    }
    interleaved
}

#[inline]
fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}
