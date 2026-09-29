#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotationType {
    Note,
    Rest,
    Clef,
    Accidental,
    Slur,
    Dynamic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NotationSymbol {
    pub symbol_type: NotationType,
    pub val: u32,
    pub x: f32,
    pub y: f32,
    pub is_visible: bool,
    pub start: u64,
    pub duration: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreAnnotation {
    pub position: u64,
    pub text: String,
    pub kind: AnnotationKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotationKind {
    Lyric,
    Chord,
    Expression,
}
impl ScoreAnnotation {
    pub fn validate(&self) -> bool {
        !self.text.trim().is_empty() && self.text.len() <= 512 && !self.text.contains('\0')
    }
}

#[derive(Default)]
pub struct AnnotationTrack {
    pub entries: Vec<ScoreAnnotation>,
}
impl AnnotationTrack {
    pub fn insert(&mut self, annotation: ScoreAnnotation) -> bool {
        if !annotation.validate() || self.entries.len() >= 65_536 {
            return false;
        }
        self.entries.push(annotation);
        self.entries.sort_by_key(|a| a.position);
        true
    }
    pub fn at(&self, position: u64) -> Vec<&ScoreAnnotation> {
        self.entries
            .iter()
            .filter(|a| a.position == position)
            .collect()
    }
    pub fn search(&self, query: &str) -> Vec<&ScoreAnnotation> {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|a| a.text.to_ascii_lowercase().contains(&query))
            .collect()
    }
    pub fn remove_at(&mut self, position: u64, kind: Option<AnnotationKind>) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|a| !(a.position == position && kind.is_none_or(|k| a.kind == k)));
        before - self.entries.len()
    }
    pub fn shift(&mut self, start: u64, delta: i64) -> bool {
        if delta == 0 {
            return true;
        }
        let mut shifted = self.entries.clone();
        for entry in &mut shifted {
            if entry.position >= start {
                let next = if delta.is_negative() {
                    entry.position.checked_sub(delta.unsigned_abs())
                } else {
                    entry.position.checked_add(delta as u64)
                };
                let Some(next) = next else {
                    return false;
                };
                entry.position = next;
            }
        }
        shifted.sort_by_key(|a| a.position);
        self.entries = shifted;
        true
    }
    pub fn audit(&self) -> bool {
        self.entries.len() <= 65_536
            && self.entries.iter().all(ScoreAnnotation::validate)
            && self
                .entries
                .windows(2)
                .all(|w| w[0].position <= w[1].position)
    }
}

impl NotationSymbol {
    pub fn validate(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.x >= 0.0
            && self.y >= 0.0
            && self.val <= 0x10FFFF
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderPrimitive {
    pub primitive_type: u8, // 0: note head, 1: stem, 2: beam, 3: slur
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub cx: f32,
    pub cy: f32,
    pub glyph_id: u32,
}

pub struct NotationOrchestrator {
    pub symbols: Vec<NotationSymbol>,
    render_primitives: Vec<RenderPrimitive>,
}

impl Default for NotationOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl NotationOrchestrator {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            render_primitives: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.symbols.clear();
        self.render_primitives.clear();
    }

    pub fn add_note(&mut self, pitch: i32, start: u64, duration: u64) {
        if !(0..=127).contains(&pitch) || duration == 0 || self.symbols.len() >= 1_000_000 {
            return;
        }
        self.symbols.push(NotationSymbol {
            symbol_type: NotationType::Note,
            val: pitch as u32,
            x: 0.0,
            y: 0.0,
            is_visible: true,
            start,
            duration,
        });
    }

    pub fn add_symbol(&mut self, symbol: NotationSymbol) -> bool {
        if !symbol.validate() || self.symbols.len() >= 1_000_000 {
            return false;
        }
        self.symbols.push(symbol);
        true
    }

    pub fn remove_symbol(&mut self, index: usize) -> bool {
        if index >= self.symbols.len() {
            return false;
        }
        self.symbols.remove(index);
        true
    }

    pub fn calculate_layout(&mut self, width: f32, height: f32) {
        self.render_primitives.clear();
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return;
        }

        self.symbols.sort_by_key(|symbol| symbol.start);
        let max_end = self
            .symbols
            .iter()
            .map(|symbol| symbol.start.saturating_add(symbol.duration))
            .max()
            .unwrap_or(1)
            .max(1);
        let usable_width = (width - 20.0).max(1.0);
        for symbol in &mut self.symbols {
            symbol.x = 10.0 + (symbol.start as f64 / max_end as f64) as f32 * usable_width;
            if symbol.symbol_type == NotationType::Note {
                symbol.y = (height * 0.5 - (symbol.val as f32 - 60.0) * 2.5)
                    .clamp(8.0, (height - 8.0).max(8.0));
            }
        }

        let mut last_x = -100_000.0f32;
        for symbol in &mut self.symbols {
            if !symbol.is_visible {
                continue;
            }
            if symbol.x - last_x < 10.0 {
                symbol.x = last_x + 10.0;
            }
            last_x = symbol.x;
        }

        for pair in self.symbols.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if !previous.is_visible
                || !current.is_visible
                || previous.symbol_type != NotationType::Note
                || current.symbol_type != NotationType::Note
            {
                continue;
            }
            if previous.duration <= 240 && current.duration <= 240 && current.x - previous.x < 100.0
            {
                let stem_y = previous.y.min(current.y) - 28.0;
                self.render_primitives.push(line_primitive(
                    2,
                    previous.x + 3.0,
                    stem_y,
                    current.x + 3.0,
                    stem_y,
                ));
            }
        }

