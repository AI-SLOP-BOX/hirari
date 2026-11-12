//! Native Rust port of the Signalsmith stretch engine.
//!
//! Faithful translation of the vendored C++ headers (`signalsmith-stretch.h`
//! and the used subset of `signalsmith-linear/fft.h` + `stft.h`) for the
//! configuration the engine runs: `f32` samples, two channels, split
//! computation, modified (half-bin-shift) real spectrum. Operation order
//! mirrors the C++ line for line, so the frozen differential fixtures —
//! generated from the last C++ build before it was retired — keep validating
//! this port.
//!
//! The C++ `useRustFft` opt-in is superseded: the stepwise path it still took
//! through the hand-written split FFT is exactly what is ported here, so the
//! numeric behaviour is unchanged while the foreign source is gone.

#![allow(clippy::needless_range_loop)]
// The port keeps the full public surface of the original C++ engine
// (preset_cheaper, flush, exact, ...), parts of which no caller uses yet.
#![allow(dead_code)]



#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    #[inline]
    pub fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    #[inline]
    pub fn zero() -> Self {
        Self { re: 0.0, im: 0.0 }
    }

    /// `_impl::mul` (forward).
    #[inline]
    fn mul(a: Complex, b: Complex) -> Complex {
        Complex {
            re: a.re * b.re - a.im * b.im,
            im: a.im * b.re + a.re * b.im,
        }
    }

    /// `_impl::mul<conjugateSecond=true>`.
    #[inline]
    fn mul_conj_second(a: Complex, b: Complex) -> Complex {
        Complex {
            re: b.re * a.re + b.im * a.im,
            im: b.re * a.im - b.im * a.re,
        }
    }

    /// `SimpleFFT::mul<conjB>` as used by the combine passes.
    #[inline]
    fn twiddle_mul<const INVERSE: bool>(self, b: Complex) -> Complex {
        if INVERSE {
            Complex {
                re: self.re * b.re + self.im * b.im,
                im: self.im * b.re - self.re * b.im,
            }
        } else {
            Complex {
                re: self.re * b.re - self.im * b.im,
                im: self.im * b.re + self.re * b.im,
            }
        }
    }

    /// `_impl::norm`.
    #[inline]
    fn norm(a: Complex) -> f32 {
        a.re * a.re + a.im * a.im
    }
}

impl std::ops::Mul<f32> for Complex {
    type Output = Complex;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Complex::new(self.re * rhs, self.im * rhs)
    }
}
impl std::ops::Div<f32> for Complex {
    type Output = Complex;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Complex::new(self.re / rhs, self.im / rhs)
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Complex::new(self.re + rhs.re, self.im + rhs.im)
    }
}
impl std::ops::Sub for Complex {
    type Output = Complex;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Complex::new(self.re - rhs.re, self.im - rhs.im)
    }
}

#[inline]
fn polar(rho: f32, theta: f32) -> Complex {
    Complex::new(rho * theta.cos(), rho * theta.sin())
}

// DynamicSTFT (stft.h subset): split computation, modified spectrum.
// ---------------------------------------------------------------------------

const ALMOST_ZERO: f32 = 1e-30;

#[derive(Clone, Default)]
pub(crate) struct StftInput {
    pub pos: usize,
    pub buffer: Vec<f32>,
}

#[derive(Clone, Default)]
pub(crate) struct StftOutput {
    pub pos: usize,
    pub buffer: Vec<f32>,
    pub window_products: Vec<f32>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WindowShape {
    Ignore,
    Acg,
    Kaiser,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Normalisation {
    None,
    Synthesis,
    RoundTrip,
}

/// `SplitFFT::fastSizeAbove` for the whole real-FFT size.
fn fast_size_above(size: usize) -> usize {
    let mut pow2 = 1usize;
    while pow2 < 16 && pow2 < size {
        pow2 *= 2;
    }
    while pow2 * 8 < size {
        pow2 *= 2;
    }
    let mut multiple = (size + pow2 - 1) / pow2; // 1-8
    if multiple == 7 {
        multiple += 1;
    }
    multiple * pow2
}

/// `DynamicSTFT<float, false, STFT_SPECTRUM_MODIFIED, useRustFft=true>`:
/// non-split, with the rustfft-backed modified real FFT the engine always
/// used (the C++ `RustRealFftBackend` shim is gone; we call it directly).
#[path = "signalsmith_stft.rs"]
mod stretch_stft;
pub(crate) use stretch_stft::*;

#[path = "signalsmith_windows.rs"]
mod stretch_windows;
pub(crate) use stretch_windows::*;

#[derive(Clone, Copy, Default)]
struct Band {
    input: Complex,
    prev_input: Complex,
    output: Complex,
    input_energy: f32,
}

#[derive(Clone, Copy)]
struct Peak {
    input: f32,
    output: f32,
}

#[derive(Clone, Copy)]
struct PitchMapPoint {
    input_bin: f32,
    freq_grad: f32,
}

#[derive(Clone, Copy, Default)]
struct Prediction {
    energy: f32,
    input: Complex,
}

impl Prediction {
    fn make_output(&self, phase: Complex) -> Complex {
        let mut phase = phase;
        let mut phase_norm = Complex::norm(phase);
        if phase_norm <= NOISE_FLOOR {
            // prediction is too weak, fall back to the input
            phase = self.input;
            phase_norm = Complex::norm(self.input) + NOISE_FLOOR;
        }
        let scale = (self.energy / phase_norm).sqrt();
        Complex::new(phase.re * scale, phase.im * scale)
    }
}

#[derive(Clone, Copy)]
struct BlockProcess {
    samples_since_last: usize,
    steps: usize,
    step: usize,

    new_spectrum: bool,
    reanalyse_prev: bool,
    mapped_frequencies: bool,
    process_formants: bool,
    freq_multiplier: f32,
    freq_tonality_limit: f32,
    formant_multiplier: f32,
    inv_formant_multiplier: f32,
    formant_base_freq: f32,
    formant_compensation: bool,
    time_factor: f32,
}

impl BlockProcess {
    /// Mirrors the C++ aggregate default member initialisers (`blockProcess = {}`).
    fn fresh() -> Self {
        Self {
            samples_since_last: usize::MAX,
            steps: 0,
            step: 0,
            new_spectrum: false,
            reanalyse_prev: false,
            mapped_frequencies: false,
            process_formants: false,
            freq_multiplier: 1.0,
            freq_tonality_limit: 0.5,
            formant_multiplier: 1.0,
            inv_formant_multiplier: 1.0,
            formant_base_freq: 0.0,
            formant_compensation: false,
            time_factor: 0.0,
        }
    }
}

/// Input view: real slices or the `Zeros` proxy `flush` uses.
enum InputView<'a> {
    Slices(&'a [&'a [f32]]),
    Zeros,
}

