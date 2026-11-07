pub struct TemporalState {
    pub sample_rate: f64,
    pub fps: f32,
    pub bpm: f32,
}

pub struct TemporalOrchestrator {
    pub state: TemporalState,
}

impl TemporalOrchestrator {
    pub fn new(sr: f64, fps: f32, bpm: f32) -> Self {
        Self {
            state: TemporalState {
                sample_rate: sr,
                fps,
                bpm,
            },
        }
    }

    /// INDUSTRIAL: Converts sample position to SMPTE timecode (HH:MM:SS:FF).
    pub fn to_smpte(&self, samples: u64) -> String {
        if !self.state.sample_rate.is_finite() || self.state.sample_rate <= 0.0 || !self.state.fps.is_finite() || self.state.fps <= 0.0 { return "00:00:00:00".into(); }
        let total_seconds = samples as f64 / self.state.sample_rate;
        let hh = (total_seconds / 3600.0) as u32;
        let mm = ((total_seconds % 3600.0) / 60.0) as u32;
        let ss = (total_seconds % 60.0) as u32;
        let ff = ((total_seconds.fract() * self.state.fps as f64).round()) as u32;

        format!("{:02}:{:02}:{:02}:{:02}", hh, mm, ss, ff)
    }

    /// Parse HH:MM:SS:FF and convert it back to the nearest sample. This keeps
    /// frame/sample synchronization reversible for video and ADR workflows.
    pub fn from_smpte(&self, value: &str) -> Option<u64> {
        if !self.state.sample_rate.is_finite() || self.state.sample_rate <= 0.0 || !self.state.fps.is_finite() || self.state.fps <= 0.0 { return None; }
        let p: Vec<_> = value.split(':').collect();
        if p.len() != 4 { return None; }
        let h: u64 = p[0].parse().ok()?; let m: u64 = p[1].parse().ok()?; let s: u64 = p[2].parse().ok()?; let f: u64 = p[3].parse().ok()?;
        if m >= 60 || s >= 60 || (f as f32) >= self.state.fps.ceil() { return None; }
        let seconds = (h * 3600 + m * 60 + s) as f64 + f as f64 / self.state.fps as f64;
        Some((seconds * self.state.sample_rate).round() as u64)
    }

    /// INDUSTRIAL: Converts sample position to musical beats.
    pub fn to_beats(&self, samples: u64) -> f64 {
        let seconds = samples as f64 / self.state.sample_rate;
        seconds * (self.state.bpm as f64 / 60.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn smpte_round_trip() { let t = TemporalOrchestrator::new(48_000.0, 24.0, 120.0); let sample = 48_000 * 63 + 2_000; let s = t.to_smpte(sample); assert_eq!(t.from_smpte(&s), Some(sample)); }
}
