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

#[cfg(test)]
#[path = "signalsmith_stretch_tests.rs"]
mod tests;

#[path = "signalsmith_impl_process.rs"]
mod process_impl_private;
#[path = "signalsmith_impl_state.rs"]
mod impl_state;
