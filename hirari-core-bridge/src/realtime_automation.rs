//! Sample-accurate automation interpolation used by the audio-thread Track renderer.

use std::slice;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeAutomationPoint {
    pub time: f64,
    pub value: f32,
    pub curve: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativePluginAutomationLaneView {
    pub processor_index: u32,
    pub parameter_id: u32,
    pub points: *const NativeAutomationPoint,
    pub point_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeTimedParameterEvent {
    pub processor_index: u32,
    pub parameter_id: u32,
    pub sample_offset: u32,
    pub normalized_value: f32,
}

fn evaluate_points(points: &[NativeAutomationPoint], time: f64, hint: &mut usize) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    if points.len() == 1 {
        return points[0].value;
    }
    if !time.is_finite() || time <= points[0].time {
        *hint = 0;
        return points[0].value;
    }
    let count = points.len();
    if time >= points[count - 1].time {
        *hint = count - 1;
        return points[count - 1].value;
    }

    let mut index = *hint;
    if index >= count - 1 {
        index = 0;
    }
    if !(points[index].time <= time && time < points[index + 1].time) {
        if index + 2 < count && points[index + 1].time <= time && time < points[index + 2].time {
            index += 1;
        } else {
            index = points.partition_point(|point| point.time <= time) - 1;
        }
        *hint = index;
    }

    let left = points[index];
    let right = points[index + 1];
    let range = right.time - left.time;
    if range <= 1.0e-9 {
        return left.value;
    }
    let amount = (((time - left.time) / range) as f32).clamp(0.0, 1.0);
    let curve = left.curve.clamp(-1.0, 1.0);
    let shaped = (amount + curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount)).clamp(0.0, 1.0);
    left.value + (right.value - left.value) * shaped
}

/// Evaluates one sorted automation lane while retaining its segment cursor.
/// The cursor fast path handles sequential sample rendering; seeks use a binary search.
#[no_mangle]
pub unsafe extern "C" fn hirari_realtime_automation_evaluate(
    points: *const NativeAutomationPoint,
    count: usize,
    time: f64,
    last_index_hint: *mut usize,
) -> f32 {
    if count == 0 || points.is_null() || last_index_hint.is_null() {
        return 0.0;
    }
    let points = unsafe { slice::from_raw_parts(points, count) };
    let hint = unsafe { &mut *last_index_hint };
    evaluate_points(points, time, hint)
}

/// Applies gain and equal-power pan automation to the live and optional dry
/// stereo buffers in one call. The routine uses caller-owned cursors and never
/// allocates, locks, or retains any buffer memory.
#[no_mangle]
pub unsafe extern "C" fn hirari_realtime_automation_apply_stereo_block(
    gain_points: *const NativeAutomationPoint,
    gain_count: usize,
    gain_hint: *mut usize,
    pan_points: *const NativeAutomationPoint,
    pan_count: usize,
    pan_hint: *mut usize,
    start_time: f64,
    left: *mut f32,
    right: *mut f32,
    dry_left: *mut f32,
    dry_right: *mut f32,
    frames: u32,
) {
    if frames == 0 || left.is_null() || right.is_null() {
        return;
    }
    if (gain_count != 0 && (gain_points.is_null() || gain_hint.is_null()))
        || (pan_count != 0 && (pan_points.is_null() || pan_hint.is_null()))
    {
        return;
    }
    let frames = frames as usize;
    let left = unsafe { slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { slice::from_raw_parts_mut(right, frames) };
    let has_dry = !dry_left.is_null() && !dry_right.is_null();
    let mut dry_left = if has_dry {
        Some(unsafe { slice::from_raw_parts_mut(dry_left, frames) })
    } else {
        None
    };
    let mut dry_right = if has_dry {
        Some(unsafe { slice::from_raw_parts_mut(dry_right, frames) })
    } else {
        None
    };
    let gain_points = if gain_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(gain_points, gain_count) }
    };
    let pan_points = if pan_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(pan_points, pan_count) }
    };
    let mut local_gain_hint = 0;
    let mut local_pan_hint = 0;
    let gain_hint = if gain_hint.is_null() {
        &mut local_gain_hint
    } else {
        unsafe { &mut *gain_hint }
    };
    let pan_hint = if pan_hint.is_null() {
        &mut local_pan_hint
    } else {
        unsafe { &mut *pan_hint }
    };
    let pi = std::f32::consts::PI;
    for sample in 0..frames {
        let time = start_time + sample as f64;
        if !gain_points.is_empty() {
            let gain = evaluate_points(gain_points, time, gain_hint).clamp(0.0, 2.0);
            left[sample] *= gain;
            right[sample] *= gain;
            if let (Some(dry_left), Some(dry_right)) =
                (dry_left.as_deref_mut(), dry_right.as_deref_mut())
            {
                dry_left[sample] *= gain;
                dry_right[sample] *= gain;
            }
        }
        if !pan_points.is_empty() {
            let pan = evaluate_points(pan_points, time, pan_hint).clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.25 * pi;
            let left_gain = angle.cos() * 1.414_213_5_f32;
            let right_gain = angle.sin() * 1.414_213_5_f32;
            left[sample] *= left_gain;
            right[sample] *= right_gain;
            if let (Some(dry_left), Some(dry_right)) =
                (dry_left.as_deref_mut(), dry_right.as_deref_mut())
            {
                dry_left[sample] *= left_gain;
                dry_right[sample] *= right_gain;
            }
        }
    }
}

