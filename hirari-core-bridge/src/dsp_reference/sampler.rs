//! Frozen pre-Rust sampler voice engine (polyphonic, zone based).

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    position: f64,
    pitch: f64,
    base_pitch: f64,
    sample_rate_ratio: f64,
    velocity: f32,
    envelope: f32,
    envelope_state: u32,
    note: u8,
    channel: u8,
    key_released: bool,
    left: *const f32,
    right: *const f32,
    length: u32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            active: false,
            position: 0.0,
            pitch: 1.0,
            base_pitch: 1.0,
            sample_rate_ratio: 1.0,
            velocity: 1.0,
            envelope: 0.0,
            envelope_state: 0,
            note: 0,
            channel: 0,
            key_released: false,
            left: std::ptr::null(),
            right: std::ptr::null(),
            length: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Zone {
    data: *const f32,
    length: u32,
    sample_rate: f64,
    root: u8,
    low_key: u8,
    high_key: u8,
    low_velocity: u8,
    high_velocity: u8,
}

struct SamplerReference {
    sample_rate: f64,
    voices: [Voice; 64],
    free_stack: [u32; 64],
    free_count: usize,
    sustain: [bool; 16],
    zones: Vec<Zone>,
    round_robin: [u32; 128],
}

impl SamplerReference {
    fn new(sample_rate: f64) -> Self {
        let mut sampler = Self {
            sample_rate,
            voices: [Voice::default(); 64],
            free_stack: [0; 64],
            free_count: 0,
            sustain: [false; 16],
            zones: Vec::new(),
            round_robin: [0; 128],
        };
        let mut index = 64u32;
        while index > 0 {
            index -= 1;
            sampler.free_stack[sampler.free_count] = index;
            sampler.free_count += 1;
        }
        sampler
    }
}

fn interpolate(data: &[f32], position: f64) -> f32 {
    if data.is_empty() || !position.is_finite() {
        return 0.0;
    }
    let at = |raw: i64| -> f32 {
        let index = raw.clamp(0, data.len() as i64 - 1);
        let value = data[index as usize];
        if value.is_finite() { value } else { 0.0 }
    };
    let i = position.floor() as i64;
    let t = (position - i as f64) as f32;
    let y0 = at(i - 1);
    let y1 = at(i);
    let y2 = at(i + 1);
    let y3 = at(i + 2);
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    let value = ((c3 * t + c2) * t + c1) * t + y1;
    if value.is_finite() { value } else { y1 }
}

fn allocate(sampler: &mut SamplerReference) -> u32 {
    if sampler.free_count != 0 {
        sampler.free_count -= 1;
        return sampler.free_stack[sampler.free_count];
    }
    let mut quietest = 0usize;
    for index in 1..sampler.voices.len() {
        if sampler.voices[index].envelope < sampler.voices[quietest].envelope {
            quietest = index;
        }
    }
    quietest as u32
}

