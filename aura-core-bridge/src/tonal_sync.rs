#[derive(Debug, Clone, Copy)]
pub enum ScaleTypeRust {
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian,
    Locrian,
    PentatonicMajor,
    PentatonicMinor,
}
pub struct TonalOrchestrator {
    pub root_note: i32,
    pub scale_type: ScaleTypeRust,
    pub version: u32,
}

impl Default for TonalOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TonalOrchestrator {
    pub fn new() -> Self {
        Self {
            root_note: 0,
            scale_type: ScaleTypeRust::Major,
            version: 0,
        }
    }

    fn get_scale_mask(st: ScaleTypeRust) -> u16 {
        match st {
            ScaleTypeRust::Major => 0xAB5,         // 101010110101
            ScaleTypeRust::Minor => 0x5AD,         // 010110101101 (Natural)
            ScaleTypeRust::HarmonicMinor => 0x9AD, // 100110101101
            ScaleTypeRust::MelodicMinor => 0xAB1,  // 101010110001
            ScaleTypeRust::Dorian => 0x6AD,        // 011010101101
            ScaleTypeRust::Phrygian => 0x56D,      // 010101101101
            ScaleTypeRust::Lydian => 0xAF5,        // 101011110101
            ScaleTypeRust::Mixolydian => 0x6B5,    // 011010110101
            ScaleTypeRust::Aeolian => 0x5AD,
            ScaleTypeRust::Locrian => 0x56B,         // 010101101011
            ScaleTypeRust::PentatonicMajor => 0x2A5, // 001010100101
            ScaleTypeRust::PentatonicMinor => 0x48D, // 010010001101
        }
    }

    pub fn set_scale(&mut self, root: i32, scale_type: ScaleTypeRust) {
        self.root_note = root % 12;
        self.scale_type = scale_type;
        self.version += 1;
    }

    pub fn is_note_in_scale(&self, midi_note: i32) -> bool {
        let relative_note = (midi_note - self.root_note).rem_euid(12) as u32;
        let mask = Self::get_scale_mask(self.scale_type);
        (mask & (1 << relative_note)) != 0
    }

    pub fn quantize_note(&self, midi_note: i32) -> i32 {
        if self.is_note_in_scale(midi_note) {
            return midi_note;
        }

        // Find the nearest note in scale (checking up/down)
        for i in 1..6 {
            if self.is_note_in_scale(midi_note + i) {
                return midi_note + i;
            }
            if self.is_note_in_scale(midi_note - i) {
                return midi_note - i;
            }
        }
        midi_note
    }

    pub fn audit_tonal_sync(&self) -> bool {
        self.root_note >= 0 && self.root_note < 12
    }
}

// Helper trait for Euclidian modulo (available in newer Rust, polyfill for older)
trait RemEuclidian {
    fn rem_euid(self, rhs: Self) -> Self;
}
impl RemEuclidian for i32 {
    fn rem_euid(self, rhs: i32) -> i32 {
        let r = self % rhs;
        if r < 0 {
            r + rhs.abs()
        } else {
            r
        }
    }
}
