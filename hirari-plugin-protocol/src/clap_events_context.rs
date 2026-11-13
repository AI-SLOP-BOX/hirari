//! CLAP input-event projection machinery and the FFI surface, split out of
//! clap_events.rs.
use super::*;

pub(crate) struct InputEventContext {
    midi_events: *const MailboxMidiEvent,
    midi_count: usize,
    extended_messages: Vec<ExtendedMessage>,
    parameters: Vec<EventParamValue>,
    ordered: Vec<EventReference>,
    converted_midi: Vec<EventMidi>,
    converted_midi2: Vec<EventMidi2>,
    converted_sysex: Vec<EventMidiSysex>,
    output_count: *mut AtomicU32,
    dropped_count: *mut AtomicU32,
    output_events: *mut MailboxOutputMidiEvent,
    frames: u32,
    state_stream: StateStreamContext,
}

const _: () = assert!(core::mem::size_of::<MailboxMidiEvent>() == 272);
const _: () = assert!(core::mem::offset_of!(MailboxMidiEvent, data) == 12);
const _: () = assert!(core::mem::offset_of!(MailboxMidiEvent, articulation_id) == 268);
const _: () = assert!(core::mem::size_of::<MailboxParameterChange>() == 16);
const _: () = assert!(core::mem::size_of::<MailboxOutputMidiEvent>() == 272);
const _: () = assert!(core::mem::offset_of!(MailboxOutputMidiEvent, data) == 12);
const _: () = assert!(core::mem::offset_of!(MailboxOutputMidiEvent, articulation_id) == 268);
const _: () = assert!(core::mem::offset_of!(MailboxParameterChange, value) == 8);
const _: () = assert!(core::mem::size_of::<ExtendedMessage>() == 16 + MAX_MESSAGE_BYTES);
const _: () = assert!(core::mem::offset_of!(ExtendedMessage, size) == 12);
const _: () = assert!(core::mem::offset_of!(ExtendedMessage, data) == 16);
const _: () = assert!(core::mem::size_of::<EventHeader>() == 16);
const _: () = assert!(core::mem::offset_of!(EventHeader, event_type) == 10);
const _: () = assert!(core::mem::size_of::<EventMidi>() == 24);
const _: () = assert!(core::mem::offset_of!(EventMidi, port_index) == 16);
const _: () = assert!(core::mem::offset_of!(EventMidi, data) == 18);
const _: () = assert!(core::mem::size_of::<EventMidi2>() == 36);
const _: () = assert!(core::mem::offset_of!(EventMidi2, data) == 20);
const _: () = assert!(core::mem::size_of::<EventMidiSysex>() == 40);
const _: () = assert!(core::mem::offset_of!(EventMidiSysex, buffer) == 24);
const _: () = assert!(core::mem::offset_of!(EventMidiSysex, size) == 32);
const _: () = assert!(core::mem::size_of::<EventParamValue>() == 56);
const _: () = assert!(core::mem::offset_of!(EventParamValue, parameter_id) == 16);
const _: () = assert!(core::mem::offset_of!(EventParamValue, cookie) == 24);
const _: () = assert!(core::mem::offset_of!(EventParamValue, value) == 32);
const _: () = assert!(core::mem::offset_of!(EventParamValue, note_id) == 40);
const _: () = assert!(core::mem::size_of::<ClapInputEvents>() == 24);
const _: () = assert!(core::mem::offset_of!(ClapInputEvents, _size) == 8);
const _: () = assert!(core::mem::offset_of!(ClapInputEvents, _get) == 16);
const _: () = assert!(core::mem::size_of::<ClapOutputEvents>() == 16);

