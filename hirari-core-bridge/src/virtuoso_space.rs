use std::ffi::c_void;

const LINES: usize = 16;
const MAGIC: u32 = 0x4155_5241;
const VERSION: u16 = 1;
const SAMPLE_RATE_MIN: f64 = 8_000.0;
const SAMPLE_RATE_MAX: f64 = 384_000.0;
const PRIMES: [usize; LINES] = [
    479, 701, 827, 1019, 1153, 1361, 1523, 1787, 1901, 2111, 2333, 2557, 2801, 3109, 3463,
    3851,
];

/// Rust-owned DSP state for the native IProcessor adapter.
pub struct VirtuosoSpaceEngine {
    sample_rate: f64,
    delay_lines: [Vec<f32>; LINES],
    write_indices: [usize; LINES],
    read_indices: [usize; LINES],
    filter_state: [f32; LINES],
    line_read: [f32; LINES],
    decay: f32,
    damping: f32,
    size: f32,
    mix: f32,
}

impl VirtuosoSpaceEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut engine = Self {
            sample_rate: 44_100.0,
            delay_lines: std::array::from_fn(|_| Vec::new()),
            write_indices: [0; LINES],
            read_indices: [0; LINES],
            filter_state: [0.0; LINES],
            line_read: [0.0; LINES],
            decay: 0.85,
            damping: 0.2,
            size: 1.0,
            mix: 0.25,
        };
        engine.prepare(sample_rate);
        engine
    }

    fn prepare(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite()
            && (SAMPLE_RATE_MIN..=SAMPLE_RATE_MAX).contains(&sample_rate)
        {
            sample_rate
        } else {
            44_100.0
        };
        self.setup_fdn();
    }

    fn setup_fdn(&mut self) {
        for (i, prime) in PRIMES.iter().enumerate() {
            let delay_len = ((*prime as f64 * (self.sample_rate / 44_100.0) * self.size as f64)
                as usize)
                .max(1);
            self.delay_lines[i].resize(delay_len, 0.0);
            self.delay_lines[i].fill(0.0);
            self.write_indices[i] = 0;
            self.read_indices[i] = 1 % delay_len;
        }
        self.filter_state.fill(0.0);
        self.line_read.fill(0.0);
    }

    fn reset(&mut self) {
        for line in &mut self.delay_lines {
            line.fill(0.0);
        }
        self.filter_state.fill(0.0);
        self.line_read.fill(0.0);
        self.write_indices.fill(0);
        for (i, line) in self.delay_lines.iter().enumerate() {
            self.read_indices[i] = 1 % line.len().max(1);
        }
    }

    fn parameter(&self, id: u32) -> f32 {
        match id {
            0 => (self.decay / 0.999).clamp(0.0, 1.0),
            1 => (self.damping / 0.99).clamp(0.0, 1.0),
            2 => ((self.size - 0.25) / 3.75).clamp(0.0, 1.0),
            3 => self.mix,
            _ => 0.0,
        }
    }

    fn set_parameter(&mut self, id: u32, value: f32) {
        if !value.is_finite() || id >= 4 {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match id {
            0 => self.decay = value * 0.999,
            1 => self.damping = value * 0.99,
            2 => {
                self.size = 0.25 + value * 3.75;
                self.setup_fdn();
            }
            3 => self.mix = value,
            _ => {}
        }
    }

    fn tail_samples(&self) -> u32 {
        let longest = 3851.0 * (self.sample_rate / 44_100.0) * self.size as f64;
        (longest * 48.0).min(self.sample_rate * 30.0) as u32
    }

    /// The control wrapper supplies the writable channel pointers. Processing
    /// is allocation-free and keeps the original C++ mono/stereo behavior.
    unsafe fn process_raw(&mut self, left: *mut f32, right: *mut f32, frames: u32) {
        if left.is_null() || right.is_null() || frames == 0 {
            return;
        }
        let decay = if self.decay.is_finite() { self.decay.clamp(0.0, 0.999) } else { 0.85 };
        let damping = if self.damping.is_finite() { self.damping.clamp(0.0, 0.99) } else { 0.2 };
        let mix = if self.mix.is_finite() { self.mix.clamp(0.0, 1.0) } else { 0.25 };
        for frame in 0..frames as usize {
            let raw_left = unsafe { *left.add(frame) };
            let raw_right = unsafe { *right.add(frame) };
            let dry_left = if raw_left.is_finite() { raw_left } else { 0.0 };
            let dry_right = if raw_right.is_finite() { raw_right } else { 0.0 };
            let input = 0.5 * (dry_left + dry_right);

            let mut sum = 0.0;
            for i in 0..LINES {
                let value = self.delay_lines[i][self.read_indices[i]];
                self.line_read[i] = if value.is_finite() { value } else { 0.0 };
                sum += self.line_read[i];
            }
            let mean = sum / LINES as f32;
            let mut wet_left = 0.0;
            let mut wet_right = 0.0;
            for i in 0..LINES {
                let diffuse = self.line_read[i] - 2.0 * mean;
                self.filter_state[i] += (diffuse - self.filter_state[i]) * (1.0 - damping);
                let injected = input + decay * self.filter_state[i];
                self.delay_lines[i][self.write_indices[i]] = if injected.is_finite() { injected } else { 0.0 };
                self.write_indices[i] = (self.write_indices[i] + 1) % self.delay_lines[i].len();
                self.read_indices[i] = (self.read_indices[i] + 1) % self.delay_lines[i].len();
                if i & 1 == 0 { wet_left += self.line_read[i]; } else { wet_right += self.line_read[i]; }
            }

            let out_left = dry_left * (1.0 - mix) + wet_left * (1.0 / 8.0) * mix;
            let out_right = dry_right * (1.0 - mix) + wet_right * (1.0 / 8.0) * mix;
            if left == right {
                let mono = 0.5 * (out_left + out_right);
                unsafe { *left.add(frame) = if mono.is_finite() { mono } else { 0.0 } };
            } else {
                unsafe {
                    *left.add(frame) = if out_left.is_finite() { out_left } else { 0.0 };
                    *right.add(frame) = if out_right.is_finite() { out_right } else { 0.0 };
                }
            }
        }
    }

    fn set_state(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() != 32 {
            return false;
        }
        let read_u16 = |offset| u16::from_ne_bytes([bytes[offset], bytes[offset + 1]]);
        let read_u32 = |offset| u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let read_f32 = |offset| f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let magic = read_u32(0);
        let version = read_u16(4);
        let flags = read_u16(6);
        let mix = read_f32(8);
        let values = [read_f32(16), read_f32(20), read_f32(24), read_f32(28)];
        if magic != MAGIC || version != VERSION || flags & !1 != 0
            || !mix.is_finite() || !(0.0..=1.0).contains(&mix)
            || values.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return false;
        }
        self.mix = mix;
        for (id, value) in values.into_iter().enumerate() {
            self.set_parameter(id as u32, value);
        }
        true
    }

    fn write_state(&self, output: &mut [u8], bypassed: bool, sidechain_bus_id: u32) -> usize {
        const SIZE: usize = 32;
        if output.len() < SIZE { return SIZE; }
        output[..SIZE].fill(0);
        output[0..4].copy_from_slice(&MAGIC.to_ne_bytes());
        output[4..6].copy_from_slice(&VERSION.to_ne_bytes());
        let flags: u16 = u16::from(bypassed);
        output[6..8].copy_from_slice(&flags.to_ne_bytes());
        output[8..12].copy_from_slice(&self.mix.to_ne_bytes());
        output[12..16].copy_from_slice(&sidechain_bus_id.to_ne_bytes());
        for id in 0..4 {
            let offset = 16 + id * 4;
            output[offset..offset + 4].copy_from_slice(&self.parameter(id as u32).to_ne_bytes());
        }
        SIZE
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_space_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoSpaceEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_destroy(state: *mut c_void) {
    if !state.is_null() { unsafe { drop(Box::from_raw(state.cast::<VirtuosoSpaceEngine>())) }; }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = unsafe { state.cast::<VirtuosoSpaceEngine>().as_mut() } { state.prepare(sample_rate); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_set_parameter(state: *mut c_void, id: u32, value: f32) {
    if let Some(state) = unsafe { state.cast::<VirtuosoSpaceEngine>().as_mut() } { state.set_parameter(id, value); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_get_parameter(state: *const c_void, id: u32) -> f32 {
    unsafe { state.cast::<VirtuosoSpaceEngine>().as_ref() }.map_or(0.0, |state| state.parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_reset(state: *mut c_void) {
    if let Some(state) = unsafe { state.cast::<VirtuosoSpaceEngine>().as_mut() } { state.reset(); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_tail_samples(state: *const c_void) -> u32 {
    unsafe { state.cast::<VirtuosoSpaceEngine>().as_ref() }.map_or(0, |state| state.tail_samples())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_process(
    state: *mut c_void, left: *mut f32, right: *mut f32, frames: u32,
) {
    if let Some(state) = unsafe { state.cast::<VirtuosoSpaceEngine>().as_mut() } {
        unsafe { state.process_raw(left, right, frames) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_write_state(
    state: *const c_void, output: *mut u8, capacity: usize,
    bypassed: bool, sidechain_bus_id: u32,
) -> usize {
    let Some(state) = (unsafe { state.cast::<VirtuosoSpaceEngine>().as_ref() }) else { return 0; };
    if output.is_null() { return 32; }
    state.write_state(
        unsafe { std::slice::from_raw_parts_mut(output, capacity) },
        bypassed,
        sidechain_bus_id,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_space_set_state(
    state: *mut c_void, input: *const u8, length: usize,
) -> bool {
    let (Some(state), Some(input)) = (
        unsafe { state.cast::<VirtuosoSpaceEngine>().as_mut() },
        (!input.is_null()).then(|| unsafe { std::slice::from_raw_parts(input, length) }),
    ) else { return false; };
    state.set_state(input)
}
