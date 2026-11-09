//! The shared-memory mailbox between the engine process and the plugin worker.
//!
//! Both sides map the same `MAP_SHARED` object, so exactly one definition may
//! describe it: this module. The worker addresses its fields directly and the
//! host publishes requests into them, while C++ keeps only the numeric bounds
//! it validates before crossing into Rust.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8};

pub const MAX_FRAMES: usize = 8192;
pub const MAX_CHANNELS: usize = 2;
pub const MAX_MIDI_EVENTS: usize = 1024;
pub const MAX_PARAMETER_CHANGES: usize = 4096;
pub const MAX_MIDI_PAYLOAD_BYTES: usize = 256;
pub const MAX_STATE_BYTES: usize = crate::MAX_STATE_BYTES;
/// Bounded ring for completed SysEx/MIDI 2.0 messages that cannot fit in the
/// legacy 256-byte event slot.
pub const EXTENDED_RING_BYTES: usize = crate::RING_STORAGE_BYTES;
pub const AUDIO_BLOCK_PROTOCOL_VERSION: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WireMidiEvent {
    pub sample_offset: u64,
    pub size: u32,
    pub data: [u8; MAX_MIDI_PAYLOAD_BYTES],
    pub articulation_id: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WireParameterChange {
    pub parameter_id: u32,
    pub sample_offset: u32,
    pub value: f64,
}

#[repr(C, align(64))]
pub struct ExtendedRingStorage {
    pub bytes: [u8; EXTENDED_RING_BYTES],
}

#[repr(C, align(64))]
pub struct StereoBlock {
    pub samples: [[f32; MAX_FRAMES]; MAX_CHANNELS],
}

#[repr(C)]
pub struct SharedAudioBlock {
    pub protocol_version: AtomicU32,
    pub request_sequence: AtomicU64,
    pub completed_sequence: AtomicU64,
    pub heartbeat: AtomicU64,
    pub frames: AtomicU32,
    pub channels: AtomicU32,
    pub active_sample_rate: AtomicU32,
    pub active_channels: AtomicU32,
    pub midi_events: AtomicU32,
    pub parameter_changes: AtomicU32,
    pub output_midi_events: AtomicU32,
    pub output_midi_dropped: AtomicU32,
    pub mailbox_overruns: AtomicU32,
    pub input_midi_truncations: AtomicU32,
    pub process_errors: AtomicU32,
    pub midi_instrument: AtomicU8,
    pub state_request_sequence: AtomicU64,
    pub state_completed_sequence: AtomicU64,
    pub state_project_generation: AtomicU64,
    pub state_plugin_generation: AtomicU64,
    pub state_audio_generation: AtomicU64,
    pub state_generation: AtomicU64,
    pub state_size: AtomicU32,
    pub state_checksum: AtomicU64,
    pub state_version: AtomicU32,
    pub state_mode: AtomicU8,
    pub state_result: AtomicU8,
    pub state_error: AtomicU8,
    pub extended_midi: ExtendedRingStorage,
    pub input: StereoBlock,
    pub output: StereoBlock,
    pub midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    pub parameter_change: [WireParameterChange; MAX_PARAMETER_CHANGES],
    pub output_midi: [WireMidiEvent; MAX_MIDI_EVENTS],
    pub state: [u8; MAX_STATE_BYTES],
    /// Processor latency in samples, as reported by the format adapter at
    /// load time. The host reads it once after READY and feeds it into PDC.
    pub plugin_latency_samples: AtomicU32,
    /// Sidechain input for the current block: stereo source audio published
    /// by the engine gate for this (track, plugin) link. Zero frames means
    /// no link; adapters feed it into their sidechain bus when present.
    pub sidechain_left: [f32; MAX_FRAMES],
    pub sidechain_right: [f32; MAX_FRAMES],
    pub sidechain_frames: AtomicU32,
}

// The host allocates exactly this many bytes for the mailbox, so a layout
// change here must be made together with the request/response framing.
const _: () = assert!(core::mem::size_of::<SharedAudioBlock>() == 9_208_256);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, request_sequence) == 8);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, heartbeat) == 24);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, frames) == 32);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, process_errors) == 72);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, midi_instrument) == 76);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, state_request_sequence) == 80);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, state_size) == 128);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, state_checksum) == 136);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, state_error) == 150);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, extended_midi) == 192);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, input) == 4_194_688);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, output) == 4_260_224);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, midi) == 4_325_760);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, parameter_change) == 4_604_288);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, output_midi) == 4_669_824);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, state) == 4_948_352);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, plugin_latency_samples) == 9_142_656);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, sidechain_left) == 9_142_660);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, sidechain_right) == 9_175_428);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, sidechain_frames) == 9_208_196);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, input) % 64 == 0);
const _: () = assert!(core::mem::offset_of!(SharedAudioBlock, extended_midi) % 64 == 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_event_slot_holds_the_legacy_payload_bound() {
        assert_eq!(core::mem::size_of::<WireMidiEvent>(), 272);
        assert_eq!(core::mem::offset_of!(WireMidiEvent, data), 12);
        assert_eq!(core::mem::offset_of!(WireMidiEvent, articulation_id), 268);
        assert_eq!(core::mem::size_of::<WireParameterChange>(), 16);
    }
}
