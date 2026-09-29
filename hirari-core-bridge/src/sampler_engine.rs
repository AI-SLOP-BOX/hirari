pub struct Voice {
    pub active: bool,
    pub position: f64,
    pub pitch_ratio: f64,
    pub sample_rate_ratio: f64,
    pub velocity: f32,
    pub env_level: f32,
    pub env_state: u32, // 0=Idle, 1=Attack, 2=Decay, 3=Sustain, 4=Release
    pub note: u8,
    pub is_streaming: bool,
    pub brightness: f32,
}

#[derive(Clone, Copy)]
struct RealtimeZone {
    data: usize,
    length: usize,
    sample_rate: f64,
    root_key: u8,
    low_key: u8,
    high_key: u8,
    low_velocity: u8,
    high_velocity: u8,
    loop_start: u32,
    loop_end: u32,
    loop_enabled: bool,
}

enum RealtimeSample {
    None,
    Raw {
        left: usize,
        length: u32,
        loop_start: u32,
        loop_end: u32,
        looping: bool,
    },
    Buffer {
        context: usize,
    },
}

struct RealtimeVoice {
    active: bool,
    position: f64,
    pitch_ratio: f64,
    base_pitch_ratio: f64,
    sample_rate_ratio: f64,
    velocity: f32,
    env_level: f32,
    env_state: u32,
    note: u8,
    channel: u8,
    sample: RealtimeSample,
    is_streaming: bool,
    key_released: bool,
    pressure_gain: f32,
}

impl RealtimeVoice {
    fn idle() -> Self {
        Self {
            active: false,
            position: 0.0,
            pitch_ratio: 1.0,
            base_pitch_ratio: 1.0,
            sample_rate_ratio: 1.0,
            velocity: 1.0,
            env_level: 0.0,
            env_state: 0,
            note: 0,
            channel: 0,
            sample: RealtimeSample::None,
            is_streaming: false,
            key_released: false,
            pressure_gain: 1.0,
        }
    }
}

type SampleBufferReader =
    unsafe extern "C" fn(*mut c_void, *mut *const f32, *mut *const f32, *mut u32) -> bool;

#[repr(C)]
pub struct SamplerMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

struct RealtimeSampler {
    sample_rate: f64,
    voices: Vec<RealtimeVoice>,
    free_stack: [usize; 64],
    free_count: usize,
    zones: Vec<RealtimeZone>,
    sustain: [bool; 16],
    volume: [f32; 16],
    expression: [f32; 16],
    zone_round_robin: [u32; 128],
    buffer_reader: Option<SampleBufferReader>,
}

impl RealtimeSampler {
    fn new(sample_rate: f64, reader: Option<SampleBufferReader>) -> Self {
        let mut result = Self {
            sample_rate: if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
            {
                sample_rate
            } else {
                44_100.0
            },
            voices: (0..64).map(|_| RealtimeVoice::idle()).collect(),
            free_stack: [0; 64],
            free_count: 0,
            zones: Vec::new(),
            sustain: [false; 16],
            volume: [1.0; 16],
            expression: [1.0; 16],
            zone_round_robin: [0; 128],
            buffer_reader: reader,
        };
        result.reset_free_voices();
        result
    }

    fn reset_free_voices(&mut self) {
        self.free_count = 0;
        for index in (0..self.voices.len()).rev() {
            self.free_stack[self.free_count] = index;
            self.free_count += 1;
        }
    }

