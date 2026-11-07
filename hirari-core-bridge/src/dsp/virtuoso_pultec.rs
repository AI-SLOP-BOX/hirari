use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy)]
struct BiquadCoefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Default for BiquadCoefficients {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

#[derive(Default)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    fn process(&mut self, input: f32, c: BiquadCoefficients) -> f32 {
        let mut output =
            c.b0 * input + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        if output.abs() < 1.0e-15 {
            output = 0.0;
        }
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn is_finite(&self) -> bool {
        self.x1.is_finite() && self.x2.is_finite() && self.y1.is_finite() && self.y2.is_finite()
    }
}

#[derive(Default)]
struct DspState {
    sample_rate: f64,
    low_boost_l: BiquadState,
    low_boost_r: BiquadState,
    low_cut_l: BiquadState,
    low_cut_r: BiquadState,
    high_boost_l: BiquadState,
    high_boost_r: BiquadState,
}

impl DspState {
    fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            44_100.0
        };
        self.reset();
    }

    fn reset(&mut self) {
        self.low_boost_l.reset();
        self.low_boost_r.reset();
        self.low_cut_l.reset();
        self.low_cut_r.reset();
        self.high_boost_l.reset();
        self.high_boost_r.reset();
    }

    fn process(&mut self, controls: &Controls, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        if frames == 0 {
            return;
        }
        let (low_freq, low_boost, low_atten, high_freq, high_boost) = controls.values();
        let low_boost_c = make_low_shelf(self.sample_rate, low_freq, low_boost);
        let low_cut_c = make_low_shelf(self.sample_rate, low_freq, -low_atten);
        let high_boost_c = make_high_shelf(self.sample_rate, high_freq, high_boost);

        for frame in 0..frames {
            let in_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let in_r = right.as_ref().map_or(in_l, |channel| {
                if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    in_l
                }
            });

            let mut out_l = self.low_boost_l.process(in_l, low_boost_c);
            out_l = self.low_cut_l.process(out_l, low_cut_c);
            out_l = self.high_boost_l.process(out_l, high_boost_c);
            let mut out_r = self.low_boost_r.process(in_r, low_boost_c);
            out_r = self.low_cut_r.process(out_r, low_cut_c);
            out_r = self.high_boost_r.process(out_r, high_boost_c);
            left[frame] = out_l;
            if let Some(channel) = right.as_deref_mut() {
                channel[frame] = out_r;
            }
        }
    }
}

struct Controls {
    low_freq: AtomicU32,
    low_boost: AtomicU32,
    low_atten: AtomicU32,
    high_freq: AtomicU32,
    high_boost: AtomicU32,
}

impl Controls {
    fn new() -> Self {
        Self {
            low_freq: AtomicU32::new(60.0_f32.to_bits()),
            low_boost: AtomicU32::new(2.0_f32.to_bits()),
            low_atten: AtomicU32::new(1.0_f32.to_bits()),
            high_freq: AtomicU32::new(12_000.0_f32.to_bits()),
            high_boost: AtomicU32::new(3.0_f32.to_bits()),
        }
    }

    fn load(value: &AtomicU32) -> f32 {
        f32::from_bits(value.load(Ordering::Relaxed))
    }
    fn store(value: &AtomicU32, next: f32) {
        value.store(next.to_bits(), Ordering::Relaxed);
    }

    fn values(&self) -> (f32, f32, f32, f32, f32) {
        (
            Self::load(&self.low_freq),
            Self::load(&self.low_boost),
            Self::load(&self.low_atten),
            Self::load(&self.high_freq),
            Self::load(&self.high_boost),
        )
    }

    fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match id {
            0 => Self::store(&self.low_freq, 20.0 + value * 180.0),
            1 => Self::store(&self.low_boost, value * 12.0),
            2 => Self::store(&self.low_atten, value * 12.0),
            3 => Self::store(&self.high_freq, 1_000.0 + value * 19_000.0),
            4 => Self::store(&self.high_boost, value * 12.0),
            _ => {}
        }
    }

    fn get_parameter(&self, id: u32) -> f32 {
        let (low_freq, low_boost, low_atten, high_freq, high_boost) = self.values();
        match id {
            0 => (low_freq - 20.0) / 180.0,
            1 => low_boost / 12.0,
            2 => low_atten / 12.0,
            3 => (high_freq - 1_000.0) / 19_000.0,
            4 => high_boost / 12.0,
            _ => 0.0,
        }
    }

    fn set_direct(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => Self::store(&self.low_freq, value.clamp(20.0, 200.0)),
            1 => Self::store(&self.low_boost, value.clamp(0.0, 12.0)),
            2 => Self::store(&self.low_atten, value.clamp(0.0, 12.0)),
            3 => Self::store(&self.high_freq, value.clamp(1_000.0, 20_000.0)),
            4 => Self::store(&self.high_boost, value.clamp(0.0, 12.0)),
            _ => {}
        }
    }
}

