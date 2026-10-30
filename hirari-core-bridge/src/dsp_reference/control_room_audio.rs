unsafe fn sanitize(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_process_monitor_reference(
    left: *mut f32,
    right: *mut f32,
    talkback: *const f32,
    frames: u32,
    monitor_gain: f32,
    talkback_gain: f32,
    talkback_enabled: bool,
) {
    if left.is_null() || right.is_null() {
        return;
    }
    let monitor = monitor_gain.clamp(0.0, 4.0);
    for index in 0..frames as usize {
        let in_left = sanitize(*left.add(index));
        let in_right = sanitize(*right.add(index));
        *left.add(index) = (in_left * monitor).clamp(-16.0, 16.0);
        *right.add(index) = (in_right * monitor).clamp(-16.0, 16.0);
    }
    if talkback.is_null() || !talkback_enabled {
        return;
    }
    let gain = talkback_gain.clamp(0.0, 4.0);
    for index in 0..frames as usize {
        let sample = {
            let value = *talkback.add(index);
            if value.is_finite() {
                value.clamp(-16.0, 16.0) * gain
            } else {
                0.0
            }
        };
        *left.add(index) = (sanitize(*left.add(index)) + sample).clamp(-16.0, 16.0);
        *right.add(index) = (sanitize(*right.add(index)) + sample).clamp(-16.0, 16.0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_control_room_mix_cue_reference(
    source_left: *const f32,
    source_right: *const f32,
    click_left: *const f32,
    click_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: u32,
    cue_gain: f32,
    click_enabled: bool,
) {
    if source_left.is_null()
        || source_right.is_null()
        || output_left.is_null()
        || output_right.is_null()
    {
        return;
    }
    for index in 0..frames as usize {
        let left = sanitize(*source_left.add(index));
        let right = sanitize(*source_right.add(index));
        let click_l = if click_enabled && !click_left.is_null() {
            sanitize(*click_left.add(index))
        } else {
            0.0
        };
        let click_r = if click_enabled && !click_right.is_null() {
            sanitize(*click_right.add(index))
        } else {
            0.0
        };
        *output_left.add(index) = (left * cue_gain + click_l).clamp(-16.0, 16.0);
        *output_right.add(index) = (right * cue_gain + click_r).clamp(-16.0, 16.0);
    }
}
