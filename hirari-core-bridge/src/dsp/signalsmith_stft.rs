//! Dynamic STFT engine (port of signalsmith-stft), split out of signalsmith_stretch.rs.
use super::*;

pub(crate) struct DynamicStft {
    pub(crate) real_fft: Option<crate::signalsmith_fft::SignalsmithModifiedRealFft>,
    pub(crate) spectrum_real: Vec<f32>,
    pub(crate) spectrum_imag: Vec<f32>,
    pub(crate) analysis_channels: usize,
    pub(crate) synthesis_channels: usize,
    pub(crate) input_length_samples: usize,
    pub(crate) block_samples: usize,
    pub(crate) fft_samples: usize,
    pub(crate) fft_bins: usize,
    pub(crate) default_interval: usize,
    pub(crate) analysis_window: Vec<f32>,
    pub(crate) synthesis_window: Vec<f32>,
    pub(crate) analysis_offset: usize,
    pub(crate) synthesis_offset: usize,
    pub(crate) spectrum_buffer: Vec<Complex>,
    pub(crate) time_buffer: Vec<f32>,
    pub(crate) samples_since_synthesis: usize,
    pub(crate) samples_since_analysis: usize,
    pub(crate) input: StftInput,
    pub(crate) output: StftOutput,
    pub(crate) norm: Normalisation,
}

impl DynamicStft {
    /// `configure(inChannels, outChannels, blockSamples, extraInputHistory)`
    /// with the defaults this engine uses (`intervalSamples = 0`,
    /// `asymmetry = 0`, `fftMinSize = 0`).
    pub(crate) fn new(
        analysis_channels: usize,
        synthesis_channels: usize,
        block_samples: usize,
        extra_input_history: usize,
    ) -> Self {
        let fft_samples = fast_size_above((block_samples + 1) / 2) * 2;
        let mut stft = Self {
            real_fft: if fft_samples >= 2 {
                Some(
                    crate::signalsmith_fft::SignalsmithModifiedRealFft::new(fft_samples)
                        .expect("modified real FFT plan"),
                )
            } else {
                None
            },
            spectrum_real: vec![0.0; fft_samples / 2],
            spectrum_imag: vec![0.0; fft_samples / 2],
            analysis_channels,
            synthesis_channels,
            input_length_samples: block_samples + extra_input_history,
            block_samples,
            fft_samples,
            fft_bins: fft_samples / 2, // modified spectrum: no +1
            default_interval: 0,
            analysis_window: vec![0.0; block_samples],
            synthesis_window: vec![0.0; block_samples],
            analysis_offset: 0,
            synthesis_offset: 0,
            spectrum_buffer: Vec::new(),
            time_buffer: Vec::new(),
            samples_since_synthesis: 0,
            samples_since_analysis: 0,
            input: StftInput::default(),
            output: StftOutput::default(),
            norm: Normalisation::RoundTrip,
        };
        if block_samples == 0 {
            // Default-constructed (unconfigured) state: the C++ never calls
            // setInterval/reset on it, so skip those here too.
            return stft;
        }
        stft.input.buffer = vec![0.0; stft.input_length_samples * analysis_channels];
        stft.output.buffer = vec![0.0; block_samples * synthesis_channels];
        stft.output.window_products = vec![0.0; block_samples];
        stft.spectrum_buffer =
            vec![Complex::zero(); stft.fft_bins * analysis_channels.max(synthesis_channels)];
        stft.time_buffer = vec![0.0; fft_samples.max(block_samples)];
        stft.set_interval(block_samples / 4, WindowShape::Acg);
        stft.reset(1.0);
        stft
    }

    pub(crate) fn block_samples(&self) -> usize {
        self.block_samples
    }

    pub(crate) fn default_interval(&self) -> usize {
        self.default_interval
    }

    pub(crate) fn bands(&self) -> usize {
        self.fft_bins
    }