impl InputView<'_> {
    #[inline]
    fn sample(&self, channel: usize, index: usize) -> f32 {
        match self {
            InputView::Slices(channels) => channels[channel][index],
            InputView::Zeros => 0.0,
        }
    }

    fn channel_count(&self) -> usize {
        match self {
            InputView::Slices(channels) => channels.len(),
            InputView::Zeros => 0,
        }
    }
}

/// `SignalsmithStretch<float, void, true>`: default-random-engine seeded, and
/// always split computation (the configuration the engine runs).
pub(crate) struct SignalsmithStretch {
    split_computation: bool,

    block_process: BlockProcess,
    silence_counter: usize,
    silence_first: bool,

    freq_multiplier: f32,
    freq_tonality_limit: f32,

    formant_compensation: bool,
    formant_multiplier: f32,
    inv_formant_multiplier: f32,

    stft: DynamicStft,
    stashed_input: StftInput,
    stashed_output: StftOutput,

    tmp_process_buffer: Vec<f32>,
    tmp_pre_roll_buffer: Vec<f32>,

    channels: usize,
    bands: usize,
    prev_input_offset: i64,
    did_seek: bool,
    seek_time_factor: f32,

    channel_bands: Vec<Band>,
    peaks: Vec<Peak>,
    energy: Vec<f32>,
    smoothed_energy: Vec<f32>,
    output_map: Vec<PitchMapPoint>,
    channel_predictions: Vec<Prediction>,

    random_engine: MinstdRand0,

    process_spectrum_steps: usize,
    smooth_energy_state: f32,

    freq_estimate_weighted: f32,
    freq_estimate_weight: f32,
    freq_estimate: f32,
    formant_metric: Vec<f32>,
    formant_base_freq: f32,
}

fn default_random_seed() -> u32 {
    // std::random_device equivalent
    let mut buf = [0u8; 4];
    getrandom_bytes(&mut buf);
    u32::from_le_bytes(buf)
}

fn getrandom_bytes(buf: &mut [u8]) {
    use std::cell::Cell;
    thread_local! {
        static COUNTER: Cell<u64> = const { Cell::new(0) };
    }
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(buf).is_ok() {
            return;
        }
    }
    COUNTER.with(|c| {
        let v = c.get().wrapping_add(0x9E3779B97F4A7C15);
        c.set(v);
        let bytes = v.to_le_bytes();
        for (slot, byte) in buf.iter_mut().zip(bytes.iter().cycle()) {
            *slot = *byte;
        }
    });
}

impl SignalsmithStretch {
    pub(crate) fn new() -> Self {
        Self::with_seed(default_random_seed())
    }

    pub(crate) fn with_seed(seed: u32) -> Self {
        Self {
            split_computation: false,
            block_process: BlockProcess::fresh(),
            silence_counter: 0,
            silence_first: true,
            freq_multiplier: 1.0,
            freq_tonality_limit: 0.5,
            formant_compensation: false,
            formant_multiplier: 1.0,
            inv_formant_multiplier: 1.0,
            stft: DynamicStft::new(0, 0, 0, 0),
            stashed_input: StftInput::default(),
            stashed_output: StftOutput::default(),
            tmp_process_buffer: Vec::new(),
            tmp_pre_roll_buffer: Vec::new(),
            channels: 0,
            bands: 0,
            prev_input_offset: -1,
            did_seek: false,
            seek_time_factor: 1.0,
            channel_bands: Vec::new(),
            peaks: Vec::new(),
            energy: Vec::new(),
            smoothed_energy: Vec::new(),
            output_map: Vec::new(),
            channel_predictions: Vec::new(),
            random_engine: MinstdRand0::new(seed),
            process_spectrum_steps: 0,
            smooth_energy_state: 0.0,
            freq_estimate_weighted: 0.0,
            freq_estimate_weight: 0.0,
            freq_estimate: 0.0,
            formant_metric: Vec::new(),
            formant_base_freq: 0.0,
        }
    }

    /// The difference between the internal position (centre of a block) and
    /// the input samples being supplied.
    pub(crate) fn input_latency(&self) -> i32 {
        self.stft.analysis_latency() as i32
    }

    pub(crate) fn output_latency(&self) -> i32 {
        (self.stft.synthesis_latency()
            + if self.split_computation {
                self.stft.default_interval()
            } else {
                0
            }) as i32
    }

    pub(crate) fn block_samples(&self) -> i32 {
        self.stft.block_samples() as i32
    }

    pub(crate) fn interval_samples(&self) -> i32 {
        self.stft.default_interval() as i32
    }

    pub(crate) fn reset(&mut self) {
        self.stft.reset(0.1);
        self.stashed_input = self.stft.input.clone();
        self.stashed_output = self.stft.output.clone();

        self.prev_input_offset = -1;
        for band in &mut self.channel_bands {
            *band = Band::default();
        }
        self.silence_counter = 0;
        self.did_seek = false;
        self.block_process = BlockProcess::fresh();
        self.freq_estimate_weighted = 0.0;
        self.freq_estimate_weight = 0.0;
    }

    /// Configures using a default preset.
    pub(crate) fn preset_default(&mut self, channels: usize, sample_rate: f32, split_computation: bool) {
        self.configure(
            channels,
            (sample_rate * 0.12) as usize,
            (sample_rate * 0.03) as usize,
            split_computation,
        );
    }

    pub(crate) fn preset_cheaper(&mut self, channels: usize, sample_rate: f32, split_computation: bool) {
        self.configure(
            channels,
            (sample_rate * 0.1) as usize,
            (sample_rate * 0.04) as usize,
            split_computation,
        );
    }

