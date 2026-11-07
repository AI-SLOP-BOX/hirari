//! Global tonality (root + scale) for the whole project.
//!
//! This module is the live implementation. It used to be a second, unwired
//! copy that shadowed `src/core/engine/tonal_sync.hpp`; the C++ class was the
//! one that actually answered `is_note_in_tonal_scale` and friends. The port
//! moved the state here and deleted the C++ class, so every mask and every
//! guard below is written to reproduce the C++ behavior *exactly*, including
//! its one quirk (see [`ScaleTypeRust::PentatonicMinor`]).
//!
//! Every constant here was cross-checked against the deleted C++ `patternFor`
//! by parsing both sources as bit lists rather than trusting a transcription:
//! C++ matched standard music theory on all twelve scales, so the live
//! behavior was already correct and preserving it is a faithful port, not a
//! fix.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl ScaleTypeRust {
    /// Maps the public `scale_type` index to a scale.
    ///
    /// The C++ bridge accepted `0..=10` and refused everything else, which
    /// left `PentatonicMinor` (index 11) declared but unreachable. Preserving
    /// that refusal is the whole point of the `Option`: callers keep seeing
    /// `false` for index 11 exactly as they did before the port.
    pub const fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::Major),
            1 => Some(Self::Minor),
            2 => Some(Self::HarmonicMinor),
            3 => Some(Self::MelodicMinor),
            4 => Some(Self::Dorian),
            5 => Some(Self::Phrygian),
            6 => Some(Self::Lydian),
            7 => Some(Self::Mixolydian),
            8 => Some(Self::Aeolian),
            9 => Some(Self::Locrian),
            10 => Some(Self::PentatonicMajor),
            _ => None,
        }
    }

    /// The twelve-bit pitch-class mask, bit `n` set when the `n`-th degree
    /// above the root belongs to the scale.
    ///
    /// `PentatonicMinor` is present in the enum but, as documented on
    /// [`Self::from_index`], unreachable through the public index range; the
    /// value is kept so the mask table stays complete and auditable.
    pub const fn scale_mask(self) -> u16 {
        match self {
            Self::Major => 0xAB5,
            Self::Minor | Self::Aeolian => 0x5AD,
            Self::HarmonicMinor => 0x9AD,
            Self::MelodicMinor => 0xAAD,
            Self::Dorian => 0x6AD,
            Self::Phrygian => 0x5AB,
            Self::Lydian => 0xAD5,
            Self::Mixolydian => 0x6B5,
            Self::Locrian => 0x56B,
            Self::PentatonicMajor => 0x295,
            Self::PentatonicMinor => 0x4A9,
        }
    }
}

/// Project-wide tonality. Mirrors the deleted `Hirari::Core::Engine::TonalSync`.
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
    /// Matches the C++ constructor, which stored `setScale(0, Major)`.
    pub fn new() -> Self {
        Self {
            root_note: 0,
            scale_type: ScaleTypeRust::Major,
            version: 0,
        }
    }

    /// Sets the tonality, normalizing the root into `0..=11`.
    ///
    /// The C++ guard was `((root % 12) + 12) % 12`, which folds a negative
    /// root up instead of leaving it negative. `rem_euclid` is the same
    /// operation; the previous `root % 12` here silently kept `-1` as `-1`,
    /// so a negative root would have shifted every degree test off by one.
    pub fn set_scale(&mut self, root: i32, scale_type: ScaleTypeRust) {
        self.root_note = root.rem_euclid(12);
        self.scale_type = scale_type;
        self.version += 1;
    }

    /// Whether `midi_note` is a member of the current scale.
    ///
    /// Notes outside the MIDI range are rejected before the degree is
    /// computed, matching the C++ guard `if (midiNote < 0 || midiNote > 127)
    /// return false;`. Without it, `quantize_note` below would happily
    /// report a scaled note outside the playable range.
    pub fn is_note_in_scale(&self, midi_note: i32) -> bool {
        if !(0..=127).contains(&midi_note) {
            return false;
        }
        let relative = (midi_note % 12 - self.root_note + 12) % 12;
        (self.scale_type.scale_mask() & (1u16 << relative)) != 0
    }

    /// Nearest note inside the scale, searching upward first.
    pub fn quantize_note(&self, midi_note: i32) -> i32 {
        if self.is_note_in_scale(midi_note) {
            return midi_note;
        }
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
        (0..12).contains(&self.root_note)
    }
}

#[cfg(test)]
mod tests {
    use super::{ScaleTypeRust, TonalOrchestrator};

    const ALL: [(ScaleTypeRust, u16); 12] = [
        (ScaleTypeRust::Major, 0xAB5),
        (ScaleTypeRust::Minor, 0x5AD),
        (ScaleTypeRust::HarmonicMinor, 0x9AD),
        (ScaleTypeRust::MelodicMinor, 0xAAD),
        (ScaleTypeRust::Dorian, 0x6AD),
        (ScaleTypeRust::Phrygian, 0x5AB),
        (ScaleTypeRust::Lydian, 0xAD5),
        (ScaleTypeRust::Mixolydian, 0x6B5),
        (ScaleTypeRust::Aeolian, 0x5AD),
        (ScaleTypeRust::Locrian, 0x56B),
        (ScaleTypeRust::PentatonicMajor, 0x295),
        (ScaleTypeRust::PentatonicMinor, 0x4A9),
    ];

