//! Bounce render planning.
//!
//! The native engine owns the audio graph, the transport and the streaming
//! writers, but every decision about what a bounce is allowed to write lives
//! here: which output paths and formats are acceptable, and how many frames
//! the offline render covers for the current arrangement.

use std::slice;

/// One timeline span contributed by a region. `loop_count` below one is
/// treated as a single pass, matching the engine's region contract.
#[repr(C)]
pub struct HirariBounceRegionSpan {
    pub start: u64,
    pub len: u64,
    pub loop_count: u32,
}

/// Everything the extent calculation needs. `limiter_tail` and `tail_millis`
/// are separate because they have different owners: the user-selected effect
/// tail and the master limiter's look-ahead.
#[repr(C)]
pub struct HirariBouncePlanInput {
    pub regions: *const HirariBounceRegionSpan,
    pub region_count: usize,
    pub midi_note_ends: *const u64,
    pub midi_note_count: usize,
    pub sample_rate: f64,
    pub range_active: u8,
    pub range_start: u64,
    pub range_end: u64,
    pub tail_millis: u32,
    pub limiter_tail: u64,
}

#[repr(C)]
#[derive(Default, PartialEq, Debug)]
pub struct HirariBouncePlan {
    pub total_samples: u64,
    pub output_rate: u32,
    /// Where the offline transport starts: the range start when a range is
    /// active, otherwise the beginning of the arrangement.
    pub playhead_start: u64,
}

pub const BOUNCE_PLAN_OK: i32 = 0;
pub const BOUNCE_PLAN_INVALID_RATE: i32 = -1;
pub const BOUNCE_PLAN_INVALID_RANGE: i32 = -2;

/// Timeline end of the arrangement. Region spans saturate instead of
/// wrapping; a loop count of zero renders one pass.
fn arrangement_end(regions: &[HirariBounceRegionSpan], midi_note_ends: &[u64]) -> u64 {
    let mut end = 0_u64;
    for region in regions {
        let loop_count = u64::from(region.loop_count.max(1));
        let timeline_length = region.len.saturating_mul(loop_count);
        end = end.max(region.start.saturating_add(timeline_length));
    }
    for &note_end in midi_note_ends {
        end = end.max(note_end);
    }
    end
}

fn plan_extent(input: &HirariBouncePlanInput) -> Result<HirariBouncePlan, i32> {
    if !input.sample_rate.is_finite()
        || input.sample_rate <= 0.0
        || input.sample_rate > 384_000.0
    {
        return Err(BOUNCE_PLAN_INVALID_RATE);
    }
    let output_rate = input.sample_rate as u32;
    let range_active = input.range_active != 0;
    if range_active && input.range_start >= input.range_end {
        return Err(BOUNCE_PLAN_INVALID_RANGE);
    }

    let regions = if input.region_count == 0 {
        &[][..]
    } else {
        // SAFETY: the caller passes a buffer of `region_count` entries that
        // stays alive for the duration of this call.
        unsafe { slice::from_raw_parts(input.regions, input.region_count) }
    };
    let midi_note_ends = if input.midi_note_count == 0 {
        &[][..]
    } else {
        // SAFETY: same contract as `regions` above.
        unsafe { slice::from_raw_parts(input.midi_note_ends, input.midi_note_count) }
    };

    let project_end = arrangement_end(regions, midi_note_ends);
    // Empty projects still need a valid, non-zero render; one second is the
    // compact fallback. Real arrangements are bounded by their content.
    let fallback_length = u64::from(output_rate);
    let mut bounded_length = if project_end > 0 {
        project_end
    } else {
        fallback_length
    };
    if range_active {
        bounded_length = input.range_end - input.range_start;
    }
    // Stateful processors keep sounding after the last region. The effect
    // tail and the master limiter's look-ahead are both part of the file.
    if input.tail_millis > 0 {
        let tail = u64::from(output_rate) * u64::from(input.tail_millis) / 1000;
        bounded_length = bounded_length.saturating_add(tail);
    }
    bounded_length = bounded_length.saturating_add(input.limiter_tail);

    Ok(HirariBouncePlan {
        total_samples: bounded_length.min(u64::from(u32::MAX)),
        output_rate,
        playhead_start: if range_active { input.range_start } else { 0 },
    })
}

/// Plans one offline render. Returns [`BOUNCE_PLAN_OK`] with `out` filled, or
/// a negative reason code that the engine maps to its own failure path.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_extent(
    input: *const HirariBouncePlanInput,
    out: *mut HirariBouncePlan,
) -> i32 {
    if input.is_null() || out.is_null() {
        return BOUNCE_PLAN_INVALID_RATE;
    }
    let plan = match plan_extent(&*input) {
        Ok(plan) => plan,
        Err(reason) => return reason,
    };
    *out = plan;
    BOUNCE_PLAN_OK
}

