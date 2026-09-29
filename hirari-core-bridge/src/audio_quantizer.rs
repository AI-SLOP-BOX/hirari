use std::ffi::c_void;
use crate::region_warp::HirariWarpMarker;

const MAX_WARP_SAMPLES: u64 = 16_777_216;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioQuantizeChannelView {
    pub samples: *const f32,
    pub span: u64,
    pub source_frames_per_timeline_frame: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HirariAudioQuantizerMapPoint {
    pub source_sample: u64,
    pub timeline_sample: u64,
}

/// Projects the shared timeline map into one region's source coordinates,
/// preserving only strictly increasing points and forcing both endpoints.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_project_group_map(
    map: *const HirariAudioQuantizerMapPoint,
    map_count: usize,
    source_span: u64,
    timeline_span: u64,
    source_frames_per_timeline_frame: f64,
    output: *mut HirariWarpMarker,
    output_capacity: usize,
    output_count: *mut usize,
) -> bool {
    if map.is_null()
        || map_count == 0
        || source_span == 0
        || timeline_span == 0
        || !source_frames_per_timeline_frame.is_finite()
        || source_frames_per_timeline_frame <= 0.0
        || output.is_null()
        || output_count.is_null()
        || output_capacity < map_count.saturating_add(2)
    {
        return false;
    }
    let map = std::slice::from_raw_parts(map, map_count);
    let mut markers = Vec::with_capacity(map_count + 2);
    for (index, point) in map.iter().enumerate() {
        let source_sample = if index + 1 == map_count {
            source_span
        } else {
            ((point.source_sample as f64 * source_frames_per_timeline_frame).round() as u64)
                .min(source_span)
        };
        let timeline_sample = point.timeline_sample;
        if markers.last().is_some_and(|previous: &HirariWarpMarker| {
            source_sample <= previous.source_sample || timeline_sample <= previous.timeline_sample
        }) {
            continue;
        }
        markers.push(HirariWarpMarker {
            source_sample,
            timeline_sample,
            transient: (index > 0 && index + 1 < map_count) as u8,
        });
    }
    if markers.first().is_none_or(|marker| marker.source_sample != 0 || marker.timeline_sample != 0)
    {
        markers.insert(0, HirariWarpMarker::default());
    }
    let replace_with_endpoint = markers.last().is_some_and(|marker| {
        marker.source_sample >= source_span || marker.timeline_sample >= timeline_span
    });
    if replace_with_endpoint {
        if let Some(last) = markers.last_mut() {
            *last = HirariWarpMarker {
                source_sample: source_span,
                timeline_sample: timeline_span,
                transient: 0,
            };
        }
    } else {
        markers.push(HirariWarpMarker {
            source_sample: source_span,
            timeline_sample: timeline_span,
            transient: 0,
        });
    }
    if markers.len() < 2 || markers.len() > output_capacity {
        return false;
    }
    std::slice::from_raw_parts_mut(output, markers.len()).copy_from_slice(&markers);
    *output_count = markers.len();
    true
}

