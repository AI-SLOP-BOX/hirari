//! Rust-owned CoreMIDI device host.
//!
//! This is a direct port of the deleted `mac_midi_device_host.mm`: the same
//! CoreMIDI calls, the same bounded input queue and the same JSON surface.

use coreaudio_sys as sys;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_QUEUED_EVENTS: usize = 1024;
const MAX_PACKET_BYTES: usize = 256;

struct MidiInputEvent {
    source: i32,
    host_time: u64,
    data: Vec<u8>,
}

#[derive(Default)]
struct MidiInputState {
    client: sys::MIDIClientRef,
    port: sys::MIDIPortRef,
    queue: VecDeque<MidiInputEvent>,
}

static INPUT_STATE: Mutex<MidiInputState> = Mutex::new(MidiInputState {
    client: 0,
    port: 0,
    queue: VecDeque::new(),
});
static DROPPED_INPUT_EVENTS: AtomicU64 = AtomicU64::new(0);

fn lock_input() -> std::sync::MutexGuard<'static, MidiInputState> {
    INPUT_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn cfstring(value: &str) -> sys::CFStringRef {
    let Ok(value) = CString::new(value) else {
        return ptr::null();
    };
    sys::CFStringCreateWithCString(
        ptr::null(),
        value.as_ptr(),
        sys::kCFStringEncodingUTF8,
    )
}

unsafe fn cfstring_to_string(value: sys::CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 512];
    let converted = sys::CFStringGetCString(
        value,
        buffer.as_mut_ptr(),
        buffer.len() as sys::CFIndex,
        sys::kCFStringEncodingUTF8,
    );
    sys::CFRelease(value.cast::<c_void>());
    if converted == 0 {
        return None;
    }
    let text = CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

unsafe fn endpoint_name(endpoint: sys::MIDIEndpointRef) -> Option<String> {
    let mut value: sys::CFStringRef = ptr::null();
    if sys::MIDIObjectGetStringProperty(
        endpoint,
        sys::kMIDIPropertyDisplayName,
        &mut value,
    ) != 0
        || value.is_null()
    {
        return None;
    }
    cfstring_to_string(value)
}

unsafe fn endpoint_unique_id(endpoint: sys::MIDIEndpointRef) -> i32 {
    let mut value: sys::SInt32 = 0;
    let _ = sys::MIDIObjectGetIntegerProperty(
        endpoint,
        sys::kMIDIPropertyUniqueID,
        &mut value,
    );
    value
}

fn append_json_string(json: &mut String, value: &str) {
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                json.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => json.push(other),
        }
    }
    json.push('"');
}