/// Samples each active plugin automation lane into a bounded block event list.
/// Lane descriptors and point storage are immutable snapshots prepared on the
/// control thread; output storage is supplied by Track on the audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_automation_render_block(
    lanes: *const NativePluginAutomationLaneView,
    lane_count: usize,
    playhead: u64,
    frames: u32,
    output_events: *mut NativeTimedParameterEvent,
    output_capacity: usize,
) -> usize {
    if frames == 0
        || output_capacity == 0
        || output_events.is_null()
        || (lane_count != 0 && lanes.is_null())
    {
        return 0;
    }
    let lanes = if lane_count == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(lanes, lane_count) }
    };
    let mut output = unsafe { slice::from_raw_parts_mut(output_events, output_capacity) };
    let active_lane_count = lanes.iter().filter(|lane| lane.point_count != 0).count();
    if active_lane_count == 0 {
        return 0;
    }
    let per_lane_budget = (output_capacity / active_lane_count).max(1);
    let block_end = playhead.saturating_add(frames as u64);
    let automation_time = playhead as f64;
    let mut output_count = 0usize;

    for lane in lanes {
        if lane.point_count == 0 || lane.points.is_null() {
            continue;
        }
        let points = unsafe { slice::from_raw_parts(lane.points, lane.point_count) };
        let mut hint = 0usize;
        let append = |events: &mut [NativeTimedParameterEvent],
                      count: &mut usize,
                      offset: u32,
                      value: f32| {
            if *count >= events.len() {
                return;
            }
            events[*count] = NativeTimedParameterEvent {
                processor_index: lane.processor_index,
                parameter_id: lane.parameter_id,
                sample_offset: offset,
                normalized_value: value.clamp(0.0, 1.0),
            };
            *count += 1;
        };
        let lane_event_start = output_count;
        append(
            &mut output,
            &mut output_count,
            0,
            evaluate_points(points, automation_time, &mut hint),
        );
        let mut remaining_lane_budget = per_lane_budget.saturating_sub(1);
        if remaining_lane_budget == 0 || frames <= 1 {
            continue;
        }

        let first_point = points.partition_point(|point| point.time <= automation_time);
        let after_block = points.partition_point(|point| point.time < block_end as f64);
        let point_count = after_block.saturating_sub(first_point);
        let point_slots = point_count.min(remaining_lane_budget);
        if point_count <= point_slots {
            for point in &points[first_point..after_block] {
                if point.time < 0.0 || point.time > u64::MAX as f64 {
                    continue;
                }
                let sample = point.time as u64;
                if sample > playhead && sample < block_end {
                    append(
                        &mut output,
                        &mut output_count,
                        (sample - playhead) as u32,
                        point.value,
                    );
                }
            }
        } else if point_slots == 1 {
            if let Some(point) = points.get(after_block.saturating_sub(1)) {
                let sample = point.time as u64;
                append(
                    &mut output,
                    &mut output_count,
                    sample.saturating_sub(playhead) as u32,
                    point.value,
                );
            }
        } else if point_slots > 1 {
            for selected in 0..point_slots {
                let point_index = selected * (point_count - 1) / (point_slots - 1);
                let point = points[first_point + point_index];
                let sample = point.time as u64;
                append(
                    &mut output,
                    &mut output_count,
                    sample.saturating_sub(playhead) as u32,
                    point.value,
                );
            }
        }

        remaining_lane_budget -= point_slots;
        let grid_slots = remaining_lane_budget.min((frames - 1) as usize);
        for grid in 1..=grid_slots {
            let offset = (grid as u64 * (frames as u64 - 1) / grid_slots as u64) as u32;
            let sample = playhead.saturating_add(offset as u64);
            let exact_point = points.partition_point(|point| point.time < sample as f64);
            if points
                .get(exact_point)
                .is_some_and(|point| point.time == sample as f64)
            {
                continue;
            }
            let value = evaluate_points(points, sample as f64, &mut hint);
            append(&mut output, &mut output_count, offset, value);
        }
        if grid_slots == 0 && output_count - lane_event_start < per_lane_budget && frames > 1 {
            let offset = frames - 1;
            let sample = playhead.saturating_add(offset as u64);
            let exact_point = points.partition_point(|point| point.time < sample as f64);
            if !points
                .get(exact_point)
                .is_some_and(|point| point.time == sample as f64)
            {
                let value = evaluate_points(points, sample as f64, &mut hint);
                append(&mut output, &mut output_count, offset, value);
            }
        }
    }

    output[..output_count].sort_unstable_by_key(|event| {
        (
            event.sample_offset,
            event.processor_index,
            event.parameter_id,
        )
    });
    output_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_automation_reference_evaluate(
            points: *const NativeAutomationPoint,
            count: usize,
            time: f64,
            last_index_hint: *mut usize,
        ) -> f32;
        fn hirari_automation_reference_apply_stereo_block(
            gain_points: *const NativeAutomationPoint,
            gain_count: usize,
            gain_hint: *mut usize,
            pan_points: *const NativeAutomationPoint,
            pan_count: usize,
            pan_hint: *mut usize,
            start_time: f64,
            left: *mut f32,
            right: *mut f32,
            dry_left: *mut f32,
            dry_right: *mut f32,
            frames: u32,
        );
        fn hirari_plugin_automation_reference_render_block(
            lanes: *const NativePluginAutomationLaneView,
            lane_count: usize,
            playhead: u64,
            frames: u32,
            output_events: *mut NativeTimedParameterEvent,
            output_capacity: usize,
        ) -> usize;
    }

    #[test]
    fn automation_point_abi_matches_native_project_point() {
        assert_eq!(std::mem::size_of::<NativeAutomationPoint>(), 16);
        assert_eq!(std::mem::offset_of!(NativeAutomationPoint, value), 8);
        assert_eq!(std::mem::offset_of!(NativeAutomationPoint, curve), 12);
    }

    #[test]
    fn plugin_lane_and_parameter_event_abis_match_cpp() {
        assert_eq!(std::mem::size_of::<NativePluginAutomationLaneView>(), 24);
        assert_eq!(
            std::mem::offset_of!(NativePluginAutomationLaneView, points),
            8
        );
        assert_eq!(
            std::mem::offset_of!(NativePluginAutomationLaneView, point_count),
            16
        );
        assert_eq!(std::mem::size_of::<NativeTimedParameterEvent>(), 16);
        assert_eq!(
            std::mem::offset_of!(NativeTimedParameterEvent, normalized_value),
            12
        );
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn cached_automation_evaluation_matches_cpp_for_playback_and_seeks() {
        let points: Vec<_> = (0..257)
            .map(|index| NativeAutomationPoint {
                time: index as f64 * 17.25,
                value: ((index * 37 % 101) as f32) / 100.0,
                curve: ((index * 19 % 201) as f32 - 100.0) / 100.0,
            })
            .collect();
        let mut times: Vec<f64> = (0..5000).map(|sample| sample as f64 * 0.75).collect();
        times.extend([4_000.25, 250.0, 4_400.0, -1.0, 100_000.0, 0.0]);
        let mut rust_hint = 0usize;
        let mut cpp_hint = 0usize;
        for time in times {
            let rust = unsafe {
                hirari_realtime_automation_evaluate(
                    points.as_ptr(),
                    points.len(),
                    time,
                    &mut rust_hint,
                )
            };
            let cpp = unsafe {
                hirari_automation_reference_evaluate(
                    points.as_ptr(),
                    points.len(),
                    time,
                    &mut cpp_hint,
                )
            };
            assert!((rust - cpp).abs() <= 1.0e-6, "time {time}: {rust} != {cpp}");
            assert_eq!(rust_hint, cpp_hint, "cursor at time {time}");
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_gain_and_pan_application_matches_cpp_with_dry_mirror() {
        let gain_points = [
            NativeAutomationPoint {
                time: 0.0,
                value: 0.25,
                curve: -0.7,
            },
            NativeAutomationPoint {
                time: 192.0,
                value: 1.8,
                curve: 0.6,
            },
            NativeAutomationPoint {
                time: 420.0,
                value: 0.65,
                curve: 0.2,
            },
            NativeAutomationPoint {
                time: 900.0,
                value: 1.2,
                curve: 0.0,
            },
        ];
        let pan_points = [
            NativeAutomationPoint {
                time: 0.0,
                value: -0.85,
                curve: 0.4,
            },
            NativeAutomationPoint {
                time: 256.0,
                value: 0.72,
                curve: -0.5,
            },
            NativeAutomationPoint {
                time: 640.0,
                value: -0.15,
                curve: 0.8,
            },
        ];
        let mut actual_left: Vec<_> = (0..256).map(|i| (i as f32 * 0.037).sin()).collect();
        let mut actual_right: Vec<_> = (0..256).map(|i| (i as f32 * 0.023).cos()).collect();
        let mut actual_dry_left: Vec<_> = actual_left.iter().map(|v| v * 0.37).collect();
        let mut actual_dry_right: Vec<_> = actual_right.iter().map(|v| v * 0.61).collect();
        let (mut expected_left, mut expected_right) = (actual_left.clone(), actual_right.clone());
        let (mut expected_dry_left, mut expected_dry_right) =
            (actual_dry_left.clone(), actual_dry_right.clone());
        let (mut rust_gain_hint, mut cpp_gain_hint) = (0usize, 0usize);
        let (mut rust_pan_hint, mut cpp_pan_hint) = (0usize, 0usize);
        let start_time = 88.0;

        unsafe {
            hirari_realtime_automation_apply_stereo_block(
                gain_points.as_ptr(),
                gain_points.len(),
                &mut rust_gain_hint,
                pan_points.as_ptr(),
                pan_points.len(),
                &mut rust_pan_hint,
                start_time,
                actual_left.as_mut_ptr(),
                actual_right.as_mut_ptr(),
                actual_dry_left.as_mut_ptr(),
                actual_dry_right.as_mut_ptr(),
                256,
            );
            hirari_automation_reference_apply_stereo_block(
                gain_points.as_ptr(),
                gain_points.len(),
                &mut cpp_gain_hint,
                pan_points.as_ptr(),
                pan_points.len(),
                &mut cpp_pan_hint,
                start_time,
                expected_left.as_mut_ptr(),
                expected_right.as_mut_ptr(),
                expected_dry_left.as_mut_ptr(),
                expected_dry_right.as_mut_ptr(),
                256,
            );
        }
        for (actual, expected) in actual_left
            .iter()
            .zip(&expected_left)
            .chain(actual_right.iter().zip(&expected_right))
            .chain(actual_dry_left.iter().zip(&expected_dry_left))
            .chain(actual_dry_right.iter().zip(&expected_dry_right))
        {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "{actual} != {expected}"
            );
        }
        assert_eq!(rust_gain_hint, cpp_gain_hint);
        assert_eq!(rust_pan_hint, cpp_pan_hint);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_plugin_event_scheduler_matches_cpp_budgets_and_sampling() {
        let points: Vec<Vec<_>> = (0..4)
            .map(|lane| {
                (0..48)
                    .map(|point| NativeAutomationPoint {
                        time: (point * 7) as f64,
                        value: ((point * 29 + lane * 13) % 137) as f32 / 100.0,
                        curve: ((point * 17 + lane * 11) % 201) as f32 / 100.0 - 1.0,
                    })
                    .collect()
            })
            .collect();
        let lanes: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(lane, points)| NativePluginAutomationLaneView {
                processor_index: (lane % 2) as u32,
                parameter_id: 300 + lane as u32,
                points: points.as_ptr(),
                point_count: points.len(),
            })
            .collect();
        let capacity = 24;
        let mut rust_events = vec![NativeTimedParameterEvent::default(); capacity];
        let mut cpp_events = vec![NativeTimedParameterEvent::default(); capacity];
        let rust_count = unsafe {
            hirari_plugin_automation_render_block(
                lanes.as_ptr(),
                lanes.len(),
                70,
                128,
                rust_events.as_mut_ptr(),
                capacity,
            )
        };
        let cpp_count = unsafe {
            hirari_plugin_automation_reference_render_block(
                lanes.as_ptr(),
                lanes.len(),
                70,
                128,
                cpp_events.as_mut_ptr(),
                capacity,
            )
        };
        assert_eq!(rust_count, cpp_count);
        for (rust, cpp) in rust_events[..rust_count]
            .iter()
            .zip(&cpp_events[..cpp_count])
        {
            assert_eq!(rust.processor_index, cpp.processor_index);
            assert_eq!(rust.parameter_id, cpp.parameter_id);
            assert_eq!(rust.sample_offset, cpp.sample_offset);
            assert!((rust.normalized_value - cpp.normalized_value).abs() <= 1.0e-6);
        }

        let sparse_points = [
            NativeAutomationPoint {
                time: 0.0,
                value: 0.1,
                curve: 0.0,
            },
            NativeAutomationPoint {
                time: 1_000.0,
                value: 0.9,
                curve: 0.4,
            },
        ];
        let sparse_lane = [NativePluginAutomationLaneView {
            processor_index: 3,
            parameter_id: 777,
            points: sparse_points.as_ptr(),
            point_count: sparse_points.len(),
        }];
        let mut sparse_rust_events = [NativeTimedParameterEvent::default(); 8];
        let mut sparse_cpp_events = [NativeTimedParameterEvent::default(); 8];
        let sparse_rust_count = unsafe {
            hirari_plugin_automation_render_block(
                sparse_lane.as_ptr(),
                1,
                0,
                32,
                sparse_rust_events.as_mut_ptr(),
                sparse_rust_events.len(),
            )
        };
        let sparse_cpp_count = unsafe {
            hirari_plugin_automation_reference_render_block(
                sparse_lane.as_ptr(),
                1,
                0,
                32,
                sparse_cpp_events.as_mut_ptr(),
                sparse_cpp_events.len(),
            )
        };
        assert_eq!(sparse_rust_count, sparse_cpp_count);
        assert_eq!(sparse_rust_count, 8);
        for (rust, cpp) in sparse_rust_events[..sparse_rust_count]
            .iter()
            .zip(&sparse_cpp_events[..sparse_cpp_count])
        {
            assert_eq!(rust.sample_offset, cpp.sample_offset);
            assert!((rust.normalized_value - cpp.normalized_value).abs() <= 1.0e-6);
        }
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_plugin_event_scheduler_respects_global_capacity() {
        let points: Vec<Vec<_>> = (0..12)
            .map(|lane| {
                vec![NativeAutomationPoint {
                    time: 0.0,
                    value: lane as f32 / 11.0,
                    curve: 0.0,
                }]
            })
            .collect();
        let lanes: Vec<_> = points
            .iter()
            .enumerate()
            .map(|(lane, points)| NativePluginAutomationLaneView {
                processor_index: 0,
                parameter_id: lane as u32,
                points: points.as_ptr(),
                point_count: 1,
            })
            .collect();
        let mut rust_events = [NativeTimedParameterEvent::default(); 8];
        let mut cpp_events = [NativeTimedParameterEvent::default(); 8];
        let rust_count = unsafe {
            hirari_plugin_automation_render_block(
                lanes.as_ptr(),
                lanes.len(),
                0,
                64,
                rust_events.as_mut_ptr(),
                rust_events.len(),
            )
        };
        let cpp_count = unsafe {
            hirari_plugin_automation_reference_render_block(
                lanes.as_ptr(),
                lanes.len(),
                0,
                64,
                cpp_events.as_mut_ptr(),
                cpp_events.len(),
            )
        };
        assert_eq!(rust_count, cpp_count);
        assert_eq!(rust_count, 8);
        assert_eq!(&rust_events[..rust_count], &cpp_events[..cpp_count]);
    }
}
