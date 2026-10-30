use crate::realtime_automation::{
    NativeAutomationPoint, NativePluginAutomationLaneView, NativeTimedParameterEvent,
};

unsafe fn evaluate(points: *const NativeAutomationPoint, count: usize, time: f64, hint: *mut usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    if count == 1 {
        return (*points).value;
    }
    if time <= (*points).time {
        *hint = 0;
        return (*points).value;
    }
    if time >= (*points.add(count - 1)).time {
        *hint = count - 1;
        return (*points.add(count - 1)).value;
    }
    let mut index = *hint;
    if index >= count - 1 {
        index = 0;
    }
    let left = &*points.add(index);
    if !(left.time <= time && time < (*points.add(index + 1)).time) {
        if index + 2 < count
            && (*points.add(index + 1)).time <= time
            && time < (*points.add(index + 2)).time
        {
            index += 1;
        } else {
            // First point strictly after `time`, minus one.
            let mut low = 0usize;
            let mut high = count;
            while low < high {
                let middle = low + (high - low) / 2;
                if time < (*points.add(middle)).time {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            index = low - 1;
        }
        *hint = index;
    }
    let a = &*points.add(index);
    let b = &*points.add(index + 1);
    let range = b.time - a.time;
    if range <= 1.0e-9 {
        return a.value;
    }
    let amount = (((time - a.time) / range) as f32).clamp(0.0, 1.0);
    let curve = a.curve.clamp(-1.0, 1.0);
    let shaped = (amount + curve * amount * (1.0 - amount) * (1.0 - 2.0 * amount)).clamp(0.0, 1.0);
    a.value + (b.value - a.value) * shaped
}

#[no_mangle]
pub unsafe extern "C" fn hirari_automation_reference_evaluate(
    points: *const NativeAutomationPoint,
    count: usize,
    time: f64,
    last_index_hint: *mut usize,
) -> f32 {
    evaluate(points, count, time, last_index_hint)
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_automation_reference_apply_stereo_block(
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
    if gain_count != 0 {
        for sample in 0..frames as usize {
            let gain =
                evaluate(gain_points, gain_count, start_time + sample as f64, gain_hint).clamp(0.0, 2.0);
            *left.add(sample) *= gain;
            *right.add(sample) *= gain;
            if !dry_left.is_null() && !dry_right.is_null() {
                *dry_left.add(sample) *= gain;
                *dry_right.add(sample) *= gain;
            }
        }
    }
    if pan_count != 0 {
        const PI: f32 = std::f32::consts::PI;
        for sample in 0..frames as usize {
            let pan =
                evaluate(pan_points, pan_count, start_time + sample as f64, pan_hint).clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.25 * PI;
            let left_gain = angle.cos() * 1.414_213_5;
            let right_gain = angle.sin() * 1.414_213_5;
            *left.add(sample) *= left_gain;
            *right.add(sample) *= right_gain;
            if !dry_left.is_null() && !dry_right.is_null() {
                *dry_left.add(sample) *= left_gain;
                *dry_right.add(sample) *= right_gain;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_automation_reference_render_block(
    lanes: *const NativePluginAutomationLaneView,
    lane_count: usize,
    playhead: u64,
    frames: u32,
    output_events: *mut NativeTimedParameterEvent,
    output_capacity: usize,
) -> usize {
    let mut automated_lane_count = 0usize;
    for lane_index in 0..lane_count {
        if (*lanes.add(lane_index)).point_count != 0 {
            automated_lane_count += 1;
        }
    }
    if automated_lane_count == 0 || frames == 0 || output_capacity == 0 {
        return 0;
    }
    let per_lane_budget = (output_capacity / automated_lane_count).max(1);
    let block_end = if playhead <= u64::MAX - frames as u64 {
        playhead + frames as u64
    } else {
        u64::MAX
    };
    unsafe fn append(
        output_events: *mut NativeTimedParameterEvent,
        capacity: usize,
        count: &mut usize,
        lane: &NativePluginAutomationLaneView,
        offset: u32,
        value: f32,
    ) {
        if *count >= capacity {
            return;
        }
        *output_events.add(*count) = NativeTimedParameterEvent {
            processor_index: lane.processor_index,
            parameter_id: lane.parameter_id,
            sample_offset: offset,
            normalized_value: value.clamp(0.0, 1.0),
        };
        *count += 1;
    }
    let mut count = 0usize;
    for lane_index in 0..lane_count {
        let lane = &*lanes.add(lane_index);
        if lane.point_count == 0 {
            continue;
        }
        let points = lane.points;
        let point_count_all = lane.point_count;
        let mut hint = 0usize;
        let initial = evaluate(points, point_count_all, playhead as f64, &mut hint).clamp(0.0, 1.0);
        let lane_event_start = count;
        append(output_events, output_capacity, &mut count, lane, 0, initial);
        let mut remaining = per_lane_budget - 1;
        if remaining == 0 || frames <= 1 {
            continue;
        }
        // First point strictly after the playhead.
        let mut first = 0usize;
        while first < point_count_all && (*points.add(first)).time <= playhead as f64 {
            first += 1;
        }
        // First point at or after the block end.
        let mut after = first;
        while after < point_count_all && (*points.add(after)).time < block_end as f64 {
            after += 1;
        }
        let point_count = after - first;
        let point_slots = point_count.min(remaining);
        if point_count <= point_slots {
            for point_index in first..after {
                let point = &*points.add(point_index);
                if point.time < 0.0 || point.time > u64::MAX as f64 {
                    continue;
                }
                let sample = point.time as u64;
                if sample > playhead && sample < block_end {
                    append(output_events, output_capacity, &mut count, lane, (sample - playhead) as u32, point.value);
                }
            }
        } else if point_slots == 1 {
            let point = &*points.add(after - 1);
            let sample = point.time as u64;
            append(output_events, output_capacity, &mut count, lane, (sample - playhead) as u32, point.value);
        } else if point_slots > 1 {
            for selected in 0..point_slots {
                let point_index = selected * (point_count - 1) / (point_slots - 1);
                let point = &*points.add(first + point_index);
                let sample = point.time as u64;
                append(output_events, output_capacity, &mut count, lane, (sample - playhead) as u32, point.value);
            }
        }
        remaining -= point_slots;
        let grid_slots = remaining.min(frames as usize - 1);
        for grid in 1..=grid_slots {
            let offset = (grid as u64 * (frames as u64 - 1) / grid_slots as u64) as u32;
            let sample = playhead + offset as u64;
            let mut exact_index = 0usize;
            while exact_index < point_count_all
                && (*points.add(exact_index)).time < sample as f64
            {
                exact_index += 1;
            }
            if exact_index != point_count_all
                && (*points.add(exact_index)).time == sample as f64
            {
                continue;
            }
            let value = evaluate(points, point_count_all, sample as f64, &mut hint);
            append(output_events, output_capacity, &mut count, lane, offset, value);
        }
        if grid_slots == 0 && count - lane_event_start < per_lane_budget && frames > 1 {
            let offset = frames - 1;
            let sample = playhead + offset as u64;
            let mut exact_index = 0usize;
            while exact_index < point_count_all
                && (*points.add(exact_index)).time < sample as f64
            {
                exact_index += 1;
            }
            if exact_index == point_count_all || (*points.add(exact_index)).time != sample as f64 {
                let value = evaluate(points, point_count_all, sample as f64, &mut hint);
                append(output_events, output_capacity, &mut count, lane, offset, value);
            }
        }
    }
    let events = std::slice::from_raw_parts_mut(output_events, count);
    events.sort_unstable_by_key(|event| {
        (
            event.sample_offset,
            event.processor_index,
            event.parameter_id,
        )
    });
    count
}