impl InputEventContext {
    pub(crate) fn new() -> Self {
        let mut extended_messages = Vec::with_capacity(EXTENDED_CAPACITY);
        extended_messages.resize_with(EXTENDED_CAPACITY, ExtendedMessage::default);
        let mut parameters = Vec::with_capacity(PARAMETER_CAPACITY);
        parameters.resize(PARAMETER_CAPACITY, EventParamValue::default());
        let mut converted_midi = Vec::with_capacity(MIDI_CAPACITY);
        converted_midi.resize(MIDI_CAPACITY, EventMidi::default());
        let mut converted_midi2 = Vec::with_capacity(MIDI_CAPACITY);
        converted_midi2.resize(MIDI_CAPACITY, EventMidi2::default());
        let mut converted_sysex = Vec::with_capacity(EXTENDED_CAPACITY);
        converted_sysex.resize(EXTENDED_CAPACITY, EventMidiSysex::default());
        let mut ordered =
            Vec::with_capacity(MIDI_CAPACITY + PARAMETER_CAPACITY + EXTENDED_CAPACITY);
        ordered.clear();
        Self {
            midi_events: ptr::null(),
            midi_count: 0,
            extended_messages,
            parameters,
            ordered,
            converted_midi,
            converted_midi2,
            converted_sysex,
            output_count: ptr::null_mut(),
            dropped_count: ptr::null_mut(),
            output_events: ptr::null_mut(),
            frames: 0,
            state_stream: StateStreamContext::default(),
        }
    }

    unsafe fn prepare(
        &mut self,
        midi_events: *const c_void,
        midi_count: u32,
        extended_ring: *mut c_void,
        parameter_changes: *const c_void,
        parameter_count: u32,
        frames: u32,
    ) -> bool {
        if (midi_count != 0 && midi_events.is_null())
            || (parameter_count != 0 && parameter_changes.is_null())
            || extended_ring.is_null()
        {
            return false;
        }
        self.midi_events = midi_events.cast();
        self.midi_count = (midi_count as usize).min(MIDI_CAPACITY);
        self.frames = frames;
        let parameters_count = (parameter_count as usize).min(PARAMETER_CAPACITY);
        self.ordered.clear();

        let parameter_source = parameter_changes.cast::<MailboxParameterChange>();
        for index in 0..parameters_count {
            // SAFETY: The caller supplies `parameter_count` initialized mailbox
            // entries and the count is bounded before indexing.
            let source = unsafe { &*parameter_source.add(index) };
            let destination = &mut self.parameters[index];
            destination.header = EventHeader {
                size: core::mem::size_of::<EventParamValue>() as u32,
                time: source.sample_offset.min(frames),
                space_id: CORE_EVENT_SPACE,
                event_type: EVENT_PARAM_VALUE,
                flags: 0,
            };
            destination.parameter_id = source.parameter_id;
            destination.cookie = ptr::null_mut();
            destination.value = source.value;
            destination.note_id = -1;
            destination.port_index = 0;
            destination.channel = -1;
            destination.key = -1;
            self.ordered.push(EventReference {
                kind: 2,
                index,
                sample_offset: destination.header.time,
            });
        }

        for index in 0..self.extended_messages.len() {
            // SAFETY: `extended_ring` is the initialized SPSC ring embedded in
            // the live shared block; this worker is its sole consumer.
            let popped = unsafe {
                hirari_midi_extended_ring_pop(
                    extended_ring,
                    (&mut self.extended_messages[index] as *mut ExtendedMessage).cast(),
                    core::mem::size_of::<ExtendedMessage>(),
                )
            };
            if !popped {
                break;
            }
            let message = &self.extended_messages[index];
            self.ordered.push(EventReference {
                kind: 1,
                index,
                sample_offset: message.sample_offset.min(u64::from(u32::MAX)) as u32,
            });
        }

        for index in 0..self.midi_count {
            // SAFETY: `midi_events` refers to the bounded shared mailbox array.
            let event = unsafe { &*self.midi_events.add(index) };
            self.ordered.push(EventReference {
                kind: 0,
                index,
                sample_offset: event.sample_offset.min(u64::from(u32::MAX)) as u32,
            });
        }
        self.ordered
            .sort_unstable_by_key(|event| (event.sample_offset, event.kind, event.index));
        true
    }

