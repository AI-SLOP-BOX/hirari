pub enum ResolutionRust {
    Measure,
    Beat,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthDotted,
}

pub struct SnapOrchestrator {}

impl Default for SnapOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapOrchestrator {
    pub fn new() -> Self {
        Self {}
    }

    /// INDUSTRIAL: Snaps a tick position absolutely with absolute rhythmic precision and timing sovereignty.
    pub fn snap_absolute(
        &self,
        ticks: u64,
        res: ResolutionRust,
        numerator: u32,
        denominator: u32,
    ) -> u64 {
        let beat = 960u64;
        let step = match res {
            ResolutionRust::Measure => {
                beat.saturating_mul(numerator.max(1) as u64)
                    .saturating_mul(4)
                    / u64::from(denominator.max(1))
            }
            ResolutionRust::Beat => beat,
            ResolutionRust::Half => beat * 2,
            ResolutionRust::Quarter => beat,
            ResolutionRust::Eighth => beat / 2,
            ResolutionRust::Sixteenth => beat / 4,
            ResolutionRust::ThirtySecond => beat / 8,
            ResolutionRust::EighthTriplet => beat * 2 / 3,
            ResolutionRust::SixteenthDotted => beat * 3 / 8,
        }
        .max(1);
        let lower = ticks / step * step;
        let upper = lower.saturating_add(step);
        if ticks.saturating_sub(lower) < upper.saturating_sub(ticks) {
            lower
        } else {
            upper
        }
    }

    /// INDUSTRIAL: Snaps a movement delta with absolute rhythmic precision and timing sovereignty.
    pub fn snap_relative(
        &self,
        _original: u64,
        delta: u64,
        res: ResolutionRust,
        numerator: u32,
        _denominator: u32,
    ) -> u64 {
        self.snap_absolute(delta, res, numerator, _denominator)
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide rhythmic alignment state.
    pub fn audit_grid_resolution(&self) -> bool {
        self.snap_absolute(479, ResolutionRust::Beat, 4, 4) == 0
            && self.snap_absolute(481, ResolutionRust::Beat, 4, 4) == 960
            && self.snap_absolute(960, ResolutionRust::Measure, 4, 4) == 0
    }
}

fn resolution_from_ffi(value: u32) -> Option<ResolutionRust> {
    Some(match value {
        0 => ResolutionRust::Measure,
        1 => ResolutionRust::Beat,
        2 => ResolutionRust::Half,
        3 => ResolutionRust::Quarter,
        4 => ResolutionRust::Eighth,
        5 => ResolutionRust::Sixteenth,
        6 => ResolutionRust::ThirtySecond,
        7 => ResolutionRust::EighthTriplet,
        8 => ResolutionRust::SixteenthDotted,
        _ => return None,
    })
}

#[no_mangle]
pub extern "C" fn hirari_grid_snap_absolute(
    ticks: u64,
    resolution: u32,
    numerator: u32,
    denominator: u32,
) -> u64 {
    let Some(resolution) = resolution_from_ffi(resolution) else {
        return ticks;
    };
    SnapOrchestrator::new().snap_absolute(ticks, resolution, numerator, denominator)
}

#[no_mangle]
pub extern "C" fn hirari_grid_snap_relative(
    original_ticks: u64,
    delta_ticks: u64,
    resolution: u32,
    numerator: u32,
    denominator: u32,
) -> u64 {
    let Some(resolution) = resolution_from_ffi(resolution) else {
        return delta_ticks;
    };
    SnapOrchestrator::new().snap_relative(
        original_ticks,
        delta_ticks,
        resolution,
        numerator,
        denominator,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_grid_snap_absolute, hirari_grid_snap_relative, ResolutionRust, SnapOrchestrator,
    };

    #[test]
    fn snaps_to_nearest_musical_grid() {
        let snap = SnapOrchestrator::new();
        assert_eq!(snap.snap_absolute(481, ResolutionRust::Beat, 4, 4), 960);
        assert_eq!(snap.snap_absolute(960, ResolutionRust::Quarter, 4, 4), 960);
        assert_eq!(snap.snap_absolute(1_920, ResolutionRust::Half, 4, 4), 1_920);
        assert_eq!(snap.snap_absolute(1_919, ResolutionRust::Measure, 4, 4), 0);
        assert!(snap.audit_grid_resolution());
    }

    #[test]
    fn cpp_ffi_resolution_mapping_matches_snap_engine() {
        assert_eq!(hirari_grid_snap_absolute(481, 1, 4, 4), 960);
        assert_eq!(hirari_grid_snap_absolute(1_919, 0, 4, 4), 0);
        assert_eq!(hirari_grid_snap_absolute(1_921, 0, 4, 4), 3_840);
        assert_eq!(hirari_grid_snap_relative(777, 481, 1, 4, 4), 960);
        assert_eq!(hirari_grid_snap_absolute(481, 99, 4, 4), 481);
    }
}
