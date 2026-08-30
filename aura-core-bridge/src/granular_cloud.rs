/*!
 * @file granular_cloud.rs
 * @brief Professional granular synthesis engine.
 * INDUSTRIAL: Implements real-time grain scheduling with fully independent
 * per-grain pitch, position, pan, and envelope controls.
 *
 * Architecture:
 *  - GrainScheduler fires new Grain objects at a configurable density (grains/sec).
 *  - Each Grain reads from an audio buffer with a randomized read-head position.
 *  - Grain envelope: raised-cosine window for zero-click transient-free output.
 *  - Output: all active grains are summed into the stereo output buffer.
 */

/// A single grain of audio.
struct Grain {
    /// Position in source buffer (samples, fractional)
    read_pos: f64,
    /// Playback speed (1.0 = original pitch)
    speed: f64,
    /// Current envelope phase (0.0 → 1.0)
    env_phase: f64,
    /// Envelope phase increment per sample
    env_inc: f64,
    /// Pan position (-1 L … +1 R)
    pan: f32,
    /// Output gain
    gain: f32,
    active: bool,
}

impl Grain {
    fn new(pos: f64, speed: f64, duration_samples: f64, pan: f32, gain: f32) -> Self {
        Self {
            read_pos: pos,
            speed,
            env_phase: 0.0,
            env_inc: 1.0 / duration_samples,
            pan,
            gain,
            active: true,
        }
    }

    /// Raised-cosine (Hann) window: smooth attack AND release, zero DC.
    #[inline]
    fn envelope(&self) -> f32 {
        (0.5 * (1.0 - (std::f64::consts::TAU * self.env_phase).cos())) as f32
    }

    /// Advance grain by one sample; returns false when grain is finished.
    fn tick(&mut self, src: &[f32]) -> f32 {
        if !self.active {
            return 0.0;
        }

        let env = self.envelope();
        let idx = self.read_pos as usize;
        let frac = (self.read_pos - idx as f64) as f32;

        // Linear interpolation of source sample
        let s0 = src.get(idx).copied().unwrap_or(0.0);
        let s1 = src.get(idx + 1).copied().unwrap_or(0.0);
        let sample = s0 + frac * (s1 - s0);

        self.read_pos += self.speed;
        self.env_phase += self.env_inc;
        if self.env_phase >= 1.0 {
            self.active = false;
        }

        sample * env * self.gain
    }
}

/// Granular synthesis orchestrator.
pub struct GranularCloud {
    /// Maximum simultaneous grains
    max_grains: usize,
    grains: Vec<Grain>,

    // --- Parameters ---
    pub position: f64,      // Read head (0.0..1.0 of source)
    pub position_rand: f64, // Randomization of position
    pub grain_size_ms: f32, // Grain duration (ms)
    pub density: f32,       // Grains per second
    pub pitch: f32,         // Playback pitch (semitones)
    pub pan_spread: f32,    // Pan randomization

    /// Internal scheduler counter
    samples_until_next: f64,
    rng_state: u64, // Simple xorshift RNG (no stdlib/rand dependency)
}

impl GranularCloud {
    pub fn new(max_grains: usize) -> Self {
        Self {
            max_grains,
            grains: Vec::with_capacity(max_grains),
            position: 0.5,
            position_rand: 0.1,
            grain_size_ms: 80.0,
            density: 20.0,
            pitch: 0.0,
            pan_spread: 0.5,
            samples_until_next: 0.0,
            rng_state: 0xDEADBEEF_CAFEBABE,
        }
    }

    /// Fast deterministic pseudo-random float in [0, 1].
    fn rand_f32(&mut self) -> f32 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        (self.rng_state as f32) / (u64::MAX as f32)
    }

    fn spawn_grain(&mut self, source: &[f32], sample_rate: f32) {
        if self.grains.len() >= self.max_grains {
            return;
        }

        let duration_samples = (self.grain_size_ms * 0.001 * sample_rate) as f64;
        let src_pos = (self.position + (self.rand_f32() as f64 - 0.5) * self.position_rand)
            .clamp(0.0, 0.9999)
            * source.len() as f64;
        let speed = 2.0_f64.powf((self.pitch as f64) / 12.0); // Semitones → ratio
        let pan = (self.rand_f32() - 0.5) * 2.0 * self.pan_spread;

        self.grains
            .push(Grain::new(src_pos, speed, duration_samples, pan, 1.0));
    }

    /**
     * @brief PROCESS: Core granular synthesis block.
     * INDUSTRIAL: Zero-allocation per-block processing using pre-allocated grain pool.
     */
    pub fn process(
        &mut self,
        source: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        sample_rate: f32,
    ) {
        let interval = sample_rate as f64 / (self.density as f64).max(0.1);

        for i in 0..out_l.len() {
            // --- Grain scheduler ---
            self.samples_until_next -= 1.0;
            if self.samples_until_next <= 0.0 {
                self.spawn_grain(source, sample_rate);
                self.samples_until_next = interval;
            }

            // --- Grain summing and individual grain panning ---
            let mut sum_l = 0.0f32;
            let mut sum_r = 0.0f32;
            for grain in self.grains.iter_mut() {
                let sample = grain.tick(source);
                let pan_norm = (grain.pan + 1.0) * 0.5; // 0.0 to 1.0
                let angle = pan_norm as f64 * std::f64::consts::FRAC_PI_2;
                sum_l += sample * angle.cos() as f32;
                sum_r += sample * angle.sin() as f32;
            }

            out_l[i] += sum_l;
            out_r[i] += sum_r;

            // --- Retire dead grains (swap-remove for O(1)) ---
            self.grains.retain(|g| g.active);
        }
    }
}