    pub(crate) fn get(&mut self, index: u32) -> *const EventHeader {
        let Some(reference) = self.ordered.get(index as usize).copied() else {
            return ptr::null();
        };
        match reference.kind {
            2 => &self.parameters[reference.index].header,
            1 => {
                let message = &self.extended_messages[reference.index];
                if message.size == 0
                    || message.size as usize > MAX_MESSAGE_BYTES
                    || message.sample_offset > u64::from(u32::MAX)
                {
                    return ptr::null();
                }
                let event = &mut self.converted_sysex[reference.index];
                event.header = EventHeader {
                    size: core::mem::size_of::<EventMidiSysex>() as u32,
                    time: message.sample_offset as u32,
                    space_id: CORE_EVENT_SPACE,
                    event_type: EVENT_MIDI_SYSEX,
                    flags: 0,
                };
                event.port_index = 0;
                event.buffer = message.data.as_ptr();
                event.size = message.size;
                &event.header
            }
            _ => {
                // SAFETY: MIDI references are only created below midi_count.
                let event = unsafe { &*self.midi_events.add(reference.index) };
                if event.size < 3 || event.sample_offset > u64::from(u32::MAX) {
                    return ptr::null();
                }
                if event.size == 16 {
                    let converted = &mut self.converted_midi2[reference.index];
                    converted.header = EventHeader {
                        size: core::mem::size_of::<EventMidi2>() as u32,
                        time: event.sample_offset as u32,
                        space_id: CORE_EVENT_SPACE,
                        event_type: EVENT_MIDI2,
                        flags: 0,
                    };
                    converted.port_index = 0;
                    converted.reserved = 0;
                    // Preserve all 128 UMP bits. The payload is byte-oriented
                    // in the mailbox, so copy without host-endian conversion.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            event.data.as_ptr(),
                            converted.data.as_mut_ptr().cast::<u8>(),
                            16,
                        );
                    }
                    &converted.header
                } else {
                    let converted = &mut self.converted_midi[reference.index];
                    converted.header = EventHeader {
                        size: core::mem::size_of::<EventMidi>() as u32,
                        time: event.sample_offset as u32,
                        space_id: CORE_EVENT_SPACE,
                        event_type: EVENT_MIDI,
                        flags: 0,
                    };
                    converted.port_index = 0;
                    converted.data.copy_from_slice(&event.data[..3]);
                    &converted.header
                }
            }
        }
    }
}

unsafe extern "C" {
    pub(crate) fn hirari_midi_extended_ring_pop(
        state: *mut c_void,
        destination_message: *mut c_void,
        destination_bytes: usize,
    ) -> bool;
}

