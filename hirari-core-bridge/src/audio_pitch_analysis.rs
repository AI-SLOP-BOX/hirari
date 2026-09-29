use crate::fft::FftPlan;
use crate::{audio_note_curve::HirariAudioNoteAnchor, region_warp::HirariWarpMarker};
use std::ffi::c_void;

const MAX_SEGMENTS: usize = 4096;
const MAX_SAMPLES: usize = 16_000_000;

#[derive(Clone, Copy)]
struct Config {
    min_hz: f64,
    max_hz: f64,
    window: usize,
    hop: usize,
    threshold: f64,
    note_change_threshold_cents: f64,
    note_change_confirmation_frames: usize,
}

#[derive(Clone, Copy)]
struct Anchor {
    position_seconds: f64,
    pitch_cents: f64,
    formant_cents: f64,
}

struct NoteSegment {
    start_seconds: f64,
    end_seconds: f64,
    detected_pitch_cents: f64,
    anchors: Vec<Anchor>,
}

impl NoteSegment {
    fn new(start_seconds: f64, end_seconds: f64, pitch: f64) -> Self {
        Self {
            start_seconds,
            end_seconds,
            detected_pitch_cents: pitch,
            anchors: Vec::new(),
        }
    }
}

struct AnalysisResult {
    segments: Vec<NoteSegment>,
}

/// Converts one detected source-time segment into region timeline seconds and
/// drops pitch anchors outside its converted extent.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_map_segment_to_timeline(
    start_seconds: *mut f64,
    end_seconds: *mut f64,
    anchors: *mut HirariAudioNoteAnchor,
    anchor_count: *mut usize,
    source_sample_rate: f64,
    timeline_sample_rate: f64,
    source_span: u64,
    timeline_length: u64,
    source_frames_per_timeline_frame: f64,
    warp_markers: *const HirariWarpMarker,
    warp_marker_count: usize,
) -> bool {
    if start_seconds.is_null()
        || end_seconds.is_null()
        || anchor_count.is_null()
        || (*anchor_count > 0 && anchors.is_null())
        || (warp_marker_count > 0 && warp_markers.is_null())
        || !source_sample_rate.is_finite()
        || source_sample_rate <= 0.0
        || !timeline_sample_rate.is_finite()
        || timeline_sample_rate <= 0.0
        || !source_frames_per_timeline_frame.is_finite()
    {
        return false;
    }
    let markers = if warp_marker_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(warp_markers, warp_marker_count)
    };
    let map_source_sample = |source_sample: f64| {
        let bounded = source_sample.clamp(0.0, source_span as f64);
        let timeline_sample = if markers.len() >= 2 {
            let upper = markers.partition_point(|marker| marker.source_sample as f64 <= bounded);
            let (left, right) = if upper == 0 {
                (&markers[0], &markers[1])
            } else if upper >= markers.len() {
                (&markers[markers.len() - 2], &markers[markers.len() - 1])
            } else {
                (&markers[upper - 1], &markers[upper])
            };
            let source_delta = right.source_sample as f64 - left.source_sample as f64;
            if source_delta <= 0.0 {
                left.timeline_sample as f64
            } else {
                let fraction = (bounded - left.source_sample as f64) / source_delta;
                (left.timeline_sample as f64
                    + fraction * (right.timeline_sample as f64 - left.timeline_sample as f64))
                    .clamp(0.0, timeline_length as f64)
            }
        } else {
            let ratio = if source_frames_per_timeline_frame > 0.0 {
                source_frames_per_timeline_frame
            } else {
                1.0
            };
            (bounded / ratio).clamp(0.0, timeline_length as f64)
        };
        timeline_sample / timeline_sample_rate
    };

    let source_start = *start_seconds * source_sample_rate;
    let source_end = *end_seconds * source_sample_rate;
    *start_seconds = map_source_sample(source_start);
    *end_seconds = map_source_sample(source_end);
    let count = *anchor_count;
    if count > 0 {
        let anchor_values = std::slice::from_raw_parts_mut(anchors, count);
        let mut write = 0usize;
        for read in 0..count {
            let mut anchor = anchor_values[read];
            anchor.position_seconds =
                map_source_sample(anchor.position_seconds * source_sample_rate);
            if anchor.position_seconds >= *start_seconds && anchor.position_seconds <= *end_seconds
            {
                anchor_values[write] = anchor;
                write += 1;
            }
        }
        *anchor_count = write;
    }
    true
}

