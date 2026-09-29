//! Compatibility module for the original step-sequencer path.
//!
//! The implementation lives in `sequencer.rs`; keeping a second pattern
//! engine here previously left a public no-op `process_sequencer` API that
//! could silently report success without producing MIDI. Re-export the real
//! deterministic implementation so both module paths behave identically.

pub use crate::sequencer::{SequencerMidiEvent, SequencerOrchestrator, StepSequencerLane};
