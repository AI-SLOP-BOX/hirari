const TICKS_PER_BEAT: u64 = 960;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HirariMidiQuantizerNote {
    pub start_tick: u64,
    pub length_ticks: u64,
    pub note: u8,
    pub velocity: u8,
}

fn resolution_to_ticks(resolution: u32) -> u64 {
    match resolution {
        0 => TICKS_PER_BEAT,
        1 => TICKS_PER_BEAT / 2,
        2 => TICKS_PER_BEAT / 4,
        3 => TICKS_PER_BEAT / 8,
        4 => (TICKS_PER_BEAT * 2) / 3,
        5 => TICKS_PER_BEAT / 3,
        6 => (TICKS_PER_BEAT * 3) / 4,
        7 => (TICKS_PER_BEAT * 3) / 8,
        _ => 0,
    }
}

fn quantized_tick(tick: u64, grid: u64, swing: f32, strength: f32) -> u64 {
    if grid == 0 || !swing.is_finite() || !strength.is_finite() || strength <= 0.0 {
        return tick;
    }
    let clamped_strength = strength.clamp(0.0, 1.0);
    let swing_shift = (swing.clamp(0.0, 1.0) - 0.5) * 2.0;
    let index = tick / grid;
    let grid_start = index.saturating_mul(grid);
    let grid_end = grid_start.saturating_add(grid);
    let end_index = if grid_end == u64::MAX {
        index
    } else {
        index.saturating_add(1)
    };
    let swing_offset = |line: u64| -> i64 {
        if line % 2 == 1 {
            (swing_shift * 0.5 * grid as f32) as i64
        } else {
            0
        }
    };
    let signed = |value: u64| value.min(i64::MAX as u64) as i64;
    let start_signed = signed(grid_start);
    let end_signed = signed(grid_end);
    let original_signed = signed(tick);
    let start_target = start_signed as i128 + swing_offset(index) as i128;
    let end_target = end_signed as i128 + swing_offset(end_index) as i128;
    let original = original_signed as i128;
    let start_distance = original - start_target;
    let end_distance = original - end_target;
    let nearest = if start_distance.abs() <= end_distance.abs() {
        start_target
    } else {
        end_target
    }
    .clamp(0, i64::MAX as i128);
    let delta = nearest - original;
    let adjusted = original + (clamped_strength * delta as f32) as i64 as i128;
    adjusted.clamp(0, i64::MAX as i128) as u64
}

unsafe fn quantize_impl(
    notes: *mut HirariMidiQuantizerNote,
    count: usize,
    resolution: u32,
    swing: f32,
    strength: f32,
    with_length: bool,
) -> bool {
    if notes.is_null() || count == 0 || !swing.is_finite() || !strength.is_finite() {
        return false;
    }
    let grid = resolution_to_ticks(resolution);
    if grid == 0 || strength <= 0.0 {
        return false;
    }
    let notes = std::slice::from_raw_parts_mut(notes, count);
    if with_length {
        for note in notes {
            let end = note.start_tick.saturating_add(note.length_ticks);
            let start = quantized_tick(note.start_tick, grid, swing, strength);
            let quantized_end = quantized_tick(end, grid, swing, strength);
            let minimum = (grid / 4).max(1);
            note.start_tick = start;
            note.length_ticks = quantized_end.saturating_sub(start).max(minimum);
        }
    } else {
        for note in notes {
            note.start_tick = quantized_tick(note.start_tick, grid, swing, strength);
        }
    }
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_quantize(
    notes: *mut HirariMidiQuantizerNote,
    count: usize,
    resolution: u32,
    swing: f32,
    strength: f32,
    with_length: bool,
) -> bool {
    // SAFETY: the caller provides `count` writable notes for this call.
    unsafe { quantize_impl(notes, count, resolution, swing, strength, with_length) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_nearest_grid_and_preserves_note_metadata() {
        let mut notes = [
            HirariMidiQuantizerNote {
                start_tick: 470,
                length_ticks: 120,
                note: 64,
                velocity: 91,
            },
            HirariMidiQuantizerNote {
                start_tick: 1_030,
                length_ticks: 240,
                note: 67,
                velocity: 77,
            },
        ];
        // Eighth-note grid: 470 is closer to 480 than 0; 1030 is closer to 960.
        assert!(unsafe {
            hirari_midi_quantize(notes.as_mut_ptr(), notes.len(), 1, 0.5, 1.0, false)
        });
        assert_eq!(notes[0].start_tick, 480);
        assert_eq!(notes[1].start_tick, 960);
        assert_eq!((notes[0].note, notes[0].velocity), (64, 91));
    }

    #[test]
    fn swing_and_strength_are_applied_to_alternating_lines() {
        let quarter = 960;
        let tick = quarter + 10;
        let neutral = quantized_tick(tick, resolution_to_ticks(0), 0.5, 1.0);
        let swung = quantized_tick(tick, resolution_to_ticks(0), 1.0, 1.0);
        let half = quantized_tick(tick, resolution_to_ticks(0), 1.0, 0.5);
        assert_eq!(neutral, quarter);
        assert_eq!(swung, quarter + 480);
        assert_eq!(half, tick + (480 - 10) / 2);
    }

    #[test]
    fn length_mode_quantizes_end_and_enforces_minimum_duration() {
        let mut notes = [
            HirariMidiQuantizerNote {
                start_tick: 470,
                length_ticks: 100,
                note: 60,
                velocity: 100,
            },
            HirariMidiQuantizerNote {
                start_tick: u64::MAX - 3,
                length_ticks: 10,
                note: 61,
                velocity: 80,
            },
        ];
        assert!(unsafe {
            hirari_midi_quantize(notes.as_mut_ptr(), notes.len(), 2, 0.5, 1.0, true)
        });
        assert_eq!(notes[0].start_tick, 480);
        assert!(notes[0].length_ticks >= 60);
        assert!(notes[1].length_ticks >= 60);
    }

    #[test]
    fn invalid_options_leave_notes_untouched() {
        let original = HirariMidiQuantizerNote {
            start_tick: 123,
            length_ticks: 44,
            note: 48,
            velocity: 99,
        };
        let mut note = original;
        assert!(!unsafe { hirari_midi_quantize(&mut note, 1, 8, 0.5, 1.0, false) });
        assert_eq!(note, original);
        assert!(!unsafe { hirari_midi_quantize(&mut note, 1, 0, f32::NAN, 1.0, false) });
        assert_eq!(note, original);
    }
}
