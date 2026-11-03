//! Minimal CLAP plugin that the sandbox worker's end-to-end tests load.
//!
//! The isolated worker is the only consumer, so the declarations below mirror
//! `hirari-plugin-protocol`'s `clap_loader` and `clap_events` ABI field for
//! field; the layout assertions are the contract check that replaces the C++
//! header the fixture used to include. Three observable hooks let the tests
//! prove the mailbox paths: echoing input MIDI and MIDI 2.0 events, switching
//! gain when a long SysEx arrives, and environment-driven crash, block delay
//! and state-load delay for the restart, overrun and timeout policies.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::time::Duration;

const PROCESS_ERROR: i32 = 0;
const PROCESS_CONTINUE: i32 = 1;
const CORE_EVENT_SPACE: u16 = 0;
const EVENT_MIDI: u16 = 2;
const EVENT_SYSEX: u16 = 6;
const EVENT_MIDI2: u16 = 10;
// Compared against `CStr::to_bytes()` of the identifier the host passes in, so
// these carry no terminator.
const PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory";
const STATE_EXTENSION_ID: &[u8] = b"clap.state";
const DEFAULT_GAIN: f32 = 0.5;
const SYSEX_GAIN_TRIGGER_BYTES: u32 = 256;
const CRASH_STATUS: i32 = 86;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHeader {
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
struct EventSysex {
    header: EventHeader,
    port_index: u16,
    _padding: [u8; 6],
    buffer: *const u8,
    size: u32,
    _tail_padding: u32,
}

#[repr(C)]
struct InputEvents {
    context: *const c_void,
    size: Option<unsafe extern "C" fn(*const InputEvents) -> u32>,
    get: Option<unsafe extern "C" fn(*const InputEvents, u32) -> *const EventHeader>,
}

#[repr(C)]
struct OutputEvents {
    context: *mut c_void,
    try_push: Option<unsafe extern "C" fn(*const OutputEvents, *const EventHeader) -> bool>,
}

#[repr(C)]
struct AudioBuffer {
    data32: *mut *mut f32,
    data64: *mut *mut f64,
    channel_count: u32,
    latency: u32,
    constant_mask: u64,
}

#[repr(C)]
struct Process {
    steady_time: i64,
    frames_count: u32,
    transport: *const c_void,
    audio_inputs: *const AudioBuffer,
    audio_inputs_count: u32,
    audio_outputs: *mut AudioBuffer,
    audio_outputs_count: u32,
    in_events: *const InputEvents,
    out_events: *mut OutputEvents,
}

#[repr(C)]
struct OStream {
    context: *mut c_void,
    write: Option<unsafe extern "C" fn(*const OStream, *const c_void, u64) -> i64>,
}

#[repr(C)]
struct IStream {
    context: *mut c_void,
    read: Option<unsafe extern "C" fn(*const IStream, *mut c_void, u64) -> i64>,
}

#[repr(C)]
struct StateExtension {
    save: Option<unsafe extern "C" fn(*const Plugin, *const OStream) -> bool>,
    load: Option<unsafe extern "C" fn(*const Plugin, *const IStream) -> bool>,
}

#[repr(C)]
struct Version {
    major: u32,
    minor: u32,
    revision: u32,
}

#[repr(C)]
struct Descriptor {
    version: Version,
    id: *const c_char,
    name: *const c_char,
    vendor: *const c_char,
    url: *const c_char,
    manual_url: *const c_char,
    support_url: *const c_char,
    version_string: *const c_char,
    features: *const *const c_char,
}

// SAFETY: every pointer in a Descriptor instance addresses a process-lifetime
// static string, so sharing the value across threads copies nothing mutable.
unsafe impl Sync for Descriptor {}

#[repr(C)]
struct Plugin {
    descriptor: *const Descriptor,
    plugin_data: *mut c_void,
    init: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    destroy: Option<unsafe extern "C" fn(*const Plugin)>,
    activate: Option<unsafe extern "C" fn(*const Plugin, f64, u32, u32) -> bool>,
    deactivate: Option<unsafe extern "C" fn(*const Plugin)>,
    start_processing: Option<unsafe extern "C" fn(*const Plugin) -> bool>,
    stop_processing: Option<unsafe extern "C" fn(*const Plugin)>,
    reset: Option<unsafe extern "C" fn(*const Plugin)>,
    process: Option<unsafe extern "C" fn(*const Plugin, *const Process) -> i32>,
    get_extension: Option<unsafe extern "C" fn(*const Plugin, *const c_char) -> *const c_void>,
    on_main_thread: Option<unsafe extern "C" fn(*const Plugin)>,
}

#[repr(C)]
struct Factory {
    get_plugin_count: Option<unsafe extern "C" fn(*const Factory) -> u32>,
    get_plugin_descriptor: Option<unsafe extern "C" fn(*const Factory, u32) -> *const Descriptor>,
    create_plugin:
        Option<unsafe extern "C" fn(*const Factory, *const c_void, *const c_char) -> *const Plugin>,
}

#[repr(C)]
struct Entry {
    version: Version,
    init: Option<unsafe extern "C" fn(*const c_char) -> bool>,
    deinit: Option<unsafe extern "C" fn()>,
    get_factory: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

// The worker reads the mailbox-shaped CLAP structs through these offsets; if an
// assertion here fires, the fixture no longer describes what the worker calls.
const _: () = assert!(core::mem::size_of::<EventHeader>() == 16);
const _: () = assert!(core::mem::size_of::<EventMidi>() == 24);
const _: () = assert!(core::mem::size_of::<EventSysex>() == 40);
const _: () = assert!(core::mem::size_of::<AudioBuffer>() == 32);
const _: () = assert!(core::mem::size_of::<Process>() == 72);
const _: () = assert!(core::mem::offset_of!(Process, in_events) == 56);
const _: () = assert!(core::mem::offset_of!(Process, out_events) == 64);
const _: () = assert!(core::mem::size_of::<Descriptor>() == 80);
const _: () = assert!(core::mem::size_of::<Plugin>() == 96);
const _: () = assert!(core::mem::offset_of!(Plugin, process) == 72);
const _: () = assert!(core::mem::size_of::<Factory>() == 24);
const _: () = assert!(core::mem::size_of::<Entry>() == 40);
const _: () = assert!(core::mem::size_of::<StateExtension>() == 16);

/// One instance of the fixture. Every plugin instance gets its own value, which
/// is what the multi-instance isolation test measures.
struct Gain {
    value: f32,
    // Recorded by activate so a stale audio format is visible in a debugger;
    // the worker, not the fixture, owns the reconfiguration policy.
    #[allow(dead_code)]
    sample_rate: f64,
    #[allow(dead_code)]
    min_frames: u32,
    #[allow(dead_code)]
    max_frames: u32,
}

impl Default for Gain {
    fn default() -> Self {
        Self {
            value: DEFAULT_GAIN,
            sample_rate: 0.0,
            min_frames: 0,
            max_frames: 0,
        }
    }
}

static ID: [u8; 29] = *b"com.hirari.test.minimal-gain\0";
static NAME: [u8; 17] = *b"Hirari Test Gain\0";
static VENDOR: [u8; 13] = *b"Hirari Tests\0";
static EMPTY: [u8; 1] = *b"\0";
static DESCRIPTOR_VERSION: [u8; 4] = *b"1.0\0";
static DESCRIPTOR: Descriptor = Descriptor {
    version: Version {
        major: 1,
        minor: 0,
        revision: 0,
    },
    id: &raw const ID as *const c_char,
    name: &raw const NAME as *const c_char,
    vendor: &raw const VENDOR as *const c_char,
    url: &raw const EMPTY as *const c_char,
    manual_url: &raw const EMPTY as *const c_char,
    support_url: &raw const EMPTY as *const c_char,
    version_string: &raw const DESCRIPTOR_VERSION as *const c_char,
    features: ptr::null(),
};
static STATE_EXTENSION: StateExtension = StateExtension {
    save: Some(save_state),
    load: Some(load_state),
};
static FACTORY: Factory = Factory {
    get_plugin_count: Some(plugin_count),
    get_plugin_descriptor: Some(plugin_descriptor),
    create_plugin: Some(create_plugin),
};

fn environment(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The fixture hooks are set by the host test process and inherited by the
/// worker it forks, so they are read on every call rather than cached.
fn delayed(name: &str, max_millis: u64) {
    let Some(text) = environment(name) else {
        return;
    };
    if let Ok(millis) = text.parse::<u64>() {
        if millis > 0 && millis <= max_millis {
            std::thread::sleep(Duration::from_millis(millis));
        }
    }
}

fn state_of(plugin: *const Plugin) -> &'static mut Gain {
    unsafe { &mut *(*plugin).plugin_data.cast::<Gain>() }
}

unsafe extern "C" fn init(_: *const Plugin) -> bool {
    true
}

unsafe extern "C" fn destroy(plugin: *const Plugin) {
    if plugin.is_null() {
        return;
    }
    let state = unsafe { (*plugin).plugin_data.cast::<Gain>() };
    drop(unsafe { Box::from_raw(plugin.cast_mut()) });
    drop(unsafe { Box::from_raw(state) });
}

unsafe extern "C" fn activate(plugin: *const Plugin, sample_rate: f64, min: u32, max: u32) -> bool {
    let state = state_of(plugin);
    state.sample_rate = sample_rate;
    state.min_frames = min;
    state.max_frames = max;
    sample_rate > 0.0 && min > 0 && max >= min
}

unsafe extern "C" fn deactivate(_: *const Plugin) {}
unsafe extern "C" fn start_processing(_: *const Plugin) -> bool {
    true
}
unsafe extern "C" fn stop_processing(_: *const Plugin) {}
unsafe extern "C" fn reset(_: *const Plugin) {}

/// Event count and header lookup for the host-supplied input queue.
fn input_events(request: &Process) -> Option<(&InputEvents, u32)> {
    if request.in_events.is_null() {
        return None;
    }
    let events = unsafe { &*request.in_events };
    let Some(size) = events.size else {
        return None;
    };
    if events.get.is_none() {
        return None;
    }
    Some((events, unsafe { size(events) }))
}

fn header_of(events: &InputEvents, index: u32) -> Option<&'static EventHeader> {
    let get = events.get?;
    let header = unsafe { get(events, index) };
    if header.is_null() {
        return None;
    }
    Some(unsafe { &*header })
}

unsafe extern "C" fn process(plugin: *const Plugin, request: *const Process) -> i32 {
    if environment("HIRARI_CLAP_FIXTURE_CRASH").is_some_and(|value| value == "1") {
        std::process::exit(CRASH_STATUS);
    }
    delayed("HIRARI_CLAP_FIXTURE_DELAY_MS", 1000);
    if request.is_null() {
        return PROCESS_ERROR;
    }
    let request = unsafe { &*request };
    if request.audio_inputs.is_null()
        || request.audio_outputs.is_null()
        || request.audio_inputs_count == 0
        || request.audio_outputs_count == 0
        || request.frames_count == 0
    {
        return PROCESS_ERROR;
    }
    let state = state_of(plugin);

    // A long SysEx message changes the gain for the blocks that follow, which
    // makes the bounded extended-event ring observable without a second
    // fixture-specific output ABI.
    if let Some((events, count)) = input_events(request) {
        for index in 0..count {
            let Some(header) = header_of(events, index) else {
                continue;
            };
            if header.event_type != EVENT_SYSEX
                || header.size < core::mem::size_of::<EventSysex>() as u32
            {
                continue;
            }
            let sysex = unsafe { &*ptr::from_ref(header).cast::<EventSysex>() };
            if !sysex.buffer.is_null() && sysex.size > SYSEX_GAIN_TRIGGER_BYTES {
                state.value = 0.25;
            }
        }
    }

    let input = unsafe { &*request.audio_inputs };
    let output = unsafe { &mut *request.audio_outputs };
    if input.data32.is_null()
        || output.data32.is_null()
        || input.channel_count == 0
        || output.channel_count == 0
    {
        return PROCESS_ERROR;
    }
    let channels = input.channel_count.min(output.channel_count);
    for channel in 0..channels {
        let source = unsafe { *input.data32.add(channel as usize) };
        let destination = unsafe { *output.data32.add(channel as usize) };
        if source.is_null() || destination.is_null() {
            return PROCESS_ERROR;
        }
        let samples = unsafe {
            std::slice::from_raw_parts(source, request.frames_count as usize)
                .iter()
                .zip(std::slice::from_raw_parts_mut(
                    destination,
                    request.frames_count as usize,
                ))
        };
        for (sample, out) in samples {
            *out = *sample * state.value;
        }
    }

    // Echo MIDI and MIDI 2.0 events so the tests prove that an event crosses
    // the mailbox in both directions through a real plugin instance.
    let output_events = unsafe { request.out_events.as_ref() };
    if let (Some(output_events), Some((events, count))) = (output_events, input_events(request)) {
        if let Some(try_push) = output_events.try_push {
            for index in 0..count {
                let Some(header) = header_of(events, index) else {
                    continue;
                };
                if header.event_type == EVENT_MIDI
                    && header.size >= core::mem::size_of::<EventMidi>() as u32
                {
                    let incoming = unsafe { &*ptr::from_ref(header).cast::<EventMidi>() };
                    let echoed = EventMidi {
                        header: EventHeader {
                            size: core::mem::size_of::<EventMidi>() as u32,
                            time: header.time,
                            space_id: CORE_EVENT_SPACE,
                            event_type: EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: incoming.port_index,
                        data: incoming.data,
                    };
                    try_push(output_events, &echoed.header);
                } else if header.event_type == EVENT_MIDI2
                    && header.size >= core::mem::size_of::<EventMidi2>() as u32
                {
                    let incoming = unsafe { &*ptr::from_ref(header).cast::<EventMidi2>() };
                    let echoed = EventMidi2 {
                        header: EventHeader {
                            size: core::mem::size_of::<EventMidi2>() as u32,
                            time: header.time,
                            space_id: CORE_EVENT_SPACE,
                            event_type: EVENT_MIDI2,
                            flags: 0,
                        },
                        port_index: incoming.port_index,
                        reserved: 0,
                        data: incoming.data,
                    };
                    try_push(output_events, &echoed.header);
                }
            }
        }
    }
    PROCESS_CONTINUE
}

unsafe extern "C" fn save_state(plugin: *const Plugin, stream: *const OStream) -> bool {
    if stream.is_null() {
        return false;
    }
    let stream = unsafe { &*stream };
    let Some(write) = stream.write else {
        return false;
    };
    let bytes = state_of(plugin).value.to_le_bytes();
    write(stream, bytes.as_ptr().cast(), bytes.len() as u64) == bytes.len() as i64
}

unsafe extern "C" fn load_state(plugin: *const Plugin, stream: *const IStream) -> bool {
    delayed("HIRARI_CLAP_FIXTURE_STATE_DELAY_MS", 10_000);
    if stream.is_null() {
        return false;
    }
    let stream = unsafe { &*stream };
    let Some(read) = stream.read else {
        return false;
    };
    let mut bytes = [0_u8; 4];
    if read(stream, bytes.as_mut_ptr().cast(), bytes.len() as u64) != bytes.len() as i64 {
        return false;
    }
    let gain = f32::from_le_bytes(bytes);
    if !(0.0..=2.0).contains(&gain) {
        return false;
    }
    state_of(plugin).value = gain;
    true
}

unsafe extern "C" fn get_extension(
    _plugin: *const Plugin,
    identifier: *const c_char,
) -> *const c_void {
    if identifier.is_null() {
        return ptr::null();
    }
    let requested = unsafe { CStr::from_ptr(identifier) }.to_bytes();
    if requested == STATE_EXTENSION_ID {
        &raw const STATE_EXTENSION as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn plugin_count(_: *const Factory) -> u32 {
    1
}

unsafe extern "C" fn plugin_descriptor(_: *const Factory, index: u32) -> *const Descriptor {
    if index == 0 {
        &raw const DESCRIPTOR
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn create_plugin(
    _: *const Factory,
    _: *const c_void,
    _: *const c_char,
) -> *const Plugin {
    let state = Box::into_raw(Box::new(Gain::default()));
    let plugin = Box::new(Plugin {
        descriptor: &raw const DESCRIPTOR,
        plugin_data: state.cast(),
        init: Some(init),
        destroy: Some(destroy),
        activate: Some(activate),
        deactivate: Some(deactivate),
        start_processing: Some(start_processing),
        stop_processing: Some(stop_processing),
        reset: Some(reset),
        process: Some(process),
        get_extension: Some(get_extension),
        on_main_thread: None,
    });
    Box::into_raw(plugin)
}

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(identifier: *const c_char) -> *const c_void {
    if identifier.is_null() {
        return ptr::null();
    }
    if unsafe { CStr::from_ptr(identifier) }.to_bytes() == PLUGIN_FACTORY_ID {
        &raw const FACTORY as *const c_void
    } else {
        ptr::null()
    }
}

/// The single exported symbol the worker resolves with `dlsym`.
#[no_mangle]
static clap_entry: Entry = Entry {
    version: Version {
        major: 1,
        minor: 0,
        revision: 0,
    },
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};