    /// Manual setup.
    pub(crate) fn configure(
        &mut self,
        channels: usize,
        block_samples: usize,
        interval_samples: usize,
        split_computation: bool,
    ) {
        self.split_computation = split_computation;
        self.channels = channels;
        self.stft = DynamicStft::new(channels, channels, block_samples, interval_samples + 1);
        self.stft.set_interval(interval_samples, WindowShape::Kaiser);
        self.stft.reset(0.1);
        self.stashed_input = self.stft.input.clone();
        self.stashed_output = self.stft.output.clone();

        self.bands = self.stft.bands();
        self.channel_bands = vec![Band::default(); self.bands * channels];

        self.peaks.clear();
        self.peaks.reserve(self.bands / 2);
        self.energy = vec![0.0; self.bands];
        self.smoothed_energy = vec![0.0; self.bands];
        self.output_map = vec![
            PitchMapPoint {
                input_bin: 0.0,
                freq_grad: 0.0
            };
            self.bands
        ];
        self.channel_predictions = vec![Prediction::default(); channels * self.bands];

        self.block_process = BlockProcess::fresh();
        self.formant_metric = vec![0.0; self.bands + 2];

        self.tmp_process_buffer = vec![0.0; block_samples + interval_samples];
        self.tmp_pre_roll_buffer = vec![0.0; self.output_latency() as usize * channels];
    }

    /// Frequency multiplier, and optional tonality limit (as multiple of sample-rate).
    pub(crate) fn set_transpose_factor(&mut self, multiplier: f32, tonality_limit: f32) {
        self.freq_multiplier = multiplier;
        if tonality_limit > 0.0 {
            // compromise between input and output limits
            self.freq_tonality_limit = tonality_limit / multiplier.sqrt();
        } else {
            self.freq_tonality_limit = 1.0;
        }
    }

    pub(crate) fn set_transpose_semitones(&mut self, semitones: f32, tonality_limit: f32) {
        self.set_transpose_factor(2.0f32.powf(semitones / 12.0), tonality_limit);
    }

    pub(crate) fn set_formant_factor(&mut self, multiplier: f32, compensate_pitch: bool) {
        self.formant_multiplier = multiplier;
        self.inv_formant_multiplier = 1.0 / multiplier;
        self.formant_compensation = compensate_pitch;
    }

    pub(crate) fn set_formant_semitones(&mut self, semitones: f32, compensate_pitch: bool) {
        self.set_formant_factor(2.0f32.powf(semitones / 12.0), compensate_pitch);
    }

    /// Rough guesstimate of the fundamental frequency, used for formant
    /// analysis. 0 means attempting to detect the pitch.
    pub(crate) fn set_formant_base(&mut self, base_freq: f32) {
        self.formant_base_freq = base_freq;
    }

    pub(crate) fn seek_length(&self) -> i32 {
        (self.stft.block_samples() + self.stft.default_interval()) as i32
    }

    /// Provide previous input ("pre-roll") to smoothly change the input
    /// location without interrupting the output.
    pub(crate) fn seek(&mut self, inputs: &[&[f32]], input_samples: usize, playback_rate: f64) {
        self.tmp_process_buffer.clear();
        self.tmp_process_buffer
            .resize(self.stft.block_samples() + self.stft.default_interval(), 0.0);

        // start position in input / in tmpProcessBuffer
        let start_index = input_samples.saturating_sub(self.tmp_process_buffer.len());
        let pad_start = self.tmp_process_buffer.len() + start_index - input_samples;

        let mut total_energy = 0.0f32;
        for c in 0..self.channels {
            let input_channel = inputs[c];
            for i in start_index..input_samples {
                let s = input_channel[i];
                total_energy += s * s;
                self.tmp_process_buffer[i - start_index + pad_start] = s;
            }

            let buffer = self.tmp_process_buffer.clone();
            self.stft.write_input(c, buffer.len(), &buffer);
        }
        self.stft.move_input(self.tmp_process_buffer.len());
        if total_energy >= NOISE_FLOOR {
            self.silence_counter = 0;
            self.silence_first = true;
        }
        self.did_seek = true;
        self.seek_time_factor = if playback_rate * self.stft.default_interval() as f64 > 1.0 {
            (1.0 / playback_rate) as f32
        } else {
            self.stft.default_interval() as f32
        };
    }

    /// Moves the input position *and* pre-calculates some output, so the next
    /// samples from `process` align to the beginning of the sample.
    pub(crate) fn output_seek(&mut self, inputs: &[&[f32]], input_length: usize) {
        // TODO upstream: fade-out parameter to avoid clicks; upstream does a full reset
        self.reset();
        // Assume we've been handed enough surplus input to produce
        // `output_latency()` samples of pre-roll.
        let surplus_input = input_length.saturating_sub(self.input_latency() as usize);
        let playback_rate = surplus_input as f32 / self.output_latency() as f32;

        // Move the input position to the start of the sound.
        let seek_samples = input_length - surplus_input;
        self.seek(inputs, seek_samples, playback_rate as f64);

        self.tmp_pre_roll_buffer
            .resize(self.output_latency() as usize * self.channels, 0.0);
        let pre_roll_length = self.output_latency() as usize;

        // Use the surplus input to produce pre-roll output (OffsetIO view).
        let mut pre_roll_buffer = std::mem::take(&mut self.tmp_pre_roll_buffer);
        {
            let offset_inputs: Vec<&[f32]> =
                inputs.iter().map(|c| &c[seek_samples..]).collect();
            let mut pre_roll_outputs: Vec<&mut [f32]> =
                pre_roll_buffer.chunks_exact_mut(pre_roll_length).collect();
            let input_view = InputView::Slices(&offset_inputs);
            self.process_impl(input_view, surplus_input, &mut pre_roll_outputs, pre_roll_length);
        }

        // put the thing down, flip it and reverse it
        for v in pre_roll_buffer.iter_mut() {
            *v = -*v;
        }
        for c in 0..self.channels {
            let segment = &mut pre_roll_buffer[c * pre_roll_length..(c + 1) * pre_roll_length];
            segment.reverse();
            let segment_copy = segment.to_vec();
            self.stft.add_output(c, pre_roll_length, &segment_copy);
        }
        self.tmp_pre_roll_buffer = pre_roll_buffer;
    }

    pub(crate) fn output_seek_length(&self, playback_rate: f32) -> i32 {
        self.input_latency() + (playback_rate * self.output_latency() as f32) as i32
    }

    pub(crate) fn process(
        &mut self,
        inputs: &[&[f32]],
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) {
        self.process_impl(InputView::Slices(inputs), input_samples, outputs, output_samples);
    }