    pub(crate) fn analysis_latency(&self) -> usize {
        self.block_samples - self.analysis_offset
    }

    pub(crate) fn synthesis_latency(&self) -> usize {
        self.synthesis_offset
    }

    pub(crate) fn bin_to_freq(&self, b: f32) -> f32 {
        (b + 0.5) / self.fft_samples as f32
    }

    pub(crate) fn freq_to_bin(&self, f: f32) -> f32 {
        f * self.fft_samples as f32 - 0.5
    }

    pub(crate) fn reset(&mut self, product_weight: f32) {
        self.input.pos = self.block_samples;
        self.output.pos = 0;
        self.input.buffer.fill(0.0);
        self.output.buffer.fill(0.0);
        self.spectrum_buffer.fill(Complex::zero());
        self.output.window_products.fill(0.0);
        if self.norm != Normalisation::None {
            self.add_window_product();
            let mut i = self.block_samples as i64 - self.default_interval as i64 - 1;
            while i >= 0 {
                self.output.window_products[i as usize] +=
                    self.output.window_products[i as usize + self.default_interval];
                i -= 1;
            }
        }
        for v in &mut self.output.window_products {
            *v = *v * product_weight + ALMOST_ZERO;
        }
        self.move_output(self.default_interval); // ready for first block immediately
    }

    pub(crate) fn write_input(&mut self, channel: usize, length: usize, input_array: &[f32]) {
        let stride = self.input_length_samples;
        let offset_pos = self.input.pos % stride;
        let input_wrap_index = stride - offset_pos;
        let chunk1 = length.min(input_wrap_index);
        let base = channel * stride;
        for i in 0..chunk1 {
            self.input.buffer[base + offset_pos + i] = input_array[i];
        }
        for i in chunk1..length {
            self.input.buffer[base + i + offset_pos - stride] = input_array[i];
        }
    }

    pub(crate) fn move_input(&mut self, samples: usize) {
        self.input.pos = (self.input.pos + samples) % self.input_length_samples;
        self.samples_since_analysis += samples;
    }

    pub(crate) fn finish_output(&mut self, strength: f32) {
        let mut max_window_product = 0.0f32;
        let chunk1 = self.block_samples.min(self.block_samples - self.output.pos).max(0);
        for i in 0..chunk1 {
            let i2 = self.output.pos + i;
            let wp = self.output.window_products[i2];
            max_window_product = max_window_product.max(wp);
            self.output.window_products[i2] += (max_window_product - wp) * strength;
        }
        for i in chunk1..self.block_samples {
            let i2 = i + self.output.pos - self.block_samples;
            let wp = self.output.window_products[i2];
            max_window_product = max_window_product.max(wp);
            self.output.window_products[i2] += (max_window_product - wp) * strength;
        }
    }

    pub(crate) fn read_output(&self, channel: usize, offset: usize, length: usize, output_array: &mut [f32]) {
        let base = channel * self.block_samples;
        let offset_pos = (self.output.pos + offset) % self.block_samples;
        let output_wrap_index = self.block_samples - offset_pos;
        let chunk1 = length.min(output_wrap_index);
        for i in 0..chunk1 {
            let i2 = offset_pos + i;
            output_array[i] = self.output.buffer[base + i2] / self.output.window_products[i2];
        }
        for i in chunk1..length {
            let i2 = i + offset_pos - self.block_samples;
            output_array[i] = self.output.buffer[base + i2] / self.output.window_products[i2];
        }
    }

    pub(crate) fn add_output(&mut self, channel: usize, length: usize, new_output_array: &[f32]) {
        let length = length.min(self.block_samples);
        let base = channel * self.block_samples;
        let offset_pos = self.output.pos % self.block_samples;
        let output_wrap_index = self.block_samples - offset_pos;
        let chunk1 = length.min(output_wrap_index);
        for i in 0..chunk1 {
            let i2 = offset_pos + i;
            self.output.buffer[base + i2] += new_output_array[i] * self.output.window_products[i2];
        }
        for i in chunk1..length {
            let i2 = i + offset_pos - self.block_samples;
            self.output.buffer[base + i2] += new_output_array[i] * self.output.window_products[i2];
        }
    }