        for pair in self.symbols.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if previous.symbol_type == NotationType::Note
                && current.symbol_type == NotationType::Note
                && previous.is_visible
                && current.is_visible
                && current.start >= previous.start.saturating_add(previous.duration)
            {
                let y = previous.y.min(current.y) - 36.0;
                self.render_primitives
                    .push(line_primitive(3, previous.x, y, current.x, y));
            }
        }

        for symbol in &self.symbols {
            if !symbol.is_visible || symbol.symbol_type != NotationType::Note {
                continue;
            }
            self.render_primitives.push(line_primitive(
                0,
                symbol.x - 4.0,
                symbol.y,
                symbol.x + 4.0,
                symbol.y,
            ));
            self.render_primitives.push(line_primitive(
                1,
                symbol.x + 3.0,
                symbol.y,
                symbol.x + 3.0,
                symbol.y - if symbol.val >= 60 { 28.0 } else { -28.0 },
            ));
        }
    }

    pub fn render_primitives(&self) -> &[RenderPrimitive] {
        &self.render_primitives
    }

    /// Returns the most recently calculated renderer-agnostic layout.
    pub fn generate_render_primitives(&self) -> Vec<RenderPrimitive> {
        self.render_primitives.clone()
    }

    pub fn audit_notation(&self) -> bool {
        self.symbols.len() <= 1_000_000 && self.symbols.iter().all(NotationSymbol::validate)
    }
}

fn line_primitive(kind: u8, x1: f32, y1: f32, x2: f32, y2: f32) -> RenderPrimitive {
    RenderPrimitive {
        primitive_type: kind,
        x1,
        y1,
        x2,
        y2,
        cx: 0.0,
        cy: 0.0,
        glyph_id: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_manifest_clips_valid_notes_and_preserves_source_identity() {
        let region_start = [4.0, 4.0, 4.0, 4.0];
        let region_length = [4.0; 4];
        let region_id = [7; 4];
        let note_start = [0.0, 1.0, 3.5, f64::NAN];
        let note_length = [1.0, 0.5, 1.0, 1.0];
        let pitches = [60, 64, 67, 70];
        let velocities = [100, 110, 90, 100];
        let note_indices = [0, 1, 2, 3];
        let mut output = [HirariMidiScoreGlyph::default(); 4];
        let count = unsafe {
            hirari_notation_midi_manifest(
                region_start.as_ptr(),
                region_length.as_ptr(),
                region_id.as_ptr(),
                note_start.as_ptr(),
                note_length.as_ptr(),
                pitches.as_ptr(),
                velocities.as_ptr(),
                note_indices.as_ptr(),
                4,
                output.as_mut_ptr(),
                output.len(),
            )
        };
        assert_eq!(count, 3);
        assert_eq!(
            output[0],
            HirariMidiScoreGlyph {
                beat: 4.0,
                staff_offset: 60.0,
                duration: 4,
                source_region_id: 7,
                source_note_index: 0,
            }
        );
        assert_eq!(output[1].beat, 5.0);
        assert_eq!(output[1].duration, 8);
        assert_eq!(output[2].beat, 7.5);
        assert_eq!(output[2].duration, 8);
        assert_eq!(output[2].source_note_index, 2);
    }

    #[test]
    fn symbol_lifecycle_rejects_invalid_values() {
        let mut score = NotationOrchestrator::new();
        let symbol = || NotationSymbol {
            symbol_type: NotationType::Note,
            val: 60,
            x: 0.0,
            y: 0.0,
            is_visible: true,
            start: 0,
            duration: 1,
        };
        assert!(score.add_symbol(symbol()));
        assert!(!score.add_symbol(NotationSymbol {
            val: 0x11_0000,
            ..symbol()
        }));
        assert!(score.remove_symbol(0));
        assert!(!score.remove_symbol(0));
    }
}
use std::slice;

/// Flat score glyph produced from a MIDI note and its owning region.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HirariMidiScoreGlyph {
    pub beat: f32,
    pub staff_offset: f32,
    pub duration: i32,
    pub source_region_id: u32,
    pub source_note_index: u32,
}

