#[no_mangle]
pub unsafe extern "C" fn hirari_track_eq_gain_reference(boost_db: f32, cut_db: f32) -> f32 {
    if !boost_db.is_finite() || !cut_db.is_finite() {
        return 1.0;
    }
    let db = (boost_db - cut_db).clamp(-120.0, 24.0);
    let gain = 10.0f32.powf(db / 20.0);
    if gain.is_finite() {
        gain
    } else {
        1.0
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_eq_process_reference(
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    low_gain: f32,
    high_gain: f32,
    state_l: *mut f32,
    state_r: *mut f32,
) {
    if left.is_null() || right.is_null() || state_l.is_null() || state_r.is_null() || frames == 0 {
        return;
    }
    if !(*state_l).is_finite() {
        *state_l = 0.0;
    }
    if !(*state_r).is_finite() {
        *state_r = 0.0;
    }
    let low_gain = if low_gain.is_finite() { low_gain } else { 1.0 };
    let high_gain = if high_gain.is_finite() { high_gain } else { 1.0 };
    for index in 0..frames as usize {
        let in_l = {
            let value = *left.add(index);
            if value.is_finite() {
                value.clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            }
        };
        let in_r = {
            let value = *right.add(index);
            if value.is_finite() {
                value.clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            }
        };
        *state_l += 0.02 * (in_l - *state_l);
        *state_r += 0.02 * (in_r - *state_r);
        let out_l = *state_l * low_gain + (in_l - *state_l) * high_gain;
        let out_r = *state_r * low_gain + (in_r - *state_r) * high_gain;
        if out_l.is_finite() {
            *left.add(index) = out_l;
        } else {
            *left.add(index) = 0.0;
            *state_l = 0.0;
        }
        if out_r.is_finite() {
            *right.add(index) = out_r;
        } else {
            *right.add(index) = 0.0;
            *state_r = 0.0;
        }
    }
}