    pub(crate) fn move_output(&mut self, samples: usize) {
        if samples == 1 {
            // avoid all the loops/chunks if we can
            for c in 0..self.synthesis_channels {
                self.output.buffer[self.output.pos + c * self.block_samples] = 0.0;
            }
            self.output.window_products[self.output.pos] = ALMOST_ZERO;
            self.output.pos += 1;
            if self.output.pos >= self.block_samples {
                self.output.pos = 0;
            }
            return;
        }
        // Zero the output buffer as we cross it
        let output_wrap_index = self.block_samples - self.output.pos;
        let chunk1 = samples.min(output_wrap_index);
        for c in 0..self.synthesis_channels {
            let base = c * self.block_samples;
            for i in 0..chunk1 {
                self.output.buffer[base + self.output.pos + i] = 0.0;
            }
            for i in chunk1..samples {
                self.output.buffer[base + i + self.output.pos - self.block_samples] = 0.0;
            }
        }
        for i in 0..chunk1 {
            self.output.window_products[self.output.pos + i] = ALMOST_ZERO;
        }
        for i in chunk1..samples {
            self.output.window_products[i + self.output.pos - self.block_samples] = ALMOST_ZERO;
        }
        self.output.pos = (self.output.pos + samples) % self.block_samples;
        self.samples_since_synthesis += samples;
    }

    pub(crate) fn spectrum_slice(&self, channel: usize) -> &[Complex] {
        let base = channel * self.fft_bins;
        &self.spectrum_buffer[base..base + self.fft_bins]
    }

    pub(crate) fn spectrum_slice_mut(&mut self, channel: usize) -> &mut [Complex] {
        let base = channel * self.fft_bins;
        let bins = self.fft_bins;
        &mut self.spectrum_buffer[base..base + bins]
    }

    /// `setInterval(defaultInterval, kaiser, asymmetry = 0)`.
    pub(crate) fn set_interval(&mut self, default_interval: usize, window_shape: WindowShape) {
        self.default_interval = default_interval;
        if window_shape == WindowShape::Ignore {
            return;
        }
        let mut synthesis_window_is_pr = false;

        if window_shape == WindowShape::Acg {
            let bandwidth = self.block_samples as f64 / default_interval as f64;
            fill_acg_window(&mut self.synthesis_window, bandwidth, 0.0, false);
        } else if window_shape == WindowShape::Kaiser {
            // With `useRustFft` and zero asymmetry, the C++ sourced this
            // window from the Rust `hirari_signalsmith_kaiser_pr_window`;
            // call the same implementation directly now.
            if default_interval > 0 && default_interval <= self.block_samples {
                self.synthesis_window =
                    crate::signalsmith_fft::kaiser_pr_window(self.block_samples, default_interval);
                synthesis_window_is_pr = true;
            }
        }

        self.analysis_offset = self.block_samples / 2;
        self.synthesis_offset = self.block_samples / 2;
        if self.analysis_channels == 0 {
            self.analysis_window.fill(1.0);
        } else {
            if !synthesis_window_is_pr {
                force_perfect_reconstruction(
                    &mut self.synthesis_window,
                    self.block_samples,
                    self.default_interval,
                );
            }
            self.analysis_window.copy_from_slice(&self.synthesis_window);
        }
        // Set offsets to the peak's index
        for i in 0..self.block_samples {
            if self.analysis_window[i] > self.analysis_window[self.analysis_offset] {
                self.analysis_offset = i;
            }
            if self.synthesis_window[i] > self.synthesis_window[self.synthesis_offset] {
                self.synthesis_offset = i;
            }
        }
    }

