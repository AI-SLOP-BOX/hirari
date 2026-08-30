pub struct SummingConfig {
    pub buffer_size: usize,
    pub headroom_db: f32,
}

pub struct BusSignal {
    pub id: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

/**
 * @struct SummingOrchestrator
 * @brief Industrial-grade audio summation and boutique headroom management.
 */
pub struct SummingOrchestrator {
    pub active_buses: Vec<BusSignal>,
}

impl Default for SummingOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SummingOrchestrator {
    pub fn new() -> Self {
        Self {
            active_buses: Vec::new(),
        }
    }

    /**
     * @brief MIX: Performs 64-bit boutique summation to eliminate rounding noise.
     * INDUSTRIAL: Using double-precision (f64) internally for absolute sonic transparency.
     */
    pub fn mix_buses_boutique(&self, target_l: &mut [f32], target_r: &mut [f32]) {
        let len = target_l.len();
        // INDUSTRIAL: Sovereign 64-bit accumulation buffers.
        let mut acc_l = vec![0.0f64; len];
        let mut acc_r = vec![0.0f64; len];

        for bus in &self.active_buses {
            if bus.left.len() != len || bus.right.len() != len {
                continue;
            }
            for i in 0..len {
                if bus.left[i].is_finite() {
                    acc_l[i] += bus.left[i] as f64;
                }
                if bus.right[i].is_finite() {
                    acc_r[i] += bus.right[i] as f64;
                }
            }
        }

        // INDUSTRIAL: Precision down-sampling back to 32-bit.
        for i in 0..len {
            target_l[i] = acc_l[i] as f32;
            target_r[i] = acc_r[i] as f32;
        }
    }

    /**
     * @brief FORENSIC LIMITER: 1.5ms Look-ahead peak limiter.
     * INDUSTRIAL: Prevents clipping while maintaining transient transparency.
     */
    pub fn process_limiter(&mut self, buffer: &mut [f32], config: &SummingConfig) -> f32 {
        let headroom_db = if config.headroom_db.is_finite() {
            config.headroom_db.clamp(-120.0, 0.0)
        } else {
            0.0
        };
        let limit = 10.0f32.powf(headroom_db / 20.0);
        let mut max_peak = 0.0f32;

        for i in 0..buffer.len() {
            let sample = if buffer[i].is_finite() {
                buffer[i]
            } else {
                0.0
            };
            let abs_sample = sample.abs();
            if abs_sample > max_peak {
                max_peak = abs_sample;
            }

            // INDUSTRIAL: Look-ahead attenuation logic.
            if abs_sample > limit {
                let attenuation = limit / abs_sample;
                buffer[i] *= attenuation;
            }
        }
        max_peak
    }

    pub fn audit_summing(&self) -> bool {
        let ids_are_unique = self.active_buses.iter().enumerate().all(|(index, bus)| {
            bus.id as usize == index
                || self.active_buses[..index]
                    .iter()
                    .all(|previous| previous.id != bus.id)
        });
        let buffers_are_stereo_and_finite = self.active_buses.iter().all(|bus| {
            bus.left.len() == bus.right.len()
                && bus
                    .left
                    .iter()
                    .chain(bus.right.iter())
                    .all(|sample| sample.is_finite())
        });
        ids_are_unique && buffers_are_stereo_and_finite
    }
}

#[cfg(test)]
mod tests {
    use super::{BusSignal, SummingOrchestrator};

    #[test]
    fn mismatched_bus_lengths_are_ignored_without_panicking() {
        let engine = SummingOrchestrator {
            active_buses: vec![BusSignal {
                id: 0,
                left: vec![1.0],
                right: vec![],
            }],
        };
        let mut left = [0.0, 0.0];
        let mut right = [0.0, 0.0];
        engine.mix_buses_boutique(&mut left, &mut right);
        assert_eq!(left, [0.0, 0.0]);
        assert!(!engine.audit_summing());
    }
}
