//! Frozen holographic panner reference (HRTF convolution or stereo delay).

const MAX_HRTF_TAPS: usize = 128;

struct HolographicPannerReference {
    sample_rate: f64,
    delay_l: [f32; 512],
    delay_r: [f32; 512],
    hrtf_l: [f32; MAX_HRTF_TAPS],
    hrtf_r: [f32; MAX_HRTF_TAPS],
    history: [f32; MAX_HRTF_TAPS],
    hrtf_taps: usize,
    history_ptr: u32,
    ptr: u32,
}

impl HolographicPannerReference {
    fn new() -> Self {
        Self {
            sample_rate: 48000.0,
            delay_l: [0.0; 512],
            delay_r: [0.0; 512],
            hrtf_l: [0.0; MAX_HRTF_TAPS],
            hrtf_r: [0.0; MAX_HRTF_TAPS],
            history: [0.0; MAX_HRTF_TAPS],
            hrtf_taps: 0,
            history_ptr: 0,
            ptr: 0,
        }
    }

    fn set_sample_rate(&mut self, rate: f64) {
        if !rate.is_finite() || rate <= 0.0 {
            return;
        }
        self.sample_rate = rate;
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.history.fill(0.0);
        self.ptr = 0;
        self.history_ptr = 0;
    }

    fn set_kernel(&mut self, left: &[f32], right: &[f32], taps: usize) -> bool {
        if taps == 0 || taps > MAX_HRTF_TAPS || left.len() < taps || right.len() < taps {
            return false;
        }
        for index in 0..taps {
            if !left[index].is_finite() || !right[index].is_finite() {
                return false;
            }
        }
        self.hrtf_taps = taps;
        self.hrtf_l[..taps].copy_from_slice(&left[..taps]);
        self.hrtf_r[..taps].copy_from_slice(&right[..taps]);
        self.hrtf_l[taps..].fill(0.0);
        self.hrtf_r[taps..].fill(0.0);
        self.history.fill(0.0);
        self.history_ptr = 0;
        true
    }

    fn clear_kernel(&mut self) {
        self.hrtf_taps = 0;
        self.hrtf_l.fill(0.0);
        self.hrtf_r.fill(0.0);
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32], mut x: f32, mut y: f32, mut z: f32) {
        x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
        y = if y.is_finite() { y.clamp(-1.0, 1.0) } else { 0.0 };
        z = if z.is_finite() { z.clamp(-1.0, 1.0) } else { 0.0 };
        let rate = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            48000.0
        };
        let distance = (1.0 - 0.35 * z.max(0.0) - 0.15 * y.abs()).clamp(0.25, 1.0);
        let angle = (x + 1.0) * 0.25 * std::f32::consts::PI;
        let left_gain = angle.cos() * distance;
        let right_gain = angle.sin() * distance;
        let delay_samples = ((x.abs() as f64 * 0.0007 * rate) as u32).min(511);

        for index in 0..left.len() {
            let in_l = left[index];
            let in_r = right[index];
            let mono = 0.5
                * ((if in_l.is_finite() { in_l } else { 0.0 })
                    + (if in_r.is_finite() { in_r } else { 0.0 }));
            let slot = (self.ptr % 512) as usize;
            self.ptr = self.ptr.wrapping_add(1);
            self.delay_l[slot] = mono;
            self.delay_r[slot] = mono;
            let delayed = (slot as u32 + 512 - delay_samples) % 512;
            if self.hrtf_taps != 0 {
                let history_slot = (self.history_ptr as usize) % MAX_HRTF_TAPS;
                self.history[history_slot] = mono;
                self.history_ptr = self.history_ptr.wrapping_add(1);
                let mut out_l = 0.0f32;
                let mut out_r = 0.0f32;
                for tap in 0..self.hrtf_taps {
                    let history_index =
                        (self.history_ptr as usize + MAX_HRTF_TAPS - 1 - tap) % MAX_HRTF_TAPS;
                    out_l += self.history[history_index] * self.hrtf_l[tap];
                    out_r += self.history[history_index] * self.hrtf_r[tap];
                }
                left[index] = if out_l.is_finite() { out_l * distance } else { 0.0 };
                right[index] = if out_r.is_finite() { out_r * distance } else { 0.0 };
            } else {
                let delayed_l = self.delay_l[delayed as usize] * left_gain;
                let delayed_r = self.delay_r[delayed as usize] * right_gain;
                left[index] = if delayed_l.is_finite() { delayed_l } else { 0.0 };
                right[index] = if delayed_r.is_finite() { delayed_r } else { 0.0 };
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_track_holographic_panner_create_reference() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(HolographicPannerReference::new())) as *mut std::ffi::c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_destroy_reference(
    state: *mut std::ffi::c_void,
) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut HolographicPannerReference));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_set_sample_rate_reference(
    state: *mut std::ffi::c_void,
    rate: f64,
) {
    if !state.is_null() {
        (*(state as *mut HolographicPannerReference)).set_sample_rate(rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_set_kernel_reference(
    state: *mut std::ffi::c_void,
    left: *const f32,
    right: *const f32,
    taps: u32,
) -> bool {
    if state.is_null() || left.is_null() || right.is_null() || taps == 0 {
        return false;
    }
    let taps = taps as usize;
    if taps > MAX_HRTF_TAPS {
        return false;
    }
    let left = std::slice::from_raw_parts(left, taps);
    let right = std::slice::from_raw_parts(right, taps);
    (*(state as *mut HolographicPannerReference)).set_kernel(left, right, taps)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_clear_kernel_reference(
    state: *mut std::ffi::c_void,
) {
    if !state.is_null() {
        (*(state as *mut HolographicPannerReference)).clear_kernel();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_holographic_panner_process_reference(
    state: *mut std::ffi::c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    x: f32,
    y: f32,
    z: f32,
) {
    if state.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let frames = frames as usize;
    let left = std::slice::from_raw_parts_mut(left, frames);
    let right = std::slice::from_raw_parts_mut(right, frames);
    (*(state as *mut HolographicPannerReference)).process(left, right, x, y, z);
}
