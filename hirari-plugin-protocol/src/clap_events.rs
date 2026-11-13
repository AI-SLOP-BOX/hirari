//! Allocation-bounded CLAP input event projection from the shared mailbox.
//!
//! The plugin still receives official CLAP C ABI callbacks, but event ordering,
//! parameter projection, MIDI 1.0/2.0 conversion and SysEx views live in Rust.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::boxed::Box;
use std::vec::Vec;

const MIDI_CAPACITY: usize = 1024;
const PARAMETER_CAPACITY: usize = 4096;
const EXTENDED_CAPACITY: usize = 4;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_FRAMES: usize = 8192;
const MAX_CHANNELS: usize = 2;
const CORE_EVENT_SPACE: u16 = 0;
const EVENT_PARAM_VALUE: u16 = 5;
const EVENT_MIDI: u16 = 2;
const EVENT_MIDI_SYSEX: u16 = 6;
const EVENT_MIDI2: u16 = 10;
const OUTPUT_MIDI_CAPACITY: usize = 1024;
const OUTPUT_PAYLOAD_CAPACITY: usize = 256;
const STATE_ERROR_OVERSIZE: u8 = 1;
const STATE_ERROR_PLUGIN: u8 = 4;
const STATE_ERROR_NONE: u8 = 0;
const STATE_MODE_SAVE: u8 = 1;
const STATE_MODE_LOAD: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct MailboxMidiEvent {
    pub(crate) sample_offset: u64,
    pub(crate) size: u32,
    pub(crate) data: [u8; 256],
    pub(crate) articulation_id: u8,
    pub(crate) _padding: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MailboxParameterChange {
    pub(crate) parameter_id: u32,
    pub(crate) sample_offset: u32,
    pub(crate) value: f64,
}

#[repr(C)]
struct MailboxOutputMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; OUTPUT_PAYLOAD_CAPACITY],
    articulation_id: u8,
    _padding: [u8; 3],
}

#[repr(C)]
#[derive(Clone)]
struct ExtendedMessage {
    sample_offset: u64,
    articulation_id: u8,
    _padding: [u8; 3],
    size: u32,
    data: [u8; MAX_MESSAGE_BYTES],
}

impl Default for ExtendedMessage {
    fn default() -> Self {
        Self {
            sample_offset: 0,
            articulation_id: 0,
            _padding: [0; 3],
            size: 0,
            data: [0; MAX_MESSAGE_BYTES],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct EventHeader {
    size: u32,
    time: u32,
    space_id: u16,
    event_type: u16,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventMidi {
    header: EventHeader,
    port_index: u16,
    data: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventMidi2 {
    header: EventHeader,
    port_index: u16,
    reserved: u16,
    data: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventMidiSysex {
    header: EventHeader,
    port_index: u16,
    _padding: [u8; 6],
    buffer: *const u8,
    size: u32,
    _tail_padding: u32,
}

impl Default for EventMidiSysex {
    fn default() -> Self {
        Self {
            header: EventHeader::default(),
            port_index: 0,
            _padding: [0; 6],
            buffer: ptr::null(),
            size: 0,
            _tail_padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventParamValue {
    header: EventHeader,
    parameter_id: u32,
    _padding: u32,
    cookie: *mut c_void,
    value: f64,
    note_id: i32,
    port_index: i16,
    channel: i16,
    key: i16,
    _tail_padding: i16,
}

impl Default for EventParamValue {
    fn default() -> Self {
        Self {
            header: EventHeader::default(),
            parameter_id: 0,
            _padding: 0,
            cookie: ptr::null_mut(),
            value: 0.0,
            note_id: -1,
            port_index: 0,
            channel: -1,
            key: -1,
            _tail_padding: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct EventReference {
    kind: u8,
    index: usize,
    sample_offset: u32,
}

#[repr(C)]
pub struct ClapInputEvents {
    context: *const c_void,
    _size: Option<unsafe extern "C" fn(*const ClapInputEvents) -> u32>,
    _get: Option<unsafe extern "C" fn(*const ClapInputEvents, u32) -> *const EventHeader>,
}

#[repr(C)]
pub struct ClapOutputEvents {
    context: *mut c_void,
    _try_push: Option<unsafe extern "C" fn(*const ClapOutputEvents, *const EventHeader) -> bool>,
}

#[repr(C)]
struct ClapAudioBuffer {
    data32: *mut *mut f32,
    data64: *mut *mut f64,
    channel_count: u32,
    latency: u32,
    constant_mask: u64,
}

#[repr(C)]
struct ClapProcess {
    steady_time: i64,
    frames_count: u32,
    transport: *const c_void,
    audio_inputs: *const ClapAudioBuffer,
    audio_inputs_count: u32,
    audio_outputs: *mut ClapAudioBuffer,
    audio_outputs_count: u32,
    in_events: *const ClapInputEvents,
    out_events: *mut ClapOutputEvents,
}

#[repr(C)]
struct ClapPluginPrefix {
    descriptor: *const c_void,
    plugin_data: *mut c_void,
    init: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    destroy: Option<unsafe extern "C" fn(*const c_void)>,
    activate: Option<unsafe extern "C" fn(*const c_void, f64, u32, u32) -> bool>,
    deactivate: Option<unsafe extern "C" fn(*const c_void)>,
    start_processing: Option<unsafe extern "C" fn(*const c_void) -> bool>,
    stop_processing: Option<unsafe extern "C" fn(*const c_void)>,
    reset: Option<unsafe extern "C" fn(*const c_void)>,
    process: Option<unsafe extern "C" fn(*const c_void, *const ClapProcess) -> i32>,
}

#[repr(C)]
struct ClapStateExtension {
    save: Option<unsafe extern "C" fn(*const c_void, *const ClapOStream) -> bool>,
    load: Option<unsafe extern "C" fn(*const c_void, *const ClapIStream) -> bool>,
}

#[repr(C)]
pub struct ClapOStream {
    context: *mut c_void,
    write: Option<unsafe extern "C" fn(*const ClapOStream, *const c_void, u64) -> i64>,
}

#[repr(C)]
pub struct ClapIStream {
    context: *mut c_void,
    read: Option<unsafe extern "C" fn(*const ClapIStream, *mut c_void, u64) -> i64>,
}

const _: () = assert!(core::mem::size_of::<ClapAudioBuffer>() == 32);
const _: () = assert!(core::mem::offset_of!(ClapAudioBuffer, constant_mask) == 24);
const _: () = assert!(core::mem::size_of::<ClapProcess>() == 72);
const _: () = assert!(core::mem::offset_of!(ClapProcess, in_events) == 56);
const _: () = assert!(core::mem::offset_of!(ClapPluginPrefix, process) == 72);
const _: () = assert!(core::mem::size_of::<ClapStateExtension>() == 16);
const _: () = assert!(core::mem::size_of::<ClapOStream>() == 16);
const _: () = assert!(core::mem::size_of::<ClapIStream>() == 16);

struct StateStreamContext {
    data: *mut u8,
    size: u32,
    cursor: u32,
    capacity: u32,
    writing: bool,
}

impl Default for StateStreamContext {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            size: 0,
            cursor: 0,
            capacity: 0,
            writing: false,
        }
    }
}

#[path = "clap_events_context.rs"]
mod context;
pub use context::*;