/// Chooses the highest-energy channel, then averages each source stride into
/// one analysis sample. Reverse playback is reflected before decimation.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_select_and_decimate(
    channels: *const *const f32,
    channel_count: usize,
    source_offset: usize,
    source_span: usize,
    stride: usize,
    reverse: u8,
    output: *mut f32,
    output_capacity: usize,
) -> usize {
    if channels.is_null()
        || channel_count == 0
        || source_span == 0
        || source_span > MAX_SAMPLES
        || stride == 0
    {
        return usize::MAX;
    }
    let output_count = (source_span - 1) / stride + 1;
    if output.is_null() || output_capacity < output_count {
        return usize::MAX;
    }
    let channels = std::slice::from_raw_parts(channels, channel_count);
    let energy_stride = (source_span / 262_144).max(1);
    let mut selected_channel = None;
    let mut strongest_energy = -1.0_f64;
    for (channel_index, channel) in channels.iter().copied().enumerate() {
        if channel.is_null() {
            continue;
        }
        let mut energy = 0.0_f64;
        let mut offset = 0usize;
        while offset < source_span {
            let value = *channel.add(source_offset + offset);
            if value.is_finite() {
                energy += (value as f64) * (value as f64);
            }
            offset = offset.saturating_add(energy_stride);
        }
        if energy > strongest_energy {
            strongest_energy = energy;
            selected_channel = Some(channel_index);
        }
    }
    let Some(selected_channel) = selected_channel else {
        return usize::MAX;
    };
    let source = channels[selected_channel];
    let output = std::slice::from_raw_parts_mut(output, output_count);
    let mut output_index = 0usize;
    let mut offset = 0usize;
    while offset < source_span {
        let end = offset.saturating_add(stride).min(source_span);
        let mut sum = 0.0_f64;
        for index in offset..end {
            let playback_index = if reverse != 0 {
                source_span - 1 - index
            } else {
                index
            };
            let value = *source.add(source_offset + playback_index);
            if value.is_finite() {
                sum += value as f64;
            }
        }
        output[output_index] = (sum / (end - offset) as f64) as f32;
        output_index += 1;
        offset = end;
    }
    output_count
}

fn clamp_like_cpp(value: f64, lower: f64, upper: f64) -> f64 {
    if value < lower {
        lower
    } else if upper < value {
        upper
    } else {
        value
    }
}