fn start_voice(
    sampler: &mut SamplerReference,
    note: u8,
    velocity: u8,
    channel: u8,
    zone: &Zone,
) {
    let mut voice = Voice::default();
    voice.active = true;
    voice.note = note;
    voice.channel = channel;
    voice.velocity = velocity as f32 / 127.0;
    voice.pitch = 2.0f64.powf((note as f64 - zone.root as f64) / 12.0);
    voice.base_pitch = voice.pitch;
    voice.sample_rate_ratio = if zone.sample_rate.is_finite()
        && zone.sample_rate >= 8000.0
        && zone.sample_rate <= 384000.0
        && sampler.sample_rate > 0.0
    {
        zone.sample_rate / sampler.sample_rate
    } else {
        1.0
    };
    voice.envelope_state = 1;
    voice.left = zone.data;
    voice.right = zone.data;
    voice.length = zone.length;
    let index = allocate(sampler) as usize;
    sampler.voices[index] = voice;
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_create(sample_rate: f64) -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(SamplerReference::new(sample_rate))) as *mut std::ffi::c_void
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut SamplerReference));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_note_on(
    state: *mut std::ffi::c_void,
    note: u8,
    velocity: u8,
    left: *const f32,
    right: *const f32,
    length: u32,
    root: u8,
    source_rate: f64,
) {
    if state.is_null() || left.is_null() || length == 0 {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    let mut voice = Voice::default();
    voice.active = true;
    voice.note = note;
    voice.velocity = velocity as f32 / 127.0;
    voice.pitch = 2.0f64.powf((note as f64 - root as f64) / 12.0);
    voice.base_pitch = voice.pitch;
    voice.sample_rate_ratio = if source_rate.is_finite()
        && source_rate >= 8000.0
        && source_rate <= 384000.0
        && sampler.sample_rate > 0.0
    {
        source_rate / sampler.sample_rate
    } else {
        1.0
    };
    voice.envelope_state = 1;
    voice.left = left;
    voice.right = if right.is_null() { left } else { right };
    voice.length = length;
    let index = allocate(sampler) as usize;
    sampler.voices[index] = voice;
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_add_zone(
    state: *mut std::ffi::c_void,
    data: *const f32,
    length: u32,
    mut sample_rate: f64,
    root: u8,
    low_key: u8,
    high_key: u8,
    low_velocity: u8,
    high_velocity: u8,
) {
    if state.is_null() || data.is_null() || length == 0 || low_key > high_key
        || low_velocity > high_velocity
    {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    if sampler.zones.len() >= 256 {
        return;
    }
    if sample_rate == 0.0 {
        sample_rate = sampler.sample_rate;
    }
    sampler.zones.push(Zone {
        data,
        length,
        sample_rate,
        root,
        low_key,
        high_key,
        low_velocity,
        high_velocity,
    });
    sampler.zones.sort_by(|left, right| {
        left.low_key
            .cmp(&right.low_key)
            .then(left.high_key.cmp(&right.high_key))
            .then(left.low_velocity.cmp(&right.low_velocity))
    });
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_note_on_zone(
    state: *mut std::ffi::c_void,
    note: u8,
    velocity: u8,
    channel: u8,
) {
    if state.is_null() {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    let matches_zone = |zone: &Zone| {
        note >= zone.low_key
            && note <= zone.high_key
            && velocity >= zone.low_velocity
            && velocity <= zone.high_velocity
    };
    let mut best_velocity_span = u32::MAX;
    let mut best_key_span = u32::MAX;
    let mut matches = 0usize;
    for zone in &sampler.zones {
        if !matches_zone(zone) {
            continue;
        }
        let velocity_span = zone.high_velocity as u32 - zone.low_velocity as u32;
        let key_span = zone.high_key as u32 - zone.low_key as u32;
        if velocity_span < best_velocity_span
            || (velocity_span == best_velocity_span && key_span < best_key_span)
        {
            best_velocity_span = velocity_span;
            best_key_span = key_span;
            matches = 1;
        } else if velocity_span == best_velocity_span && key_span == best_key_span {
            matches += 1;
        }
    }
    if matches == 0 {
        return;
    }
    let target = if matches > 1 {
        let slot = &mut sampler.round_robin[note as usize];
        let value = *slot % matches as u32;
        *slot = (*slot).wrapping_add(1);
        value as usize
    } else {
        0
    };
    let mut ordinal = 0usize;
    let mut selected = None;
    for (index, zone) in sampler.zones.iter().enumerate() {
        if !matches_zone(zone) {
            continue;
        }
        let velocity_span = zone.high_velocity as u32 - zone.low_velocity as u32;
        let key_span = zone.high_key as u32 - zone.low_key as u32;
        if velocity_span != best_velocity_span || key_span != best_key_span {
            continue;
        }
        if ordinal == target {
            selected = Some(index);
            break;
        }
        ordinal += 1;
    }
    if let Some(index) = selected {
        let zone = sampler.zones[index];
        start_voice(sampler, note, velocity, channel, &zone);
    }
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_note_off(
    state: *mut std::ffi::c_void,
    note: u8,
    channel: u8,
) {
    if state.is_null() {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    for voice in sampler.voices.iter_mut() {
        if voice.active && voice.note == note && (channel == 0xff || voice.channel == channel) {
            if channel < 16 && sampler.sustain[channel as usize] {
                voice.key_released = true;
            } else {
                voice.envelope_state = 4;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_set_sustain(
    state: *mut std::ffi::c_void,
    channel: u8,
    held: bool,
) {
    if state.is_null() || channel >= 16 {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    sampler.sustain[channel as usize] = held;
    if !held {
        for voice in sampler.voices.iter_mut() {
            if voice.active && voice.channel == channel && voice.key_released {
                voice.key_released = false;
                voice.envelope_state = 4;
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_pitch_bend(
    state: *mut std::ffi::c_void,
    channel: u8,
    bend: f32,
) {
    if state.is_null() || !bend.is_finite() {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    let ratio = 2.0f64.powf((bend as f64).clamp(-1.0, 1.0) * (2.0 / 12.0));
    for voice in sampler.voices.iter_mut() {
        if voice.active && voice.channel == channel {
            voice.pitch = voice.base_pitch * ratio;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_process(
    state: *mut std::ffi::c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let sampler = &mut *(state as *mut SamplerReference);
    let frames = frames as usize;
    let left_slice = std::slice::from_raw_parts_mut(left, frames);
    left_slice.fill(0.0);
    let mut right_slice = if right.is_null() {
        None
    } else {
        let slice = std::slice::from_raw_parts_mut(right, frames);
        slice.fill(0.0);
        Some(slice)
    };

    for index in 0..sampler.voices.len() {
        if !sampler.voices[index].active
            || sampler.voices[index].left.is_null()
            || sampler.voices[index].length == 0
        {
            continue;
        }
        for frame in 0..frames {
            if sampler.voices[index].envelope_state == 0 || !sampler.voices[index].active {
                sampler.voices[index].active = false;
                sampler.voices[index].left = std::ptr::null();
                sampler.voices[index].right = std::ptr::null();
                sampler.free_stack[sampler.free_count] = index as u32;
                sampler.free_count += 1;
                break;
            }
            if sampler.voices[index].position as u64 >= sampler.voices[index].length as u64 {
                sampler.voices[index].active = false;
                sampler.voices[index].envelope_state = 0;
                sampler.voices[index].left = std::ptr::null();
                sampler.voices[index].right = std::ptr::null();
                sampler.free_stack[sampler.free_count] = index as u32;
                sampler.free_count += 1;
                break;
            }
            let voice = sampler.voices[index];
            let source_left = std::slice::from_raw_parts(voice.left, voice.length as usize);
            let source_right = std::slice::from_raw_parts(voice.right, voice.length as usize);
            let sample_left = interpolate(source_left, voice.position);
            let sample_right = interpolate(source_right, voice.position);
            if voice.envelope_state == 1 {
                sampler.voices[index].envelope += 0.002;
                if sampler.voices[index].envelope >= 1.0 {
                    sampler.voices[index].envelope = 1.0;
                    sampler.voices[index].envelope_state = 2;
                }
            } else if voice.envelope_state == 2 {
                sampler.voices[index].envelope -= 0.0005;
                if sampler.voices[index].envelope <= 0.8 {
                    sampler.voices[index].envelope = 0.8;
                    sampler.voices[index].envelope_state = 3;
                }
            } else if voice.envelope_state == 4 {
                sampler.voices[index].envelope -= 0.001;
                if sampler.voices[index].envelope <= 0.0 {
                    sampler.voices[index].envelope = 0.0;
                    sampler.voices[index].active = false;
                    sampler.voices[index].envelope_state = 0;
                }
            }
            let gain = sampler.voices[index].envelope * voice.velocity;
            let out_left = sample_left * gain;
            let out_right = sample_right * gain;
            if out_left.is_finite() {
                left_slice[frame] += out_left;
            }
            if let Some(right_slice) = right_slice.as_mut() {
                if out_right.is_finite() {
                    right_slice[frame] += out_right;
                }
            }
            sampler.voices[index].position += voice.pitch * voice.sample_rate_ratio;
        }
    }
}

#[repr(C)]
struct MidiEventReference {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

const _: () = assert!(std::mem::size_of::<MidiEventReference>() == 272);

#[no_mangle]
pub unsafe extern "C" fn sampler_reference_process_midi_events(
    state: *mut std::ffi::c_void,
    events: *const MidiEventReference,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let event_count = event_count.min(1024);
    let events = if events.is_null() {
        &[][..]
    } else {
        std::slice::from_raw_parts(events, event_count)
    };
    for event in events {
        if event.size == 16 && (event.data[0] >> 4) == 0x4 {
            let status = event.data[1] & 0xf0;
            let channel = event.data[1] & 0x0f;
            let note = event.data[2] & 0x7f;
            let value = event.data[4];
            if status == 0x90 && value != 0 {
                sampler_reference_note_on_zone(state, note, value, channel);
            } else if status == 0x80 || (status == 0x90 && value == 0) {
                sampler_reference_note_off(state, note, channel);
            } else if status == 0xb0 && (event.data[2] & 0x7f) == 64 {
                sampler_reference_set_sustain(state, channel, value >= 64);
            } else if status == 0xe0 {
                sampler_reference_pitch_bend(state, channel, (value as f32 - 128.0) / 128.0);
            }
            continue;
        }
        if event.size < 2 || event.data[0] < 0x80 {
            continue;
        }
        let status = event.data[0] & 0xf0;
        let channel = event.data[0] & 0x0f;
        let pitch = event.data[1] & 0x7f;
        let velocity = if event.size >= 3 {
            event.data[2] & 0x7f
        } else {
            0
        };
        if status == 0x90 && velocity > 0 {
            sampler_reference_note_on_zone(state, pitch, velocity, channel);
        } else if status == 0x80 || (status == 0x90 && velocity == 0) {
            sampler_reference_note_off(state, pitch, channel);
        } else if status == 0xb0 && pitch == 64 {
            sampler_reference_set_sustain(state, channel, velocity >= 64);
        } else if status == 0xe0 && event.size >= 3 {
            let value = ((event.data[2] & 0x7f) as i32) << 7 | (event.data[1] & 0x7f) as i32;
            sampler_reference_pitch_bend(
                state,
                channel,
                (value - 8192) as f32 / 8192.0,
            );
        }
    }
    sampler_reference_process(state, left, right, frames);
}
