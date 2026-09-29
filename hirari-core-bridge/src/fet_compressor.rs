use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// Stateful FET compressor used by the Master Suite. Its DSP state is owned by
/// the audio thread; controls are published atomically from the host thread.
pub struct FetCompressorEngine {
    sample_rate: f64,
    input_gain: AtomicU32,
    output_gain: AtomicU32,
    threshold: AtomicU32,
    ratio_flat: AtomicU32,
    attack: AtomicU32,
    release: AtomicU32,
    attack_ms: AtomicU32,
    release_ms: AtomicU32,
    envelope: f32,
}

impl FetCompressorEngine {
    pub fn new(sample_rate: f64) -> Self {
        let valid_rate = if sample_rate.is_finite() && (100.0..=384_000.0).contains(&sample_rate) {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            sample_rate: valid_rate,
            input_gain: AtomicU32::new(1.0_f32.to_bits()),
            output_gain: AtomicU32::new(1.0_f32.to_bits()),
            threshold: AtomicU32::new((-24.0_f32).to_bits()),
            ratio_flat: AtomicU32::new(0.75_f32.to_bits()),
            attack: AtomicU32::new(0.99_f32.to_bits()),
            release: AtomicU32::new(0.999_f32.to_bits()),
            attack_ms: AtomicU32::new(1.0_f32.to_bits()),
            release_ms: AtomicU32::new(100.0_f32.to_bits()),
            envelope: 1.0,
        }
    }

    fn load(value: &AtomicU32) -> f32 {
        f32::from_bits(value.load(Ordering::Relaxed))
    }

    fn store(value: &AtomicU32, next: f32) {
        value.store(next.to_bits(), Ordering::Relaxed);
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        if sample_rate.is_finite() && (100.0..=384_000.0).contains(&sample_rate) {
            self.sample_rate = sample_rate;
        }
        self.update_attack();
        self.update_release();
        self.reset();
    }

    fn update_attack(&self) {
        let ms = Self::load(&self.attack_ms).max(0.1);
        let alpha = (-1.0_f32 / (ms * 0.001 * self.sample_rate.max(1_000.0) as f32)).exp();
        Self::store(&self.attack, alpha);
    }

    fn update_release(&self) {
        let ms = Self::load(&self.release_ms).max(1.0);
        let alpha = (-1.0_f32 / (ms * 0.001 * self.sample_rate.max(1_000.0) as f32)).exp();
        Self::store(&self.release, alpha);
    }

    pub fn set_threshold(&self, db: f32) {
        Self::store(&self.threshold, db);
    }

    pub fn set_input_gain_db(&self, db: f32) {
        let db = if db.is_finite() { db } else { 0.0 }.clamp(-60.0, 24.0);
        Self::store(&self.input_gain, 10.0_f32.powf(db / 20.0));
    }

    pub fn set_output_gain_db(&self, db: f32) {
        let db = if db.is_finite() { db } else { 0.0 }.clamp(-60.0, 24.0);
        Self::store(&self.output_gain, 10.0_f32.powf(db / 20.0));
    }

    pub fn set_ratio(&self, ratio: i32) {
        Self::store(&self.ratio_flat, 1.0 - 1.0 / ratio.clamp(1, 20) as f32);
    }

    pub fn set_attack(&self, milliseconds: f32) {
        let ms = if milliseconds.is_finite() {
            milliseconds
        } else {
            1.0
        }
        .clamp(0.1, 200.0);
        Self::store(&self.attack_ms, ms);
        self.update_attack();
    }

    pub fn set_release(&self, milliseconds: f32) {
        let ms = if milliseconds.is_finite() {
            milliseconds
        } else {
            100.0
        }
        .clamp(1.0, 2_000.0);
        Self::store(&self.release_ms, ms);
        self.update_release();
    }