pub struct VirtuosoPultecEngine {
    controls: Controls,
    dsp: DspState,
}

impl VirtuosoPultecEngine {
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            controls: Controls::new(),
            dsp: DspState {
                sample_rate: rate,
                ..DspState::default()
            },
        }
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.dsp.prepare_to_play(sample_rate);
    }

    pub fn reset(&mut self) {
        self.dsp.reset();
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        self.controls.set_parameter(id, value);
    }
    pub fn set_direct(&self, id: u32, value: f32) {
        self.controls.set_direct(id, value);
    }
    pub fn get_parameter(&self, id: u32) -> f32 {
        self.controls.get_parameter(id)
    }
    pub fn process(&mut self, left: &mut [f32], right: Option<&mut [f32]>) {
        self.dsp.process(&self.controls, left, right);
    }

    pub fn audit_virtuoso_pultec(&self) -> bool {
        let (low_freq, low_boost, low_atten, high_freq, high_boost) = self.controls.values();
        self.dsp.sample_rate.is_finite()
            && self.dsp.sample_rate > 0.0
            && low_freq.is_finite()
            && (20.0..=200.0).contains(&low_freq)
            && low_boost.is_finite()
            && (0.0..=12.0).contains(&low_boost)
            && low_atten.is_finite()
            && (0.0..=12.0).contains(&low_atten)
            && high_freq.is_finite()
            && (1_000.0..=20_000.0).contains(&high_freq)
            && high_boost.is_finite()
            && (0.0..=12.0).contains(&high_boost)
            && [
                &self.dsp.low_boost_l,
                &self.dsp.low_boost_r,
                &self.dsp.low_cut_l,
                &self.dsp.low_cut_r,
                &self.dsp.high_boost_l,
                &self.dsp.high_boost_r,
            ]
            .iter()
            .all(|state| state.is_finite())
    }
}

fn make_low_shelf(sample_rate: f64, frequency: f32, gain_db: f32) -> BiquadCoefficients {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * frequency / sample_rate as f32;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / 2.0 * (((a + 1.0 / a) * (1.0 / 0.707 - 1.0)) + 2.0).sqrt();
    let sqrt_a = a.sqrt();
    let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
    BiquadCoefficients {
        b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
        b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
        b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
        a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
        a2: ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
    }
}

fn make_high_shelf(sample_rate: f64, frequency: f32, gain_db: f32) -> BiquadCoefficients {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * frequency / sample_rate as f32;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / 2.0 * (((a + 1.0 / a) * (1.0 / 0.707 - 1.0)) + 2.0).sqrt();
    let sqrt_a = a.sqrt();
    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
    BiquadCoefficients {
        b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
        b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
        b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
        a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
        a2: ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_pultec_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoPultecEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<VirtuosoPultecEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_prepare(state: *mut c_void, sample_rate: f64) {
    if state.is_null() {
        return;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let dsp = &mut *std::ptr::addr_of_mut!((*ptr).dsp);
    dsp.prepare_to_play(sample_rate);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_reset(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let dsp = &mut *std::ptr::addr_of_mut!((*ptr).dsp);
    dsp.reset();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if state.is_null() {
        return;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let controls = &*std::ptr::addr_of!((*ptr).controls);
    controls.set_parameter(id, value);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_set_direct(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if state.is_null() {
        return;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let controls = &*std::ptr::addr_of!((*ptr).controls);
    controls.set_direct(id, value);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_get_parameter(
    state: *const c_void,
    id: u32,
) -> f32 {
    if state.is_null() {
        return 0.0;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let controls = &*std::ptr::addr_of!((*ptr).controls);
    controls.get_parameter(id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if state.is_null() || left.is_null() {
        return;
    }
    let ptr = state.cast::<VirtuosoPultecEngine>();
    let controls = &*std::ptr::addr_of!((*ptr).controls);
    let dsp = &mut *std::ptr::addr_of_mut!((*ptr).dsp);
    let left = std::slice::from_raw_parts_mut(left, frames);
    if right.is_null() {
        dsp.process(controls, left, None);
    } else {
        dsp.process(
            controls,
            left,
            Some(std::slice::from_raw_parts_mut(right, frames)),
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_latency() -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_pultec_tail() -> u32 {
    1024
}
