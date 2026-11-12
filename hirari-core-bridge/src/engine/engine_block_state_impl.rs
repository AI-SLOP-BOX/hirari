//! Engine block state edit ops, split out of engine_block_host.rs.
use super::*;

impl EngineBlockState {
    /// The device format the graph was last prepared for. The engine used to hold
    /// its own copy and pass it in as an argument on every block.
    pub(super) fn sample_rate(&self) -> f64 {
        f64::from_bits(self.device_sample_rate.load(Ordering::Relaxed))
    }

    pub(super) fn storage(&self, track: *mut c_void, kind: u32) -> *mut c_void {
        unsafe { hirari_track_storage(track, kind) }
    }

    pub(super) fn track_type(&self, track: *mut c_void) -> u8 {
        unsafe { hirari_track_type_byte(track) }
    }

    pub(super) fn runtime(&self, track: *mut c_void) -> Option<&'static TrackRuntime> {
        track_runtime::runtime(self.storage(track, STORAGE_RUNTIME))
    }

    pub(super) fn bus_state(&self, track: *mut c_void) -> *mut c_void {
        if self.track_type(track) != TRACK_TYPE_BUS {
            return ptr::null_mut();
        }
        self.storage(track, STORAGE_BUS_STATE)
    }

    pub(super) fn solo(&self, track: *mut c_void) -> bool {
        unsafe { hirari_channel_strip_is_solo(self.storage(track, STORAGE_STRIP)) }
    }

    pub(super) fn record(&self, track: *mut c_void, playhead: u64) {
        unsafe { hirari_track_record_state(track, playhead) };
    }

    pub(super) fn click(&self, side: usize) -> *mut f32 {
        let scratch = unsafe { &mut *self.click };
        scratch[side].as_mut_ptr()
    }

    /// The stereo master window the device callback owns, or null when the
    /// declared block does not fit the buffer holding it.
    pub(super) fn window(
        &self,
        output: *mut c_void,
        off: u32,
        frames: u32,
    ) -> Option<(*mut f32, *mut f32, *const *mut f32, u32)> {
        let (planes, count, capacity) = track_runtime::channels_of(output);
        if planes.is_null() || count < 2 || capacity < off.checked_add(frames)? {
            return None;
        }
        let planes = unsafe { slice::from_raw_parts(planes, count as usize) };
        if planes[0].is_null() || planes[1].is_null() {
            return None;
        }
        let offset = off as usize;
        Some((
            unsafe { planes[0].add(offset) },
            unsafe { planes[1].add(offset) },
            planes.as_ptr(),
            count,
        ))
    }

    // ---- the block head: what the callback reads from a Track ----------

    /// The graph window the callback traverses: the published snapshot the Rust
    /// track table double-buffers, read without a lock exactly as the C++
    /// `trackView()` did. Publication only fills the buffer this is not reading.
    pub(super) fn view(&self) -> View {
        let mut published = HirariTrackSnapshot::default();
        unsafe { hirari_engine_track_snapshot(self.track_table, &mut published) };
        View {
            tracks: published.active_tracks,
            map: published.track_map,
            count: published.track_count,
            generation: published.audio_config_generation,
        }
    }

    /// Silences the window the device callback owns, the whole buffer's
    /// channel set, exactly as `output.clear(off, frames)` did.
    pub(super) fn clear_window(planes: *const *mut f32, count: u32, off: usize, frames: usize) {
        if planes.is_null() || frames == 0 {
            return;
        }
        for index in 0..count as usize {
            let plane = unsafe { *planes.add(index) };
            if !plane.is_null() {
                unsafe { ptr::write_bytes(plane.add(off), 0, frames) };
            }
        }
    }

    pub(super) fn midi_input(&self, track: *mut c_void) -> bool {
        matches!(self.track_type(track), TRACK_TYPE_MIDI | TRACK_TYPE_INSTRUMENT)
    }

    pub(super) fn articulation(&self, track: *mut c_void) -> *mut c_void {
        self.storage(track, STORAGE_ARTICULATION)
    }

    pub(super) fn plugin_midi(&self, track: *mut c_void) -> *mut c_void {
        self.storage(track, STORAGE_PLUGIN_MIDI)
    }

    /// One slot of the held preview-voice table, or null when the pair is out of
    /// range for a malformed scheduled note.
    pub(super) fn held_note(&self, channel: u8, pitch: u8) -> *mut u32 {
        let channel = usize::from(channel);
        let pitch = usize::from(pitch);
        if channel >= FALLBACK_CHANNELS || pitch >= FALLBACK_PITCHES {
            return ptr::null_mut();
        }
        let table = unsafe { &mut *self.fallback_notes };
        unsafe { table[channel].as_mut_ptr().add(pitch) }
    }

    /// Copies one note's generated events into the track's own input buffer, the
    /// same expansion the C++ wrapper performed for every MIDI input.
    pub(super) fn push_generated(&self, midi: *mut c_void, events: &[GeneratedMidiEvent]) {
        if midi.is_null() {
            return;
        }
        for event in events {
            unsafe {
                hirari_midi_buffer_add(
                    midi,
                    event.sample_offset,
                    event.data.as_ptr(),
                    u32::from(event.size),
                    event.articulation_id,
                )
            };
        }
    }

    pub(super) fn clear_midi(&self, track: *mut c_void) {
        let midi = self.plugin_midi(track);
        if !midi.is_null() {
            unsafe { hirari_midi_buffer_clear(midi) };
        }
    }

    pub(super) fn note_on(
        &self,
        track: *mut c_void,
        channel: u8,
        pitch: u8,
        velocity: u8,
        offset: u64,
        articulation: u8,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_GENERATED_EVENTS];
        let count = unsafe {
            hirari_track_midi_note_on(
                state,
                channel,
                pitch,
                velocity,
                offset,
                articulation,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        self.push_generated(self.plugin_midi(track), &events[..count]);
    }

    pub(super) fn note_off(
        &self,
        track: *mut c_void,
        channel: u8,
        pitch: u8,
        offset: u64,
        articulation: u8,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_GENERATED_EVENTS];
        let count = unsafe {
            hirari_track_midi_note_off(
                state,
                channel,
                pitch,
                offset,
                articulation,
                events.as_mut_ptr(),
                events.len(),
            )
        };
        self.push_generated(self.plugin_midi(track), &events[..count]);
    }

    pub(super) fn all_notes_off(&self, track: *mut c_void, offset: u64) {
        let state = self.articulation(track);
        let midi = self.plugin_midi(track);
        if state.is_null() || midi.is_null() {
            return;
        }
        let mut events = [GeneratedMidiEvent::default(); MAX_MIDI_EVENTS];
        let capacity = unsafe { hirari_midi_buffer_remaining_capacity(midi) }.min(events.len());
        let count = unsafe {
            hirari_track_midi_all_notes_off(
                state,
                offset,
                if capacity == 0 {
                    ptr::null_mut()
                } else {
                    events.as_mut_ptr()
                },
                capacity,
            )
        };
        self.push_generated(midi, &events[..count]);
    }

    /// Emits one articulation's timed key-switch note-offs, mapped through the
    /// engine's tempo map at the block's sample rate. `transition` selects the
    /// pre-Note-On set, which the instrument must release before the key switch
    /// itself sounds.
    pub(super) fn schedule_key_switches(
        &self,
        track: *mut c_void,
        articulation: u8,
        transition: bool,
        start_beat: f64,
        sample_rate: f64,
        offset: u64,
    ) {
        let state = self.articulation(track);
        if state.is_null() {
            return;
        }
        let mut outputs = [TimedKeySwitchOutput::default(); 16];
        let count = unsafe {
            hirari_track_midi_timed_outputs(
                state,
                articulation,
                transition,
                outputs.as_mut_ptr(),
                outputs.len(),
            )
        };
        let midi = self.plugin_midi(track);
        for output in &outputs[..count] {
            let end_beat = start_beat + f64::from(output.length_ticks) / 960.0;
            let end_sample =
                unsafe { hirari_tempo_map_beats_to_samples(self.tempo, end_beat, sample_rate) };
            if !unsafe {
                hirari_track_midi_schedule_note_off(state, output.channel, output.pitch, end_sample)
            } {
                // An overloaded schedule must still release the physical key.
                if !midi.is_null() {
                    unsafe {
                        hirari_midi_buffer_add_note_off(midi, output.channel + 1, output.pitch, offset)
                    };
                }
            }
        }
    }

    /// Releases every voice the callback held: the built-in synth, the fallback
    /// counts and each instrument's held keys.
    pub(super) fn reset_preview_voices(&self, view: View) {
        unsafe { hirari_preview_synth_reset(self.preview_synth) };
        let table = unsafe { &mut *self.fallback_notes };
        for channel in table.iter_mut() {
            channel.fill(0);
        }
        for &track in view.active() {
            if !track.is_null() && self.midi_input(track) {
                self.all_notes_off(track, 0);
            }
        }
    }

    /// Applies the commands that arrived while the callback was rendering. A
    /// command that waited in the queue through a device rebuild never applies
    /// to a different audio configuration.
    pub(super) fn drain_commands(&self, generation: u64, view: View) {
        let rings = self.rings.cast::<EngineRings>();
        let mut record = CommandRecord::default();
        while unsafe { hirari_engine_command_ring_pop(rings, &mut record) } {
            if record.expected_audio_generation != generation {
                continue;
            }
            let track = view.track_id(record.tid);
            if track.is_null() {
                continue;
            }
            let mut value = 0.0_f32;
            let command = unsafe { hirari_engine_audio_command(record.type_, record.val, &mut value) };
            if matches!(
                command,
                AUDIO_COMMAND_VOLUME | AUDIO_COMMAND_PAN | AUDIO_COMMAND_MUTE | AUDIO_COMMAND_SOLO
            ) {
                self.apply_scalar(track, command, value);
            }
        }
    }

    /// Writes one drained command to the track's own storage under the control
    /// edit protocol the track keeps: the fader, pan and solo live on the
    /// channel strip, mute on the runtime.
    pub(super) fn apply_scalar(&self, track: *mut c_void, command: u32, value: f32) {
        unsafe { hirari_track_begin_edit(track) };
        let strip = self.storage(track, STORAGE_STRIP);
        match command {
            AUDIO_COMMAND_VOLUME => unsafe { hirari_channel_strip_set_gain(strip, value) },
            AUDIO_COMMAND_PAN => unsafe { hirari_channel_strip_set_pan(strip, value) },
            AUDIO_COMMAND_MUTE => unsafe {
                hirari_track_runtime_set_muted(
                    self.storage(track, STORAGE_RUNTIME),
                    u8::from(value >= 0.5),
                )
            },
            _ => unsafe { hirari_channel_strip_set_solo(strip, value >= 0.5) },
        }
        unsafe { hirari_track_end_edit(track) };
    }

    /// Hands the timeline's scheduled notes to the graph: a note addressed to a
    /// live track enters that track's input, an orphan note feeds the built-in
    /// preview, and a transport discontinuity releases every held voice.
    pub(in crate::engine_block_host) unsafe fn stage_midi(
        &self,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
        playback_active: bool,
        view: View,
    ) {
        let mut stage = MidiStage {
            state: self,
            view,
            sample_rate,
        };
        let context = ptr::from_mut(&mut stage).cast();
        unsafe {
            hirari_midi_playback_prepare_block(
                self.midi_clock,
                self.midi_snapshots,
                playhead,
                frames,
                u8::from(playback_active),
                context,
                Some(stage_panic),
                Some(stage_note),
            )
        };
        // Preview dispatch reads the block in timestamp order, same-sample events
        // in the order they were scheduled.
        unsafe { hirari_midi_buffer_sort_owned(self.block_midi) };
    }

    /// Runs the native MIDI policy over the staged block, then the built-in
    /// voices that make playback audible without a plug-in.
    pub(in crate::engine_block_host) unsafe fn render_preview(
        &self,
        left: *mut f32,
        right: *mut f32,
        frames: u32,
        playhead: u64,
        sample_rate: f64,
    ) {
        let events = unsafe { hirari_midi_buffer_event_data(self.block_midi) };
        let count = unsafe { hirari_midi_buffer_event_count(self.block_midi) };
        let mpe = self.mpe.cast::<MpeState>();
        unsafe {
            hirari_midi_process_mpe(mpe, events, count);
            hirari_midi_apply_articulations_from_state(mpe.cast_const(), events, count);
        }
        let rate = sample_rate.max(1.0);
        unsafe {
            hirari_preview_synth_process(
                self.preview_synth,
                events,
                count,
                left,
                right,
                frames,
                playhead,
                rate,
                self.macro_control,
            );
            hirari_preview_sampler_render(self.preview_sampler, rate, left, right, frames);
        }
        if self.test_tone.load(Ordering::Relaxed) {
            unsafe { hirari_audio_test_tone_render(left, right, frames, playhead, sample_rate) };
        }
    }
}
