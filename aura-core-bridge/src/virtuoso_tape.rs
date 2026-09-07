pub struct VirtuosoTapeEngine {
    pub sample_rate: f64,
    pub delay_buffers: [Vec<f32>; 2],
    pub lp_state: [f32; 2],
    pub write_idx: usize,
    pub delay_offset: f32,
    pub drive_db: f32,
    pub mix: f32,
    pub bias: f32,
    pub hiss_level: f32,
    pub wow_depth: f32,
    pub flutter_depth: f32,
    pub wow_phase: f64,
    pub flutter_phase: f64,
    pub z1: [f32; 2],
    pub rng_state: u32,
}

impl VirtuosoTapeEngine {
    const BUFFER_SIZE: usize = 1024;

    pub fn new(sr: f64) -> Self {
        Self {
            sample_rate: sr,
            delay_buffers: [vec![0.0; Self::BUFFER_SIZE], vec![0.0; Self::BUFFER_SIZE]],
            lp_state: [0.0; 2],
            write_idx: 0,
            delay_offset: 50.0,
            drive_db: 12.0,
            mix: 1.0,
            bias: 0.05,
            hiss_level: 0.00001,
            wow_depth: 0.15,
            flutter_depth: 0.05,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            z1: [0.0; 2],
            rng_state: 0xACE1, // Seed
        }
    }

    pub fn reset(&mut self) {
        for buf in &mut self.delay_buffers {
            buf.fill(0.0);
        }
        self.lp_state.fill(0.0);
        self.write_idx = 0;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        self.z1.fill(0.0);
    }

    fn xorshift32(&mut self) -> u32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        x
    }

    fn update_lfos(&mut self) {
        self.wow_phase += (2.0 * std::f64::consts::PI * 0.5) / self.sample_rate; // 0.5Hz Wow
        self.flutter_phase += (2.0 * std::f64::consts::PI * 15.6) / self.sample_rate; // 15.6Hz Flutter
        if self.wow_phase > 2.0 * std::f64::consts::PI {
            self.wow_phase -= 2.0 * std::f64::consts::PI;
        }
        if self.flutter_phase > 2.0 * std::f64::consts::PI {
            self.flutter_phase -= 2.0 * std::f64::consts::PI;
        }
    }

    /// INDUSTRIAL: Magnetic saturation and tape velocity modulation.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len();
        let drive = 10.0f32.powf(self.drive_db / 20.0);
        let mask = Self::BUFFER_SIZE - 1;

        for s in 0..len {
            // 1. WOW & FLUTTER (Tape speed instability)
            self.update_lfos();
            let modulation = (self.wow_phase.sin() as f32 * self.wow_depth)
                + (self.flutter_phase.sin() as f32 * self.flutter_depth);

            let mut process_channel = |data: &mut [f32], ch_idx: usize| {
                let in_val = data[s];

                // Write to delay buffer for flutter
                self.delay_buffers[ch_idx][self.write_idx] = in_val;

                // Read with fractional modulation (Linear Interpolation)
                let mut read_pos = self.write_idx as f32 - (self.delay_offset + modulation * 10.0);
                while read_pos < 0.0 {
                    read_pos += Self::BUFFER_SIZE as f32;
                }
                let p0 = read_pos as usize & mask;
                let p1 = (p0 + 1) & mask;
                let frac = read_pos - read_pos.floor();

                let x = self.delay_buffers[ch_idx][p0]
                    + (self.delay_buffers[ch_idx][p1] - self.delay_buffers[ch_idx][p0]) * frac;

                // --- HONEST 2x OVERSAMPLING ---
                let mut out_val = 0.0;
                for over in 0..2 {
                    let x_over = if over == 0 {
                        0.5 * (self.z1[ch_idx] + x)
                    } else {
                        x
                    };

                    // 2. MAGNETIC SATURATION (Tanh-based with Bias)
                    let distorted =
                        ((x_over + self.bias) * drive).tanh() - (self.bias * drive).tanh();

                    // 3. ANALOG HISS (Deep Obsidian Floor)
                    let rand_val = (self.xorshift32() as f32 / 4294967295.0) * 2.0 - 1.0;
                    let noise = rand_val * self.hiss_level;

                    // 4. LOW-PASS ROLLOFF (Tape Head Damping)
                    self.lp_state[ch_idx] = (distorted + noise) * 0.7 + self.lp_state[ch_idx] * 0.3;

                    if over == 1 {
                        // 2.0x stage output
                        out_val = self.lp_state[ch_idx] * self.mix + in_val * (1.0 - self.mix);
                    }
                }
                self.z1[ch_idx] = x;
                data[s] = out_val;
            };

            process_channel(l, 0);
            process_channel(r, 1);

            self.write_idx = (self.write_idx + 1) & mask;
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Virtuoso Tape state.
    pub fn audit_virtuoso_tape(&self) -> bool {
        self.sample_rate.is_finite() && self.sample_rate > 0.0
            && self.delay_buffers.iter().all(|buffer| {
                buffer.len() == Self::BUFFER_SIZE && buffer.iter().all(|sample| sample.is_finite())
            })
            && self.write_idx < Self::BUFFER_SIZE
            && self.lp_state.iter().all(|value| value.is_finite())
            && self.delay_offset.is_finite() && self.delay_offset >= 0.0
            && self.drive_db.is_finite()
            && self.mix.is_finite() && (0.0..=1.0).contains(&self.mix)
            && self.bias.is_finite() && self.hiss_level.is_finite() && self.hiss_level >= 0.0
            && self.wow_depth.is_finite() && self.flutter_depth.is_finite()
            && self.wow_phase.is_finite() && self.flutter_phase.is_finite()
            && self.z1.iter().all(|value| value.is_finite())
            && self.rng_state != 0
    }
}