/// RMS-normalizes each microphone, resamples it at the timeline rate, and
/// combines the channels into one phase-coherent analysis envelope.
#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_build_group_envelope(
    channels: *const HirariAudioQuantizeChannelView,
    channel_count: usize,
    timeline_length: u64,
    output: *mut f32,
    output_capacity: usize,
) -> bool {
    if channels.is_null()
        || output.is_null()
        || channel_count == 0
        || channel_count > 32
        || timeline_length < 2
        || timeline_length > MAX_WARP_SAMPLES
        || output_capacity < timeline_length as usize
    {
        return false;
    }
    let channels = std::slice::from_raw_parts(channels, channel_count);
    let mut rms_values = Vec::with_capacity(channel_count);
    let mut strongest_rms = 0.0_f64;
    for channel in channels {
        if channel.samples.is_null()
            || channel.span < 2
            || !channel.source_frames_per_timeline_frame.is_finite()
            || channel.source_frames_per_timeline_frame <= 0.0
        {
            return false;
        }
        let mut energy = 0.0_f64;
        let mut finite_samples = 0_u64;
        for frame in 0..channel.span as usize {
            let sample = unsafe { *channel.samples.add(frame) };
            if sample.is_finite() {
                energy += sample as f64 * sample as f64;
                finite_samples += 1;
            }
        }
        let rms = if finite_samples > 0 {
            (energy / finite_samples as f64).sqrt()
        } else {
            0.0
        };
        if !rms.is_finite() {
            return false;
        }
        strongest_rms = strongest_rms.max(rms);
        rms_values.push(rms);
    }
    if strongest_rms <= 1.0e-12 {
        return false;
    }
    let rms_floor = (strongest_rms * 0.05).max(1.0e-9);
    let scales: Vec<f64> = rms_values
        .iter()
        .map(|rms| 1.0 / rms.max(rms_floor))
        .collect();
    let output = std::slice::from_raw_parts_mut(output, timeline_length as usize);
    for (frame, output_sample) in output.iter_mut().enumerate() {
        let mut energy = 0.0_f64;
        let mut active_channels = 0_u32;
        for (channel, scale) in channels.iter().zip(scales.iter().copied()) {
            let source_position = (frame as f64 * channel.source_frames_per_timeline_frame)
                .clamp(0.0, (channel.span - 1) as f64);
            let left = source_position as usize;
            let right = (left + 1).min(channel.span as usize - 1);
            let fraction = source_position - left as f64;
            let a = unsafe { *channel.samples.add(left) };
            let b = unsafe { *channel.samples.add(right) };
            if !a.is_finite() || !b.is_finite() {
                continue;
            }
            let normalized = (a as f64 + (b as f64 - a as f64) * fraction) * scale;
            if !normalized.is_finite() {
                continue;
            }
            energy += normalized * normalized;
            active_channels += 1;
        }
        *output_sample = if active_channels > 0 {
            (energy / active_channels as f64).sqrt() as f32
        } else {
            0.0
        };
    }
    true
}

fn sample_windowed_sinc(input: &[f32], position: f64, source_per_output: f64) -> f32 {
    if input.is_empty()
        || !position.is_finite()
        || !source_per_output.is_finite()
        || source_per_output <= 0.0
    {
        return 0.0;
    }
    let position = position.clamp(0.0, (input.len() - 1) as f64);
    let base = position.floor() as i64;
    let cutoff = 1.0 / source_per_output.max(1.0);
    let mut sum = 0.0;
    let mut weight_sum = 0.0;
    for tap in -3..=4 {
        let distance = (base + tap) as f64 - position;
        let normalized = distance / 4.0;
        if normalized.abs() >= 1.0 {
            continue;
        }
        let argument = distance * cutoff;
        let sinc = if argument.abs() < 1.0e-12 {
            1.0
        } else {
            (std::f64::consts::PI * argument).sin() / (std::f64::consts::PI * argument)
        };
        let window = 0.42
            + 0.5 * (std::f64::consts::PI * normalized).cos()
            + 0.08 * (2.0 * std::f64::consts::PI * normalized).cos();
        let weight = cutoff * sinc * window;
        let index = (base + tap).clamp(0, input.len() as i64 - 1) as usize;
        let sample = input[index];
        if !sample.is_finite() {
            continue;
        }
        sum += sample as f64 * weight;
        weight_sum += weight;
    }
    if !sum.is_finite() || !weight_sum.is_finite() || weight_sum.abs() < 1.0e-12 {
        return 0.0;
    }
    let result = sum / weight_sum;
    if result.is_finite() {
        result as f32
    } else {
        0.0
    }
}

