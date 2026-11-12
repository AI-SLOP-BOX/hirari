//! Spectrum prediction machinery, split out of signalsmith_impl_process.rs.
use super::*;

impl SignalsmithStretch {
    pub(in crate::signalsmith_stretch) fn update_process_spectrum_steps(&mut self) {
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

    pub(in crate::signalsmith_stretch) fn process_spectrum(&mut self, step: usize) {
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
    pub(in crate::signalsmith_stretch) fn smooth_energy(&mut self, step: usize, smoothing_bins: f32) {
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

    pub(in crate::signalsmith_stretch) fn map_freq(&self, freq: f32) -> f32 {
        if freq > self.block_process.freq_tonality_limit {
            return freq
                + (self.block_process.freq_multiplier - 1.0) * self.block_process.freq_tonality_limit;
        }
        freq * self.block_process.freq_multiplier
    }

    /// Identifies spectral peaks using energy across all channels.
    pub(in crate::signalsmith_stretch) fn find_peaks(&mut self) {
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

    pub(in crate::signalsmith_stretch) fn update_output_map(&mut self) {
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
    pub(in crate::signalsmith_stretch) fn inv_map_formant(&self, freq: f32) -> f32 {
        if freq * self.block_process.inv_formant_multiplier > self.block_process.freq_tonality_limit {
            return freq + (1.0 - self.block_process.formant_multiplier)
                * self.block_process.freq_tonality_limit;
        }
        freq * self.block_process.inv_formant_multiplier
    }

    pub(in crate::signalsmith_stretch) fn estimate_frequency(&mut self) -> f32 {
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

    pub(in crate::signalsmith_stretch) fn update_formants(&mut self, step: usize) {
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
