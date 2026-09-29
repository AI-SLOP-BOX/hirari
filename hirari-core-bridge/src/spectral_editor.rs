use crate::fft::FftPlan;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MAX_FFT_SIZE: usize = 4096;

struct SpectralEditor {
    fft_size: usize,
    hop_size: usize,
    write_pos: usize,
    fft: FftPlan,
    window: Vec<f32>,
    analysis: Vec<f32>,
    real: Vec<f32>,
    imag: Vec<f32>,
    accumulator: Vec<f32>,
    normalization: Vec<f32>,
    noise_profile: Vec<f32>,
    scratch: Vec<f32>,
    input_staging: Vec<f32>,
    noise_frames: AtomicU32,
    learn_mode: AtomicBool,
    restoration_active: AtomicBool,
    clear_noise_profile: AtomicBool,
    denoise_threshold: AtomicU32,
    erase_harmonics: AtomicBool,
    fundamental: AtomicU32,
    sample_rate: AtomicU32,
    bandwidth: AtomicU32,
}

impl SpectralEditor {
    fn new(requested: u32) -> Option<Self> {
        let requested = (requested as usize).clamp(64, MAX_FFT_SIZE);
        let fft_size = requested.next_power_of_two().min(MAX_FFT_SIZE);
        let mut window = vec![0.0; fft_size];
        for (index, value) in window.iter_mut().enumerate() {
            *value = 0.5
                * (1.0
                    - (2.0 * std::f64::consts::PI * index as f64 / (fft_size - 1) as f64).cos()
                        as f32);
        }
        Some(Self {
            fft_size,
            hop_size: (fft_size / 4).max(1),
            write_pos: 0,
            fft: FftPlan::new(fft_size)?,
            window,
            analysis: vec![0.0; fft_size * 4],
            real: vec![0.0; fft_size],
            imag: vec![0.0; fft_size],
            accumulator: vec![0.0; fft_size * 4],
            normalization: vec![0.0; fft_size * 4],
            noise_profile: vec![0.0; fft_size],
            scratch: vec![0.0; fft_size],
            input_staging: vec![0.0; fft_size * 4],
            noise_frames: AtomicU32::new(0),
            learn_mode: AtomicBool::new(false),
            restoration_active: AtomicBool::new(false),
            clear_noise_profile: AtomicBool::new(false),
            denoise_threshold: AtomicU32::new(1.0f32.to_bits()),
            erase_harmonics: AtomicBool::new(false),
            fundamental: AtomicU32::new(0.0f32.to_bits()),
            sample_rate: AtomicU32::new(44100.0f32.to_bits()),
            bandwidth: AtomicU32::new(10.0f32.to_bits()),
        })
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len();
        if len == 0 || len > self.accumulator.len() || output.len() < len {
            return;
        }
        if self.clear_noise_profile.swap(false, Ordering::Relaxed) {
            self.noise_profile.fill(0.0);
            self.noise_frames.store(0, Ordering::Relaxed);
        }
        for &sample in input {
            if self.write_pos < self.analysis.len() {
                self.analysis[self.write_pos] = sample;
                self.write_pos += 1;
            }
        }
        while self.write_pos >= self.fft_size {
            for i in 0..self.fft_size {
                self.scratch[i] = self.analysis[i] * self.window[i];
            }
            self.real.copy_from_slice(&self.scratch);
            self.imag.fill(0.0);
            self.fft.forward(&mut self.real, &mut self.imag);

            if self.learn_mode.load(Ordering::Relaxed) {
                let frame = self.noise_frames.load(Ordering::Relaxed);
                let alpha = 1.0 / (frame.saturating_add(1).min(64) as f32);
                for i in 0..self.fft_size {
                    let magnitude =
                        (self.real[i] * self.real[i] + self.imag[i] * self.imag[i]).sqrt();
                    let magnitude = if magnitude.is_finite() {
                        magnitude
                    } else {
                        0.0
                    };
                    self.noise_profile[i] += (magnitude - self.noise_profile[i]) * alpha;
                }
                self.noise_frames.fetch_add(1, Ordering::Relaxed);
            } else if self.restoration_active.load(Ordering::Relaxed) {
                let threshold = f32::from_bits(self.denoise_threshold.load(Ordering::Relaxed));
                for i in 0..self.fft_size {
                    let magnitude =
                        (self.real[i] * self.real[i] + self.imag[i] * self.imag[i]).sqrt();
                    let profile = if self.noise_profile[i].is_finite() {
                        self.noise_profile[i] * threshold
                    } else {
                        0.0
                    };
                    if magnitude < profile {
                        let gain = (magnitude / (profile + 1.0e-9)) * 0.1;
                        self.real[i] *= gain;
                        self.imag[i] *= gain;
                    }
                }
            }

            if self.erase_harmonics.load(Ordering::Relaxed) {
                self.erase_harmonic_bins();
            }
            self.fft.inverse(&mut self.real, &mut self.imag);
            for i in 0..self.fft_size {
                let window = self.window[i];
                self.accumulator[i] += self.real[i] * window;
                self.normalization[i] += window * window;
            }
            self.analysis.copy_within(self.hop_size..self.write_pos, 0);
            self.write_pos -= self.hop_size;
        }

        for i in 0..len {
            let norm = self.normalization[i];
            let value = self.accumulator[i];
            output[i] = if value.is_finite() && norm > 1.0e-6 {
                value / norm
            } else {
                0.0
            };
        }
        self.accumulator.copy_within(len.., 0);
        let accumulator_tail = self.accumulator.len() - len;
        self.accumulator[accumulator_tail..].fill(0.0);
        self.normalization.copy_within(len.., 0);
        let normalization_tail = self.normalization.len() - len;
        self.normalization[normalization_tail..].fill(0.0);
    }