unsafe extern "C" fn midi_read_proc(
    packet_list: *const sys::MIDIPacketList,
    _read_proc_ref: *mut c_void,
    source_connection_ref: *mut c_void,
) {
    if packet_list.is_null() {
        return;
    }
    let mut source = 0i32;
    if !source_connection_ref.is_null() {
        source = endpoint_unique_id(source_connection_ref as usize as sys::MIDIEndpointRef);
    }

    let mut state = lock_input();
    let list = &*packet_list;
    let mut packet = list.packet.as_ptr();
    for _ in 0..list.numPackets {
        let current = &*packet;
        if current.length > 0 && current.length as usize <= MAX_PACKET_BYTES {
            if state.queue.len() < MAX_QUEUED_EVENTS {
                state.queue.push_back(MidiInputEvent {
                    source,
                    host_time: current.timeStamp,
                    data: current.data[..current.length as usize].to_vec(),
                });
            } else {
                DROPPED_INPUT_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
        } else if current.length > 0 {
            DROPPED_INPUT_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        // CoreMIDI's MIDIPacketNext macro is offsetof(data) + length; the
        // bindgen struct exposes the same fixed 256-byte data field.
        let step = std::mem::offset_of!(sys::MIDIPacket, data) + current.length as usize;
        packet = (packet as *const u8).add(step).cast::<sys::MIDIPacket>();
    }
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_list_devices_json() -> *mut c_char {
    let mut json = String::from("[");
    let mut first = true;
    unsafe {
        for index in 0..sys::MIDIGetNumberOfSources() {
            let endpoint = sys::MIDIGetSource(index);
            let Some(name) = endpoint_name(endpoint) else {
                continue;
            };
            append_endpoint(&mut json, &mut first, endpoint, "input", index, &name);
        }
        for index in 0..sys::MIDIGetNumberOfDestinations() {
            let endpoint = sys::MIDIGetDestination(index);
            let Some(name) = endpoint_name(endpoint) else {
                continue;
            };
            append_endpoint(&mut json, &mut first, endpoint, "output", index, &name);
        }
    }
    json.push(']');
    into_c_string(json)
}

unsafe fn append_endpoint(
    json: &mut String,
    first: &mut bool,
    endpoint: sys::MIDIEndpointRef,
    direction: &str,
    index: sys::ItemCount,
    name: &str,
) {
    let unique_id = endpoint_unique_id(endpoint);
    if !*first {
        json.push(',');
    }
    *first = false;
    json.push_str(&format!(
        "{{\"id\":\"coremidi:{unique_id}:{direction}\",\"name\":"
    ));
    append_json_string(json, name);
    json.push_str(&format!(
        ",\"direction\":\"{direction}\",\"index\":{index}}}"
    ));
}

#[no_mangle]
pub unsafe extern "C" fn hirari_core_midi_send_message(
    unique_id: u32,
    data: *const u8,
    size: usize,
) -> bool {
    if data.is_null() || size == 0 || size > MAX_PACKET_BYTES {
        return false;
    }
    let mut destination: sys::MIDIEndpointRef = 0;
    for index in 0..sys::MIDIGetNumberOfDestinations() {
        let candidate = sys::MIDIGetDestination(index);
        if candidate != 0
            && endpoint_unique_id(candidate) as u32 == unique_id
        {
            destination = candidate;
            break;
        }
    }
    if destination == 0 {
        return false;
    }

    let mut client: sys::MIDIClientRef = 0;
    let mut port: sys::MIDIPortRef = 0;
    let client_name = cfstring("Hirari MIDI");
    let client_status = sys::MIDIClientCreate(client_name, None, ptr::null_mut(), &mut client);
    if !client_name.is_null() {
        sys::CFRelease(client_name.cast::<c_void>());
    }
    if client_status != 0 {
        return false;
    }
    let port_name = cfstring("Hirari Output");
    let port_status = sys::MIDIOutputPortCreate(client, port_name, &mut port);
    if !port_name.is_null() {
        sys::CFRelease(port_name.cast::<c_void>());
    }
    if port_status != 0 {
        let _ = sys::MIDIClientDispose(client);
        return false;
    }

    let mut storage = PacketStorage::default();
    let packets = storage.0.as_mut_ptr().cast::<sys::MIDIPacketList>();
    let packet = sys::MIDIPacketListInit(packets);
    let packet = sys::MIDIPacketListAdd(
        packets,
        storage.0.len() as sys::ByteCount,
        packet,
        0,
        size as sys::ByteCount,
        data,
    );
    let status = if packet.is_null() {
        -1
    } else {
        sys::MIDISend(port, destination, packets)
    };
    let _ = sys::MIDIPortDispose(port);
    let _ = sys::MIDIClientDispose(client);
    status == 0
}

const PACKET_STORAGE_BYTES: usize = std::mem::size_of::<sys::MIDIPacketList>() + 512;

#[repr(C, align(8))]
struct PacketStorage([u8; PACKET_STORAGE_BYTES]);

impl Default for PacketStorage {
    fn default() -> Self {
        Self([0u8; PACKET_STORAGE_BYTES])
    }
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_start_input() -> bool {
    let mut state = lock_input();
    if state.port != 0 {
        return true;
    }
    unsafe {
        let client_name = cfstring("Hirari MIDI Input");
        let client_status =
            sys::MIDIClientCreate(client_name, None, ptr::null_mut(), &mut state.client);
        if !client_name.is_null() {
            sys::CFRelease(client_name.cast::<c_void>());
        }
        let port_name = cfstring("Hirari Input");
        let port_status = sys::MIDIInputPortCreate(
            state.client,
            port_name,
            Some(midi_read_proc),
            ptr::null_mut(),
            &mut state.port,
        );
        if !port_name.is_null() {
            sys::CFRelease(port_name.cast::<c_void>());
        }
        if client_status != 0 || port_status != 0 {
            if state.port != 0 {
                let _ = sys::MIDIPortDispose(state.port);
            }
            if state.client != 0 {
                let _ = sys::MIDIClientDispose(state.client);
            }
            state.port = 0;
            state.client = 0;
            return false;
        }
        for index in 0..sys::MIDIGetNumberOfSources() {
            let source = sys::MIDIGetSource(index);
            let _ = sys::MIDIPortConnectSource(
                state.port,
                source,
                source as usize as *mut c_void,
            );
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_stop_input() {
    let mut state = lock_input();
    unsafe {
        if state.port != 0 {
            let _ = sys::MIDIPortDispose(state.port);
        }
        if state.client != 0 {
            let _ = sys::MIDIClientDispose(state.client);
        }
    }
    state.port = 0;
    state.client = 0;
    state.queue.clear();
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_poll_input_json() -> *mut c_char {
    let mut state = lock_input();
    let mut json = String::from("[");
    let mut first = true;
    while let Some(event) = state.queue.pop_front() {
        if !first {
            json.push(',');
        }
        first = false;
        json.push_str(&format!(
            "{{\"source_unique_id\":{},\"host_time\":{},\"data_hex\":\"",
            event.source, event.host_time
        ));
        for byte in event.data {
            json.push_str(&format!("{byte:02x}"));
        }
        json.push_str("\"}");
    }
    json.push(']');
    into_c_string(json)
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_host_time_now() -> u64 {
    unsafe { mach_absolute_time() }
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_host_time_delta_samples(
    start: u64,
    event: u64,
    sample_rate: f64,
) -> u64 {
    if event <= start || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0;
    }
    let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
    let status = unsafe { mach_timebase_info(&mut timebase) };
    if status != 0 || timebase.denom == 0 {
        return 0;
    }
    let elapsed_nanos =
        (event - start) as f64 * timebase.numer as f64 / timebase.denom as f64;
    let frames = elapsed_nanos * sample_rate / 1_000_000_000.0;
    if !frames.is_finite() || frames <= 0.0 || frames >= u64::MAX as f64 {
        return 0;
    }
    (frames + 0.5) as u64
}

#[no_mangle]
pub extern "C" fn hirari_core_midi_dropped_input_events() -> u64 {
    DROPPED_INPUT_EVENTS.load(Ordering::Relaxed)
}

/// # Safety
/// `value` must be null or a pointer returned by one of this module's string
/// getters, and must not be used again after this call.
#[no_mangle]
pub unsafe extern "C" fn hirari_core_midi_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

fn into_c_string(value: String) -> *mut c_char {
    match CString::new(value) {
        Ok(value) => value.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_time_delta_rejects_invalid_inputs() {
        assert_eq!(hirari_core_midi_host_time_delta_samples(10, 10, 48_000.0), 0);
        assert_eq!(hirari_core_midi_host_time_delta_samples(10, 5, 48_000.0), 0);
        assert_eq!(hirari_core_midi_host_time_delta_samples(0, 10, f64::NAN), 0);
        assert_eq!(hirari_core_midi_host_time_delta_samples(0, 10, 0.0), 0);
    }

    #[test]
    fn host_time_now_is_not_zero() {
        assert!(hirari_core_midi_host_time_now() > 0);
    }
}