    fn process_impl(
        &mut self,
        inputs: InputView<'_>,
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) {
        let mut prev_copied_input = 0usize;

        let mut total_energy = 0.0f32;
        for c in 0..self.channels {
            for i in 0..input_samples {
                let s = inputs.sample(c, i);
                total_energy += s * s;
            }
        }

        if total_energy < NOISE_FLOOR {
            if self.silence_counter >= 2 * self.stft.block_samples() {
                if self.silence_first {
                    // first block of silence processing
                    self.silence_first = false;
                    self.block_process = BlockProcess::fresh();
                    for band in &mut self.channel_bands {
                        band.input = Complex::zero();
                        band.prev_input = Complex::zero();
                        band.output = Complex::zero();
                        band.input_energy = 0.0;
                    }
                }

                if input_samples > 0 {
                    // copy from the input, wrapping around if needed
                    for output_index in 0..output_samples {
                        let input_index = output_index % input_samples;
                        for c in 0..self.channels {
                            outputs[c][output_index] = inputs.sample(c, input_index);
                        }
                    }
                } else {
                    for c in 0..self.channels {
                        for v in outputs[c][..output_samples].iter_mut() {
                            *v = 0.0;
                        }
                    }
                }

                // Store input in history buffer
                self.copy_input(&inputs, input_samples, &mut prev_copied_input);
                return;
            }
            self.silence_counter += input_samples;
        } else {
            self.silence_counter = 0;
            self.silence_first = true;
        }

        for output_index in 0..output_samples {
            let new_block = self.block_process.samples_since_last >= self.stft.default_interval();
            if new_block {
                self.block_process.step = 0;
                self.block_process.steps = 0; // how many processing steps this block will have
                self.block_process.samples_since_last = 0;

                // Time to process a spectrum! Where should it come from in the input?
                let input_offset = (output_index as f32 * input_samples as f32 / output_samples as f32)
                    .round() as i64;
                let input_interval = input_offset - self.prev_input_offset;
                self.prev_input_offset = input_offset;

                self.copy_input(&inputs, input_offset as usize, &mut prev_copied_input);
                self.stashed_input = self.stft.input.clone(); // save the input state for later analysis
                if self.split_computation {
                    self.stashed_output = self.stft.output.clone(); // save the current output, and read from it
                    self.stft.move_output(self.stft.default_interval()); // input jumps forward in time by one interval
                }

                self.block_process.new_spectrum = self.did_seek || input_interval > 0;
                // Latch controls for the whole spectrum so automation updates
                // cannot change the mapping halfway through.
                self.block_process.freq_multiplier = self.freq_multiplier;
                self.block_process.freq_tonality_limit = self.freq_tonality_limit;
                self.block_process.formant_multiplier = self.formant_multiplier;
                self.block_process.inv_formant_multiplier = self.inv_formant_multiplier;
                self.block_process.formant_compensation = self.formant_compensation;
                self.block_process.formant_base_freq = self.formant_base_freq;
                self.block_process.mapped_frequencies = self.block_process.freq_multiplier != 1.0;
                if self.block_process.new_spectrum {
                    // make sure the previous input is the correct distance in the past (give or take 1 sample)
                    self.block_process.reanalyse_prev = self.did_seek
                        || (input_interval - self.stft.default_interval() as i64).abs() > 1;
                    if self.block_process.reanalyse_prev {
                        self.block_process.steps += self.stft.analyse_steps() + 1;
                    }
                    // analyse a new input
                    self.block_process.steps += self.stft.analyse_steps() + 1;
                }

                self.block_process.process_formants = self.block_process.formant_multiplier != 1.0
                    || (self.block_process.formant_compensation
                        && self.block_process.mapped_frequencies);

                self.block_process.time_factor = if self.did_seek {
                    self.seek_time_factor
                } else {
                    self.stft.default_interval() as f32 / 1.max(input_interval) as f32
                };
                self.did_seek = false;

                self.update_process_spectrum_steps();
                self.block_process.steps += self.process_spectrum_steps;

                self.block_process.steps += self.stft.synthesise_steps() + 1;
            }

            let mut process_to_step = if new_block {
                self.block_process.steps
            } else {
                0
            };
            if self.split_computation {
                // C++ overwrites processToStep here (it does not min with the
                // newBlock value).
                let process_ratio = (self.block_process.samples_since_last + 1) as f32
                    / self.stft.default_interval() as f32;
                process_to_step =
                    ((self.block_process.steps as f32 + 0.999) * process_ratio) as usize;
                process_to_step = process_to_step.min(self.block_process.steps);
            }

            while self.block_process.step < process_to_step {
                let step = self.block_process.step;
                self.block_process.step += 1;
                let mut step = step;
                if self.block_process.new_spectrum {
                    if self.block_process.reanalyse_prev {
                        // analyse past input
                        if step < self.stft.analyse_steps() {
                            self.analyse_stashed(step, self.stft.default_interval());
                            continue;
                        }
                        step -= self.stft.analyse_steps();
                        if step < 1 {
                            // Copy previous analysis to our band objects
                            for c in 0..self.channels {
                                for b in 0..self.bands {
                                    let spectrum = self.stft.spectrum_slice(c);
                                    self.channel_bands[b + c * self.bands].prev_input =
                                        spectrum[b];
                                }
                            }
                            continue;
                        }
                        step -= 1;
                    }

                    // Analyse latest (stashed) input
                    if step < self.stft.analyse_steps() {
                        self.analyse_stashed(step, 0);
                        continue;
                    }
                    step -= self.stft.analyse_steps();
                    if step < 1 {
                        // Copy analysed spectrum into our band objects
                        for c in 0..self.channels {
                            for b in 0..self.bands {
                                let spectrum = self.stft.spectrum_slice(c);
                                self.channel_bands[b + c * self.bands].input = spectrum[b];
                            }
                        }
                        continue;
                    }
                    step -= 1;
                }

                if step < self.process_spectrum_steps {
                    self.process_spectrum(step);
                    continue;
                }
                step -= self.process_spectrum_steps;

                if step < 1 {
                    // Copy band objects into spectrum
                    for c in 0..self.channels {
                        for b in 0..self.bands {
                            let value = self.channel_bands[b + c * self.bands].output;
                            self.stft.spectrum_slice_mut(c)[b] = value;
                        }
                    }
                    continue;
                }
                step -= 1;

                if step < self.stft.synthesise_steps() {
                    self.stft.synthesise_step(step);
                    continue;
                }
            }

            self.block_process.samples_since_last += 1;
            if self.split_computation {
                std::mem::swap(&mut self.stashed_output, &mut self.stft.output);
            }
            for c in 0..self.channels {
                let mut v = [0.0f32; 1];
                self.stft.read_output(c, 0, 1, &mut v);
                outputs[c][output_index] = v[0];
            }
            self.stft.move_output(1);
            if self.split_computation {
                std::mem::swap(&mut self.stashed_output, &mut self.stft.output);
            }
        }

        self.copy_input(&inputs, input_samples, &mut prev_copied_input);
        self.prev_input_offset -= input_samples as i64;
    }