    fn allocate_voice(&mut self) -> usize {
        if self.free_count > 0 {
            self.free_count -= 1;
            self.free_stack[self.free_count]
        } else {
            self.voices
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.env_level.total_cmp(&b.env_level))
                .map(|(index, _)| index)
                .unwrap_or(0)
        }
    }

    fn start_voice(
        &mut self,
        index: usize,
        note: u8,
        velocity: u8,
        channel: u8,
        root: u8,
        source_rate: f64,
        sample: RealtimeSample,
    ) {
        let rate_ratio = if source_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&source_rate)
            && self.sample_rate > 0.0
        {
            source_rate / self.sample_rate
        } else {
            1.0
        };
        let voice = &mut self.voices[index];
        voice.active = true;
        voice.position = 0.0;
        voice.pitch_ratio = 2.0f64.powf((note as f64 - root as f64) / 12.0);
        voice.base_pitch_ratio = voice.pitch_ratio;
        voice.sample_rate_ratio = rate_ratio;
        voice.velocity = velocity as f32 / 127.0;
        voice.env_level = 0.0;
        voice.env_state = 1;
        voice.note = note;
        voice.channel = channel;
        voice.sample = sample;
        voice.is_streaming = false;
        voice.key_released = false;
        voice.pressure_gain = 1.0;
    }

    fn add_zone(&mut self, mut zone: RealtimeZone) {
        if zone.sample_rate == 0.0 {
            zone.sample_rate = self.sample_rate;
        }
        if zone.data == 0
            || zone.length == 0
            || self.zones.len() >= 256
            || zone.low_key > zone.high_key
            || zone.low_velocity > zone.high_velocity
            || (zone.loop_enabled
                && (zone.loop_start >= zone.loop_end || zone.loop_end as usize > zone.length))
            || (zone.sample_rate != 0.0
                && (!zone.sample_rate.is_finite()
                    || !(8_000.0..=384_000.0).contains(&zone.sample_rate)))
        {
            return;
        }
        self.zones.push(zone);
        self.zones
            .sort_by_key(|item| (item.low_key, item.high_key, item.low_velocity));
    }

    fn note_on_zone(&mut self, note: u8, velocity: u8, channel: u8) {
        let mut best_velocity_span = u16::MAX;
        let mut best_key_span = u16::MAX;
        let mut equally_specific = 0usize;
        for zone in &self.zones {
            if note < zone.low_key
                || note > zone.high_key
                || velocity < zone.low_velocity
                || velocity > zone.high_velocity
            {
                continue;
            }
            let velocity_span = zone.high_velocity as u16 - zone.low_velocity as u16;
            let key_span = zone.high_key as u16 - zone.low_key as u16;
            if velocity_span < best_velocity_span
                || (velocity_span == best_velocity_span && key_span < best_key_span)
            {
                best_velocity_span = velocity_span;
                best_key_span = key_span;
                equally_specific = 1;
            } else if velocity_span == best_velocity_span && key_span == best_key_span {
                equally_specific += 1;
            }
        }
        if equally_specific == 0 {
            return;
        }
        let ordinal = self.zone_round_robin[note as usize] as usize % equally_specific;
        self.zone_round_robin[note as usize] = self.zone_round_robin[note as usize].wrapping_add(1);
        let Some(zone) = self
            .zones
            .iter()
            .filter(|zone| {
                note >= zone.low_key
                    && note <= zone.high_key
                    && velocity >= zone.low_velocity
                    && velocity <= zone.high_velocity
                    && zone.high_velocity as u16 - zone.low_velocity as u16 == best_velocity_span
                    && zone.high_key as u16 - zone.low_key as u16 == best_key_span
            })
            .nth(ordinal)
            .copied()
        else {
            return;
        };
        let index = self.allocate_voice();
        let length = zone.length.min(u32::MAX as usize) as u32;
        let looping =
            zone.loop_enabled && zone.loop_start < zone.loop_end && zone.loop_end <= length;
        self.start_voice(
            index,
            note,
            velocity,
            channel,
            zone.root_key,
            zone.sample_rate,
            RealtimeSample::Raw {
                left: zone.data,
                length,
                loop_start: zone.loop_start,
                loop_end: zone.loop_end,
                looping,
            },
        );
    }

    fn note_off(&mut self, note: u8, channel: u8) {
        let any_sustain = self.sustain.iter().any(|held| *held);
        for voice in &mut self.voices {
            if !voice.active || voice.note != note || (channel != 0xff && voice.channel != channel)
            {
                continue;
            }
            if if channel < 16 {
                self.sustain[channel as usize]
            } else {
                any_sustain
            } {
                voice.key_released = true;
            } else {
                voice.env_state = 4;
            }
        }
    }

    fn set_sustain(&mut self, channel: u8, held: bool) {
        if channel >= 16 || self.sustain[channel as usize] == held {
            return;
        }
        self.sustain[channel as usize] = held;
        if !held {
            for voice in &mut self.voices {
                if voice.active && voice.channel == channel && voice.key_released {
                    voice.key_released = false;
                    voice.env_state = 4;
                }
            }
        }
    }

    fn pitch_bend(&mut self, channel: u8, bend: f32) {
        if !bend.is_finite() {
            return;
        }
        let ratio = 2.0f64.powf(bend.clamp(-1.0, 1.0) as f64 * (2.0 / 12.0));
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel {
                voice.pitch_ratio = voice.base_pitch_ratio * ratio;
            }
        }
    }

    fn channel_expression(&mut self, channel: u8, controller: u8, value: u8) {
        if channel >= 16 {
            return;
        }
        let normalized = value as f32 / 127.0;
        if controller == 7 {
            self.volume[channel as usize] = normalized;
        } else if controller == 11 {
            self.expression[channel as usize] = normalized;
        }
    }

    fn channel_pressure(&mut self, channel: u8, value: u8) {
        if channel >= 16 {
            return;
        }
        let gain = 0.5 + 0.5 * (value as f32 / 127.0);
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel {
                voice.pressure_gain = gain;
            }
        }
    }

    fn voice_pressure(&mut self, channel: u8, note: u8, value: u8) {
        let gain = 0.5 + 0.5 * (value as f32 / 127.0);
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel && voice.note == note {
                voice.pressure_gain = gain;
            }
        }
    }

    fn render(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        let output_right_ptr = right
            .as_deref_mut()
            .map_or(std::ptr::null_mut(), |channel| channel.as_mut_ptr());
        left[..frames].fill(0.0);
        if !output_right_ptr.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(output_right_ptr, frames).fill(0.0);
            }
        }
        for index in 0..self.voices.len() {
            if !self.voices[index].active {
                continue;
            }
            let source = match self.voices[index].sample {
                RealtimeSample::None => continue,
                RealtimeSample::Raw {
                    left,
                    length,
                    loop_start,
                    loop_end,
                    looping,
                } => (left, left, length, loop_start, loop_end, looping),
                RealtimeSample::Buffer { context } => {
                    let Some(reader) = self.buffer_reader else {
                        continue;
                    };
                    let mut sample_left = std::ptr::null();
                    let mut sample_right = std::ptr::null();
                    let mut length = 0u32;
                    if !unsafe {
                        reader(
                            context as *mut c_void,
                            &mut sample_left,
                            &mut sample_right,
                            &mut length,
                        )
                    } || sample_left.is_null()
                        || length == 0
                    {
                        continue;
                    }
                    (
                        sample_left as usize,
                        sample_right as usize,
                        length,
                        0,
                        0,
                        false,
                    )
                }
            };
            for frame in 0..frames {
                let voice = &mut self.voices[index];
                if voice.env_state == 0 || !voice.active {
                    voice.active = false;
                    voice.env_state = 0;
                    voice.sample = RealtimeSample::None;
                    if self.free_count < self.free_stack.len() {
                        self.free_stack[self.free_count] = index;
                        self.free_count += 1;
                    }
                    break;
                }
                let mut position = if voice.position.is_finite() {
                    voice.position
                } else {
                    0.0
                };
                let (left_ptr, right_ptr, length, loop_start, loop_end, looping) = source;
                if looping && position >= loop_end as f64 {
                    let span = (loop_end - loop_start) as f64;
                    position = loop_start as f64 + (position - loop_start as f64).max(0.0) % span;
                    voice.position = position;
                } else if position.floor() as u64 >= length as u64 {
                    voice.active = false;
                    voice.env_state = 0;
                    voice.sample = RealtimeSample::None;
                    if self.free_count < self.free_stack.len() {
                        self.free_stack[self.free_count] = index;
                        self.free_count += 1;
                    }
                    break;
                }
                let sample_left = unsafe {
                    interpolate_sample(
                        left_ptr as *const f32,
                        length,
                        position,
                        looping,
                        loop_start,
                        loop_end,
                    )
                };
                let sample_right = unsafe {
                    interpolate_sample(
                        right_ptr as *const f32,
                        length,
                        position,
                        looping,
                        loop_start,
                        loop_end,
                    )
                };
                update_realtime_envelope(voice);
                let gain = voice.env_level
                    * voice.velocity
                    * self.volume[voice.channel as usize]
                    * self.expression[voice.channel as usize]
                    * voice.pressure_gain;
                let out_left = sample_left * gain;
                if out_left.is_finite() {
                    left[frame] += out_left;
                }
                if !output_right_ptr.is_null() {
                    let out_right = sample_right * gain;
                    if out_right.is_finite() {
                        unsafe {
                            *output_right_ptr.add(frame) += out_right;
                        }
                    }
                }
                let step = voice.pitch_ratio * voice.sample_rate_ratio;
                voice.position += step;
                if !voice.position.is_finite() {
                    voice.position = 0.0;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.sustain.fill(false);
        self.volume.fill(1.0);
        self.expression.fill(1.0);
        self.zone_round_robin.fill(0);
        for voice in &mut self.voices {
            *voice = RealtimeVoice::idle();
        }
        self.reset_free_voices();
    }
}

fn update_realtime_envelope(voice: &mut RealtimeVoice) {
    match voice.env_state {
        1 => {
            voice.env_level += 0.002;
            if voice.env_level >= 1.0 {
                voice.env_level = 1.0;
                voice.env_state = 2;
            }
        }
        2 => {
            voice.env_level -= 0.0005;
            if voice.env_level <= 0.8 {
                voice.env_level = 0.8;
                voice.env_state = 3;
            }
        }
        4 => {
            voice.env_level -= 0.001;
            if voice.env_level <= 0.0 {
                voice.env_level = 0.0;
                voice.active = false;
                voice.env_state = 0;
            }
        }
        _ => {}
    }
}

unsafe fn interpolate_sample(
    data: *const f32,
    length: u32,
    position: f64,
    looping: bool,
    loop_start: u32,
    loop_end: u32,
) -> f32 {
    if data.is_null() || length == 0 || !position.is_finite() {
        return 0.0;
    }
    let at = |raw: i64| {
        let index = if looping && loop_end > loop_start + 1 {
            let start = loop_start as i64;
            let end = loop_end as i64;
            let span = end - start;
            if raw < start {
                let index = end - ((start - raw) % span);
                if index == end {
                    start
                } else {
                    index
                }
            } else if raw >= end {
                start + ((raw - start) % span)
            } else {
                raw
            }
        } else {
            raw.clamp(0, length as i64 - 1)
        };
        let value = unsafe { *data.add(index as usize) };
        if value.is_finite() {
            value
        } else {
            0.0
        }
    };
    let base = position.floor() as i64;
    let t = (position - base as f64) as f32;
    let y0 = at(base - 1);
    let y1 = at(base);
    let y2 = at(base + 1);
    let y3 = at(base + 2);
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    let result = ((c3 * t + c2) * t + c1) * t + y1;
    if result.is_finite() {
        result
    } else {
        y1
    }
}

#[no_mangle]
pub extern "C" fn hirari_sampler_create(
    sample_rate: f64,
    reader: Option<SampleBufferReader>,
) -> *mut c_void {
    Box::into_raw(Box::new(RealtimeSampler::new(sample_rate, reader))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<RealtimeSampler>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_note_on_buffer(
    state: *mut c_void,
    note: u8,
    velocity: u8,
    buffer: *mut c_void,
    root: u8,
    source_rate: f64,
) {
    if state.is_null() || buffer.is_null() {
        return;
    }
    let sampler = &mut *state.cast::<RealtimeSampler>();
    let index = sampler.allocate_voice();
    sampler.start_voice(
        index,
        note,
        velocity,
        0,
        root,
        source_rate,
        RealtimeSample::Buffer {
            context: buffer as usize,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_add_zone(
    state: *mut c_void,
    data: *const f32,
    length: u64,
    source_rate: f64,
    root: u8,
    low_key: u8,
    high_key: u8,
    low_velocity: u8,
    high_velocity: u8,
    loop_start: u32,
    loop_end: u32,
    loop_enabled: bool,
) {
    if state.is_null() {
        return;
    }
    (*state.cast::<RealtimeSampler>()).add_zone(RealtimeZone {
        data: data as usize,
        length: length.min(usize::MAX as u64) as usize,
        sample_rate: source_rate,
        root_key: root,
        low_key,
        high_key,
        low_velocity,
        high_velocity,
        loop_start,
        loop_end,
        loop_enabled,
    });
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_note_on_zone(
    state: *mut c_void,
    note: u8,
    velocity: u8,
    channel: u8,
) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).note_on_zone(note, velocity, channel);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_note_on_raw(
    state: *mut c_void,
    note: u8,
    velocity: u8,
    data: *const f32,
    length: u32,
    root: u8,
    channel: u8,
    loop_start: u32,
    loop_end: u32,
    looping: bool,
    source_rate: f64,
) {
    if state.is_null() || data.is_null() || length == 0 {
        return;
    }
    let sampler = &mut *state.cast::<RealtimeSampler>();
    let index = sampler.allocate_voice();
    sampler.start_voice(
        index,
        note,
        velocity,
        channel,
        root,
        source_rate,
        RealtimeSample::Raw {
            left: data as usize,
            length,
            loop_start,
            loop_end,
            looping: looping && loop_start < loop_end && loop_end <= length,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_note_off(state: *mut c_void, note: u8, channel: u8) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).note_off(note, channel);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_set_sustain(state: *mut c_void, channel: u8, held: bool) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).set_sustain(channel, held);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_any_sustain(state: *const c_void) -> bool {
    !state.is_null()
        && (*state.cast::<RealtimeSampler>())
            .sustain
            .iter()
            .any(|held| *held)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_pitch_bend(state: *mut c_void, channel: u8, bend: f32) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).pitch_bend(channel, bend);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_channel_expression(
    state: *mut c_void,
    channel: u8,
    controller: u8,
    value: u8,
) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).channel_expression(channel, controller, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_channel_pressure(
    state: *mut c_void,
    channel: u8,
    value: u8,
) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).channel_pressure(channel, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_voice_pressure(
    state: *mut c_void,
    channel: u8,
    note: u8,
    value: u8,
) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).voice_pressure(channel, note, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let sampler = &mut *state.cast::<RealtimeSampler>();
    let left = std::slice::from_raw_parts_mut(left, frames as usize);
    let right = if right.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts_mut(right, frames as usize))
    };
    sampler.render(left, right);
}

/// Applies the MIDI 1.0 and MIDI 2.0 channel-voice events used by Sampler,
/// then renders one audio block. Sample offsets intentionally retain the
/// historical block-level behavior: the old C++ adapter applied all events
/// before rendering the block.
#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_process_midi_events(
    state: *mut c_void,
    events: *const SamplerMidiEvent,
    event_count: usize,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
) {
    if state.is_null() || left.is_null() || frames == 0 {
        return;
    }
    let sampler = &mut *state.cast::<RealtimeSampler>();
    if !events.is_null() {
        let events = std::slice::from_raw_parts(events, event_count.min(1024));
        for event in events {
            if event.size == 16 && (event.data[0] >> 4) == 0x4 {
                let status = event.data[1] & 0xf0;
                let channel = event.data[1] & 0x0f;
                let note = event.data[2] & 0x7f;
                let value = event.data[4];
                if status == 0x90 && value != 0 {
                    sampler.note_on_zone(note, value, channel);
                } else if status == 0x80 || (status == 0x90 && value == 0) {
                    sampler.note_off(note, channel);
                } else if status == 0xb0 && (event.data[2] & 0x7f) == 64 {
                    sampler.set_sustain(channel, value >= 64);
                } else if status == 0xb0
                    && ((event.data[2] & 0x7f) == 7 || (event.data[2] & 0x7f) == 11)
                {
                    sampler.channel_expression(channel, event.data[2] & 0x7f, value);
                } else if status == 0xa0 {
                    sampler.voice_pressure(channel, note, value);
                } else if status == 0xd0 {
                    sampler.channel_pressure(channel, value);
                } else if status == 0xe0 {
                    sampler.pitch_bend(channel, (value as f32 - 128.0) / 128.0);
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
                sampler.note_on_zone(pitch, velocity, channel);
            } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                sampler.note_off(pitch, channel);
            } else if status == 0xb0 && pitch == 64 {
                sampler.set_sustain(channel, velocity >= 64);
            } else if status == 0xb0 && (pitch == 7 || pitch == 11) {
                sampler.channel_expression(channel, pitch, velocity);
            } else if status == 0xa0 && event.size >= 3 {
                sampler.voice_pressure(channel, pitch, velocity);
            } else if status == 0xd0 {
                sampler.channel_pressure(channel, pitch);
            } else if status == 0xe0 && event.size >= 3 {
                let value = ((event.data[2] as i32 & 0x7f) << 7) | (event.data[1] as i32 & 0x7f);
                sampler.pitch_bend(channel, (value - 8192) as f32 / 8192.0);
            }
        }
    }
    let left = std::slice::from_raw_parts_mut(left, frames as usize);
    let right = if right.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts_mut(right, frames as usize))
    };
    sampler.render(left, right);
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_prepare(state: *mut c_void, sample_rate: f64) {
    if state.is_null() {
        return;
    }
    let sampler = &mut *state.cast::<RealtimeSampler>();
    if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
        sampler.sample_rate = sample_rate;
    }
    sampler.reset();
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_reset(state: *mut c_void) {
    if !state.is_null() {
        (*state.cast::<RealtimeSampler>()).reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_streaming_count(state: *const c_void) -> usize {
    if state.is_null() {
        0
    } else {
        (*state.cast::<RealtimeSampler>())
            .voices
            .iter()
            .filter(|voice| voice.active)
            .count()
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_sampler_streaming_at(state: *const c_void, index: usize) -> bool {
    if state.is_null() {
        return false;
    }
    (*state.cast::<RealtimeSampler>())
        .voices
        .iter()
        .filter(|voice| voice.active)
        .nth(index)
        .is_some_and(|voice| voice.is_streaming)
}

#[cfg(all(test, feature = "dsp-differential-reference"))]
#[path = "sampler_differential_tests.rs"]
mod differential_tests;

/// Audio owned by the sampler.  The right channel may be omitted for mono data.
pub struct SampleData {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub loop_start: usize,
    pub loop_end: usize,
    pub looping: bool,
    /// Native rate of the asset; zero uses the engine/project rate.
    pub source_sample_rate: f64,
}

pub struct SamplerEngineEngine {
    pub sample_rate: f64,
    pub voices: Vec<Voice>,
    pub free_voices: Vec<usize>,
    pub sample: Option<SampleData>,
}

impl SamplerEngineEngine {
    pub fn new(sample_rate: f64) -> Self {
        let mut voices = Vec::with_capacity(64);
        let mut free_voices = Vec::with_capacity(64);
        for i in 0..64 {
            voices.push(Voice {
                active: false,
                position: 0.0,
                pitch_ratio: 1.0,
                sample_rate_ratio: 1.0,
                velocity: 1.0,
                env_level: 0.0,
                env_state: 0,
                note: 0,
                is_streaming: false,
                brightness: 1.0,
            });
            free_voices.push(i);
        }
        Self {
            sample_rate,
            voices,
            free_voices,
            sample: None,
        }
    }

    pub fn set_sample(&mut self, sample: Option<SampleData>) {
        self.sample = sample.filter(|sample| {
            !sample.left.is_empty()
                && sample.left.len() <= 64 * 1024 * 1024
                && (sample.right.is_empty() || sample.right.len() == sample.left.len())
                && sample.loop_start <= sample.loop_end
                && sample.loop_end <= sample.left.len()
                && (sample.source_sample_rate == 0.0
                    || (sample.source_sample_rate.is_finite()
                        && (8_000.0..=384_000.0).contains(&sample.source_sample_rate)))
        });
    }

    pub fn note_on(&mut self, note: u8, velocity: u8, root_note: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        let voice_idx = if let Some(idx) = self.free_voices.pop() {
            idx
        } else {
            // Priority-based stealing
            self.find_voice_to_steal()
        };

        self.start_voice(voice_idx, note, velocity, root_note);
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.active && v.note == note {
                v.env_state = 4;
            }
        }
    }

    /// INDUSTRIAL: Processes an audio block with Multi-Voice Sample Playback.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let num_samples = l.len().min(r.len());

        for v_idx in 0..self.voices.len() {
            let v = &mut self.voices[v_idx];
            if !v.active {
                continue;
            }

            // LOD Threshold: Professional quality vs Performance
            let use_high_quality = (v.env_level * v.velocity > 0.15) || (v.brightness > 0.8);

            for s in 0..num_samples {
                self.update_envelope(v_idx);
                let v = &self.voices[v_idx]; // Re-borrow after mutation
                if !v.active {
                    self.free_voices.push(v_idx);
                    break;
                }

                let (val_l, val_r) = self.sample.as_ref().map_or((0.0, 0.0), |sample| {
                    let position = if v.position.is_finite() {
                        v.position
                    } else {
                        0.0
                    };
                    let l = Self::interpolate(&sample.left, position, sample, use_high_quality);
                    let r_source = if sample.right.is_empty() {
                        &sample.left
                    } else {
                        &sample.right
                    };
                    let r = Self::interpolate(r_source, position, sample, use_high_quality);
                    (l, r)
                });

                let master_gain = v.velocity * v.env_level;
                l[s] = ((if l[s].is_finite() { l[s] } else { 0.0 }) + val_l * master_gain)
                    .clamp(-1.0e6, 1.0e6);
                r[s] = ((if r[s].is_finite() { r[s] } else { 0.0 }) + val_r * master_gain)
                    .clamp(-1.0e6, 1.0e6);

                let v_mut = &mut self.voices[v_idx];
                let step = if v_mut.pitch_ratio.is_finite() && v_mut.sample_rate_ratio.is_finite() {
                    v_mut.pitch_ratio * v_mut.sample_rate_ratio
                } else {
                    0.0
                };
                v_mut.position += step.max(0.0);
                if !v_mut.position.is_finite() {
                    v_mut.position = 0.0;
                }
            }
        }
    }

    fn interpolate(data: &[f32], position: f64, sample: &SampleData, hermite: bool) -> f32 {
        if data.is_empty() || !position.is_finite() {
            return 0.0;
        }
        let len = data.len();
        let loop_start = sample.loop_start.min(len.saturating_sub(1));
        let loop_end = sample.loop_end.min(len);
        let valid_loop = sample.looping && loop_end > loop_start + 1;
        let mut p = position;
        if valid_loop && p >= loop_end as f64 {
            p = loop_start as f64
                + (p - loop_start as f64).rem_euclid((loop_end - loop_start) as f64);
        }
        if p < 0.0 || p >= len as f64 {
            return 0.0;
        }
        let i = p.floor() as usize;
        let frac = (p - i as f64) as f32;
        let at = |index: isize| -> f32 {
            let idx = if valid_loop && index >= loop_end as isize {
                loop_start + (index - loop_start as isize) as usize % (loop_end - loop_start)
            } else if index < 0 {
                0
            } else {
                (index as usize).min(len - 1)
            };
            let value = data[idx];
            if value.is_finite() {
                value
            } else {
                0.0
            }
        };
        let a = at(i as isize);
        let b = at(i as isize + 1);
        if !hermite {
            return a + (b - a) * frac;
        }
        let y0 = at(i as isize - 1);
        let y1 = a;
        let y2 = b;
        let y3 = at(i as isize + 2);
        let c0 = y1;
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        (c0 + frac * (c1 + frac * (c2 + frac * c3))).clamp(-1.0e6, 1.0e6)
    }

    fn find_voice_to_steal(&self) -> usize {
        let mut best_idx = 0;
        let mut min_level = 2.0f32;
        for i in 0..self.voices.len() {
            if self.voices[i].env_level < min_level {
                min_level = self.voices[i].env_level;
                best_idx = i;
            }
        }
        best_idx
    }

    fn start_voice(&mut self, idx: usize, note: u8, velocity: u8, root_note: u8) {
        let v = &mut self.voices[idx];
        v.active = true;
        v.note = note;
        v.velocity = velocity as f32 / 127.0;
        v.position = 0.0;
        v.pitch_ratio = 2.0f64.powf((note as f64 - root_note as f64) / 12.0);
        v.sample_rate_ratio = self
            .sample
            .as_ref()
            .map(|sample| {
                if sample.source_sample_rate > 0.0 && self.sample_rate > 0.0 {
                    sample.source_sample_rate / self.sample_rate
                } else {
                    1.0
                }
            })
            .unwrap_or(1.0);
        v.env_state = 1;
        v.env_level = 0.0;
    }

    fn update_envelope(&mut self, idx: usize) {
        let v = &mut self.voices[idx];
        let attack_step = 0.002f32;
        let release_step = 0.001f32;
        let sustain_level = 0.8f32;

        match v.env_state {
            1 => {
                v.env_level += attack_step;
                if v.env_level >= 1.0 {
                    v.env_level = 1.0;
                    v.env_state = 2;
                }
            }
            2 => {
                v.env_level -= 0.0005;
                if v.env_level <= sustain_level {
                    v.env_level = sustain_level;
                    v.env_state = 3;
                }
            }
            4 => {
                v.env_level -= release_step;
                if v.env_level <= 0.0 {
                    v.env_level = 0.0;
                    v.active = false;
                    v.env_state = 0;
                }
            }
            _ => {}
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide Sampler state.
    pub fn audit_sampler_engine(&self) -> bool {
        let free_voice_ledger_valid = self.free_voices.len() <= self.voices.len()
            && self
                .free_voices
                .iter()
                .enumerate()
                .all(|(position, &voice_idx)| {
                    voice_idx < self.voices.len()
                        && self.voices[voice_idx].active == false
                        && self.free_voices[..position]
                            .iter()
                            .all(|&prior| prior != voice_idx)
                })
            && self.voices.iter().enumerate().all(|(voice_idx, voice)| {
                voice.active
                    || self
                        .free_voices
                        .iter()
                        .any(|&free_idx| free_idx == voice_idx)
            });

        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.voices.len() == 64
            && free_voice_ledger_valid
            && self.voices.iter().all(|voice| {
                voice.position.is_finite()
                    && voice.pitch_ratio.is_finite()
                    && voice.sample_rate_ratio.is_finite()
                    && (0.0..=64.0).contains(&voice.sample_rate_ratio)
                    && voice.velocity.is_finite()
                    && voice.env_level.is_finite()
                    && voice.brightness.is_finite()
                    && voice.env_level >= 0.0
                    && voice.env_level <= 1.0
            })
            && self.sample.as_ref().is_none_or(|sample| {
                !sample.left.is_empty()
                    && (sample.right.is_empty() || sample.right.len() == sample.left.len())
                    && sample.loop_start <= sample.loop_end
                    && sample.loop_end <= sample.left.len()
                    && (sample.source_sample_rate == 0.0
                        || (sample.source_sample_rate.is_finite()
                            && (8_000.0..=384_000.0).contains(&sample.source_sample_rate)))
                    && sample.left.iter().all(|value| value.is_finite())
                    && sample.right.iter().all(|value| value.is_finite())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_sample() -> SampleData {
        SampleData {
            left: vec![1.0, 0.5, 0.25, 0.0],
            right: vec![],
            loop_start: 0,
            loop_end: 0,
            looping: false,
            source_sample_rate: 0.0,
        }
    }

    #[test]
    fn renders_loaded_sample_data() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        engine.set_sample(Some(impulse_sample()));
        engine.note_on(60, 127, 60);
        let mut left = vec![0.0; 16];
        let mut right = vec![0.0; 16];
        engine.process(&mut left, &mut right);

        assert!(left.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(left, right);
    }

    #[test]
    fn source_sample_rate_is_applied_to_voice_step() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        let mut sample = impulse_sample();
        sample.source_sample_rate = 24_000.0;
        engine.set_sample(Some(sample));
        engine.note_on(60, 127, 60);
        assert!((engine.voices[63].sample_rate_ratio - 0.5).abs() < f64::EPSILON);
        assert!(engine.audit_sampler_engine());

        let mut invalid = impulse_sample();
        invalid.source_sample_rate = 1_000.0;
        engine.set_sample(Some(invalid));
        assert!(engine.sample.is_none());
    }

    #[test]
    fn audit_rejects_corrupt_free_voice_ledger() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        assert!(engine.audit_sampler_engine());

        engine.free_voices.push(0);
        assert!(!engine.audit_sampler_engine());

        engine.free_voices.pop();
        engine.free_voices[0] = 64;
        assert!(!engine.audit_sampler_engine());

        engine.free_voices[0] = 0;
        engine.voices[0].active = true;
        assert!(!engine.audit_sampler_engine());
    }

    #[test]
    fn non_looping_sample_falls_silent_after_end() {
        let mut engine = SamplerEngineEngine::new(48_000.0);
        engine.set_sample(Some(impulse_sample()));
        engine.note_on(60, 127, 60);
        let mut first = vec![0.0; 4];
        let mut right = vec![0.0; 4];
        engine.process(&mut first, &mut right);
        let mut tail = vec![0.0; 8];
        let mut tail_right = vec![0.0; 8];
        engine.process(&mut tail, &mut tail_right);

        assert!(tail.iter().all(|sample| sample.abs() < 1.0e-6));
    }
}
use std::ffi::c_void;
