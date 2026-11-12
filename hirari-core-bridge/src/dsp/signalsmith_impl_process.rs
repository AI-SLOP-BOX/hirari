//! SignalsmithStretch impl, part 2 (process, process_impl, spectrum/prediction
//! machinery), split out of signalsmith_stretch.rs.
use super::*;

impl SignalsmithStretch {
    pub(crate) fn process(
        &mut self,
        inputs: &[&[f32]],
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) {
        self.process_impl(InputView::Slices(inputs), input_samples, outputs, output_samples);
    }

    pub(in crate::signalsmith_stretch) fn process_impl(
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

    pub(in crate::signalsmith_stretch) fn copy_input(&mut self, inputs: &InputView<'_>, to_index: usize, prev_copied_input: &mut usize) {
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
    pub(in crate::signalsmith_stretch) fn analyse_stashed(&mut self, step: usize, samples_in_past: usize) {
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
    pub(in crate::signalsmith_stretch) fn band_at(&self, channel: usize, index: i64) -> Option<&Band> {
        if index < 0 || index >= self.bands as i64 {
            return None;
        }
        Some(&self.channel_bands[index as usize + channel * self.bands])
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_input(&self, channel: usize, index: i64) -> Complex {
        self.band_at(channel, index).map(|b| b.input).unwrap_or(Complex::zero())
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_prev_input(&self, channel: usize, index: i64) -> Complex {
        self.band_at(channel, index).map(|b| b.prev_input).unwrap_or(Complex::zero())
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_input_energy(&self, channel: usize, index: i64) -> f32 {
        self.band_at(channel, index).map(|b| b.input_energy).unwrap_or(0.0)
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn frac(low: Complex, high: Complex, fractional: f32) -> Complex {
        low + (high - low) * fractional
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_frac_input(&self, channel: usize, low_index: i64, fractional: f32) -> Complex {
        let low = self.get_input(channel, low_index);
        let high = self.get_input(channel, low_index + 1);
        Self::frac(low, high, fractional)
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_frac_prev_input(&self, channel: usize, low_index: i64, fractional: f32) -> Complex {
        let low = self.get_prev_input(channel, low_index);
        let high = self.get_prev_input(channel, low_index + 1);
        Self::frac(low, high, fractional)
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_frac_input_at(&self, channel: usize, input_index: f32) -> Complex {
        let low_index = input_index.floor() as i64;
        let frac_index = input_index - low_index as f32;
        self.get_frac_input(channel, low_index, frac_index)
    }

    #[inline]
    pub(in crate::signalsmith_stretch) fn get_frac_input_energy(&self, channel: usize, low_index: i64, fractional: f32) -> f32 {
        let low = self.get_input_energy(channel, low_index);
        let high = self.get_input_energy(channel, low_index + 1);
        low + (high - low) * fractional
    }

    pub(in crate::signalsmith_stretch) fn band_to_freq(&self, b: f32) -> f32 {
        self.stft.bin_to_freq(b)
    }

    pub(in crate::signalsmith_stretch) fn freq_to_band(&self, f: f32) -> f32 {
        self.stft.freq_to_bin(f)
    }
}

#[path = "signalsmith_impl_spectrum.rs"]
mod spectrum_impl;