    fn copy_input(&mut self, inputs: &InputView<'_>, to_index: usize, prev_copied_input: &mut usize) {
        let length = (self.stft.block_samples() + self.stft.default_interval())
            .min(to_index - *prev_copied_input);
        self.tmp_process_buffer.resize(length, 0.0);
        let offset = to_index - length;
        for c in 0..self.channels {
            for i in 0..length {
                self.tmp_process_buffer[i] = inputs.sample(c, i + offset);
            }
            let buffer = self.tmp_process_buffer.clone();
            self.stft.write_input(c, length, &buffer);
        }
        self.stft.move_input(length);
        *prev_copied_input = to_index;
    }

    /// Runs the stashed input's analysis step: swap the stash in, analyse,
    /// swap back (C++ `stashedInput.swap(stft.input)`).
    fn analyse_stashed(&mut self, step: usize, samples_in_past: usize) {
        std::mem::swap(&mut self.stashed_input.pos, &mut self.stft.input.pos);
        std::mem::swap(&mut self.stashed_input.buffer, &mut self.stft.input.buffer);
        self.stft.analyse_step(step, samples_in_past);
        std::mem::swap(&mut self.stashed_input.pos, &mut self.stft.input.pos);
        std::mem::swap(&mut self.stashed_input.buffer, &mut self.stft.input.buffer);
    }

    /// `flush`: read the remaining output, providing no further input.
    pub(crate) fn flush(&mut self, outputs: &mut [&mut [f32]], output_samples: usize, playback_rate: f32) {
        // If we're asked for more than an interval of extra output, then zero-pad the input
        let output_block = output_samples.saturating_sub(self.stft.default_interval());
        if output_block > 0 {
            let zero_input = (output_block as f32 * playback_rate) as usize;
            self.process_impl(InputView::Zeros, zero_input, outputs, output_block);
        }

        let tail_samples = output_samples - output_block; // at most one interval
        self.tmp_process_buffer.resize(tail_samples, 0.0);
        self.stft.finish_output(1.0);
        for c in 0..self.channels {
            self.stft
                .read_output(c, 0, tail_samples, &mut self.tmp_process_buffer);
            for i in 0..tail_samples {
                outputs[c][output_block + i] = self.tmp_process_buffer[i];
            }
            self.stft
                .read_output(c, tail_samples, tail_samples, &mut self.tmp_process_buffer);
            for i in 0..tail_samples {
                outputs[c][output_block + tail_samples - 1 - i] -= self.tmp_process_buffer[i];
            }
        }
        self.stft.reset(0.1);
        // Reset the phase-vocoder stuff, so the next block gets a fresh start
        for c in 0..self.channels {
            for b in 0..self.bands {
                let band = &mut self.channel_bands[b + c * self.bands];
                band.prev_input = Complex::zero();
                band.output = Complex::zero();
            }
        }
    }

    /// Process a complete audio buffer all in one go.
    pub(crate) fn exact(
        &mut self,
        inputs: &[&[f32]],
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) -> bool {
        let playback_rate = input_samples as f32 / output_samples as f32;
        let seek_length = self.output_seek_length(playback_rate) as usize;
        if input_samples < seek_length {
            // too short for this - zero the output just to be polite
            for c in 0..self.channels {
                for v in outputs[c][..output_samples].iter_mut() {
                    *v = 0.0;
                }
            }
            return false;
        }

        self.output_seek(inputs, seek_length);

        let output_index = output_samples - (seek_length as f32 / playback_rate) as usize;
        let offset_inputs: Vec<&[f32]> = inputs.iter().map(|c| &c[seek_length..]).collect();
        let mut offset_outputs: Vec<&mut [f32]> =
            outputs.iter_mut().map(|c| &mut c[output_index..]).collect();
        self.process(
            &offset_inputs,
            input_samples - seek_length,
            &mut offset_outputs,
            output_index,
        );

        let mut tail_outputs: Vec<&mut [f32]> =
            outputs.iter_mut().map(|c| &mut c[output_index..]).collect();
        self.flush(&mut tail_outputs, output_samples - output_index, playback_rate);
        true
    }

    // ---- band accessors (the C++ pointer-to-member `getBand` family) ----

    #[inline]
    fn band_at(&self, channel: usize, index: i64) -> Option<&Band> {
        if index < 0 || index >= self.bands as i64 {
            return None;
        }
        Some(&self.channel_bands[index as usize + channel * self.bands])
    }

    #[inline]
    fn get_input(&self, channel: usize, index: i64) -> Complex {
        self.band_at(channel, index).map(|b| b.input).unwrap_or(Complex::zero())
    }

    #[inline]
    fn get_prev_input(&self, channel: usize, index: i64) -> Complex {
        self.band_at(channel, index).map(|b| b.prev_input).unwrap_or(Complex::zero())
    }

    #[inline]
    fn get_input_energy(&self, channel: usize, index: i64) -> f32 {
        self.band_at(channel, index).map(|b| b.input_energy).unwrap_or(0.0)
    }

    #[inline]
    fn frac(low: Complex, high: Complex, fractional: f32) -> Complex {
        low + (high - low) * fractional
    }

    #[inline]
    fn get_frac_input(&self, channel: usize, low_index: i64, fractional: f32) -> Complex {
        let low = self.get_input(channel, low_index);
        let high = self.get_input(channel, low_index + 1);
        Self::frac(low, high, fractional)
    }

    #[inline]
    fn get_frac_prev_input(&self, channel: usize, low_index: i64, fractional: f32) -> Complex {
        let low = self.get_prev_input(channel, low_index);
        let high = self.get_prev_input(channel, low_index + 1);
        Self::frac(low, high, fractional)
    }

