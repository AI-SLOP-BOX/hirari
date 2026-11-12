//! Window synthesis + FFT helper fns, split out.

pub(crate) fn fill_acg_window(data: &mut [f32], bandwidth: f64, warp: f64, is_for_synthesis: bool) {
    let sigma = 0.3 / bandwidth.sqrt();
    let gaussian_factor = 0.0625 / (sigma * sigma);
    let gaussian = |x: f64| (-x * x * gaussian_factor).exp();
    let size = data.len();
    let inv_size = 1.0 / size as f64;
    let offset_scale = gaussian(1.0) / (gaussian(3.0) + gaussian(-1.0));
    let norm = 1.0 / (gaussian(0.0) - 2.0 * offset_scale * (gaussian(2.0)));
    let offset_i: usize = if size & 1 == 1 {
        1
    } else if is_for_synthesis {
        0
    } else {
        2
    };
    for (i, slot) in data.iter_mut().enumerate() {
        let mut r = (2 * i + offset_i) as f64 * inv_size - 1.0;
        r = (r + warp) / (1.0 + r * warp);
        *slot = (norm * (gaussian(r) - offset_scale * (gaussian(r - 2.0) + gaussian(r + 2.0))))
            as f32;
    }
    if warp != 0.0 {
        // Warp window vertically as well, to restore some width
        for slot in data.iter_mut() {
            *slot *= (((warp + 1.0) / (1.0 + warp * (2.0 * *slot as f64 - 1.0))).sqrt()) as f32;
        }
    }
}

/// `forcePerfectReconstruction`.
pub(crate) fn force_perfect_reconstruction(data: &mut [f32], window_length: usize, interval: usize) {
    for i in 0..interval {
        let mut sum2 = 0.0f64;
        let mut index = i;
        while index < window_length {
            sum2 += (data[index] * data[index]) as f64;
            index += interval;
        }
        let factor = (1.0 / sum2.sqrt()) as f32;
        let mut index = i;
        while index < window_length {
            data[index] *= factor;
            index += interval;
        }
    }
}

// ---------------------------------------------------------------------------
// SignalsmithStretch (signalsmith-stretch.h)
// ---------------------------------------------------------------------------

pub(crate) const NOISE_FLOOR: f32 = 1e-15;
pub(crate) const MAX_CLEAN_STRETCH: f32 = 2.0; // time-stretch ratio before we randomise phases
pub(crate) const SMOOTH_ENERGY_STEPS: usize = 3;
pub(crate) const SPLIT_MAIN_PREDICTION: usize = 8; // heavy, but we blend up to 4 phase predictions

/// `std::default_random_engine` on libc++: `minstd_rand` (multiplier 48271).
pub(crate) struct MinstdRand0 {
    state: u32,
}

impl MinstdRand0 {
    pub(crate) fn new(seed: u32) -> Self {
        let mut state = seed % 2147483647;
        if state == 0 {
            state = 1;
        }
        Self { state }
    }

    pub(crate) fn next(&mut self) -> u32 {
        self.state = ((48271u64 * self.state as u64) % 2147483647) as u32;
        self.state
    }
}

/// `std::uniform_real_distribution<float>` as libc++ implements it: one draw
/// scaled into [a, b).
pub(crate) fn uniform_real(a: f32, b: f32, rng: &mut MinstdRand0) -> f32 {
    let canonical = rng.next() as f32 / 2147483648.0f32;
    a + canonical * (b - a)
}
