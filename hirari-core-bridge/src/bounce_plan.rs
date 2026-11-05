//! Bounce render planning.
//!
//! The native engine owns the audio graph and the transport, but every decision
//! about what a bounce is allowed to write lives here: which output paths and
//! formats are acceptable, and how many frames the offline render covers for
//! the current arrangement. The driver gathers the arrangement straight from
//! the Rust stores — the region table, the MIDI snapshot, the device format and
//! the master limiter — so no plan data crosses the FFI boundary.

use std::slice;

/// One timeline span contributed by a region. `loop_count` below one is
/// treated as a single pass, matching the engine's region contract.
#[derive(Clone, Copy)]
pub struct RegionSpan {
    pub start: u64,
    pub len: u64,
    pub loop_count: u32,
}

/// Everything the extent calculation needs. `limiter_tail` and `tail_millis`
/// are separate because they have different owners: the user-selected effect
/// tail and the master limiter's look-ahead.
pub(crate) struct BouncePlanInput<'a> {
    pub regions: &'a [RegionSpan],
    pub midi_note_ends: &'a [u64],
    pub sample_rate: f64,
    pub range_active: bool,
    pub range_start: u64,
    pub range_end: u64,
    pub tail_millis: u32,
    pub limiter_tail: u64,
}

/// The extent one offline render covers.
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
fn arrangement_end(regions: &[RegionSpan], midi_note_ends: &[u64]) -> u64 {
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

pub(crate) fn plan_extent(input: &BouncePlanInput) -> Result<HirariBouncePlan, i32> {
    if !input.sample_rate.is_finite()
        || input.sample_rate <= 0.0
        || input.sample_rate > 384_000.0
    {
        return Err(BOUNCE_PLAN_INVALID_RATE);
    }
    let output_rate = input.sample_rate as u32;
    if input.range_active && input.range_start >= input.range_end {
        return Err(BOUNCE_PLAN_INVALID_RANGE);
    }

    let project_end = arrangement_end(input.regions, input.midi_note_ends);
    // Empty projects still need a valid, non-zero render; one second is the
    // compact fallback. Real arrangements are bounded by their content.
    let fallback_length = u64::from(output_rate);
    let mut bounded_length = if project_end > 0 {
        project_end
    } else {
        fallback_length
    };
    if input.range_active {
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
        playhead_start: if input.range_active {
            input.range_start
        } else {
            0
        },
    })
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

/// Bounce lifecycle states, matching the engine's published values.
pub const BOUNCE_STATE_QUEUED: u32 = 1;
pub const BOUNCE_STATE_RENDERING: u32 = 2;
pub const BOUNCE_STATE_COMPLETED: u32 = 3;
pub const BOUNCE_STATE_FAILED: u32 = 4;
pub const BOUNCE_STATE_CANCELLED: u32 = 5;
pub const BOUNCE_STATE_PAUSED: u32 = 6;

/// True once a render has finished, failed or been cancelled. A terminal
/// render may be retired or replaced by a new one.
#[no_mangle]
pub extern "C" fn hirari_bounce_state_terminal(state: u32) -> bool {
    matches!(
        state,
        BOUNCE_STATE_COMPLETED | BOUNCE_STATE_FAILED | BOUNCE_STATE_CANCELLED
    )
}

/// True while a cancellation request can still take effect.
#[no_mangle]
pub extern "C" fn hirari_bounce_state_cancellable(state: u32) -> bool {
    matches!(
        state,
        BOUNCE_STATE_QUEUED | BOUNCE_STATE_RENDERING | BOUNCE_STATE_PAUSED
    )
}

/// Only a rendering bounce can pause; resuming is the inverse transition.
#[no_mangle]
pub extern "C" fn hirari_bounce_state_pausable(state: u32) -> bool {
    state == BOUNCE_STATE_RENDERING
}

#[no_mangle]
pub extern "C" fn hirari_bounce_state_resumable(state: u32) -> bool {
    state == BOUNCE_STATE_PAUSED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: u64, len: u64, loop_count: u32) -> RegionSpan {
        RegionSpan {
            start,
            len,
            loop_count,
        }
    }

    fn input<'a>(
        regions: &'a [RegionSpan],
        notes: &'a [u64],
    ) -> BouncePlanInput<'a> {
        BouncePlanInput {
            regions,
            midi_note_ends: notes,
            sample_rate: 48_000.0,
            range_active: false,
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
        let regions = [span(0, 10_000, 1)];
        let mut values = input(&regions, &[]);
        values.range_active = true;
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
        values.range_active = true;
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
    fn bounce_states_follow_the_lifecycle_sets() {
        let states = [
            (0, false, false, false, false),
            (1, false, true, false, false),
            (2, false, true, true, false),
            (3, true, false, false, false),
            (4, true, false, false, false),
            (5, true, false, false, false),
            (6, false, true, false, true),
        ];
        for (state, terminal, cancellable, pausable, resumable) in states {
            assert_eq!(hirari_bounce_state_terminal(state), terminal, "{state}");
            assert_eq!(hirari_bounce_state_cancellable(state), cancellable, "{state}");
            assert_eq!(hirari_bounce_state_pausable(state), pausable, "{state}");
            assert_eq!(hirari_bounce_state_resumable(state), resumable, "{state}");
        }
        assert!(!hirari_bounce_state_terminal(99));
        assert!(!hirari_bounce_state_cancellable(99));
    }
}