    fn process_in_place(&mut self, samples: &mut [f32]) {
        let len = samples.len();
        if len == 0 || len > self.input_staging.len() || len > self.accumulator.len() {
            return;
        }
        let mut staging = std::mem::take(&mut self.input_staging);
        staging[..len].copy_from_slice(samples);
        self.process(&staging[..len], samples);
        self.input_staging = staging;
    }

    fn erase_harmonic_bins(&mut self) {
        let sr = f32::from_bits(self.sample_rate.load(Ordering::Relaxed));
        let fund = f32::from_bits(self.fundamental.load(Ordering::Relaxed));
        let bw = f32::from_bits(self.bandwidth.load(Ordering::Relaxed));
        if !sr.is_finite() || sr <= 0.0 || !fund.is_finite() || !bw.is_finite() {
            return;
        }
        let bin_width = sr / self.fft_size as f32;
        for harmonic in 1..16 {
            let frequency = fund * harmonic as f32;
            if frequency > sr / 2.0 {
                break;
            }
            let center = (frequency / bin_width) as i32;
            let width = (bw / bin_width) as i32;
            for bin in center - width / 2..=center + width / 2 {
                if bin >= 0 && (bin as usize) < self.fft_size {
                    self.real[bin as usize] = 0.0;
                    self.imag[bin as usize] = 0.0;
                    let mirror = self.fft_size as i32 - bin;
                    if mirror >= 0 && (mirror as usize) < self.fft_size {
                        self.real[mirror as usize] = 0.0;
                        self.imag[mirror as usize] = 0.0;
                    }
                }
            }
        }
    }

    fn reset(&mut self) {
        self.write_pos = 0;
        self.learn_mode.store(false, Ordering::Relaxed);
        self.restoration_active.store(false, Ordering::Relaxed);
        self.clear_noise_profile.store(false, Ordering::Relaxed);
        self.erase_harmonics.store(false, Ordering::Relaxed);
        self.analysis.fill(0.0);
        self.accumulator.fill(0.0);
        self.normalization.fill(0.0);
        self.clear_noise_profile.store(true, Ordering::Relaxed);
        self.noise_frames.store(0, Ordering::Relaxed);
        self.real.fill(0.0);
        self.imag.fill(0.0);
        self.scratch.fill(0.0);
    }
}