fn is_wav_suffix(suffix: &[u8]) -> bool {
    match suffix.len() {
        3 => suffix.eq_ignore_ascii_case(b"wav"),
        4 => suffix.eq_ignore_ascii_case(b"rf64"),
        _ => false,
    }
}

fn output_path_supported(path: &[u8], format: u32) -> bool {
    if format > 3 || path.is_empty() {
        return false;
    }
    match path.iter().rposition(|&byte| byte == b'.') {
        Some(dot) => is_wav_suffix(&path[dot + 1..]),
        None => false,
    }
}

/// True when `format` and the path extension describe a container this bounce
/// may write. Formats 0/3 are PCM16/PCM24 RIFF/RF64, 1 is Float32 WAVE64 and 2
/// is Float32 RIFF/RF64.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_output_path_supported(
    path: *const u8,
    len: usize,
    format: u32,
) -> bool {
    if path.is_null() || len == 0 {
        return false;
    }
    // SAFETY: the caller passes `len` readable bytes.
    output_path_supported(slice::from_raw_parts(path, len), format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: u64, len: u64, loop_count: u32) -> HirariBounceRegionSpan {
        HirariBounceRegionSpan {
            start,
            len,
            loop_count,
        }
    }

    fn input<'a>(
        regions: &'a [HirariBounceRegionSpan],
        notes: &'a [u64],
    ) -> HirariBouncePlanInput {
        HirariBouncePlanInput {
            regions: regions.as_ptr(),
            region_count: regions.len(),
            midi_note_ends: notes.as_ptr(),
            midi_note_count: notes.len(),
            sample_rate: 48_000.0,
            range_active: 0,
            range_start: 0,
            range_end: 0,
            tail_millis: 0,
            limiter_tail: 0,
        }
    }

    #[test]
    fn empty_project_uses_one_second_fallback() {
        let plan = plan_extent(&input(&[], &[])).expect("valid plan");
        assert_eq!(plan.output_rate, 48_000);
        assert_eq!(plan.total_samples, 48_000);
    }

    #[test]
    fn loops_and_midi_notes_extend_the_arrangement() {
        let regions = [span(100, 1_000, 0), span(50, 200, 3)];
        let notes = [4_000_u64];
        let plan = plan_extent(&input(&regions, &notes)).expect("valid plan");
        assert_eq!(plan.total_samples, 4_000);
    }

    #[test]
    fn saturating_spans_do_not_wrap() {
        let regions = [span(u64::MAX - 10, 100, 2)];
        let plan = plan_extent(&input(&regions, &[])).expect("valid plan");
        assert_eq!(plan.total_samples, u64::from(u32::MAX));
    }

    #[test]
    fn range_tail_and_limiter_add_up() {
        let mut values = input(&[span(0, 10_000, 1)], &[]);
        values.range_active = 1;
        values.range_start = 1_000;
        values.range_end = 3_000;
        values.tail_millis = 500;
        values.limiter_tail = 64;
        let plan = plan_extent(&values).expect("valid plan");
        assert_eq!(plan.total_samples, 2_000 + 24_000 + 64);
        assert_eq!(plan.playhead_start, 1_000);
    }

    #[test]
    fn inverted_range_is_a_range_failure() {
        let mut values = input(&[], &[]);
        values.range_active = 1;
        values.range_start = 10;
        values.range_end = 10;
        assert_eq!(plan_extent(&values), Err(BOUNCE_PLAN_INVALID_RANGE));
    }

    #[test]
    fn rate_must_be_finite_and_supported() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, 384_001.0] {
            let mut values = input(&[], &[]);
            values.sample_rate = rate;
            assert_eq!(plan_extent(&values), Err(BOUNCE_PLAN_INVALID_RATE));
        }
    }

    #[test]
    fn output_paths_accept_wav_and_rf64_case_insensitively() {
        assert!(output_path_supported(b"mix.wav", 0));
        assert!(output_path_supported(b"mix.WAV", 3));
        assert!(output_path_supported(b"stem.rf64", 2));
        assert!(output_path_supported(b"stem.RF64", 1));
        assert!(!output_path_supported(b"mix.mp3", 0));
        assert!(!output_path_supported(b"mix", 0));
        assert!(!output_path_supported(b"mix.wav", 4));
        assert!(!output_path_supported(b"", 0));
    }

    #[test]
    fn extent_entry_point_validates_pointers() {
        let mut plan = HirariBouncePlan::default();
        assert_eq!(
            unsafe { hirari_bounce_render_extent(std::ptr::null(), &mut plan) },
            BOUNCE_PLAN_INVALID_RATE
        );
        let values = input(&[], &[]);
        assert_eq!(
            unsafe { hirari_bounce_render_extent(&values, std::ptr::null_mut()) },
            BOUNCE_PLAN_INVALID_RATE
        );
        assert_eq!(
            unsafe { hirari_bounce_render_extent(&values, &mut plan) },
            BOUNCE_PLAN_OK
        );
        assert_eq!(plan.total_samples, 48_000);
    }
}