    /// Each mask is restated as an explicit degree list so the value cannot
    /// drift from its meaning; the hex constant is then derived and compared.
    #[test]
    fn every_scale_mask_matches_the_degree_list_it_represents() {
        for (scale, expected) in ALL {
            let degrees: Vec<u8> = match scale {
                ScaleTypeRust::Major => vec![0, 2, 4, 5, 7, 9, 11],
                ScaleTypeRust::Minor | ScaleTypeRust::Aeolian => vec![0, 2, 3, 5, 7, 8, 10],
                ScaleTypeRust::HarmonicMinor => vec![0, 2, 3, 5, 7, 8, 11],
                ScaleTypeRust::MelodicMinor => vec![0, 2, 3, 5, 7, 9, 11],
                ScaleTypeRust::Dorian => vec![0, 2, 3, 5, 7, 9, 10],
                ScaleTypeRust::Phrygian => vec![0, 1, 3, 5, 7, 8, 10],
                ScaleTypeRust::Lydian => vec![0, 2, 4, 6, 7, 9, 11],
                ScaleTypeRust::Mixolydian => vec![0, 2, 4, 5, 7, 9, 10],
                ScaleTypeRust::Locrian => vec![0, 1, 3, 5, 6, 8, 10],
                ScaleTypeRust::PentatonicMajor => vec![0, 2, 4, 7, 9],
                ScaleTypeRust::PentatonicMinor => vec![0, 3, 5, 7, 10],
            };
            let derived = degrees.iter().fold(0u16, |acc, &d| acc | (1u16 << d));
            assert_eq!(derived, expected, "{scale:?} mask disagrees with its degrees");
            assert_eq!(scale.scale_mask(), expected);
        }
    }

    #[test]
    fn new_starts_on_c_major_and_rejects_the_leading_tone() {
        let tonal = TonalOrchestrator::new();
        assert_eq!(tonal.root_note, 0);
        assert_eq!(tonal.scale_type, ScaleTypeRust::Major);
        assert!(tonal.is_note_in_scale(0)); // C
        assert!(!tonal.is_note_in_scale(1)); // C#
        assert!(tonal.is_note_in_scale(4)); // E
        assert!(!tonal.is_note_in_scale(6)); // F#
    }

    #[test]
    fn the_public_index_range_stops_at_pentatonic_major() {
        for index in 0..=10 {
            assert!(
                ScaleTypeRust::from_index(index).is_some(),
                "index {index} was accepted by the C++ bridge and must stay accepted"
            );
        }
        for index in 11..=32 {
            assert!(
                ScaleTypeRust::from_index(index).is_none(),
                "index {index} was refused by the C++ bridge and must stay refused"
            );
        }
        assert_eq!(
            ScaleTypeRust::from_index(10),
            Some(ScaleTypeRust::PentatonicMajor)
        );
    }

    #[test]
    fn a_negative_root_folds_up_instead_of_staying_negative() {
        let mut tonal = TonalOrchestrator::new();
        tonal.set_scale(-1, ScaleTypeRust::Major);
        assert_eq!(tonal.root_note, 11, "(-1 % 12 + 12) % 12 is 11");
        // Degree 0 above B is B itself; C# (1) is not in C major relative to B.
        assert!(tonal.is_note_in_scale(71)); // B4
        assert!(!tonal.is_note_in_scale(72)); // C5
        let mut tonal = TonalOrchestrator::new();
        tonal.set_scale(24, ScaleTypeRust::Major);
        assert_eq!(tonal.root_note, 0, "two octaves of root lands back on C");
    }

    #[test]
    fn notes_outside_the_midi_range_are_refused() {
        let tonal = TonalOrchestrator::new();
        assert!(tonal.is_note_in_scale(0));
        assert!(tonal.is_note_in_scale(127));
        assert!(!tonal.is_note_in_scale(-1), "below 0 is not a MIDI note");
        assert!(!tonal.is_note_in_scale(128), "above 127 is not a MIDI note");
        assert!(!tonal.is_note_in_scale(i32::MIN));
        assert!(!tonal.is_note_in_scale(i32::MAX));
    }

    #[test]
    fn setting_a_scale_bumps_the_version_and_audits_the_root() {
        let mut tonal = TonalOrchestrator::new();
        let before = tonal.version;
        tonal.set_scale(3, ScaleTypeRust::Dorian);
        assert_eq!(tonal.version, before + 1);
        assert!(tonal.audit_tonal_sync());
        tonal.root_note = -1;
        assert!(!tonal.audit_tonal_sync());
    }

    #[test]
    fn quantization_prefers_the_upper_neighbor_and_stays_in_range() {
        let tonal = TonalOrchestrator::new(); // C major
        assert_eq!(tonal.quantize_note(0), 0);
        assert_eq!(tonal.quantize_note(1), 2, "C# resolves up to D");
        assert_eq!(tonal.quantize_note(6), 7, "F# resolves up to G");
        assert_eq!(tonal.quantize_note(127), 127, "B is already in C major");
    }
}
