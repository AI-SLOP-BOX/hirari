#[no_mangle]
pub unsafe extern "C" fn hirari_bus_accumulate_stereo_reference(
    bus_left: *mut f32,
    bus_right: *mut f32,
    input_left: *const f32,
    input_right: *const f32,
    frames: u32,
    gain: f32,
) -> bool {
    if bus_left.is_null()
        || bus_right.is_null()
        || input_left.is_null()
        || input_right.is_null()
        || frames == 0
        || frames > 8192
        || !gain.is_finite()
    {
        return false;
    }
    for frame in 0..frames as usize {
        let left = *input_left.add(frame);
        let right = *input_right.add(frame);
        *bus_left.add(frame) += if left.is_finite() { left } else { 0.0 } * gain;
        *bus_right.add(frame) += if right.is_finite() { right } else { 0.0 } * gain;
    }
    true
}
