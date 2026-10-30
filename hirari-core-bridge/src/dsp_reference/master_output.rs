unsafe fn sanitize(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_master_output_reference_process(
    left: *mut f32,
    right: *mut f32,
    click_left: *mut f32,
    click_right: *mut f32,
    frames: u32,
    master_gain: f32,
    include_click: bool,
) {
    if left.is_null()
        || right.is_null()
        || (include_click && (click_left.is_null() || click_right.is_null()))
    {
        return;
    }
    if include_click {
        for sample in 0..frames as usize {
            let left_click = sanitize(*click_left.add(sample));
            let right_click = sanitize(*click_right.add(sample));
            *click_left.add(sample) = left_click;
            *click_right.add(sample) = right_click;
            *left.add(sample) = (sanitize(*left.add(sample)) + left_click).clamp(-16.0, 16.0);
            *right.add(sample) = (sanitize(*right.add(sample)) + right_click).clamp(-16.0, 16.0);
        }
    }
    for sample in 0..frames as usize {
        *left.add(sample) = sanitize(*left.add(sample)) * master_gain;
        *right.add(sample) = sanitize(*right.add(sample)) * master_gain;
    }
}