    #[inline]
    fn get_frac_input_at(&self, channel: usize, input_index: f32) -> Complex {
        let low_index = input_index.floor() as i64;
        let frac_index = input_index - low_index as f32;
        self.get_frac_input(channel, low_index, frac_index)
    }

    #[inline]
    fn get_frac_input_energy(&self, channel: usize, low_index: i64, fractional: f32) -> f32 {
        let low = self.get_input_energy(channel, low_index);
        let high = self.get_input_energy(channel, low_index + 1);
        low + (high - low) * fractional
    }

    fn band_to_freq(&self, b: f32) -> f32 {
        self.stft.bin_to_freq(b)
    }

    fn freq_to_band(&self, f: f32) -> f32 {
        self.stft.freq_to_bin(f)
    }

    fn update_process_spectrum_steps(&mut self) {
        self.process_spectrum_steps = 0;
        if self.block_process.new_spectrum {
            self.process_spectrum_steps += self.channels;
        }
        if self.block_process.mapped_frequencies {
            self.process_spectrum_steps += SMOOTH_ENERGY_STEPS;
            self.process_spectrum_steps += 1; // findPeaks
        }
        self.process_spectrum_steps += 1; // updating the output map
        self.process_spectrum_steps += self.channels; // preliminary phase-vocoder prediction
        self.process_spectrum_steps += SPLIT_MAIN_PREDICTION;
        if self.block_process.new_spectrum {
            self.process_spectrum_steps += 1; // .input -> .prevInput
        }
        if self.block_process.process_formants {
            self.process_spectrum_steps += 3;
        }
    }

    fn process_spectrum(&mut self, step: usize) {
        let mut step = step;
        let time_factor = self.block_process.time_factor;

        let smoothing_bins = self.stft.fft_samples as f32 / self.stft.default_interval() as f32;
        let long_vertical_step = smoothing_bins.round() as i64;
        let time_factor = time_factor.max(1.0 / MAX_CLEAN_STRETCH);
        let random_time_factor = time_factor > MAX_CLEAN_STRETCH;

        if self.block_process.new_spectrum {
            if step < self.channels {
                let channel = step;
                // Complex rot = polar(1, bandToFreq(0)*interval*2pi); ...
                let mut rot = polar(
                    1.0,
                    self.band_to_freq(0.0)
                        * self.stft.default_interval() as f32
                        * (2.0 * std::f64::consts::PI) as f32,
                );
                let freq_step = self.band_to_freq(1.0) - self.band_to_freq(0.0);
                let rot_step = polar(
                    1.0,
                    freq_step * self.stft.default_interval() as f32 * (2.0 * std::f64::consts::PI) as f32,
                );

                for b in 0..self.bands {
                    let bin = &mut self.channel_bands[b + channel * self.bands];
                    bin.output = Complex::mul(bin.output, rot);
                    bin.prev_input = Complex::mul(bin.prev_input, rot);
                    rot = Complex::mul(rot, rot_step);
                }
                return;
            }
            step -= self.channels;
        }
        if self.block_process.mapped_frequencies {
            if step < SMOOTH_ENERGY_STEPS {
                self.smooth_energy(step, smoothing_bins);
                return;
            }
            step -= SMOOTH_ENERGY_STEPS;
            if step == 0 {
                self.find_peaks();
                return;
            }
            step -= 1;
        }
        if step == 0 {
            if self.block_process.mapped_frequencies {
                self.update_output_map();
            } else {
                // we're not pitch-shifting, so no need to find peaks etc.
                for c in 0..self.channels {
                    for b in 0..self.bands {
                        let bin = &mut self.channel_bands[b + c * self.bands];
                        bin.input_energy = Complex::norm(bin.input);
                    }
                }

                for b in 0..self.bands {
                    self.output_map[b] = PitchMapPoint {
                        input_bin: b as f32,
                        freq_grad: 1.0,
                    };
                }
            }
            return;
        }
        step -= 1;
        if self.block_process.process_formants {
            if step < 3 {
                self.update_formants(step);
                return;
            }
            step -= 3;
        }
        // Preliminary output prediction from phase-vocoder
        if step < self.channels {
            let c = step;
            for b in 0..self.bands {
                let map_point = self.output_map[b];
                let low_index = map_point.input_bin.floor() as i64;
                let frac_index = map_point.input_bin - low_index as f32;

                let prev_energy = self.channel_predictions[b + c * self.bands].energy;
                let mut new_prediction = Prediction::default();
                new_prediction.energy = self.get_frac_input_energy(c, low_index, frac_index);
                // scale the energy according to local stretch factor
                new_prediction.energy *= map_point.freq_grad.max(0.0);
                new_prediction.input = self.get_frac_input(c, low_index, frac_index);

                let prev_input = self.get_frac_prev_input(c, low_index, frac_index);
                let freq_twist = Complex::mul_conj_second(new_prediction.input, prev_input);
                let denominator = prev_energy.max(new_prediction.energy) + NOISE_FLOOR;

                let bin_output = self.channel_bands[b + c * self.bands].output;
                let phase = Complex::mul(bin_output, freq_twist);
                self.channel_bands[b + c * self.bands].output = phase / denominator;
                self.channel_predictions[b + c * self.bands] = new_prediction;
            }
            return;
        }
        step -= self.channels;

        if step < SPLIT_MAIN_PREDICTION {
            // Re-predict using phase differences between frequencies
            let chunk = step;
            let start_b = self.bands * chunk / SPLIT_MAIN_PREDICTION;
            let end_b = self.bands * (chunk + 1) / SPLIT_MAIN_PREDICTION;
            for b in start_b..end_b {
                // Find maximum-energy channel and calculate that
                let mut max_channel = 0usize;
                let mut max_energy = self.channel_predictions[b].energy;
                for c in 1..self.channels {
                    let e = self.channel_predictions[b + c * self.bands].energy;
                    if e > max_energy {
                        max_channel = c;
                        max_energy = e;
                    }
                }

                let prediction = self.channel_predictions[b + max_channel * self.bands];
                let map_point = self.output_map[b];

                let mut phase = Complex::zero();

                // Upwards vertical steps
                if b > 0 {
                    let bin_time_factor = if random_time_factor {
                        uniform_real(
                            MAX_CLEAN_STRETCH * 2.0 * (random_time_factor as i32 as f32)
                                - time_factor,
                            time_factor,
                            &mut self.random_engine,
                        )
                    } else {
                        time_factor
                    };
                    let down_input =
                        self.get_frac_input_at(max_channel, map_point.input_bin - bin_time_factor);
                    let short_vertical_twist =
                        Complex::mul_conj_second(prediction.input, down_input);

                    let down_bin = self.channel_bands[(b - 1) + max_channel * self.bands];
                    let contrib = Complex::mul(down_bin.output, short_vertical_twist);
                    phase = phase + contrib;

                    if b as i64 >= long_vertical_step {
                        let long_down_input = self.get_frac_input_at(
                            max_channel,
                            map_point.input_bin - long_vertical_step as f32 * bin_time_factor,
                        );
                        let long_vertical_twist =
                            Complex::mul_conj_second(prediction.input, long_down_input);

                        let long_down_bin = self.channel_bands
                            [(b - long_vertical_step as usize) + max_channel * self.bands];
                        phase = phase + Complex::mul(long_down_bin.output, long_vertical_twist);
                    }
                }
                // Downwards vertical steps
                if b < self.bands - 1 {
                    let up_prediction = self.channel_predictions[(b + 1) + max_channel * self.bands];
                    let up_map_point = self.output_map[b + 1];

                    let bin_time_factor = if random_time_factor {
                        uniform_real(
                            MAX_CLEAN_STRETCH * 2.0 * (random_time_factor as i32 as f32)
                                - time_factor,
                            time_factor,
                            &mut self.random_engine,
                        )
                    } else {
                        time_factor
                    };
                    let down_input = self
                        .get_frac_input_at(max_channel, up_map_point.input_bin - bin_time_factor);
                    let short_vertical_twist =
                        Complex::mul_conj_second(up_prediction.input, down_input);

                    let up_bin = self.channel_bands[(b + 1) + max_channel * self.bands];
                    phase = phase + Complex::mul_conj_second(up_bin.output, short_vertical_twist);

                    if (b as i64) < (self.bands as i64 - long_vertical_step) {
                        let long_up_prediction = self.channel_predictions
                            [(b + long_vertical_step as usize) + max_channel * self.bands];
                        let long_up_map_point = self.output_map[b + long_vertical_step as usize];

                        let long_down_input = self.get_frac_input_at(
                            max_channel,
                            long_up_map_point.input_bin
                                - long_vertical_step as f32 * bin_time_factor,
                        );
                        let long_vertical_twist =
                            Complex::mul_conj_second(long_up_prediction.input, long_down_input);

                        let long_up_bin = self.channel_bands
                            [(b + long_vertical_step as usize) + max_channel * self.bands];
                        phase =
                            phase + Complex::mul_conj_second(long_up_bin.output, long_vertical_twist);
                    }
                }

                let output = prediction.make_output(phase);
                self.channel_bands[b + max_channel * self.bands].output = output;

                // All other bins are locked in phase
                for c in 0..self.channels {
                    if c != max_channel {
                        let channel_twist = Complex::mul_conj_second(
                            self.channel_predictions[b + c * self.bands].input,
                            prediction.input,
                        );
                        let channel_phase = Complex::mul(output, channel_twist);
                        let channel_output = self.channel_predictions[b + c * self.bands]
                            .make_output(channel_phase);
                        self.channel_bands[b + c * self.bands].output = channel_output;
                    }
                }
            }
            return;
        }
        step -= SPLIT_MAIN_PREDICTION;

        if self.block_process.new_spectrum {
            if step == 0 {
                for bin in &mut self.channel_bands {
                    bin.prev_input = bin.input;
                }
            }
        }
    }