    pub fn set_parameters(
        &self,
        input_db: f32,
        output_db: f32,
        threshold_db: f32,
        attack_ms: f32,
        release_ms: f32,
        ratio: i32,
    ) {
        self.set_input_gain_db(input_db);
        self.set_output_gain_db(output_db);
        self.set_threshold(threshold_db);
        self.set_attack(attack_ms);
        self.set_release(release_ms);
        self.set_ratio(ratio);
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match id {
            0 => Self::store(
                &self.input_gain,
                10.0_f32.powf((-60.0 + value * 84.0) / 20.0),
            ),
            1 => Self::store(
                &self.output_gain,
                10.0_f32.powf((-60.0 + value * 84.0) / 20.0),
            ),
            2 => self.set_threshold(-60.0 + value * 60.0),
            3 => self.set_ratio((1.0 + value * 19.0).round() as i32),
            4 => self.set_attack(0.1 + value * 199.9),
            5 => self.set_release(1.0 + value * 1_999.0),
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((20.0 * Self::load(&self.input_gain).max(1.0e-6).log10() + 60.0) / 84.0)
                .clamp(0.0, 1.0),
            1 => ((20.0 * Self::load(&self.output_gain).max(1.0e-6).log10() + 60.0) / 84.0)
                .clamp(0.0, 1.0),
            2 => ((Self::load(&self.threshold) + 60.0) / 60.0).clamp(0.0, 1.0),
            3 => ((1.0 / (1.0 - Self::load(&self.ratio_flat)).max(1.0e-6) - 1.0) / 19.0)
                .clamp(0.0, 1.0),
            4 => ((Self::load(&self.attack_ms) - 0.1) / 199.9).clamp(0.0, 1.0),
            5 => ((Self::load(&self.release_ms) - 1.0) / 1_999.0).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.envelope = 1.0;
    }

    pub fn tail_samples(&self) -> u32 {
        let rate = self.sample_rate.clamp(100.0, 384_000.0);
        let release = Self::load(&self.release_ms).clamp(1.0, 2_000.0) as f64;
        (30.0 * rate).min(release * 0.001 * self.sample_rate * 8.0) as u32
    }

    #[inline]
    fn fast_log2(value: f32) -> f32 {
        value.to_bits() as f32 * 1.192_092_9e-7 - 126.942_695
    }

    #[inline]
    fn fast_pow2(power: f32) -> f32 {
        let clipped = if power < -126.0 { -126.0 } else { power };
        f32::from_bits(((clipped + 126.942_695) * 8_388_608.0) as u32)
    }

    pub fn process(&mut self, channels: &mut [*mut f32], frames: usize) {
        if channels.is_empty() || frames == 0 {
            return;
        }
        let input_gain = Self::load(&self.input_gain);
        let output_gain = Self::load(&self.output_gain);
        let threshold = Self::load(&self.threshold);
        let ratio_flat = Self::load(&self.ratio_flat);
        let attack = Self::load(&self.attack);
        let release = Self::load(&self.release);

        for frame in 0..frames {
            let mut peak = 0.0_f32;
            for channel in channels.iter() {
                if channel.is_null() {
                    continue;
                }
                let sample = unsafe { *(*channel).add(frame) };
                if sample.is_finite() {
                    peak = peak.max((sample * input_gain).abs());
                }
            }
            let detector = peak.max(1.0e-8);
            let coeff = if detector > self.envelope {
                attack
            } else {
                release
            };
            self.envelope += (detector - self.envelope) * (1.0 - coeff);
            let level_db = 6.020_599_9 * Self::fast_log2(self.envelope.max(1.0e-8));
            let over = (level_db - threshold).max(0.0);
            let reduction_db = over * ratio_flat.clamp(0.0, 0.95);
            let gain = (Self::fast_pow2(-reduction_db / 6.020_599_9) * output_gain).clamp(0.0, 4.0);

            for channel in channels.iter() {
                if channel.is_null() {
                    continue;
                }
                let sample = unsafe { &mut *(*channel).add(frame) };
                let input = if sample.is_finite() { *sample } else { 0.0 };
                let output = input * gain;
                *sample = if output.is_finite() { output } else { 0.0 };
            }
        }
    }

    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        self.process(&mut channels, frames);
    }
}

#[no_mangle]
pub extern "C" fn hirari_fet_compressor_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(FetCompressorEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<FetCompressorEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<FetCompressorEngine>().as_mut() {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<FetCompressorEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<FetCompressorEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_get_parameter(state: *const c_void, id: u32) -> f32 {
    state
        .cast::<FetCompressorEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_set_control(
    state: *const c_void,
    control: u32,
    value: f32,
) {
    let Some(state) = state.cast::<FetCompressorEngine>().as_ref() else {
        return;
    };
    match control {
        0 => state.set_threshold(value),
        1 => state.set_ratio(value.round() as i32),
        2 => state.set_attack(value),
        3 => state.set_release(value),
        4 => state.set_input_gain_db(value),
        5 => state.set_output_gain_db(value),
        _ => {}
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_set_parameters(
    state: *const c_void,
    input_db: f32,
    output_db: f32,
    threshold_db: f32,
    attack_ms: f32,
    release_ms: f32,
    ratio: i32,
) {
    if let Some(state) = state.cast::<FetCompressorEngine>().as_ref() {
        state.set_parameters(
            input_db,
            output_db,
            threshold_db,
            attack_ms,
            release_ms,
            ratio,
        );
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_process(
    state: *mut c_void,
    channels: *mut *mut f32,
    channel_count: usize,
    frames: usize,
) {
    if channels.is_null() || channel_count == 0 {
        return;
    }
    let Some(state) = state.cast::<FetCompressorEngine>().as_mut() else {
        return;
    };
    state.process(
        std::slice::from_raw_parts_mut(channels, channel_count),
        frames,
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_fet_compressor_tail(state: *const c_void) -> u32 {
    state
        .cast::<FetCompressorEngine>()
        .as_ref()
        .map_or(0, |state| state.tail_samples())
}