    pub(crate) fn analyse_steps(&self) -> usize {
        self.analysis_channels
    }

    pub(crate) fn analyse_step(&mut self, step: usize, samples_in_past: usize) {
        // Non-split STFT: fftSteps = 0, so channel = step and every step runs
        // the window copy followed by the whole-buffer FFT.
        let fft_steps = 0usize;
        let channel = step / (fft_steps + 1);
        let step = step - channel * (fft_steps + 1);

        if step == 0 {
            // extra step at start of each channel: copy windowed input into buffer
            let stride = self.input_length_samples;
            let offset_pos =
                (stride * 2 + self.input.pos + stride - self.block_samples - samples_in_past)
                    % stride;
            let input_wrap_index = stride - offset_pos;
            let chunk1 = self.analysis_offset.min(input_wrap_index);
            let chunk2 = self.analysis_offset.max(self.block_samples.min(input_wrap_index));

            self.samples_since_analysis = samples_in_past;
            let buffer_size = self.time_buffer.len(); // handles the undersized-FFT case
            let base = channel * stride;
            for i in 0..chunk1 {
                let w = -self.analysis_window[i]; // modified spectrum
                let ti = i + (buffer_size - self.analysis_offset);
                let bi = offset_pos + i;
                self.time_buffer[ti] = self.input.buffer[base + bi] * w;
            }
            for i in chunk1..self.analysis_offset {
                let w = -self.analysis_window[i];
                let ti = i + (buffer_size - self.analysis_offset);
                let bi = i + offset_pos - stride;
                self.time_buffer[ti] = self.input.buffer[base + bi] * w;
            }
            for i in self.analysis_offset..chunk2 {
                let w = self.analysis_window[i];
                let ti = i - self.analysis_offset;
                let bi = offset_pos + i;
                self.time_buffer[ti] = self.input.buffer[base + bi] * w;
            }
            for i in chunk2..self.block_samples {
                let w = self.analysis_window[i];
                let ti = i - self.analysis_offset;
                let bi = i + offset_pos - stride;
                self.time_buffer[ti] = self.input.buffer[base + bi] * w;
            }
            if self.fft_samples >= self.block_samples {
                for i in (self.block_samples - self.analysis_offset)
                    ..(self.fft_samples - self.analysis_offset)
                {
                    self.time_buffer[i] = 0.0;
                }
            } else {
                // undersized FFT - move the first part of the analysis window forwards
                let delta = self.block_samples - self.fft_samples;
                for i in (self.fft_samples - self.analysis_offset)
                    ..(self.block_samples - self.analysis_offset)
                {
                    self.time_buffer[i] += self.time_buffer[delta + i];
                }
                for i in (self.block_samples - self.analysis_offset)..self.fft_samples {
                    self.time_buffer[i] = self.time_buffer[delta + i];
                }
            }
        }

        // Whole-buffer modified real FFT (rustfft backend).
        let base = channel * self.fft_bins;
        let bins = self.fft_bins;
        let ok = match self.real_fft.as_mut() {
            Some(fft) => fft.forward(
                &self.time_buffer[..self.fft_samples],
                &mut self.spectrum_real,
                &mut self.spectrum_imag,
            ),
            None => false,
        };
        let (real, imag) = (&self.spectrum_real, &self.spectrum_imag);
        for bin in 0..bins {
            self.spectrum_buffer[base + bin] = if ok {
                Complex::new(real[bin], imag[bin])
            } else {
                Complex::zero()
            };
        }
    }

    pub(crate) fn synthesise_steps(&self) -> usize {
        self.synthesis_channels
    }

