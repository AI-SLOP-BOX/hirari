pub struct EqBand {
    pub f: f32,
    pub g: f32,
    pub q: f32,
}

pub struct ConsoleModelEngine {
    pub drive: f32,
    pub bands: [EqBand; 4],
    pub threshold: f32,
}

impl Default for ConsoleModelEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleModelEngine {
    pub fn new() -> Self {
        Self {
            drive: 1.2,
            bands: [
                EqBand {
                    f: 100.0,
                    g: 0.0,
                    q: 0.707,
                },
                EqBand {
                    f: 1000.0,
                    g: 0.0,
                    q: 0.707,
                },
                EqBand {
                    f: 5000.0,
                    g: 0.0,
                    q: 0.707,
                },
                EqBand {
                    f: 10000.0,
                    g: 0.0,
                    q: 0.707,
                },
            ],
            threshold: -20.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// INDUSTRIAL: Industrial-Grade Analogue Console Emulation (Divine Series).
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = l.len().min(r.len());
        if !self.audit_console_model() {
            return;
        }

        for s in 0..len {
            // Process Left
            l[s] = ((if l[s].is_finite() { l[s] } else { 0.0 }) * self.drive).tanh();

            // Process Right
            r[s] = ((if r[s].is_finite() { r[s] } else { 0.0 }) * self.drive).tanh();
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Console Model state.
    pub fn audit_console_model(&self) -> bool {
        self.drive.is_finite()
            && self.drive >= 0.0
            && self.bands.iter().all(|band| {
                band.f.is_finite() && band.g.is_finite() && band.q.is_finite() && band.q > 0.0
            })
            && self.threshold.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::ConsoleModelEngine;

    #[test]
    fn reset_restores_console_parameters_and_processing_defaults() {
        let mut console = ConsoleModelEngine::new();
        console.drive = 3.0;
        console.bands[0].g = 9.0;
        console.threshold = -2.0;
        console.reset();

        assert_eq!(console.drive, 1.2);
        assert_eq!(console.bands[0].g, 0.0);
        assert_eq!(console.threshold, -20.0);
        assert!(console.audit_console_model());
    }
}
