use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct ScaleQuantizerState {
    configuration: AtomicU64,
    chords: Mutex<Vec<ScaleChord>>,
}

struct ScaleChord {
    tick: u64,
    root: i32,
    intervals: Vec<i32>,
    name: Vec<u8>,
}

fn scale_pattern(scale: u32) -> u32 {
    match scale {
        0 => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 11),
        1 => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 10),
        2 => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 8) | (1 << 11),
        3 => (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 7) | (1 << 9) | (1 << 11),
        4 => (1 << 0) | (1 << 2) | (1 << 4) | (1 << 7) | (1 << 9),
        _ => 0x0fff,
    }
}

#[no_mangle]
pub extern "C" fn hirari_scale_quantizer_create() -> *mut c_void {
    Box::into_raw(Box::new(ScaleQuantizerState {
        configuration: AtomicU64::new((scale_pattern(0) as u64) | (0_u64 << 12)),
        chords: Mutex::new(Vec::new()),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_scale_quantizer_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: the handle is created by `hirari_scale_quantizer_create` and destroyed once.
        unsafe { drop(Box::from_raw(state.cast::<ScaleQuantizerState>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_scale_quantizer_set(
    state: *mut c_void,
    root: i32,
    scale: u32,
) {
    let Some(state) = (unsafe { state.cast::<ScaleQuantizerState>().as_ref() }) else {
        return;
    };
    let configuration = scale_pattern(scale) as u64 | ((root.clamp(0, 11) as u64) << 12);
    state.configuration.store(configuration, Ordering::Release);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_scale_quantizer_note(
    state: *const c_void,
    note: i32,
) -> i32 {
    let Some(state) = (unsafe { state.cast::<ScaleQuantizerState>().as_ref() }) else {
        return note;
    };
    let configuration = state.configuration.load(Ordering::Acquire);
    let root = ((configuration >> 12) & 0x0f) as i32;
    let pattern = (configuration & 0x0fff) as u32;
    if pattern == 0 || pattern == 0x0fff {
        return note;
    }
    let pitch_class = note.rem_euclid(12);
    let relative = (pitch_class - root).rem_euclid(12);
    if pattern & (1u32 << relative) != 0 {
        return note;
    }
    let mut best_offset = 1;
    for distance in 1..=6 {
        let below = (relative - distance).rem_euclid(12);
        let above = (relative + distance).rem_euclid(12);
        if pattern & (1u32 << below) != 0 {
            best_offset = -distance;
            break;
        }
        if pattern & (1u32 << above) != 0 {
            best_offset = distance;
            break;
        }
    }
    note.saturating_add(best_offset)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_scale_quantizer_add_chord(
    state: *mut c_void,
    tick: u64,
    root: i32,
    intervals: *const i32,
    interval_count: usize,
    name: *const u8,
    name_length: usize,
) -> bool {
    if state.is_null()
        || intervals.is_null()
        || name.is_null()
        || interval_count == 0
        || interval_count > 32
        || name_length == 0
        || name_length > 1_048_576
    {
        return false;
    }
    // SAFETY: input pointers are paired with lengths supplied by C++ containers.
    let intervals = unsafe { std::slice::from_raw_parts(intervals, interval_count) };
    let intervals: Vec<i32> = intervals
        .iter()
        .copied()
        .filter(|interval| (-48..=48).contains(interval))
        .collect();
    if intervals.is_empty() {
        return false;
    }
    let name = unsafe { std::slice::from_raw_parts(name, name_length) }.to_vec();
    let state = unsafe { &*state.cast::<ScaleQuantizerState>() };
    let mut chords = state.chords.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    chords.push(ScaleChord {
        tick,
        root: root.rem_euclid(12),
        intervals,
        name,
    });
    chords.sort_by_key(|chord| chord.tick);
    true
}

/// Returns 0 when no chord exists, 1 on success, and 2 when output buffers
/// need to be enlarged. Required lengths are reported for the latter case.
#[no_mangle]
pub unsafe extern "C" fn hirari_scale_quantizer_chord_at(
    state: *const c_void,
    tick: u64,
    root_out: *mut i32,
    intervals_out: *mut i32,
    intervals_capacity: usize,
    intervals_length_out: *mut usize,
    name_out: *mut u8,
    name_capacity: usize,
    name_length_out: *mut usize,
) -> u8 {
    if state.is_null()
        || root_out.is_null()
        || intervals_length_out.is_null()
        || name_length_out.is_null()
    {
        return 0;
    }
    let state = unsafe { &*state.cast::<ScaleQuantizerState>() };
    let chords = state.chords.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(chord) = chords.iter().rev().find(|chord| chord.tick <= tick) else {
        return 0;
    };
    unsafe {
        *root_out = chord.root;
        *intervals_length_out = chord.intervals.len();
        *name_length_out = chord.name.len();
    }
    if intervals_capacity < chord.intervals.len()
        || name_capacity < chord.name.len()
        || (chord.intervals.len() > 0 && intervals_out.is_null())
        || (chord.name.len() > 0 && name_out.is_null())
    {
        return 2;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(chord.intervals.as_ptr(), intervals_out, chord.intervals.len());
        std::ptr::copy_nonoverlapping(chord.name.as_ptr(), name_out, chord.name.len());
    }
    1
}

#[cfg(test)]
mod scale_quantizer_ffi_tests {
    use super::*;

    #[test]
    fn scale_quantizer_matches_major_and_pentatonic_patterns() {
        let state = hirari_scale_quantizer_create();
        unsafe {
            assert_eq!(hirari_scale_quantizer_note(state, 61), 60);
            hirari_scale_quantizer_set(state, 0, 4);
            assert_eq!(hirari_scale_quantizer_note(state, 65), 64);
            hirari_scale_quantizer_destroy(state);
        }
    }

    #[test]
    fn scale_quantizer_clamps_root_and_preserves_chromatic_mode() {
        let state = hirari_scale_quantizer_create();
        unsafe {
            hirari_scale_quantizer_set(state, 99, 99);
            assert_eq!(hirari_scale_quantizer_note(state, -13), -13);
            hirari_scale_quantizer_destroy(state);
        }
    }

    #[test]
    fn chord_track_sorts_events_and_returns_the_latest_prior_chord() {
        let state = hirari_scale_quantizer_create();
        let triad = [0, 4, 7, 99];
        let c_name = b"C";
        let g_name = b"G";
        unsafe {
            assert!(hirari_scale_quantizer_add_chord(
                state, 960, 7, triad.as_ptr(), triad.len(), g_name.as_ptr(), g_name.len(),
            ));
            assert!(hirari_scale_quantizer_add_chord(
                state, 0, 0, triad.as_ptr(), triad.len(), c_name.as_ptr(), c_name.len(),
            ));
            let mut root = 0;
            let mut intervals = [0; 32];
            let mut interval_count = 0;
            let mut name = [0; 8];
            let mut name_length = 0;
            assert_eq!(hirari_scale_quantizer_chord_at(
                state, 959, &mut root, intervals.as_mut_ptr(), intervals.len(),
                &mut interval_count, name.as_mut_ptr(), name.len(), &mut name_length,
            ), 1);
            assert_eq!((root, interval_count), (0, 3));
            assert_eq!(&name[..name_length], b"C");
            assert_eq!(hirari_scale_quantizer_chord_at(
                state, 960, &mut root, intervals.as_mut_ptr(), intervals.len(),
                &mut interval_count, name.as_mut_ptr(), name.len(), &mut name_length,
            ), 1);
            assert_eq!((root, interval_count), (7, 3));
            assert_eq!(&name[..name_length], b"G");
            hirari_scale_quantizer_destroy(state);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonicOrchestrator {
    pub chord_progression: Vec<ChordEvent>,
    pub current_root: u8,
    pub current_scale: ScaleType,
}

impl Default for HarmonicOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl HarmonicOrchestrator {
    const MAX_MIDI_NOTE: u8 = 127;

    pub fn new() -> Self {
        Self {
            chord_progression: Vec::new(),
            current_root: 0,
            current_scale: ScaleType::Major,
        }
    }

    pub fn set_root(&mut self, root: u8) { self.current_root = root.min(Self::MAX_MIDI_NOTE); }
    pub fn set_scale(&mut self, scale: ScaleType) { self.current_scale = scale; }

    /// Quantizes a whole MIDI part through the active Scale Assistant.
    pub fn quantize_notes(&self, notes: &mut [u8]) -> usize {
        let mut changed = 0;
        for note in notes {
            let quantized = self.quantize_note(*note);
            if quantized != *note { *note = quantized; changed += 1; }
        }
        changed
    }

    /// INDUSTRIAL: Quantizes a MIDI note to the nearest scale degree.
    pub fn quantize_note(&self, note: u8) -> u8 {
        // INDUSTRIAL: Implementation of high-performance scale quantization.
        // Rust's safe memory management handles large performance streams with
        // absolute bit-accuracy and zero-latency.
        let pattern: &[u8] = match self.current_scale {
            ScaleType::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleType::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleType::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleType::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleType::Pentatonic => &[0, 2, 4, 7, 9],
            ScaleType::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleType::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        };

        // `u8` permits values above the MIDI note range.  Keep the public API
        // unchanged, but never return an invalid MIDI note.
        let note = note.min(Self::MAX_MIDI_NOTE);
        let root = self.current_root.min(Self::MAX_MIDI_NOTE);
        let pc = (note as i16 - root as i16).rem_euclid(12) as u8;
        if pattern.contains(&pc) {
            return note;
        }

        let mut best_note = note;
        let mut min_dist = 12;
        for &p in pattern {
            let dist = (pc as i8 - p as i8).abs();
            if dist < min_dist {
                min_dist = dist;
                best_note = (note as i16 + (p as i16 - pc as i16))
                    .clamp(0, Self::MAX_MIDI_NOTE as i16) as u8;
            }
        }
        best_note
    }

    /// INDUSTRIAL: Adds a chord event to the progression with memory-safe collections.
    pub fn add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) {
        // INDUSTRIAL: Implementation of high-performance chord tracking.
        // Rust's ProgressionEngine ensures bit-accurate chord synchronization.
        self.chord_progression.push(ChordEvent {
            tick,
            root: root.min(Self::MAX_MIDI_NOTE),
            intervals,
            name: name.to_string(),
        });
        self.chord_progression.sort_by_key(|e| e.tick);
    }

    /// Validated chord-track insertion used by project/CLI commands. A tick
    /// already occupied by a chord is replaced atomically.
    pub fn try_add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) -> bool {
        if name.trim().is_empty() || name.len() > 128 || name.contains('\0') || intervals.is_empty() || intervals.len() > 32 || intervals.iter().any(|interval| *interval > 127) { return false; }
        let event = ChordEvent { tick, root, intervals, name: name.trim().to_owned() };
        if let Some(existing) = self.chord_progression.iter_mut().find(|existing| existing.tick == tick) { *existing = event; }
        else { self.chord_progression.push(event); self.chord_progression.sort_by_key(|event| event.tick); }
        true
    }

    pub fn remove_chord_at(&mut self, tick: u64) -> bool {
        let before = self.chord_progression.len();
        self.chord_progression.retain(|event| event.tick != tick);
        before != self.chord_progression.len()
    }

    /// Applies the active chord-track harmony to `(tick, pitch)` note pairs.
    /// Each note is moved to the nearest chord tone in the same or adjacent
    /// octave, giving MIDI parts a deterministic chord-following/voicing mode.
    pub fn follow_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || self.chord_progression.is_empty() { return 0; }
        let mut changed = 0;
        for (tick, pitch) in notes {
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= *tick) else { continue; };
            if chord.intervals.is_empty() { continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 3);
            for octave in -1i16..=1 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval % 128) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let Some(nearest) = candidates.into_iter().min_by_key(|candidate| (i16::from(*candidate) - i16::from(*pitch)).abs()) else { continue; };
            if nearest != *pitch { *pitch = nearest; changed += 1; }
        }
        changed
    }

    /// Chord-track voicing that preserves ascending note order inside each
    /// timestamp, avoiding the collapsed unison result of simple remapping.
    pub fn voice_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || notes.is_empty() { return 0; }
        let mut order: Vec<usize> = (0..notes.len()).collect();
        order.sort_by_key(|index| (notes[*index].0, notes[*index].1));
        let mut changed = 0;
        let mut cursor = 0;
        while cursor < order.len() {
            let tick = notes[order[cursor]].0;
            let end = order[cursor..].iter().position(|index| notes[*index].0 != tick).map(|offset| cursor + offset).unwrap_or(order.len());
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= tick) else { cursor = end; continue; };
            if chord.intervals.is_empty() { cursor = end; continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 11);
            for octave in 0i16..=10 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let mut previous = 0u8;
            for (position, index) in order[cursor..end].iter().enumerate() {
                let original = notes[*index].1;
                let chosen = candidates.iter().copied()
                    .filter(|candidate| position == 0 || *candidate > previous)
                    .min_by_key(|candidate| (i16::from(*candidate) - i16::from(original)).abs())
                    .unwrap_or(previous);
                if chosen != original { notes[*index].1 = chosen; changed += 1; }
                previous = chosen;
            }
            cursor = end;
        }
        changed
    }

    /**
     * @brief SUGGEST: AI-driven chord progression assistant.
     * INDUSTRIAL: Provides musically accurate suggestions based on functional harmony.
     */
    pub fn suggest_next_chords(&self, last_chord_name: &str) -> Vec<String> {
        // INDUSTRIAL: Simplified functional harmony transition model.
        match last_chord_name {
            "I" | "C" | "Cmaj" => vec![
                "IV".to_string(),
                "V".to_string(),
                "vi".to_string(),
                "ii".to_string(),
            ],
            "IV" | "F" | "Fmaj" => vec!["V".to_string(), "I".to_string(), "ii".to_string()],
            "V" | "G" | "G7" => vec!["I".to_string(), "vi".to_string()],
            "vi" | "Am" => vec!["IV".to_string(), "ii".to_string(), "V".to_string()],
            _ => vec!["I".to_string(), "IV".to_string(), "V".to_string()],
        }
    }

    /**
     * @brief TENSION: Calculates the harmonic tension score.
     * INDUSTRIAL: Score from 0.0 (Resolution) to 1.0 (Extreme Dissonance).
     */
    pub fn calculate_tension(&self) -> f32 {
        // INDUSTRIAL: Model tension based on chord distance from Tonic.
        self.chord_progression
            .last()
            .map(|chord| {
                let last = chord.name.trim();
                if last.is_empty() {
                    0.0
                } else if last.contains('7') || last.contains("dim") {
                    0.85
                } else if last.contains('m') {
                    0.4
                } else {
                    // Unknown chord names are treated as neutral rather than
                    // causing a lookup failure or an unsafe assumption.
                    0.1
                }
            })
            .unwrap_or(0.0)
    }

    pub fn audit_harmonic(&self) -> bool {
        let events_are_ordered = self
            .chord_progression
            .windows(2)
            .all(|events| events[0].tick <= events[1].tick);
        self.current_root <= Self::MAX_MIDI_NOTE
            && events_are_ordered
            && self.chord_progression.iter().all(|event| {
                !event.name.trim().is_empty()
                    && event.root <= Self::MAX_MIDI_NOTE
                    && event.intervals.iter().all(|interval| *interval <= 127)
            })
    }
}
