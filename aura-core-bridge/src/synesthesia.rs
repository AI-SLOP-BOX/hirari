/**
 * @struct SynesthesiaEngine
 * @brief Professional mapping engine for musical synesthesia.
 * INDUSTRIAL: Implements Scriabin's "Clavier à lumières" theory to translate 
 * tonal frequencies into a dynamic color palette.
 */
pub struct SynesthesiaEngine {}

impl SynesthesiaEngine {
    pub fn new() -> Self { Self {} }

    /**
     * @brief GET COLOR: Maps a MIDI note to an RGB color value.
     * INDUSTRIAL: Uses the Circle of Fifths color mapping for maximum harmonic coherence.
     */
    pub fn get_color_for_note(note: u8) -> [f32; 3] {
        let pc = note % 12;
        match pc {
            0 => [1.0, 0.1, 0.1], // C  - Red (Root)
            1 => [0.5, 0.1, 0.8], // C# - Deep Violet
            2 => [1.0, 1.0, 0.1], // D  - Yellow
            3 => [0.1, 0.1, 0.8], // D# - Sky Blue
            4 => [0.9, 0.9, 0.9], // E  - White (Purity)
            5 => [0.8, 0.1, 0.1], // F  - Dark Red
            6 => [0.1, 0.8, 0.8], // F# - Cyan
            7 => [0.1, 0.1, 1.0], // G  - Blue
            8 => [1.0, 0.5, 0.1], // G# - Orange
            9 => [0.1, 1.0, 0.1], // A  - Green
            10 => [1.0, 0.1, 1.0], // A# - Magenta
            11 => [0.1, 0.5, 0.1], // B  - Dark Green
            _ => [0.5, 0.5, 0.5],
        }
    }

    /**
     * @brief GET PALETTE: Returns a combined color for a set of notes (a chord).
     */
    pub fn get_palette_for_chord(notes: &[u8]) -> [f32; 3] {
        if notes.is_empty() { return [0.1, 0.1, 0.1]; }
        let mut r = 0.0;
        let mut g = 0.0;
        let mut b = 0.0;
        for &note in notes {
            let col = Self::get_color_for_note(note);
            r += col[0];
            g += col[1];
            b += col[2];
        }
        [r / notes.len() as f32, g / notes.len() as f32, b / notes.len() as f32]
    }
}