fn quantize(
    input: &[f32],
    output: &mut [f32],
    bpm: f32,
    sample_rate: f64,
    strength: f32,
    swing: f32,
) -> bool {
    let len = input.len();
    if len == 0
        || len as u64 > MAX_WARP_SAMPLES
        || output.len() < len
        || !bpm.is_finite()
        || bpm <= 0.0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !strength.is_finite()
        || !swing.is_finite()
    {
        return false;
    }
    let step_samples = (60.0 / bpm as f64 / 4.0) * sample_rate;
    if step_samples <= 10.0 {
        return false;
    }
    let mut transients = Vec::new();
    let mut targets = Vec::new();
    transients.push(0.0);
    targets.push(0.0);
    let num_steps = (len as f64 / step_samples) as u64;
    for k in 1..num_steps {
        let start = (k as f64 - 0.5) * step_samples;
        let end = (k as f64 + 0.5) * step_samples;
        let mut peak_pos = k as f64 * step_samples;
        let mut max_amp = 0.0f32;
        let first = (start.max(0.0) as u64).min(len as u64);
        let last = (end as u64).min(len as u64);
        for i in first..last {
            let amplitude = input[i as usize].abs();
            if amplitude > max_amp {
                max_amp = amplitude;
                peak_pos = i as f64;
            }
        }
        let mut grid_pos = k as f64 * step_samples;
        if k % 2 == 1 {
            grid_pos += swing.clamp(-1.0, 1.0) as f64 * 0.3 * step_samples;
        }
        let target_pos = peak_pos + strength.clamp(0.0, 1.0) as f64 * (grid_pos - peak_pos);
        transients.push(peak_pos);
        targets.push(target_pos);
    }
    transients.push(len as f64);
    targets.push(len as f64);
    for segment in 0..transients.len().saturating_sub(1) {
        let t0 = transients[segment];
        let t1 = transients[segment + 1];
        let g0 = targets[segment];
        let g1 = targets[segment + 1];
        let out_start = (g0.max(0.0) as u64).min(len as u64);
        let out_end = (g1.max(0.0) as u64).min(len as u64);
        let out_duration = g1 - g0;
        let in_duration = t1 - t0;
        for x in out_start..out_end {
            let u = if out_duration > 0.0 {
                (x as f64 - g0) / out_duration
            } else {
                0.0
            };
            let source = t0 + u * in_duration;
            output[x as usize] = if source >= 0.0 && source <= (len - 1) as f64 {
                sample_windowed_sinc(
                    input,
                    source,
                    if out_duration > 0.0 {
                        in_duration / out_duration
                    } else {
                        1.0
                    },
                )
            } else {
                0.0
            };
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantize(
    input: *const f32,
    output: *mut f32,
    len: u64,
    bpm: f32,
    sample_rate: f64,
    strength: f32,
    swing: f32,
) -> bool {
    if input.is_null() || output.is_null() || len == 0 || len > MAX_WARP_SAMPLES {
        return false;
    }
    let source = std::slice::from_raw_parts(input, len as usize).to_vec();
    quantize(
        &source,
        std::slice::from_raw_parts_mut(output, len as usize),
        bpm,
        sample_rate,
        strength,
        swing,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantize_group(
    inputs: *const *const f32,
    outputs: *const *mut f32,
    channels: u32,
    len: u64,
    bpm: f32,
    sample_rate: f64,
    strength: f32,
    swing: f32,
) -> bool {
    if inputs.is_null()
        || outputs.is_null()
        || channels == 0
        || channels > 32
        || len == 0
        || len > MAX_WARP_SAMPLES
        || !bpm.is_finite()
        || bpm <= 0.0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !strength.is_finite()
        || !swing.is_finite()
    {
        return false;
    }
    let input_ptrs = std::slice::from_raw_parts(inputs, channels as usize);
    let output_ptrs = std::slice::from_raw_parts(outputs, channels as usize);
    if input_ptrs.iter().any(|p| p.is_null()) || output_ptrs.iter().any(|p| p.is_null()) {
        return false;
    }
    let len = len as usize;
    let mut copies = Vec::with_capacity(channels as usize);
    let mut channel_rms = Vec::with_capacity(channels as usize);
    let mut strongest_rms = 0.0f64;
    for &ptr in input_ptrs {
        let samples = std::slice::from_raw_parts(ptr, len).to_vec();
        let energy: f64 = samples
            .iter()
            .filter(|x| x.is_finite())
            .map(|x| *x as f64 * *x as f64)
            .sum();
        let rms = (energy / len as f64).sqrt();
        strongest_rms = strongest_rms.max(rms);
        channel_rms.push(rms);
        copies.push(samples);
    }
    let rms_floor = 1.0e-9f64.max(strongest_rms * 0.05);
    let scales: Vec<f64> = channel_rms
        .iter()
        .map(|rms| 1.0 / rms.max(rms_floor))
        .collect();
    let normalized_energy = |frame: usize| {
        let mut energy = 0.0;
        let mut active = 0u32;
        for channel in 0..channels as usize {
            let sample = copies[channel][frame];
            if !sample.is_finite() || channel_rms[channel] <= 1.0e-12 {
                continue;
            }
            let normalized = sample as f64 * scales[channel];
            energy += normalized * normalized;
            active += 1;
        }
        if active > 0 {
            energy / active as f64
        } else {
            0.0
        }
    };
    let step_samples = (60.0 / bpm as f64 / 4.0) * sample_rate;
    if step_samples <= 10.0 {
        return false;
    }
    let mut transients = vec![0.0];
    let mut targets = vec![0.0];
    let num_steps = (len as f64 / step_samples) as u64;
    for k in 1..num_steps {
        let start = (k as f64 - 0.5) * step_samples;
        let end = (k as f64 + 0.5) * step_samples;
        let first = (start.max(0.0) as usize).min(len);
        let last = (end.max(0.0) as usize).min(len);
        let mut peak = k as f64 * step_samples;
        let mut amplitude = 0.0;
        for frame in first..last {
            let value = normalized_energy(frame).max(0.0).sqrt();
            if value > amplitude {
                amplitude = value;
                peak = frame as f64;
            }
        }
        let mut grid = k as f64 * step_samples;
        if k & 1 != 0 {
            grid += swing.clamp(-1.0, 1.0) as f64 * 0.3 * step_samples;
        }
        transients.push(peak);
        targets.push(peak + strength.clamp(0.0, 1.0) as f64 * (grid - peak));
    }
    transients.push(len as f64);
    targets.push(len as f64);
    let mut rendered = Vec::with_capacity(channels as usize);
    for source in &copies {
        let mut output = vec![0.0f32; len];
        for segment in 0..transients.len().saturating_sub(1) {
            let t0 = transients[segment];
            let t1 = transients[segment + 1];
            let g0 = targets[segment];
            let g1 = targets[segment + 1];
            let out_start = (g0.max(0.0) as u64).min(len as u64);
            let out_end = (g1.max(0.0) as u64).min(len as u64);
            let out_duration = g1 - g0;
            let in_duration = t1 - t0;
            for x in out_start..out_end {
                let u = if out_duration > 0.0 {
                    (x as f64 - g0) / out_duration
                } else {
                    0.0
                };
                let source_position = (t0 + u * in_duration).clamp(0.0, (len - 1) as f64);
                output[x as usize] = sample_windowed_sinc(
                    source,
                    source_position,
                    if out_duration > 0.0 {
                        in_duration / out_duration
                    } else {
                        1.0
                    },
                );
            }
        }
        rendered.push(output);
    }
    for channel in 0..channels as usize {
        std::ptr::copy_nonoverlapping(rendered[channel].as_ptr(), output_ptrs[channel], len);
    }
    true
}

type SamplesToBeats = unsafe extern "C" fn(*mut c_void, u64) -> f64;
type BeatsToSamples = unsafe extern "C" fn(*mut c_void, f64) -> u64;

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_build_group_map(
    inputs: *const *const f32,
    channels: u32,
    len: u64,
    timeline_start: u64,
    grid_beats: f64,
    strength: f32,
    swing: f32,
    context: *mut c_void,
    samples_to_beats: Option<SamplesToBeats>,
    beats_to_samples: Option<BeatsToSamples>,
) -> *mut c_void {
    if inputs.is_null()
        || channels == 0
        || channels > 32
        || !(2..=MAX_WARP_SAMPLES).contains(&len)
        || timeline_start > u64::MAX - len
        || !grid_beats.is_finite()
        || grid_beats <= 0.0
        || !strength.is_finite()
        || !swing.is_finite()
    {
        return std::ptr::null_mut();
    }
    let (Some(samples_to_beats), Some(beats_to_samples)) = (samples_to_beats, beats_to_samples)
    else {
        return std::ptr::null_mut();
    };
    let inputs = std::slice::from_raw_parts(inputs, channels as usize);
    if inputs.iter().any(|pointer| pointer.is_null()) {
        return std::ptr::null_mut();
    }
    let timeline_end = timeline_start + len;
    let start_beat = samples_to_beats(context, timeline_start);
    let end_beat = samples_to_beats(context, timeline_end);
    if !start_beat.is_finite() || !end_beat.is_finite() || end_beat <= start_beat {
        return std::ptr::null_mut();
    }

    let mut channel_rms = Vec::with_capacity(channels as usize);
    let mut strongest_rms = 0.0f64;
    for &pointer in inputs {
        let samples = std::slice::from_raw_parts(pointer, len as usize);
        let energy: f64 = samples
            .iter()
            .filter(|sample| sample.is_finite())
            .map(|sample| *sample as f64 * *sample as f64)
            .sum();
        let rms = (energy / len as f64).sqrt();
        channel_rms.push(rms);
        strongest_rms = strongest_rms.max(rms);
    }
    if !strongest_rms.is_finite() || strongest_rms <= 1.0e-12 {
        return std::ptr::null_mut();
    }
    let rms_floor = 1.0e-9f64.max(strongest_rms * 0.05);
    let channel_scale: Vec<f64> = channel_rms
        .iter()
        .map(|rms| 1.0 / rms.max(rms_floor))
        .collect();
    let normalized_frame_energy = |frame: u64| {
        let mut energy = 0.0;
        let mut active = 0u32;
        for channel in 0..channels as usize {
            let sample = unsafe { *inputs[channel].add(frame as usize) };
            if !sample.is_finite() || channel_rms[channel] <= 1.0e-12 {
                continue;
            }
            let normalized = sample as f64 * channel_scale[channel];
            energy += normalized * normalized;
            active += 1;
        }
        if active > 0 {
            energy / active as f64
        } else {
            0.0
        }
    };

    let mut markers = Vec::new();
    markers.push((0, 0));
    let strength = strength.clamp(0.0, 1.0) as f64;
    let swing = swing.clamp(-1.0, 1.0) as f64;
    let first_grid = (start_beat / grid_beats).floor() + 1.0;
    let last_grid = (end_beat / grid_beats).ceil();
    if !first_grid.is_finite()
        || !last_grid.is_finite()
        || last_grid - first_grid > len as f64 + 1.0
    {
        return std::ptr::null_mut();
    }
    let mut grid_index = first_grid;
    while grid_index < last_grid {
        let grid_beat = grid_index * grid_beats;
        let first_beat = start_beat.max(grid_beat - grid_beats * 0.5);
        let last_beat = end_beat.min(grid_beat + grid_beats * 0.5);
        let first_absolute = beats_to_samples(context, first_beat);
        let last_absolute = beats_to_samples(context, last_beat);
        if last_absolute <= first_absolute || first_absolute >= timeline_end {
            grid_index += 1.0;
            continue;
        }
        let first = if first_absolute > timeline_start {
            len.min(first_absolute - timeline_start)
        } else {
            0
        };
        let last = if last_absolute > timeline_start {
            len.min(last_absolute - timeline_start)
        } else {
            0
        };
        let window = ((last - first) / 8).clamp(8, 128);
        let scan_hop = ((last - first) / 32).clamp(1, 8);
        let mut preceding_energy = 0.0;
        let mut following_energy = 0.0;
        let preceding_start = if first > window { first - window } else { 0 };
        for frame in preceding_start..first {
            preceding_energy += normalized_frame_energy(frame);
        }
        for frame in first..(first + window).min(len) {
            following_energy += normalized_frame_energy(frame);
        }
        let mut strongest_onset = 0.0;
        let mut peak_sample = first;
        let mut candidate = first;
        while candidate < last {
            let preceding_rms = (preceding_energy.max(0.0) / window as f64).sqrt();
            let following_rms = (following_energy.max(0.0) / window as f64).sqrt();
            let onset_flux = following_rms - preceding_rms;
            if onset_flux > strongest_onset {
                strongest_onset = onset_flux;
                peak_sample = candidate;
            }
            let next = last.min(candidate + scan_hop);
            for frame in candidate..next {
                if frame >= window {
                    preceding_energy -= normalized_frame_energy(frame - window);
                }
                preceding_energy += normalized_frame_energy(frame);
                following_energy -= normalized_frame_energy(frame);
                if frame <= u64::MAX - window && frame + window < len {
                    following_energy += normalized_frame_energy(frame + window);
                }
            }
            candidate = next;
        }
        if strongest_onset >= 0.025 {
            let peak_beat = samples_to_beats(context, timeline_start + peak_sample.min(len - 1));
            if !peak_beat.is_finite() {
                return std::ptr::null_mut();
            }
            let swing_beat = if grid_index % 2.0 >= 0.5 {
                swing * 0.3 * grid_beats
            } else {
                0.0
            };
            let target_beat = peak_beat + strength * (grid_beat + swing_beat - peak_beat);
            if !target_beat.is_finite() {
                return std::ptr::null_mut();
            }
            let target_absolute = beats_to_samples(context, target_beat);
            let timeline_sample = if target_absolute > timeline_start {
                len.saturating_sub(1).min(target_absolute - timeline_start)
            } else {
                1
            };
            let source_sample = peak_sample.clamp(1, len - 1);
            if source_sample > markers.last().unwrap().0
                && timeline_sample > markers.last().unwrap().1
            {
                markers.push((source_sample, timeline_sample));
            }
        }
        grid_index += 1.0;
    }
    if markers
        .last()
        .is_some_and(|marker| marker.0 < len && marker.1 < len)
    {
        markers.push((len, len));
    }
    if markers.len() < 2 || markers.last() != Some(&(len, len)) {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(markers)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_map_destroy(map: *mut c_void) {
    if !map.is_null() {
        drop(Box::from_raw(map.cast::<Vec<(u64, u64)>>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_map_count(map: *const c_void) -> usize {
    if map.is_null() {
        0
    } else {
        (*map.cast::<Vec<(u64, u64)>>()).len()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_quantizer_map_get(
    map: *const c_void,
    index: usize,
    source: *mut u64,
    timeline: *mut u64,
) -> bool {
    if map.is_null() || source.is_null() || timeline.is_null() {
        return false;
    }
    let Some((source_value, timeline_value)) = (&*map.cast::<Vec<(u64, u64)>>()).get(index) else {
        return false;
    };
    *source = *source_value;
    *timeline = *timeline_value;
    true
}

#[cfg(test)]
mod group_envelope_tests {
    use super::{
        hirari_audio_quantizer_build_group_envelope, hirari_audio_quantizer_project_group_map,
        HirariAudioQuantizeChannelView, HirariAudioQuantizerMapPoint,
    };
    use crate::region_warp::HirariWarpMarker;

    #[test]
    fn group_envelope_normalizes_microphone_gain_and_resamples() {
        let quiet = [0.5_f32; 4];
        let loud = [1.0_f32; 4];
        let channels = [
            HirariAudioQuantizeChannelView {
                samples: quiet.as_ptr(),
                span: quiet.len() as u64,
                source_frames_per_timeline_frame: 1.0,
            },
            HirariAudioQuantizeChannelView {
                samples: loud.as_ptr(),
                span: loud.len() as u64,
                source_frames_per_timeline_frame: 1.0,
            },
        ];
        let mut output = [0.0_f32; 4];
        assert!(unsafe {
            hirari_audio_quantizer_build_group_envelope(
                channels.as_ptr(), channels.len(), output.len() as u64,
                output.as_mut_ptr(), output.len(),
            )
        });
        assert_eq!(output, [1.0; 4]);
    }

    #[test]
    fn group_envelope_rejects_silence_and_short_output() {
        let silent = [0.0_f32; 4];
        let channels = [HirariAudioQuantizeChannelView {
            samples: silent.as_ptr(),
            span: silent.len() as u64,
            source_frames_per_timeline_frame: 1.0,
        }];
        let mut output = [0.0_f32; 4];
        assert!(!unsafe {
            hirari_audio_quantizer_build_group_envelope(
                channels.as_ptr(), channels.len(), 4, output.as_mut_ptr(), output.len(),
            )
        });
        assert!(!unsafe {
            hirari_audio_quantizer_build_group_envelope(
                channels.as_ptr(), channels.len(), 4, output.as_mut_ptr(), output.len() - 1,
            )
        });
    }

    #[test]
    fn shared_timeline_map_projects_to_source_with_forced_endpoints() {
        let map = [
            HirariAudioQuantizerMapPoint { source_sample: 0, timeline_sample: 0 },
            HirariAudioQuantizerMapPoint { source_sample: 2, timeline_sample: 2 },
            HirariAudioQuantizerMapPoint { source_sample: 3, timeline_sample: 2 },
            HirariAudioQuantizerMapPoint { source_sample: 4, timeline_sample: 4 },
        ];
        let mut output = [HirariWarpMarker::default(); 6];
        let mut count = 0;
        assert!(unsafe {
            hirari_audio_quantizer_project_group_map(
                map.as_ptr(), map.len(), 8, 4, 2.0,
                output.as_mut_ptr(), output.len(), &mut count,
            )
        });
        assert_eq!(count, 3);
        assert_eq!((output[0].source_sample, output[0].timeline_sample), (0, 0));
        assert_eq!((output[1].source_sample, output[1].timeline_sample), (4, 2));
        assert_eq!(output[1].transient, 1);
        assert_eq!((output[2].source_sample, output[2].timeline_sample), (8, 4));
        assert_eq!(output[2].transient, 0);
    }
}