    pub(crate) fn synthesise_step(&mut self, step: usize) {
        if step == 0 {
            // Extra first step which adds in the effective gain for a pure
            // analysis-synthesis cycle
            self.add_window_product();
        }

        let channel = step;
        let base = channel * self.fft_bins;
        let bins = self.fft_bins;
        for bin in 0..bins {
            let value = self.spectrum_buffer[base + bin];
            self.spectrum_real[bin] = value.re;
            self.spectrum_imag[bin] = value.im;
        }
        let ok = match self.real_fft.as_mut() {
            Some(fft) => fft.inverse(
                &self.spectrum_real,
                &self.spectrum_imag,
                &mut self.time_buffer[..self.fft_samples],
            ),
            None => false,
        };
        if !ok {
            self.time_buffer[..self.fft_samples].fill(0.0);
        }

        // windowed overlap-add
        let base = channel * self.block_samples;
        let output_wrap_index = self.block_samples - self.output.pos;
        let chunk1 = self.synthesis_offset.min(output_wrap_index);
        let chunk2 = self.block_samples.min(self.synthesis_offset.max(output_wrap_index));

        for i in 0..chunk1 {
            let w = -self.synthesis_window[i]; // modified spectrum
            let ti = i + (self.fft_samples - self.synthesis_offset);
            let bi = self.output.pos + i;
            self.output.buffer[base + bi] += self.time_buffer[ti] * w;
        }
        for i in chunk1..self.synthesis_offset {
            let w = -self.synthesis_window[i];
            let ti = i + (self.fft_samples - self.synthesis_offset);
            let bi = i + self.output.pos - self.block_samples;
            self.output.buffer[base + bi] += self.time_buffer[ti] * w;
        }
        for i in self.synthesis_offset..chunk2 {
            let w = self.synthesis_window[i];
            let ti = i - self.synthesis_offset;
            let bi = self.output.pos + i;
            self.output.buffer[base + bi] += self.time_buffer[ti] * w;
        }
        for i in chunk2..self.block_samples {
            let w = self.synthesis_window[i];
            let ti = i - self.synthesis_offset;
            let bi = i + self.output.pos - self.block_samples;
            self.output.buffer[base + bi] += self.time_buffer[ti] * w;
        }
    }

    pub(crate) fn add_window_product(&mut self) {
        self.samples_since_synthesis = 0;

        let window_shift = self.synthesis_offset as i64 - self.analysis_offset as i64;
        let w_min = window_shift.max(0) as usize;
        let w_max = (self.block_samples as i64 + window_shift)
            .min(self.block_samples as i64)
            .max(0) as usize;

        let output_wrap_index = self.block_samples - self.output.pos;
        let chunk1 = w_max.min(output_wrap_index.max(w_min));
        let scaling = self.fft_samples as f32;
        if self.norm == Normalisation::RoundTrip {
            for i in w_min..chunk1 {
                let wa = self.analysis_window[(i as i64 - window_shift) as usize];
                let ws = self.synthesis_window[i];
                let bi = self.output.pos + i;
                self.output.window_products[bi] += wa * ws * scaling;
            }
            for i in chunk1..w_max {
                let wa = self.analysis_window[(i as i64 - window_shift) as usize];
                let ws = self.synthesis_window[i];
                let bi = i + self.output.pos - self.block_samples;
                self.output.window_products[bi] += wa * ws * scaling;
            }
        } else if self.norm == Normalisation::Synthesis {
            for i in w_min..chunk1 {
                let ws = self.synthesis_window[i];
                let bi = self.output.pos + i;
                self.output.window_products[bi] += ws * scaling;
            }
            for i in chunk1..w_max {
                let ws = self.synthesis_window[i];
                let bi = i + self.output.pos - self.block_samples;
                self.output.window_products[bi] += ws * scaling;
            }
        } else {
            for i in w_min..chunk1 {
                let bi = self.output.pos + i;
                self.output.window_products[bi] = scaling;
            }
            for i in chunk1..w_max {
                let bi = i + self.output.pos - self.block_samples;
                self.output.window_products[bi] = scaling;
            }
        }
    }
}