    /// Produces smoothed energy across all channels.
    fn smooth_energy(&mut self, step: usize, smoothing_bins: f32) {
        let smoothing_slew = 1.0 / (1.0 + smoothing_bins * 0.5);
        if step == 0 {
            self.energy.fill(0.0);
            for c in 0..self.channels {
                for b in 0..self.bands {
                    let bin = &mut self.channel_bands[b + c * self.bands];
                    let e = Complex::norm(bin.input);
                    bin.input_energy = e; // Used for interpolating prediction energy
                    self.energy[b] += e;
                }
            }
            self.smoothed_energy.copy_from_slice(&self.energy);
            self.smooth_energy_state = 0.0;
            return;
        }

        // The two other steps are repeated smoothing passes, down and up
        let mut e = self.smooth_energy_state;
        for b in (0..self.bands).rev() {
            e += (self.smoothed_energy[b] - e) * smoothing_slew;
            self.smoothed_energy[b] = e;
        }
        for b in 0..self.bands {
            e += (self.smoothed_energy[b] - e) * smoothing_slew;
            self.smoothed_energy[b] = e;
        }
        self.smooth_energy_state = e;
    }

    fn map_freq(&self, freq: f32) -> f32 {
        if freq > self.block_process.freq_tonality_limit {
            return freq
                + (self.block_process.freq_multiplier - 1.0) * self.block_process.freq_tonality_limit;
        }
        freq * self.block_process.freq_multiplier
    }

    /// Identifies spectral peaks using energy across all channels.
    fn find_peaks(&mut self) {
        self.peaks.clear();

        let mut start = 0usize;
        while start < self.bands {
            if self.energy[start] > self.smoothed_energy[start] {
                let mut end = start;
                let mut band_sum = 0.0f32;
                let mut energy_sum = 0.0f32;
                while end < self.bands && self.energy[end] > self.smoothed_energy[end] {
                    band_sum += end as f32 * self.energy[end];
                    energy_sum += self.energy[end];
                    end += 1;
                }
                let avg_band = band_sum / energy_sum;
                let avg_freq = self.band_to_freq(avg_band);
                self.peaks.push(Peak {
                    input: avg_band,
                    output: self.freq_to_band(self.map_freq(avg_freq)),
                });

                start = end;
            }
            start += 1;
        }
    }

