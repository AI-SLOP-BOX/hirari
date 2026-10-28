//! Auto-arrangement policy: size the project from its region spans, ask the
//! structure analyzer for sections and emit one marker per section.
//!
//! The engine keeps the track/region storage and the event queue; this module
//! decides where the markers go and how long they are.

use std::ffi::c_void;

use crate::analysis_views::analysis_structure_sections;
use crate::region_edit_host::{HirariRegionHostOps, HirariRegionState};

/// Latest region end, saturating instead of wrapping past the end of the
/// timeline. Regions that start beyond every other end still extend the
/// project.
pub fn project_end(spans: &[(u64, u64)]) -> u64 {
    spans
        .iter()
        .fold(0u64, |end, (start, len)| end.max(start.saturating_add(*len)))
}

/// Section markers as `(start_sample, length_samples)`. The analyzer receives
/// the region starts and the project end; every section covers the project
/// without gaps, so the last marker always reaches the end.
pub fn section_markers(
    spans: &[(u64, u64)],
    bpm: f64,
    sample_rate: f64,
    tension: f32,
    valence: f32,
) -> Vec<(u64, u64)> {
    let starts: Vec<u64> = spans.iter().map(|(start, _)| *start).collect();
    analysis_structure_sections(&starts, project_end(spans), bpm, sample_rate, tension, valence)
        .into_iter()
        .map(|section| {
            (
                section.start_sample,
                section.end_sample.saturating_sub(section.start_sample),
            )
        })
        .collect()
}

pub type EmitSectionFn = unsafe extern "C" fn(*mut c_void, u64, u64);

/// Collects every region span through the existing host listing. Walking the
/// live tables instead of a dedicated bulk call keeps the host surface
/// unchanged; a region removed mid-walk simply drops out.
unsafe fn gather_spans(host: *mut c_void, ops: &HirariRegionHostOps) -> Option<Vec<(u64, u64)>> {
    let (Some(list_tracks), Some(list_regions), Some(find)) =
        (ops.list_tracks, ops.list_regions, ops.find_region)
    else {
        return None;
    };
    let track_total = unsafe { list_tracks(host, std::ptr::null_mut(), 0) };
    if track_total == 0 {
        return None;
    }
    let mut tracks = vec![0u32; track_total];
    let track_total = unsafe { list_tracks(host, tracks.as_mut_ptr(), tracks.len()) }.min(track_total);
    let mut spans = Vec::new();
    for &track in &tracks[..track_total] {
        let region_total = unsafe { list_regions(host, track, std::ptr::null_mut(), 0) };
        let mut regions = vec![0u32; region_total];
        let region_total =
            unsafe { list_regions(host, track, regions.as_mut_ptr(), regions.len()) }
                .min(region_total);
        for &region in &regions[..region_total] {
            let mut state = HirariRegionState::default();
            if unsafe { find(host, track, region, &mut state) } {
                spans.push((state.start, state.len));
            }
        }
    }
    Some(spans)
}

/// Runs one auto-arrangement pass. Returns false only when the project has no
/// tracks, matching the engine's previous empty-project rejection.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_auto_arrange(
    host: *mut c_void,
    bpm: f32,
    sample_rate: f64,
    tension: f32,
    valence: f32,
    emit: Option<EmitSectionFn>,
) -> bool {
    let Some(ops) = crate::region_edit_host::host_ops() else {
        return false;
    };
    let Some(spans) = (unsafe { gather_spans(host, &ops) }) else {
        return false;
    };
    let markers = section_markers(&spans, bpm as f64, sample_rate, tension, valence);
    if let Some(emit) = emit {
        for (start, length) in markers {
            unsafe { emit(host, start, length) };
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_timeline_has_no_extent() {
        assert_eq!(project_end(&[]), 0);
        assert_eq!(project_end(&[(0, 0)]), 0);
    }

    #[test]
    fn project_end_uses_the_latest_region_end() {
        assert_eq!(project_end(&[(0, 480), (960, 480), (240, 120)]), 1440);
        // A region that would wrap past the end of the timeline saturates.
        assert_eq!(project_end(&[(u64::MAX - 4, 16)]), u64::MAX);
    }

    #[test]
    fn markers_cover_the_project_without_gaps() {
        // 120 BPM at 48 kHz -> 8-bar block of 768000 samples.
        let spans = [(0u64, 1_000_000u64), (1_000_000, 536_000)];
        let markers = section_markers(&spans, 120.0, 48_000.0, 0.5, 0.5);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].0, 0);
        assert_eq!(markers[0].1, 768_000);
        assert_eq!(markers[1].0, 768_000);
        assert_eq!(markers[1].0 + markers[1].1, 1_536_000);
    }

    #[test]
    fn a_project_without_regions_yields_no_markers() {
        assert!(section_markers(&[], 120.0, 48_000.0, 0.5, 0.5).is_empty());
        assert!(section_markers(&[(0, 0)], 120.0, 48_000.0, 0.5, 0.5).is_empty());
    }

    #[test]
    fn invalid_tempo_or_rate_yields_no_markers() {
        let spans = [(0u64, 96_000u64)];
        assert!(section_markers(&spans, 0.0, 48_000.0, 0.5, 0.5).is_empty());
        assert!(section_markers(&spans, f64::NAN, 48_000.0, 0.5, 0.5).is_empty());
        assert!(section_markers(&spans, 120.0, 0.0, 0.5, 0.5).is_empty());
    }

    #[test]
    fn missing_host_ops_fail_closed() {
        assert!(!unsafe {
            hirari_engine_auto_arrange(std::ptr::null_mut(), 120.0, 48_000.0, 0.5, 0.5, None)
        });
    }
}