#[no_mangle]
pub extern "C" fn hirari_spectral_editor_create(size: u32) -> *mut c_void {
    SpectralEditor::new(size).map_or(std::ptr::null_mut(), |state| {
        Box::into_raw(Box::new(state)).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SpectralEditor>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_process(
    state: *mut c_void,
    input: *const f32,
    output: *mut f32,
    len: u32,
) {
    if state.is_null() || input.is_null() || output.is_null() {
        return;
    }
    let editor = &mut *state.cast::<SpectralEditor>();
    let len = len as usize;
    if len > editor.accumulator.len() {
        return;
    }
    let input = std::slice::from_raw_parts(input, len);
    let output = std::slice::from_raw_parts_mut(output, len);
    editor.process(input, output);
}

/// Processes a buffer in place while keeping the alias-safe input copy inside
/// the Rust-owned editor state. This avoids a large staging array in each C++
/// facade instance and never constructs overlapping Rust input/output slices.
#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_process_in_place(
    state: *mut c_void,
    samples: *mut f32,
    len: u32,
) {
    if state.is_null() || samples.is_null() {
        return;
    }
    let editor = &mut *state.cast::<SpectralEditor>();
    let len = len as usize;
    if len == 0 || len > editor.input_staging.len() || len > editor.accumulator.len() {
        return;
    }
    let samples = std::slice::from_raw_parts_mut(samples, len);
    editor.process_in_place(samples);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_flush(
    state: *mut c_void,
    output: *mut f32,
    len: u32,
) {
    if state.is_null() || output.is_null() {
        return;
    }
    let editor = &mut *state.cast::<SpectralEditor>();
    let output = std::slice::from_raw_parts_mut(output, len as usize);
    let zeros = [0.0f32; 256];
    let mut written = 0;
    while written < output.len() {
        let count = (output.len() - written).min(zeros.len());
        editor.process(&zeros[..count], &mut output[written..written + count]);
        written += count;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_latency(state: *const c_void) -> u32 {
    if state.is_null() {
        0
    } else {
        (*state.cast::<SpectralEditor>()).fft_size as u32
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_tail(state: *const c_void) -> u32 {
    if state.is_null() {
        0
    } else {
        let editor = &*state.cast::<SpectralEditor>();
        (editor.fft_size + editor.hop_size) as u32
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_set_learn(state: *mut c_void, active: bool) {
    if state.is_null() {
        return;
    }
    let editor = &*state.cast::<SpectralEditor>();
    editor.learn_mode.store(active, Ordering::Relaxed);
    if active {
        editor.clear_noise_profile.store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_clear_profile(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<SpectralEditor>())
            .clear_noise_profile
            .store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_profile_ready(state: *const c_void) -> bool {
    !state.is_null()
        && (*state.cast::<SpectralEditor>())
            .noise_frames
            .load(Ordering::Relaxed)
            > 0
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_set_active(state: *mut c_void, active: bool) {
    if !state.is_null() {
        (*state.cast::<SpectralEditor>())
            .restoration_active
            .store(active, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_set_threshold(state: *mut c_void, threshold: f32) {
    if !state.is_null() {
        let threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 8.0)
        } else {
            1.0
        };
        (*state.cast::<SpectralEditor>())
            .denoise_threshold
            .store(threshold.to_bits(), Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_set_harmonics(
    state: *const c_void,
    fund: f32,
    sr: f32,
    bw: f32,
) {
    if state.is_null() {
        return;
    }
    let editor = &*state.cast::<SpectralEditor>();
    if !fund.is_finite()
        || !sr.is_finite()
        || !bw.is_finite()
        || fund <= 0.0
        || sr <= 100.0
        || bw <= 0.0
    {
        editor.erase_harmonics.store(false, Ordering::Relaxed);
        return;
    }
    editor.fundamental.store(fund.to_bits(), Ordering::Relaxed);
    editor.sample_rate.store(sr.to_bits(), Ordering::Relaxed);
    editor.bandwidth.store(bw.to_bits(), Ordering::Relaxed);
    editor.erase_harmonics.store(true, Ordering::Relaxed);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_editor_reset(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<SpectralEditor>()).reset();
    }
}

const SPECTRAL_RESTORATION_CHANNELS: usize = 12;
const SPECTRAL_RESTORATION_STATE_SIZE: usize = 24;
const SPECTRAL_RESTORATION_STATE_MAGIC: u32 = 0x4155_5253;

struct SpectralRestorationRuntime {
    sample_rate: f64,
    editors: Vec<SpectralEditor>,
    restoration_active: AtomicBool,
    denoise_threshold: AtomicU32,
}

impl SpectralRestorationRuntime {
    fn new(sample_rate: f64) -> Option<Self> {
        let mut editors = Vec::with_capacity(SPECTRAL_RESTORATION_CHANNELS);
        for _ in 0..SPECTRAL_RESTORATION_CHANNELS {
            editors.push(SpectralEditor::new(2048)?);
        }
        Some(Self {
            sample_rate: normalize_restoration_sample_rate(sample_rate),
            editors,
            restoration_active: AtomicBool::new(false),
            denoise_threshold: AtomicU32::new(1.0f32.to_bits()),
        })
    }

    fn set_restoration_active(&self, active: bool) {
        self.restoration_active.store(active, Ordering::Relaxed);
        for editor in &self.editors {
            editor.restoration_active.store(active, Ordering::Relaxed);
        }
    }

    fn set_denoise_threshold(&self, threshold: f32) {
        let threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 8.0)
        } else {
            1.0
        };
        self.denoise_threshold
            .store(threshold.to_bits(), Ordering::Relaxed);
        for editor in &self.editors {
            editor
                .denoise_threshold
                .store(threshold.to_bits(), Ordering::Relaxed);
        }
    }

    fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match id {
            0 => self.set_restoration_active(value >= 0.5),
            1 => self.set_denoise_threshold(value * 8.0),
            _ => {}
        }
    }

    fn parameter(&self, id: u32) -> f32 {
        match id {
            0 => self.restoration_active.load(Ordering::Relaxed) as u8 as f32,
            1 => (f32::from_bits(self.denoise_threshold.load(Ordering::Relaxed)) / 8.0)
                .clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    fn reset(&mut self) {
        self.set_restoration_active(false);
        self.set_denoise_threshold(1.0);
        for editor in &mut self.editors {
            editor.reset();
            editor.learn_mode.store(false, Ordering::Relaxed);
        }
    }
}

fn normalize_restoration_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sample_rate
    } else {
        44_100.0
    }
}

#[no_mangle]
pub extern "C" fn hirari_spectral_restoration_create(sample_rate: f64) -> *mut c_void {
    SpectralRestorationRuntime::new(sample_rate).map_or(std::ptr::null_mut(), |runtime| {
        Box::into_raw(Box::new(runtime)).cast()
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<SpectralRestorationRuntime>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_sample_rate(state: *const c_void) -> f64 {
    if state.is_null() {
        44_100.0
    } else {
        (*state.cast::<SpectralRestorationRuntime>()).sample_rate
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_prepare(state: *mut c_void, sample_rate: f64) {
    if state.is_null() {
        return;
    }
    let runtime = &mut *state.cast::<SpectralRestorationRuntime>();
    runtime.sample_rate = normalize_restoration_sample_rate(sample_rate);
    runtime.reset();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_reset(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<SpectralRestorationRuntime>()).reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_latency(state: *const c_void) -> u32 {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }) else {
        return 0;
    };
    runtime
        .editors
        .first()
        .map_or(0, |editor| editor.fft_size as u32)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_tail(state: *const c_void) -> u32 {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }) else {
        return 0;
    };
    runtime
        .editors
        .first()
        .map_or(0, |editor| (editor.fft_size + editor.hop_size) as u32)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(runtime) = unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() } {
        runtime.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_get_parameter(
    state: *const c_void,
    id: u32,
) -> f32 {
    unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }
        .map_or(0.0, |runtime| runtime.parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_process(
    state: *mut c_void,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_mut() }) else {
        return;
    };
    if channel_count == 0 || channels.is_null() {
        return;
    }
    let channels = unsafe { std::slice::from_raw_parts(channels, channel_count as usize) };
    for (editor, samples) in runtime.editors.iter_mut().zip(channels) {
        if samples.is_null() {
            continue;
        }
        let samples = unsafe { std::slice::from_raw_parts_mut(*samples, frames as usize) };
        editor.process_in_place(samples);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_flush(
    state: *mut c_void,
    channels: *const *mut f32,
    channel_count: u32,
    frames: u32,
) {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_mut() }) else {
        return;
    };
    if channel_count == 0 || channels.is_null() {
        return;
    }
    let channels = unsafe { std::slice::from_raw_parts(channels, channel_count as usize) };
    for (editor, output) in runtime.editors.iter_mut().zip(channels) {
        if output.is_null() {
            continue;
        }
        let output = unsafe { std::slice::from_raw_parts_mut(*output, frames as usize) };
        let zeros = [0.0f32; 256];
        let mut written = 0;
        while written < output.len() {
            let count = (output.len() - written).min(zeros.len());
            editor.process(&zeros[..count], &mut output[written..written + count]);
            written += count;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_set_learn(state: *const c_void, active: bool) {
    if let Some(runtime) = unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() } {
        for editor in &runtime.editors {
            editor.learn_mode.store(active, Ordering::Relaxed);
            if active {
                editor.clear_noise_profile.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_clear_profile(state: *const c_void) {
    if let Some(runtime) = unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() } {
        for editor in &runtime.editors {
            editor.clear_noise_profile.store(true, Ordering::Relaxed);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_profile_ready(state: *const c_void) -> bool {
    unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }
        .and_then(|runtime| runtime.editors.first())
        .is_some_and(|editor| editor.noise_frames.load(Ordering::Relaxed) > 0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_set_harmonics(
    state: *const c_void,
    fundamental: f32,
    bandwidth: f32,
) {
    if let Some(runtime) = unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() } {
        for editor in &runtime.editors {
            hirari_spectral_editor_set_harmonics(
                (editor as *const SpectralEditor).cast(),
                fundamental,
                runtime.sample_rate as f32,
                bandwidth,
            );
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_write_state(
    state: *const c_void,
    bypassed: bool,
    sidechain_bus: u32,
    output: *mut u8,
    output_size: usize,
) -> bool {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }) else {
        return false;
    };
    if output.is_null() || output_size != SPECTRAL_RESTORATION_STATE_SIZE {
        return false;
    }
    let mut bytes = [0u8; SPECTRAL_RESTORATION_STATE_SIZE];
    bytes[..4].copy_from_slice(&SPECTRAL_RESTORATION_STATE_MAGIC.to_ne_bytes());
    bytes[4..6].copy_from_slice(&1u16.to_ne_bytes());
    bytes[6..8].copy_from_slice(&(bypassed as u16).to_ne_bytes());
    bytes[8..12].copy_from_slice(&sidechain_bus.to_ne_bytes());
    bytes[12..16].copy_from_slice(&runtime.parameter(0).to_ne_bytes());
    bytes[16..20].copy_from_slice(&runtime.parameter(1).to_ne_bytes());
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_spectral_restoration_restore_state(
    state: *mut c_void,
    input: *const u8,
    input_size: usize,
    bypassed: *mut bool,
    sidechain_bus: *mut u32,
) -> bool {
    let Some(runtime) = (unsafe { state.cast::<SpectralRestorationRuntime>().as_ref() }) else {
        return false;
    };
    if input.is_null()
        || input_size != SPECTRAL_RESTORATION_STATE_SIZE
        || bypassed.is_null()
        || sidechain_bus.is_null()
    {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(input, input_size) };
    let magic = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
    let version = u16::from_ne_bytes(bytes[4..6].try_into().unwrap());
    let flags = u16::from_ne_bytes(bytes[6..8].try_into().unwrap());
    let sidechain = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    let active = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
    let threshold = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
    if magic != SPECTRAL_RESTORATION_STATE_MAGIC
        || version != 1
        || flags & !1 != 0
        || !active.is_finite()
        || !threshold.is_finite()
        || !(0.0..=1.0).contains(&active)
        || !(0.0..=1.0).contains(&threshold)
    {
        return false;
    }
    runtime.set_parameter(0, active);
    runtime.set_parameter(1, threshold);
    unsafe {
        bypassed.write(flags & 1 != 0);
        sidechain_bus.write(sidechain);
    }
    true
}

#[cfg(test)]
mod restoration_runtime_tests {
    use super::*;

    #[test]
    fn restoration_state_round_trip_preserves_parameters_and_host_state() {
        let runtime = SpectralRestorationRuntime::new(48_000.0).unwrap();
        runtime.set_parameter(0, 1.0);
        runtime.set_parameter(1, 0.375);
        let mut state = [0u8; SPECTRAL_RESTORATION_STATE_SIZE];
        assert!(unsafe {
            hirari_spectral_restoration_write_state(
                (&runtime as *const SpectralRestorationRuntime).cast(),
                true,
                7,
                state.as_mut_ptr(),
                state.len(),
            )
        });

        let restored = SpectralRestorationRuntime::new(44_100.0).unwrap();
        let mut bypassed = false;
        let mut sidechain = 0;
        assert!(unsafe {
            hirari_spectral_restoration_restore_state(
                (&restored as *const SpectralRestorationRuntime)
                    .cast_mut()
                    .cast(),
                state.as_ptr(),
                state.len(),
                &mut bypassed,
                &mut sidechain,
            )
        });
        assert!(bypassed);
        assert_eq!(sidechain, 7);
        assert_eq!(restored.parameter(0), 1.0);
        assert_eq!(restored.parameter(1), 0.375);
    }

    #[test]
    fn invalid_restoration_state_is_rejected_without_mutating_outputs_or_parameters() {
        let runtime = SpectralRestorationRuntime::new(48_000.0).unwrap();
        runtime.set_parameter(0, 1.0);
        let mut state = [0u8; SPECTRAL_RESTORATION_STATE_SIZE];
        state[0] ^= 0xff;
        let mut bypassed = true;
        let mut sidechain = 19;
        assert!(!unsafe {
            hirari_spectral_restoration_restore_state(
                (&runtime as *const SpectralRestorationRuntime)
                    .cast_mut()
                    .cast(),
                state.as_ptr(),
                state.len(),
                &mut bypassed,
                &mut sidechain,
            )
        });
        assert!(bypassed);
        assert_eq!(sidechain, 19);
        assert_eq!(runtime.parameter(0), 1.0);
    }

    #[test]
    fn channel_runtime_matches_independent_editor_processing_and_flush() {
        let mut runtime = SpectralRestorationRuntime::new(48_000.0).unwrap();
        let mut references = [
            SpectralEditor::new(2048).unwrap(),
            SpectralEditor::new(2048).unwrap(),
        ];
        for block_size in [127usize, 281, 256] {
            let mut runtime_channels = [vec![0.0; block_size], vec![0.0; block_size]];
            let mut reference_channels = runtime_channels.clone();
            for channel in 0..2 {
                for (frame, sample) in runtime_channels[channel].iter_mut().enumerate() {
                    *sample = ((frame + channel * 17) as f32 * 0.071).sin();
                }
                reference_channels[channel].copy_from_slice(&runtime_channels[channel]);
            }
            let pointers = [
                runtime_channels[0].as_mut_ptr(),
                runtime_channels[1].as_mut_ptr(),
            ];
            unsafe {
                hirari_spectral_restoration_process(
                    (&mut runtime as *mut SpectralRestorationRuntime).cast(),
                    pointers.as_ptr(),
                    pointers.len() as u32,
                    block_size as u32,
                );
            }
            for channel in 0..2 {
                references[channel].process_in_place(&mut reference_channels[channel]);
                assert_eq!(runtime_channels[channel], reference_channels[channel]);
            }
        }

        let mut runtime_tails = [vec![0.0; 701], vec![0.0; 701]];
        let mut reference_tails = runtime_tails.clone();
        let pointers = [runtime_tails[0].as_mut_ptr(), runtime_tails[1].as_mut_ptr()];
        unsafe {
            hirari_spectral_restoration_flush(
                (&mut runtime as *mut SpectralRestorationRuntime).cast(),
                pointers.as_ptr(),
                pointers.len() as u32,
                701,
            );
        }
        for channel in 0..2 {
            let zeros = [0.0f32; 256];
            let mut written = 0;
            while written < reference_tails[channel].len() {
                let count = (reference_tails[channel].len() - written).min(zeros.len());
                references[channel].process(
                    &zeros[..count],
                    &mut reference_tails[channel][written..written + count],
                );
                written += count;
            }
            assert_eq!(runtime_tails[channel], reference_tails[channel]);
        }
    }
}
