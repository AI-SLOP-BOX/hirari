pub struct SampleZone {
    pub low_key: u8,
    pub high_key: u8,
    pub low_vel: u8,
    pub high_vel: u8,
}

pub struct SamplerOrchestrator {
    pub zones: Vec<SampleZone>,
    source: Vec<f32>,
    cursor: f64,
}

impl Default for SamplerOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplerOrchestrator {
    pub fn new() -> Self {
        Self {
            zones: Vec::new(),
            source: Vec::new(),
            cursor: 0.0,
        }
    }

    /// Installs an audio source. Call this from the control thread, not the RT callback.
    pub fn set_source(&mut self, source: Vec<f32>) {
        self.source = source;
        self.cursor = 0.0;
    }

    pub fn clear_source(&mut self) {
        self.source.clear();
        self.cursor = 0.0;
    }

    /// Renders the source with direct interpolated playback.
    pub fn process_classic(&mut self, buffer: &mut [f32]) {
        self.process_with(buffer, |this, position| this.linear_sample(position));
    }

    /// Renders overlapping, Hann-weighted grains without allocating in the render loop.
    pub fn process_granular(&mut self, buffer: &mut [f32]) {
        const GRAIN_SIZE: f64 = 128.0;
        self.process_with(buffer, |this, position| {
            let phase = (position % GRAIN_SIZE) / GRAIN_SIZE;
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * phase).cos();
            this.linear_sample(position) * window as f32
        });
    }

    /// Renders a small additive reconstruction from neighboring source samples.
    pub fn process_additive(&mut self, buffer: &mut [f32]) {
        self.process_with(buffer, |this, position| {
            let fundamental = this.linear_sample(position);
            let harmonic = this.linear_sample(position * 2.0) * 0.35;
            (fundamental * 0.75 + harmonic).clamp(-1.0, 1.0)
        });
    }

    /// Renders a bounded three-tap spectral-style smoothing pass.
    pub fn process_spectral(&mut self, buffer: &mut [f32]) {
        self.process_with(buffer, |this, position| {
            (this.linear_sample(position - 1.0)
                + this.linear_sample(position)
                + this.linear_sample(position + 1.0))
                / 3.0
        });
    }

    fn process_with<F>(&mut self, buffer: &mut [f32], mut render: F)
    where
        F: FnMut(&Self, f64) -> f32,
    {
        if self.source.is_empty() {
            buffer.fill(0.0);
            return;
        }
        for sample in buffer.iter_mut() {
            let value = render(self, self.cursor);
            *sample = if value.is_finite() { value } else { 0.0 };
            self.cursor += 1.0;
            if self.cursor >= self.source.len() as f64 {
                self.cursor = 0.0;
            }
        }
    }

    fn linear_sample(&self, position: f64) -> f32 {
        if self.source.is_empty() {
            return 0.0;
        }
        let length = self.source.len() as f64;
        let wrapped = position.rem_euclid(length);
        let index = wrapped.floor() as usize;
        let next = (index + 1) % self.source.len();
        let fraction = (wrapped - index as f64) as f32;
        self.source[index] * (1.0 - fraction) + self.source[next] * fraction
    }

    /// Performs a structural audit of the sampler configuration.
    pub fn audit_sampler(&self) -> bool {
        self.zones
            .iter()
            .all(|zone| zone.low_key <= zone.high_key && zone.low_vel <= zone.high_vel)
            && self.source.iter().all(|sample| sample.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_source_in_all_modes() {
        let mut sampler = SamplerOrchestrator::new();
        sampler.set_source(vec![0.25, -0.5, 0.75]);
        for process in [
            SamplerOrchestrator::process_classic as fn(&mut SamplerOrchestrator, &mut [f32]),
            SamplerOrchestrator::process_granular,
            SamplerOrchestrator::process_additive,
            SamplerOrchestrator::process_spectral,
        ] {
            let mut buffer = [0.0; 8];
            process(&mut sampler, &mut buffer);
            assert!(buffer.iter().any(|sample| *sample != 0.0));
            assert!(buffer.iter().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn empty_source_is_silent_and_invalid_zones_fail_audit() {
        let mut sampler = SamplerOrchestrator::new();
        let mut buffer = [1.0; 4];
        sampler.process_classic(&mut buffer);
        assert_eq!(buffer, [0.0; 4]);
        sampler.zones.push(SampleZone {
            low_key: 100,
            high_key: 20,
            low_vel: 0,
            high_vel: 127,
        });
        assert!(!sampler.audit_sampler());
    }
}
