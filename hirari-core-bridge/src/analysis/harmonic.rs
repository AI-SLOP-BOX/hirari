use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScaleType {
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Pentatonic,
    Lydian,
    Mixolydian,
}

pub struct Scale {
    pub name: String,
    pub intervals: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChordEvent {
    pub tick: u64,
    pub root: u8,
    pub intervals: Vec<u8>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordQuality {
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Dominant7,
    Major7,
    Minor7,
}

impl ChordQuality {
    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProChordEvent {
    pub tick: u64,
    pub root: u8,
    pub quality: ChordQuality,
    /// Additional intervals from the root, for example 14 for a ninth.
    pub tensions: Vec<u8>,
    /// Pitch class for slash chords; `None` links bass to the root.
    pub bass: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordMidiNote {
    pub tick: u64,
    pub duration: u64,
    pub pitch: u8,
    pub velocity: u8,
}

include!("harmonic_orchestrator.rs");

mod harmonic_tests;

#[path = "harmonic_pads.rs"]
mod harmonic_pads;
pub use harmonic_pads::*;