    fn update_output_map(&mut self) {
        if self.peaks.is_empty() {
            for b in 0..self.bands {
                self.output_map[b] = PitchMapPoint {
                    input_bin: b as f32,
                    freq_grad: 1.0,
                };
            }
            return;
        }
        let bottom_offset = self.peaks[0].input - self.peaks[0].output;
        let bottom_end = self.bands.min(self.peaks[0].output.ceil() as usize);
        for b in 0..bottom_end {
            self.output_map[b] = PitchMapPoint {
                input_bin: b as f32 + bottom_offset,
                freq_grad: 1.0,
            };
        }
        // Interpolate between points
        for p in 1..self.peaks.len() {
            let prev = self.peaks[p - 1];
            let next = self.peaks[p];
            let range_scale = 1.0 / (next.output - prev.output);
            let out_offset = prev.input - prev.output;
            let out_scale = next.input - next.output - prev.input + prev.output;
            let grad_scale = out_scale * range_scale;
            let start_bin = (prev.output.ceil() as i64).max(0) as usize;
            let end_bin = (next.output.ceil() as i64).clamp(0, self.bands as i64) as usize;
            for b in start_bin..end_bin {
                let r = (b as f32 - prev.output) * range_scale;
                let h = r * r * (3.0 - 2.0 * r);
                let out_b = b as f32 + out_offset + h * out_scale;

                let grad_h = 6.0 * r * (1.0 - r);
                let grad_b = 1.0 + grad_h * grad_scale;

                self.output_map[b] = PitchMapPoint {
                    input_bin: out_b,
                    freq_grad: grad_b,
                };
            }
        }
        let top_offset = self.peaks[self.peaks.len() - 1].input - self.peaks[self.peaks.len() - 1].output;
        let top_start = (self.peaks[self.peaks.len() - 1].output as i64).max(0) as usize;
        for b in top_start..self.bands {
            self.output_map[b] = PitchMapPoint {
                input_bin: b as f32 + top_offset,
                freq_grad: 1.0,
            };
        }
    }

    /// If we mapped formants the same way as `map_freq`, this would be the inverse.
    fn inv_map_formant(&self, freq: f32) -> f32 {
        if freq * self.block_process.inv_formant_multiplier > self.block_process.freq_tonality_limit {
            return freq + (1.0 - self.block_process.formant_multiplier)
                * self.block_process.freq_tonality_limit;
        }
        freq * self.block_process.inv_formant_multiplier
    }

    fn estimate_frequency(&mut self) -> f32 {
        // 3 highest peaks in the input
        let mut peak_indices = [0usize; 3];
        for b in 1..self.bands - 1 {
            let e = self.formant_metric[b];
            // local maxima only
            if e < self.formant_metric[b - 1] || e <= self.formant_metric[b + 1] {
                continue;
            }

            if e > self.formant_metric[peak_indices[0]] {
                if e > self.formant_metric[peak_indices[1]] {
                    if e > self.formant_metric[peak_indices[2]] {
                        peak_indices = [peak_indices[1], peak_indices[2], b];
                    } else {
                        peak_indices = [peak_indices[1], b, peak_indices[2]];
                    }
                } else {
                    peak_indices[0] = b;
                }
            }
        }

        // VERY rough pitch estimation
        let mut peak_estimate = peak_indices[2] as i64;
        if self.formant_metric[peak_indices[1]] > self.formant_metric[peak_indices[2]] * 0.1 {
            let mut diff = (peak_estimate - peak_indices[1] as i64).abs();
            if diff > peak_estimate / 8 && diff < peak_estimate * 7 / 8 {
                peak_estimate %= diff;
            }
            if self.formant_metric[peak_indices[0]] > self.formant_metric[peak_indices[2]] * 0.01 {
                diff = (peak_estimate - peak_indices[0] as i64).abs();
                if diff > peak_estimate / 8 && diff < peak_estimate * 7 / 8 {
                    peak_estimate %= diff;
                }
            }
        }
        let weight = self.formant_metric[peak_indices[2]];
        // Smooth it out a bit
        self.freq_estimate_weighted +=
            (peak_estimate as f32 * weight - self.freq_estimate_weighted) * 0.25;
        self.freq_estimate_weight += (weight - self.freq_estimate_weight) * 0.25;

        self.freq_estimate_weighted / (self.freq_estimate_weight + 1e-30)
    }

    fn update_formants(&mut self, step: usize) {
        if step == 0 {
            self.formant_metric.fill(0.0);
            for c in 0..self.channels {
                for b in 0..self.bands {
                    let e = self.channel_bands[b + c * self.bands].input_energy;
                    self.formant_metric[b] += e;
                }
            }

            self.freq_estimate = self.freq_to_band(self.block_process.formant_base_freq);
            if self.block_process.formant_base_freq <= 0.0 {
                self.freq_estimate = self.estimate_frequency();
            }
        } else if step == 1 {
            let mut decay = 1.0 - 1.0 / (self.freq_estimate * 0.5 + 1.0);
            let mut e = 0.0f32;
            for _repeat in 0..2 {
                for b in (0..self.bands).rev() {
                    e = self.formant_metric[b].max(e * decay);
                    self.formant_metric[b] = e;
                }
                for b in 0..self.bands {
                    e = self.formant_metric[b].max(e * decay);
                    self.formant_metric[b] = e;
                }
            }
            decay = 1.0 / decay;
            for _repeat in 0..2 {
                for b in (0..self.bands).rev() {
                    e = self.formant_metric[b].min(e * decay);
                    self.formant_metric[b] = e;
                }
                for b in 0..self.bands {
                    e = self.formant_metric[b].min(e * decay);
                    self.formant_metric[b] = e;
                }
            }
        } else {
            let get_formant = |metric: &[f32], band: f32| -> f32 {
                if band < 0.0 {
                    return 0.0;
                }
                let band = band.min(self.bands as f32);
                let floor_band = band.floor() as usize;
                let frac_band = band - floor_band as f32;
                let low = metric[floor_band];
                let high = metric[floor_band + 1];
                low + (high - low) * frac_band
            };

            for b in 0..self.bands {
                let input_f = self.band_to_freq(b as f32);
                let output_f = if self.block_process.formant_compensation {
                    self.map_freq(input_f)
                } else {
                    input_f
                };
                let output_f = self.inv_map_formant(output_f);

                let input_e = self.formant_metric[b];
                let target_e = get_formant(&self.formant_metric, self.freq_to_band(output_f));

                let formant_ratio = target_e / (input_e + 1e-30);
                let energy_ratio = formant_ratio;

                for c in 0..self.channels {
                    // This is what's used to decide the output energy
                    self.channel_bands[b + c * self.bands].input_energy *= energy_ratio;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "signalsmith_stretch_tests.rs"]
mod tests;

