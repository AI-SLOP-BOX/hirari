//! SignalsmithStretch impl, part 1 (state edits, presets, seek, dispatch),
//! split out of signalsmith_stretch.rs.
use super::*;

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
}
