use std::ffi::c_void;

#[derive(Clone)]
struct ConsoleState {
    sample_rate: f64,
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    gain_envelope: f32,
    drive: f32,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self { sample_rate: 44_100.0, threshold: 0.5, ratio: 4.0, attack: 0.01,
            release: 0.1, gain_envelope: 1.0, drive: 1.0 }
    }
}

#[no_mangle]
pub extern "C" fn hirari_divine_console_create() -> *mut c_void {
    Box::into_raw(Box::new(ConsoleState::default())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_clone(state: *const c_void) -> *mut c_void {
    let Some(existing) = state.cast::<ConsoleState>().as_ref() else { return std::ptr::null_mut() };
    Box::into_raw(Box::new(existing.clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<ConsoleState>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_prepare(state: *mut c_void, sample_rate: f64) {
    let Some(s) = state.cast::<ConsoleState>().as_mut() else { return };
    if sample_rate.is_finite() && sample_rate > 0.0 { s.sample_rate = sample_rate; }
    s.gain_envelope = 1.0;
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_reset(state: *mut c_void) {
    if let Some(s) = state.cast::<ConsoleState>().as_mut() { s.gain_envelope = 1.0; }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_set_drive(state: *mut c_void, drive: f32) {
    let Some(s) = state.cast::<ConsoleState>().as_mut() else { return };
    s.drive = 1.0 + if drive.is_finite() { drive.clamp(0.0, 7.0) } else { 0.0 };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_set_params(state: *mut c_void, threshold_db: f32, ratio: f32) {
    let Some(s) = state.cast::<ConsoleState>().as_mut() else { return };
    let db = if threshold_db.is_finite() { threshold_db.clamp(-100.0, 0.0) } else { -6.0 };
    s.threshold = 10.0f32.powf(db / 20.0);
    s.ratio = if ratio.is_finite() { ratio.clamp(1.0, 20.0) } else { 4.0 };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_divine_console_process(
    state: *mut c_void, channels: *const *mut f32, channel_count: u32,
    sample_count: u32, sidechain: *const f32,
) -> bool {
    let Some(s) = state.cast::<ConsoleState>().as_mut() else { return false };
    if channels.is_null() || channel_count == 0 || sample_count == 0 { return false; }
    let channels = std::slice::from_raw_parts(channels, channel_count as usize);
    if channels.iter().any(|ptr| ptr.is_null()) { return false; }
    let side = if sidechain.is_null() { None } else { Some(std::slice::from_raw_parts(sidechain, sample_count as usize)) };
    let attack_denominator = (s.sample_rate * s.attack as f64) as f32;
    let attack_coeff = (-1.0f32 / attack_denominator).exp();
    let release_denominator = (s.sample_rate * s.release as f64) as f32;
    let release_coeff = (-1.0f32 / release_denominator).exp();
    let saturation_norm = s.drive.tanh();
    if !s.gain_envelope.is_finite() { s.gain_envelope = 1.0; }
    for i in 0..sample_count as usize {
        let mut detector = 0.0f32;
        if let Some(side) = side {
            if side[i].is_finite() { detector = side[i].abs(); }
        } else {
            for &channel in channels {
                let value = *channel.add(i);
                if value.is_finite() { detector = detector.max(value.abs()); }
            }
        }
        let target_gain = if detector > s.threshold {
            (s.threshold / detector).powf(1.0 - 1.0 / s.ratio)
        } else { 1.0 };
        let coeff = if target_gain < s.gain_envelope { attack_coeff } else { release_coeff };
        s.gain_envelope += (target_gain - s.gain_envelope) * (1.0 - coeff);
        s.gain_envelope = if s.gain_envelope.is_finite() { s.gain_envelope.clamp(0.0, 1.0) } else { 1.0 };
        for &channel in channels {
            let output = channel.add(i);
            let sample = *output;
            let input = if sample.is_finite() { sample.clamp(-8.0, 8.0) } else { 0.0 };
            let saturated = (input * s.drive).tanh() / saturation_norm;
            *output = if saturated.is_finite() { saturated * s.gain_envelope } else { 0.0 };
        }
    }
    true
}