fn analyze(samples: &[f32], sample_rate: f64, mut config: Config) -> Vec<NoteSegment> {
    if samples.is_empty()
        || samples.len() > MAX_SAMPLES
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
    {
        return Vec::new();
    }
    config.min_hz = clamp_like_cpp(config.min_hz, 20.0, 2000.0);
    config.max_hz = clamp_like_cpp(config.max_hz, config.min_hz + 1.0, 4000.0);
    config.window = config.window.clamp(256, 8192);
    config.hop = config.hop.clamp(64, config.window);
    config.threshold = config.threshold.clamp(0.5, 0.99);
    config.note_change_threshold_cents = config.note_change_threshold_cents.clamp(35.0, 600.0);
    config.note_change_confirmation_frames = config.note_change_confirmation_frames.clamp(1, 8);

    let min_lag = ((sample_rate / config.max_hz) as usize).max(1);
    let max_lag = (config.window - 1).min((sample_rate / config.min_hz) as usize);
    let mut fft_size = 1usize;
    while fft_size < config.window * 2 {
        fft_size <<= 1;
    }
    let Some(fft) = FftPlan::new(fft_size) else {
        return Vec::new();
    };

    let mut fft_real = vec![0.0_f32; fft_size];
    let mut fft_imag = vec![0.0_f32; fft_size];
    let mut windowed_energy = vec![0.0_f64; config.window + 1];
    let mut correlation = vec![0.0_f64; max_lag.saturating_add(1)];
    let window: Vec<f64> = (0..config.window)
        .map(|index| {
            0.5 - 0.5
                * (2.0 * std::f64::consts::PI * index as f64 / (config.window - 1) as f64).cos()
        })
        .collect();

    let mut segments = Vec::new();
    let mut active = false;
    let mut current: Option<NoteSegment> = None;
    let mut stable_pitch_cents = 0.0;
    let mut candidate_pitch_cents = 0.0;
    let mut candidate_start_seconds = 0.0;
    let mut candidate_frames = 0usize;

    let finish_current = |segments: &mut Vec<NoteSegment>,
                          current: &mut Option<NoteSegment>,
                          active: &mut bool,
                          candidate_frames: &mut usize| {
        if *active {
            if let Some(segment) = current.take() {
                if segments.len() < MAX_SEGMENTS {
                    segments.push(segment);
                }
            }
        }
        *active = false;
        *candidate_frames = 0;
    };

    let mut offset = 0usize;
    while offset < samples.len() {
        let sample_at = |index: usize| -> f64 {
            if index >= samples.len() {
                return 0.0;
            }
            let value = samples[index];
            if value.is_finite() {
                value as f64
            } else {
                0.0
            }
        };
        let available = config.window.min(samples.len() - offset);
        let mut energy = 0.0;
        let mut mean = 0.0;
        for index in 0..config.window {
            let value = sample_at(offset + index);
            energy += value * value;
            if index < available {
                mean += value;
            }
        }
        if energy < 1.0e-8 {
            finish_current(
                &mut segments,
                &mut current,
                &mut active,
                &mut candidate_frames,
            );
            offset = offset.saturating_add(config.hop);
            continue;
        }
        mean /= available.max(1) as f64;
        fft_real.fill(0.0);
        fft_imag.fill(0.0);
        windowed_energy.fill(0.0);
        for index in 0..config.window {
            let value = (sample_at(offset + index) - mean) * window[index];
            fft_real[index] = if value.is_finite() { value as f32 } else { 0.0 };
            windowed_energy[index + 1] =
                windowed_energy[index] + fft_real[index] as f64 * fft_real[index] as f64;
        }
        fft.forward(&mut fft_real, &mut fft_imag);
        for bin in 0..fft_size {
            let power = fft_real[bin] as f64 * fft_real[bin] as f64
                + fft_imag[bin] as f64 * fft_imag[bin] as f64;
            fft_real[bin] = if power.is_finite() { power as f32 } else { 0.0 };
            fft_imag[bin] = 0.0;
        }
        fft.inverse(&mut fft_real, &mut fft_imag);

        if min_lag <= max_lag {
            for lag in min_lag..=max_lag {
                let left_energy = windowed_energy[config.window - lag];
                let right_energy = windowed_energy[config.window] - windowed_energy[lag];
                let denominator = (left_energy * right_energy).max(0.0).sqrt();
                let value = if denominator > 1.0e-12 {
                    fft_real[lag] as f64 / denominator
                } else {
                    -1.0
                };
                correlation[lag] = if value.is_finite() { value } else { -1.0 };
            }
        }

        let mut best_lag = 0usize;
        let mut best = -1.0_f64;
        if min_lag <= max_lag {
            for lag in min_lag..=max_lag {
                if correlation[lag] > best {
                    best = correlation[lag];
                    best_lag = lag;
                }
            }
        }
        let voiced = best_lag > 0 && best >= config.threshold;
        if !voiced {
            finish_current(
                &mut segments,
                &mut current,
                &mut active,
                &mut candidate_frames,
            );
            offset = offset.saturating_add(config.hop);
            continue;
        }

        let mut refined_lag = best_lag as f64;
        if best_lag > min_lag && best_lag < max_lag {
            let ym = correlation[best_lag - 1];
            let y0 = best;
            let yp = correlation[best_lag + 1];
            let denominator = ym - 2.0 * y0 + yp;
            if denominator.is_finite() && denominator.abs() > 1.0e-12 {
                refined_lag += 0.5 * (ym - yp) / denominator;
            }
        }
        refined_lag = refined_lag.clamp(min_lag as f64, max_lag as f64);
        let time = offset as f64 / sample_rate;
        let hz = sample_rate / refined_lag;
        let cents = if hz.is_finite() && hz > 0.0 {
            1200.0 * (hz / 440.0).log2() + 6900.0
        } else {
            0.0
        };
        let clip_end =
            (samples.len() as f64 / sample_rate).min(time + config.window as f64 / sample_rate);

        if !active {
            if segments.len() >= MAX_SEGMENTS {
                break;
            }
            let mut segment = NoteSegment::new(time, clip_end, cents);
            segment.anchors.push(Anchor {
                position_seconds: time,
                pitch_cents: 0.0,
                formant_cents: 0.0,
            });
            current = Some(segment);
            stable_pitch_cents = cents;
            candidate_frames = 0;
            active = true;
        } else {
            let segment = current.as_mut().expect("active pitch segment");
            segment.end_seconds = clip_end;
            let deviation = cents - stable_pitch_cents;
            if deviation.abs() >= config.note_change_threshold_cents {
                if candidate_frames == 0
                    || (cents - candidate_pitch_cents).abs() >= config.note_change_threshold_cents
                {
                    candidate_pitch_cents = cents;
                    candidate_start_seconds = time;
                    candidate_frames = 1;
                } else {
                    candidate_pitch_cents +=
                        (cents - candidate_pitch_cents) / (candidate_frames + 1) as f64;
                    candidate_frames += 1;
                }
                if candidate_frames >= config.note_change_confirmation_frames
                    && (candidate_pitch_cents - stable_pitch_cents).abs()
                        >= config.note_change_threshold_cents
                {
                    if segments.len() < MAX_SEGMENTS - 1 {
                        let boundary = (0.5 * (candidate_start_seconds + time))
                            .clamp(segment.start_seconds + 1.0 / sample_rate, time);
                        segment.end_seconds = boundary;
                        segments.push(current.take().expect("active pitch segment"));
                        let mut next = NoteSegment::new(boundary, clip_end, candidate_pitch_cents);
                        next.anchors.push(Anchor {
                            position_seconds: boundary,
                            pitch_cents: 0.0,
                            formant_cents: 0.0,
                        });
                        current = Some(next);
                    } else {
                        segment.detected_pitch_cents +=
                            (candidate_pitch_cents - segment.detected_pitch_cents) * 0.1;
                    }
                    stable_pitch_cents = candidate_pitch_cents;
                    candidate_frames = 0;
                }
                offset = offset.saturating_add(config.hop);
                continue;
            }

            candidate_frames = 0;
            stable_pitch_cents += deviation.clamp(-35.0, 35.0) * 0.08;
            segment.detected_pitch_cents +=
                (stable_pitch_cents - segment.detected_pitch_cents) * 0.08;
            if (offset / config.hop) % 2 == 0 && segment.anchors.len() < 4096 {
                let delta = cents - segment.detected_pitch_cents;
                segment.anchors.push(Anchor {
                    position_seconds: time,
                    pitch_cents: delta.clamp(-2400.0, 2400.0),
                    formant_cents: 0.0,
                });
            }
        }
        offset = offset.saturating_add(config.hop);
    }
    finish_current(
        &mut segments,
        &mut current,
        &mut active,
        &mut candidate_frames,
    );
    segments.retain(|segment| {
        segment.start_seconds.is_finite()
            && segment.end_seconds.is_finite()
            && segment.end_seconds > segment.start_seconds
            && segment.detected_pitch_cents.is_finite()
    });
    segments
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_analyze(
    samples: *const f32,
    count: usize,
    sample_rate: f64,
    min_hz: f64,
    max_hz: f64,
    window: usize,
    hop: usize,
    threshold: f64,
    note_change_threshold_cents: f64,
    note_change_confirmation_frames: usize,
) -> *mut c_void {
    if samples.is_null() || count == 0 || count > MAX_SAMPLES {
        return std::ptr::null_mut();
    }
    let config = Config {
        min_hz,
        max_hz,
        window,
        hop,
        threshold,
        note_change_threshold_cents,
        note_change_confirmation_frames,
    };
    let segments = analyze(
        std::slice::from_raw_parts(samples, count),
        sample_rate,
        config,
    );
    Box::into_raw(Box::new(AnalysisResult { segments })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_result_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<AnalysisResult>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_segment_count(state: *const c_void) -> usize {
    if state.is_null() {
        return 0;
    }
    (*state.cast::<AnalysisResult>()).segments.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_get_segment(
    state: *const c_void,
    index: usize,
    start_seconds: *mut f64,
    end_seconds: *mut f64,
    detected_pitch_cents: *mut f64,
) -> bool {
    if state.is_null()
        || start_seconds.is_null()
        || end_seconds.is_null()
        || detected_pitch_cents.is_null()
    {
        return false;
    }
    let result = &*state.cast::<AnalysisResult>();
    let Some(segment) = result.segments.get(index) else {
        return false;
    };
    *start_seconds = segment.start_seconds;
    *end_seconds = segment.end_seconds;
    *detected_pitch_cents = segment.detected_pitch_cents;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_anchor_count(
    state: *const c_void,
    segment_index: usize,
) -> usize {
    if state.is_null() {
        return 0;
    }
    let result = &*state.cast::<AnalysisResult>();
    result
        .segments
        .get(segment_index)
        .map_or(0, |segment| segment.anchors.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_pitch_get_anchor(
    state: *const c_void,
    segment_index: usize,
    anchor_index: usize,
    position_seconds: *mut f64,
    pitch_cents: *mut f64,
    formant_cents: *mut f64,
) -> bool {
    if state.is_null()
        || position_seconds.is_null()
        || pitch_cents.is_null()
        || formant_cents.is_null()
    {
        return false;
    }
    let result = &*state.cast::<AnalysisResult>();
    let Some(anchor) = result
        .segments
        .get(segment_index)
        .and_then(|segment| segment.anchors.get(anchor_index))
    else {
        return false;
    };
    *position_seconds = anchor.position_seconds;
    *pitch_cents = anchor.pitch_cents;
    *formant_cents = anchor.formant_cents;
    true
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_audio_pitch_map_segment_to_timeline, hirari_audio_pitch_select_and_decimate,
    };
    use crate::{audio_note_curve::HirariAudioNoteAnchor, region_warp::HirariWarpMarker};

    #[test]
    fn channel_selection_reverse_and_decimation_match_control_path() {
        let quiet = [0.1_f32; 7];
        let loud = [0.0_f32, 2.0, f32::NAN, 6.0, 8.0, 10.0];
        let channels = [quiet.as_ptr(), loud.as_ptr()];
        let mut output = [0.0_f32; 3];
        let count = unsafe {
            hirari_audio_pitch_select_and_decimate(
                channels.as_ptr(),
                channels.len(),
                1,
                5,
                2,
                1,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        assert_eq!(count, 3);
        assert_eq!(output, [9.0, 3.0, 2.0]);
    }

    #[test]
    fn channel_decimation_rejects_missing_or_undersized_buffers() {
        let samples = [1.0_f32; 4];
        let channels = [samples.as_ptr()];
        let mut output = [0.0_f32; 1];
        assert_eq!(
            unsafe {
                hirari_audio_pitch_select_and_decimate(
                    channels.as_ptr(),
                    1,
                    0,
                    4,
                    2,
                    0,
                    output.as_mut_ptr(),
                    output.len(),
                )
            },
            usize::MAX
        );
        assert_eq!(
            unsafe {
                hirari_audio_pitch_select_and_decimate(
                    channels.as_ptr(),
                    1,
                    0,
                    4,
                    2,
                    0,
                    std::ptr::null_mut(),
                    2,
                )
            },
            usize::MAX
        );
    }

    #[test]
    fn detected_note_and_anchors_follow_warp_map_and_clip_to_note_bounds() {
        let markers = [
            HirariWarpMarker {
                source_sample: 0,
                timeline_sample: 0,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 100,
                timeline_sample: 200,
                transient: 0,
            },
            HirariWarpMarker {
                source_sample: 200,
                timeline_sample: 300,
                transient: 0,
            },
        ];
        let mut anchors = [
            HirariAudioNoteAnchor {
                position_seconds: 0.0,
                pitch_cents: 0.0,
                formant_cents: 0.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 0.5,
                pitch_cents: 10.0,
                formant_cents: 0.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 1.5,
                pitch_cents: 20.0,
                formant_cents: 0.0,
            },
            HirariAudioNoteAnchor {
                position_seconds: 2.0,
                pitch_cents: 30.0,
                formant_cents: 0.0,
            },
        ];
        let mut anchor_count = anchors.len();
        let mut start = 0.5;
        let mut end = 1.5;
        assert!(unsafe {
            hirari_audio_pitch_map_segment_to_timeline(
                &mut start,
                &mut end,
                anchors.as_mut_ptr(),
                &mut anchor_count,
                100.0,
                100.0,
                200,
                300,
                2.0,
                markers.as_ptr(),
                markers.len(),
            )
        });
        assert_eq!((start, end), (1.0, 2.5));
        assert_eq!(anchor_count, 2);
        assert_eq!(anchors[0].position_seconds, 1.0);
        assert_eq!(anchors[1].position_seconds, 2.5);
    }
}