pub(crate) unsafe fn push_output_event(
    context: &InputEventContext,
    header_ptr: *const EventHeader,
) -> bool {
    if context.output_count.is_null()
        || context.dropped_count.is_null()
        || context.output_events.is_null()
        || header_ptr.is_null()
    {
        return false;
    }
    // SAFETY: The callback receives a live CLAP event header; event sizes are
    // checked before accessing any concrete event representation.
    let header = unsafe { &*header_ptr };
    if header.space_id != CORE_EVENT_SPACE || header.time >= context.frames {
        return false;
    }
    let mut payload = [0_u8; OUTPUT_PAYLOAD_CAPACITY];
    let payload_size;
    match header.event_type {
        EVENT_MIDI => {
            if header.size < core::mem::size_of::<EventMidi>() as u32 {
                return false;
            }
            let event = unsafe { &*header_ptr.cast::<EventMidi>() };
            payload[..3].copy_from_slice(&event.data);
            payload_size = 3;
        }
        EVENT_MIDI_SYSEX => {
            if header.size < core::mem::size_of::<EventMidiSysex>() as u32 {
                return false;
            }
            let event = unsafe { &*header_ptr.cast::<EventMidiSysex>() };
            if event.buffer.is_null()
                || event.size == 0
                || event.size as usize > OUTPUT_PAYLOAD_CAPACITY
            {
                unsafe { &*context.dropped_count }.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            unsafe {
                ptr::copy_nonoverlapping(event.buffer, payload.as_mut_ptr(), event.size as usize)
            };
            payload_size = event.size as usize;
        }
        EVENT_MIDI2 => {
            if header.size < core::mem::size_of::<EventMidi2>() as u32 {
                return false;
            }
            let event = unsafe { &*header_ptr.cast::<EventMidi2>() };
            unsafe {
                ptr::copy_nonoverlapping(event.data.as_ptr().cast::<u8>(), payload.as_mut_ptr(), 16)
            };
            payload_size = 16;
        }
        _ => return false,
    }
    let count = unsafe { &*context.output_count };
    let index = count.load(Ordering::Relaxed) as usize;
    if index >= OUTPUT_MIDI_CAPACITY {
        unsafe { &*context.dropped_count }.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let destination = unsafe { &mut *context.output_events.add(index) };
    destination.sample_offset = u64::from(header.time);
    destination.size = payload_size as u32;
    destination.articulation_id = 0;
    destination.data[..payload_size].copy_from_slice(&payload[..payload_size]);
    count.store((index + 1) as u32, Ordering::Release);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_input_events_size(input: *const ClapInputEvents) -> u32 {
    if input.is_null() {
        return 0;
    }
    let context = unsafe { (*input).context };
    if context.is_null() {
        return 0;
    }
    unsafe { &*context.cast::<InputEventContext>() }
        .ordered
        .len() as u32
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_input_events_get(
    input: *const ClapInputEvents,
    index: u32,
) -> *const EventHeader {
    if input.is_null() {
        return ptr::null();
    }
    let context = unsafe { (*input).context };
    if context.is_null() {
        return ptr::null();
    }
    // CLAP exposes the context pointer as const, while event conversion uses
    // worker-owned scratch buffers that are validly updated by this callback.
    unsafe { &mut *(context as *mut InputEventContext) }.get(index)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_output_events_try_push(
    output: *const ClapOutputEvents,
    header: *const EventHeader,
) -> bool {
    if output.is_null() {
        return false;
    }
    let context = unsafe { (*output).context };
    if context.is_null() {
        return false;
    }
    unsafe { push_output_event(&*context.cast::<InputEventContext>(), header) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_state_stream_configure(
    context: *mut c_void,
    data: *mut u8,
    size: u32,
    capacity: u32,
    writing: bool,
) -> *mut c_void {
    if context.is_null() || (capacity != 0 && data.is_null()) || size > capacity {
        return ptr::null_mut();
    }
    let context = unsafe { &mut *context.cast::<InputEventContext>() };
    context.state_stream = StateStreamContext {
        data,
        size,
        cursor: 0,
        capacity,
        writing,
    };
    ptr::addr_of_mut!(context.state_stream).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_state_stream_size(stream: *const c_void) -> u32 {
    if stream.is_null() {
        return 0;
    }
    unsafe { (*stream.cast::<StateStreamContext>()).size }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_state_write(
    stream: *const ClapOStream,
    data: *const c_void,
    size: u64,
) -> i64 {
    if stream.is_null() {
        return -1;
    }
    let context = unsafe { (*stream).context };
    if context.is_null() || (size != 0 && data.is_null()) {
        return -1;
    }
    let state = unsafe { &mut *context.cast::<StateStreamContext>() };
    if !state.writing
        || state.cursor > state.capacity
        || size > u64::from(state.capacity - state.cursor)
    {
        return -1;
    }
    if size != 0 {
        unsafe {
            ptr::copy_nonoverlapping(
                data.cast::<u8>(),
                state.data.add(state.cursor as usize),
                size as usize,
            )
        };
    }
    state.cursor += size as u32;
    state.size = state.cursor;
    size as i64
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_state_read(
    stream: *const ClapIStream,
    data: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() {
        return -1;
    }
    let context = unsafe { (*stream).context };
    if context.is_null() || (size != 0 && data.is_null()) {
        return -1;
    }
    let state = unsafe { &mut *context.cast::<StateStreamContext>() };
    if state.writing || state.cursor > state.size || size > u64::from(state.size - state.cursor) {
        return -1;
    }
    if size != 0 {
        unsafe {
            ptr::copy_nonoverlapping(
                state.data.add(state.cursor as usize),
                data.cast::<u8>(),
                size as usize,
            )
        };
    }
    state.cursor += size as u32;
    size as i64
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_input_context_create(
    output_count: *mut c_void,
    dropped_count: *mut c_void,
    output_events: *mut c_void,
) -> *mut c_void {
    if output_count.is_null() || dropped_count.is_null() || output_events.is_null() {
        return ptr::null_mut();
    }
    let mut context = InputEventContext::new();
    context.output_count = output_count.cast();
    context.dropped_count = dropped_count.cast();
    context.output_events = output_events.cast();
    Box::into_raw(Box::new(context)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_input_context_destroy(context: *mut c_void) {
    if !context.is_null() {
        // SAFETY: The pointer originates from `hirari_clap_input_context_create`
        // and is destroyed exactly once by the worker after processing stops.
        drop(unsafe { Box::from_raw(context.cast::<InputEventContext>()) });
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_clap_input_context_prepare(
    context: *mut c_void,
    midi_events: *const c_void,
    midi_count: u32,
    extended_ring: *mut c_void,
    parameter_changes: *const c_void,
    parameter_count: u32,
    frames: u32,
) -> bool {
    if context.is_null() {
        return false;
    }
    // SAFETY: The opaque context is owned by the single worker thread. Input
    // pointers are valid shared-block ranges for the duration of this call.
    unsafe { &mut *context.cast::<InputEventContext>() }.prepare(
        midi_events,
        midi_count,
        extended_ring,
        parameter_changes,
        parameter_count,
        frames,
    )
}

/// Projects one shared-mailbox block into CLAP's audio and event ABI, invokes
/// the plugin, and clears output on a rejected block or plugin error.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_process_block(
    plugin: *const c_void,
    context: *mut c_void,
    input_audio: *mut f32,
    output_audio: *mut f32,
    channels: u32,
    frames: u32,
    midi_events: *const c_void,
    midi_count: u32,
    extended_ring: *mut c_void,
    parameter_changes: *const c_void,
    parameter_count: u32,
) -> bool {
    if plugin.is_null()
        || context.is_null()
        || input_audio.is_null()
        || output_audio.is_null()
        || channels == 0
        || channels as usize > MAX_CHANNELS
        || frames == 0
        || frames as usize > MAX_FRAMES
    {
        return false;
    }
    let prepared = unsafe {
        hirari_clap_input_context_prepare(
            context,
            midi_events,
            midi_count,
            extended_ring,
            parameter_changes,
            parameter_count,
            frames,
        )
    };
    if !prepared {
        unsafe { ptr::write_bytes(output_audio, 0, MAX_CHANNELS * MAX_FRAMES) };
        return false;
    }

    let mut input_channels = [ptr::null_mut(); MAX_CHANNELS];
    let mut output_channels = [ptr::null_mut(); MAX_CHANNELS];
    for channel in 0..channels as usize {
        let offset = channel * MAX_FRAMES;
        input_channels[channel] = unsafe { input_audio.add(offset) };
        output_channels[channel] = unsafe { output_audio.add(offset) };
    }
    let input = ClapAudioBuffer {
        data32: input_channels.as_mut_ptr(),
        data64: ptr::null_mut(),
        channel_count: channels,
        latency: 0,
        constant_mask: 0,
    };
    let mut output = ClapAudioBuffer {
        data32: output_channels.as_mut_ptr(),
        data64: ptr::null_mut(),
        channel_count: channels,
        latency: 0,
        constant_mask: 0,
    };
    let input_events = ClapInputEvents {
        context,
        _size: Some(hirari_clap_input_events_size),
        _get: Some(hirari_clap_input_events_get),
    };
    let mut output_events = ClapOutputEvents {
        context,
        _try_push: Some(hirari_clap_output_events_try_push),
    };
    let process = ClapProcess {
        steady_time: 0,
        frames_count: frames,
        transport: ptr::null(),
        audio_inputs: &input,
        audio_inputs_count: 1,
        audio_outputs: &mut output,
        audio_outputs_count: 1,
        in_events: &input_events,
        out_events: &mut output_events,
    };
    let plugin = unsafe { &*plugin.cast::<ClapPluginPrefix>() };
    let succeeded = plugin.process.is_some_and(|process_plugin| unsafe {
        process_plugin(plugin as *const _ as *const c_void, &process) != 0
    });
    if !succeeded {
        unsafe { ptr::write_bytes(output_audio, 0, MAX_CHANNELS * MAX_FRAMES) };
    }
    succeeded
}

/// Performs a CLAP state save/load transaction and owns its stream framing,
/// checksum validation, metadata publication, and active-state restoration.
#[no_mangle]
pub unsafe extern "C" fn hirari_clap_state_block(
    plugin: *const c_void,
    extension: *const c_void,
    context: *mut c_void,
    mode: u8,
    state: *mut u8,
    state_size: *const AtomicU32,
    state_version: *const AtomicU32,
    state_checksum: *const AtomicU64,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
) -> u16 {
    if plugin.is_null()
        || extension.is_null()
        || context.is_null()
        || state.is_null()
        || state_size.is_null()
        || state_version.is_null()
        || state_checksum.is_null()
    {
        return u16::from(STATE_ERROR_PLUGIN) << 8;
    }
    let plugin = unsafe { &*plugin.cast::<ClapPluginPrefix>() };
    let extension = unsafe { &*extension.cast::<ClapStateExtension>() };
    let state_size = unsafe { &*state_size };
    let state_version = unsafe { &*state_version };
    let state_checksum = unsafe { &*state_checksum };
    let mut state_ok = false;
    let mut error = STATE_ERROR_PLUGIN;
    let mut suspended = false;

    if mode == STATE_MODE_LOAD {
        if let (Some(stop), Some(deactivate)) = (plugin.stop_processing, plugin.deactivate) {
            unsafe { stop(plugin as *const _ as *const c_void) };
            unsafe { deactivate(plugin as *const _ as *const c_void) };
            suspended = true;
        }
    }

    if mode == STATE_MODE_SAVE {
        if let Some(save) = extension.save {
            let stream_context = unsafe {
                hirari_clap_state_stream_configure(
                    context,
                    state,
                    0,
                    crate::MAX_STATE_BYTES as u32,
                    true,
                )
            };
            let stream = ClapOStream {
                context: stream_context,
                write: Some(hirari_clap_state_write),
            };
            state_ok = !stream_context.is_null()
                && unsafe { save(plugin as *const _ as *const c_void, &stream) };
            let size = unsafe { hirari_clap_state_stream_size(stream_context) };
            if state_ok && size <= crate::MAX_STATE_BYTES as u32 {
                state_size.store(size, Ordering::Release);
                state_version.store(crate::STATE_PROTOCOL_VERSION, Ordering::Release);
                state_checksum.store(
                    unsafe { crate::hirari_plugin_state_checksum(state, size as usize) },
                    Ordering::Release,
                );
                error = STATE_ERROR_NONE;
            }
        }
    } else if mode == STATE_MODE_LOAD {
        if let Some(load) = extension.load {
            let size = state_size.load(Ordering::Acquire);
            let version = state_version.load(Ordering::Acquire);
            if size > crate::MAX_STATE_BYTES as u32 {
                error = STATE_ERROR_OVERSIZE;
            } else {
                let stream_context = unsafe {
                    hirari_clap_state_stream_configure(
                        context,
                        state,
                        size,
                        crate::MAX_STATE_BYTES as u32,
                        false,
                    )
                };
                error = unsafe {
                    crate::hirari_plugin_state_validate(
                        state,
                        size as usize,
                        version,
                        state_checksum.load(Ordering::Acquire),
                    )
                };
                if error == STATE_ERROR_NONE && !stream_context.is_null() {
                    let stream = ClapIStream {
                        context: stream_context,
                        read: Some(hirari_clap_state_read),
                    };
                    state_ok = unsafe { load(plugin as *const _ as *const c_void, &stream) };
                    if !state_ok {
                        error = STATE_ERROR_PLUGIN;
                    }
                }
            }
        }
    }

    if suspended {
        let reactivated = plugin.activate.is_some_and(|activate| unsafe {
            activate(
                plugin as *const _ as *const c_void,
                sample_rate,
                min_frames,
                max_frames,
            )
        });
        let restarted = reactivated
            && plugin
                .start_processing
                .is_some_and(|start| unsafe { start(plugin as *const _ as *const c_void) });
        if !reactivated || !restarted {
            state_ok = false;
            error = STATE_ERROR_PLUGIN;
        }
    }
    (u16::from(error) << 8) | u16::from(state_ok)
}
