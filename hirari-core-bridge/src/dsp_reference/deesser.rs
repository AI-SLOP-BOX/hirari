use std::ffi::c_void;

struct DeEsser {
    sample_rate: f64,
    threshold: f32,
    intensity: f32,
    envelope: f32,
    current_gain: f32,
    z1: f32,
    z2: f32,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl DeEsser {
    fn new() -> Self {
        Self {
            sample_rate: 44_100.0,
            threshold: 0.5,
            intensity: 0.8,
            envelope: 0.0,
            current_gain: 1.0,
            z1: 0.0,
            z2: 0.0,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    fn prepare(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || !(100.0..=384_000.0).contains(&sample_rate) {
            return;
        }
        self.sample_rate = sample_rate;
        let w0 = 2.0 * std::f64::consts::PI * 6000.0 / sample_rate;
        let alpha = w0.sin() / 2.0;
        let a0 = 1.0 + alpha;
        self.b0 = (alpha / a0) as f32;
        self.b1 = 0.0;
        self.b2 = (-alpha / a0) as f32;
        self.a1 = (-2.0 * w0.cos() / a0) as f32;
        self.a2 = ((1.0 - alpha) / a0) as f32;
    }

    fn reset(&mut self) {
        self.envelope = 0.0;
        self.current_gain = 1.0;
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn filter(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.z1 + self.b2 * self.z2
            - self.a1 * self.z1
            - self.a2 * self.z2;
        self.z2 = self.z1;
        self.z1 = output;
        output
    }
}

#[no_mangle]
pub extern "C" fn hirari_deesser_reference_create() -> *mut c_void {
    Box::into_raw(Box::new(DeEsser::new())) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reference_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut DeEsser));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reference_prepare(state: *mut c_void, sample_rate: f64) {
    if !state.is_null() {
        (*(state as *mut DeEsser)).prepare(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reference_set_parameters(
    state: *mut c_void,
    threshold: f32,
    intensity: f32,
) {
    if state.is_null() {
        return;
    }
    let reference = &mut *(state as *mut DeEsser);
    reference.threshold = threshold;
    reference.intensity = intensity;
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reference_tail_samples(state: *const c_void) -> u32 {
    if state.is_null() {
        return 0;
    }
    let reference = &*(state as *const DeEsser);
    let sample_rate = reference.sample_rate.clamp(100.0, 384_000.0);
    (30.0f64 * sample_rate).min(0.32 * sample_rate) as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_deesser_reference_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let reference = &mut *(state as *mut DeEsser);
    let threshold = reference.threshold.clamp(0.001, 1.0);
    let intensity = reference.intensity.clamp(0.0, 2.0);
    for frame in 0..frames as usize {
        let in_left = {
            let value = *left.add(frame);
            if value.is_finite() { value } else { 0.0 }
        };
        let in_right = if !right.is_null() {
            let value = *right.add(frame);
            if value.is_finite() { value } else { in_left }
        } else {
            in_left
        };
        let signal = reference.filter(0.5 * (in_left + in_right));
        let level = signal.abs();
        if level > reference.envelope {
            reference.envelope = 0.9 * reference.envelope + 0.1 * level;
        } else {
            reference.envelope = 0.999 * reference.envelope + 0.001 * level;
        }
        if reference.envelope.abs() < 1.0e-24 {
            reference.envelope = 0.0;
        }
        let excess = (reference.envelope - threshold).max(0.0);
        let target_gain = 1.0 / (1.0 + intensity * excess * 12.0);
        reference.current_gain += (target_gain - reference.current_gain) * 0.08;
        *left.add(frame) = in_left * reference.current_gain;
        if !right.is_null() {
            *right.add(frame) = in_right * reference.current_gain;
        }
    }
}
