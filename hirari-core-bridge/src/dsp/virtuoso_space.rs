
const LINES: usize = 16;
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

    /// Process a block in place. Mirrors the C++/FFI entry point so in-crate
    /// tests and callers can drive the reverb without raw channel pointers.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len()) as u32;
        // SAFETY: both slices are valid for `frames` elements and the engine
        // only reads and writes within them.
        unsafe { self.process_raw(left.as_mut_ptr(), right.as_mut_ptr(), frames) };
    }

    /// Consistency check for the reverb's tuning and delay geometry.
    pub fn audit_virtuoso_space(&self) -> bool {
        self.sample_rate.is_finite()
            && (SAMPLE_RATE_MIN..=SAMPLE_RATE_MAX).contains(&self.sample_rate)
            && self.decay.is_finite()
            && (0.0..=0.999).contains(&self.decay)
            && self.damping.is_finite()
            && (0.0..=0.99).contains(&self.damping)
            && self.size.is_finite()
            && self.mix.is_finite()
            && (0.0..=1.0).contains(&self.mix)
            && self.delay_lines.len() == LINES
            && self.delay_lines.iter().all(|line| !line.is_empty())
    }

        }