/// Converts audio regions into notation timeline anchors. The native Track
/// remains the owner of region objects; beat conversion, validation, and
/// duration encoding are handled by Rust alongside MIDI glyph generation.
#[no_mangle]
pub unsafe extern "C" fn hirari_notation_audio_manifest(
    starts: *const u64,
    lengths: *const u64,
    muted: *const u8,
    sample_rates: *const f64,
    count: usize,
    output: *mut HirariMidiScoreGlyph,
    output_capacity: usize,
) -> usize {
    if count == 0
        || output.is_null()
        || output_capacity < count
        || starts.is_null()
        || lengths.is_null()
        || muted.is_null()
        || sample_rates.is_null()
    {
        return 0;
    }
    let starts = unsafe { slice::from_raw_parts(starts, count) };
    let lengths = unsafe { slice::from_raw_parts(lengths, count) };
    let muted = unsafe { slice::from_raw_parts(muted, count) };
    let sample_rates = unsafe { slice::from_raw_parts(sample_rates, count) };
    let glyphs = unsafe { slice::from_raw_parts_mut(output, output_capacity) };
    let mut written = 0;
    for index in 0..count {
        if muted[index] != 0
            || lengths[index] == 0
            || !sample_rates[index].is_finite()
            || sample_rates[index] <= 0.0
        {
            continue;
        }
        let beat = starts[index] as f64 / sample_rates[index] * 2.0;
        let duration_beats = (lengths[index] as f64 / sample_rates[index] * 2.0).max(1.0 / 16.0);
        if !beat.is_finite() || !duration_beats.is_finite() {
            continue;
        }
        glyphs[written] = HirariMidiScoreGlyph {
            beat: beat.max(0.0) as f32,
            staff_offset: 0.0,
            duration: (1.0 / duration_beats).round().clamp(1.0, i32::MAX as f64) as i32,
            source_region_id: 0,
            source_note_index: 0,
        };
        written += 1;
    }
    written
}

/// Converts a flattened MIDI-region snapshot into ordered score glyphs.
/// The caller provides one region descriptor for each note so region clipping,
/// validation, duration quantization, and ordering are owned by Rust.
#[no_mangle]
pub unsafe extern "C" fn hirari_notation_midi_manifest(
    region_starts: *const f64,
    region_lengths: *const f64,
    region_ids: *const u32,
    note_starts: *const f64,
    note_lengths: *const f64,
    pitches: *const u8,
    velocities: *const u8,
    note_indices: *const u32,
    count: usize,
    output: *mut HirariMidiScoreGlyph,
    output_capacity: usize,
) -> usize {
    if count == 0
        || output_capacity < count
        || output.is_null()
        || region_starts.is_null()
        || region_lengths.is_null()
        || region_ids.is_null()
        || note_starts.is_null()
        || note_lengths.is_null()
        || pitches.is_null()
        || velocities.is_null()
        || note_indices.is_null()
    {
        return 0;
    }
    let region_starts = slice::from_raw_parts(region_starts, count);
    let region_lengths = slice::from_raw_parts(region_lengths, count);
    let region_ids = slice::from_raw_parts(region_ids, count);
    let note_starts = slice::from_raw_parts(note_starts, count);
    let note_lengths = slice::from_raw_parts(note_lengths, count);
    let pitches = slice::from_raw_parts(pitches, count);
    let velocities = slice::from_raw_parts(velocities, count);
    let note_indices = slice::from_raw_parts(note_indices, count);

    let mut glyphs = Vec::with_capacity(count);
    for index in 0..count {
        let region_start = region_starts[index];
        let region_length = region_lengths[index];
        let note_start = note_starts[index];
        let note_length = note_lengths[index];
        if !region_start.is_finite()
            || !region_length.is_finite()
            || region_length <= 0.0
            || velocities[index] == 0
            || pitches[index] > 127
            || !note_start.is_finite()
            || !note_length.is_finite()
            || note_start < 0.0
            || note_length <= 0.0
        {
            continue;
        }
        let region_end = region_start + region_length;
        let beat = region_start + note_start;
        let end = beat + note_length;
        if !region_end.is_finite()
            || !beat.is_finite()
            || !end.is_finite()
            || end <= beat
            || beat >= region_end
        {
            continue;
        }
        let duration_beats = note_length.min(region_end - beat);
        let denominator = (4.0 / duration_beats).round();
        let duration = if denominator.is_finite() {
            denominator.clamp(1.0, i32::MAX as f64) as i32
        } else {
            i32::MAX
        };
        glyphs.push(HirariMidiScoreGlyph {
            beat: beat.max(0.0) as f32,
            staff_offset: pitches[index] as f32,
            duration,
            source_region_id: region_ids[index],
            source_note_index: note_indices[index],
        });
    }
    glyphs.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let length = glyphs.len();
    slice::from_raw_parts_mut(output, output_capacity)[..length].copy_from_slice(&glyphs);
    length
}
