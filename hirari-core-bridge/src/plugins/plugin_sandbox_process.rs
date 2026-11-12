//! Parameter/MIDI staging and quarantine state machines for the isolated
//! plugin worker, split out of plugin_sandbox_host.rs.
use super::*;

impl SandboxHost {
    pub(in crate::plugin_sandbox_host) fn remember_parameter(&self, parameter_id: u32, value: f64) -> bool {
        self.parameter_cache.set(parameter_id, value)
    }

    pub(in crate::plugin_sandbox_host) fn parameter_value(&self, parameter_id: u32, value: &mut f64) -> bool {
        match self.parameter_cache.get(parameter_id) {
            Some(found) => {
                *value = found;
                true
            }
            None => false,
        }
    }

    pub(in crate::plugin_sandbox_host) fn set_generation_context(
        &self,
        project: u64,
        plugin: u64,
        audio: u64,
        state: u64,
    ) -> bool {
        let context = Generation {
            project,
            plugin,
            audio,
            state,
        };
        if !context.valid() {
            return false;
        }
        let _control = lock_control(&self.control);
        let mut state_lock = lock_control(&self.state);
        state_lock.generation = context;
        true
    }

    pub(in crate::plugin_sandbox_host) fn is_quarantined(&self) -> bool {
        self.failure() == FAILURE_PLUGIN_QUARANTINED
    }

        pub(in crate::plugin_sandbox_host) fn clear_quarantine(&self) {
        let mut control = lock_control(&self.control);
        if self.is_quarantined() {
            control.restart_attempts = 0;
            self.failure.store(FAILURE_NONE, Ordering::Release);
        }
    }

    pub(in crate::plugin_sandbox_host) fn can_retry(&self) -> bool {
        matches!(
            self.failure(),
            FAILURE_PLUGIN_QUARANTINED
                | FAILURE_PROCESS_EXITED
                | FAILURE_PROCESS_HUNG
                | FAILURE_PROCESS_FAILED
                | FAILURE_PLUGIN_LOAD_FAILED
                | FAILURE_PLUGIN_ABI_INVALID
                | FAILURE_PLUGIN_INSTANCE_FAILED
        )
    }

    pub(in crate::plugin_sandbox_host) fn can_auto_retry(&self) -> bool {
        let failure = self.failure();
        failure != FAILURE_NONE && failure != FAILURE_PLUGIN_QUARANTINED && self.can_retry()
    }

        pub(in crate::plugin_sandbox_host) fn enqueue_parameter_change(&self, parameter_id: u32, value: f64, sample_offset: u32) -> bool {
        if !value.is_finite() || sample_offset as usize > MAX_FRAMES {
            return false;
        }
        let write = self.parameters.write.load(Ordering::Relaxed);
        let read = self.parameters.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= MAX_PARAMETER_CHANGES as u64 {
            return false;
        }
        let slot = (write % MAX_PARAMETER_CHANGES as u64) as usize;
        // SAFETY: The engine has a single producer for this queue; the index
        // was published after the previous slot write.
        unsafe {
            (*self.parameters.slots.get())[slot] = WireParameterChange {
                parameter_id,
                sample_offset,
                value,
            };
        }
        self.parameters
            .write
            .store(write.wrapping_add(1), Ordering::Release);
        true
    }

    pub(in crate::plugin_sandbox_host) fn reset_staged_midi(&self) {
        let Some(_access) = self.acquire_audio_access() else {
            return;
        };
        self.audio_state_mut().pending_midi_count = 0;
    }

    pub(in crate::plugin_sandbox_host) fn stage_input_midi(
        &self,
        sample_offset: u64,
        articulation_id: u8,
        data: &[u8],
        frames: u32,
    ) -> bool {
        let Some(_audio_access) = self.acquire_audio_access() else {
            return false;
        };
        let Some(access) = self.acquire_shared_access() else {
            return false;
        };
        // SAFETY: The access guard keeps the mapping alive until this event is
        // staged and prevents a state transition from overlapping the write.
        let shared = unsafe { &mut *access.ptr };
        let completed = shared.completed_sequence.load(Ordering::Acquire);
        let audio_state = self.audio_state();
        if audio_state.last_submitted != 0
            && shared.request_sequence.load(Ordering::Acquire) != completed
        {
            return false;
        }
        if sample_offset >= frames as u64 {
            shared
                .input_midi_truncations
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let audio = self.audio_state_mut();
        if data.len() > MAX_MIDI_PAYLOAD_BYTES {
            let ring = std::ptr::addr_of_mut!((*shared).extended_midi.bytes).cast::<c_void>();
            let ok = unsafe {
                hirari_midi_extended_ring_push(
                    ring,
                    sample_offset,
                    articulation_id,
                    data.as_ptr(),
                    data.len(),
                )
            };
            if !ok {
                shared
                    .input_midi_truncations
                    .fetch_add(1, Ordering::Relaxed);
            }
            return ok;
        }
        if audio.pending_midi_count as usize >= MAX_MIDI_EVENTS {
            shared
                .input_midi_truncations
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let event = &mut audio.pending_midi[audio.pending_midi_count as usize];
        event.sample_offset = sample_offset;
        event.size = data.len() as u32;
        event.articulation_id = articulation_id;
        event.data[..data.len()].copy_from_slice(data);
        audio.pending_midi_count += 1;
        true
    }

    pub(in crate::plugin_sandbox_host) fn process(&self, left: &mut [f32], right: &mut [f32], channels: u32, frames: u32) -> u8 {
        const PRODUCED: u8 = 1;
        const CLEAR_MIDI: u8 = 2;
        self.last_process_overrun.store(false, Ordering::Release);
        let Some(_audio_access) = self.acquire_audio_access() else {
            return CLEAR_MIDI;
        };
        let Some(_shared_access) = self.acquire_shared_access() else {
            return CLEAR_MIDI;
        };
        self.process_locked(left, right, channels, frames, PRODUCED, CLEAR_MIDI)
    }

    pub(in crate::plugin_sandbox_host) fn process_locked(
        &self,
        left: &mut [f32],
        right: &mut [f32],
        channels: u32,
        frames: u32,
        produced_flag: u8,
        clear_midi_flag: u8,
    ) -> u8 {
        self.audio_state_mut().output_midi_count = 0;
        if !self.is_alive() || self.state_transition.load(Ordering::Acquire) {
            return clear_midi_flag;
        }
        let shared_ptr = self.shared();
        if shared_ptr.is_null() {
            return clear_midi_flag;
        }
        let frames_usize = frames as usize;
        let channels_usize = channels as usize;
        if channels == 0
            || channels > self.configured_channels.load(Ordering::Acquire)
            || channels_usize > MAX_CHANNELS
            || frames == 0
            || frames_usize > MAX_FRAMES
            || left.len() < frames_usize
            || (channels_usize > 1 && right.len() < frames_usize)
        {
            return clear_midi_flag;
        }
        // SAFETY: Liveness was checked and the mapping outlives this call.
        let shared = unsafe { &mut *shared_ptr };
        let audio = self.audio_state_mut();
        let completed = shared.completed_sequence.load(Ordering::Acquire);
        let request_in_flight = audio.last_submitted != 0
            && shared.request_sequence.load(Ordering::Acquire) != completed;
        if request_in_flight {
            // This call cannot stage a new block. The stale pending audio is
            // resubmitted after collection (matching the one-block pipeline),
            // but MIDI and parameter automation belong to the rejected call
            // and must not be replayed with it.
            audio.pending_midi_count = 0;
            audio.pending_parameter_count = 0;
        } else {
            for channel in 0..channels_usize {
                let source = if channel == 0 { &*left } else { &*right };
                audio.pending_input[channel][..frames_usize]
                    .copy_from_slice(&source[..frames_usize]);
            }
            audio.pending_channels = channels;
            audio.pending_frames = frames;
            let write = self.parameters.write.load(Ordering::Acquire);
            let read = self.parameters.read.load(Ordering::Relaxed);
            let available = write.wrapping_sub(read);
            let count = available.min(MAX_PARAMETER_CHANGES as u64) as u32;
            for index in 0..count as usize {
                let slot = ((read + index as u64) % MAX_PARAMETER_CHANGES as u64) as usize;
                // SAFETY: Single-consumer queue; the producer publishes the
                // write index with Release after filling each slot.
                let change = unsafe { (*self.parameters.slots.get())[slot] };
                shared.parameter_change[index] = change;
            }
            self.parameters
                .read
                .store(read.wrapping_add(count as u64), Ordering::Release);
            audio.pending_parameter_count = count;
        }

        let mut produced = false;
        if audio.last_submitted != 0 && completed == audio.last_submitted {
            for channel in 0..channels_usize {
                let destination = if channel == 0 { &mut *left } else { &mut *right };
                destination[..frames_usize]
                    .copy_from_slice(&shared.output.samples[channel][..frames_usize]);
            }
            let output_count = shared
                .output_midi_events
                .load(Ordering::Acquire)
                .min(MAX_MIDI_EVENTS as u32);
            let mut kept = 0u32;
            for index in 0..output_count as usize {
                let event = &shared.output_midi[index];
                if (event.sample_offset as u32) < frames
                    && event.size > 0
                    && event.size as usize <= MAX_MIDI_PAYLOAD_BYTES
                {
                    audio.output_midi[kept as usize] = *event;
                    kept += 1;
                }
            }
            audio.output_midi_count = kept;
            produced = true;
            audio.completed_sequence_had_overrun = audio.sequence_had_overrun;
            audio.sequence_had_overrun = false;
            audio.last_submitted = 0;
            audio.pending_age_frames = 0;
            audio.overrun_reported_for_sequence = false;
            if shared.process_errors.swap(0, Ordering::AcqRel) > 0 {
                self.failure
                    .store(FAILURE_PROCESS_FAILED, Ordering::Release);
                return 0;
            }
        }

        if shared.request_sequence.load(Ordering::Acquire) != completed {
            // Audio-thread deadline accounting is expressed in frames rather
            // than wall-clock calls; hang detection stays on pollHealth().
            let deadline_frames = (self.sample_rate() * 0.005).max(1.0);
            audio.pending_age_frames = audio.pending_age_frames.saturating_add(frames as u64);
            if !audio.overrun_reported_for_sequence
                && audio.pending_age_frames as f64 >= deadline_frames
            {
                shared.mailbox_overruns.fetch_add(1, Ordering::Relaxed);
                self.last_process_overrun.store(true, Ordering::Release);
                audio.sequence_had_overrun = true;
                audio.overrun_reported_for_sequence = true;
            }
            return if produced { produced_flag } else { 0 };
        }

        let submit_channels = audio.pending_channels;
        let submit_frames = audio.pending_frames;
        if submit_channels == 0
            || submit_channels > MAX_CHANNELS as u32
            || submit_frames == 0
            || submit_frames > MAX_FRAMES as u32
        {
            return if produced { produced_flag } else { 0 };
        }
        shared.channels.store(submit_channels, Ordering::Relaxed);
        shared.frames.store(submit_frames, Ordering::Relaxed);
        // Sidechain delivery: fetch this block's published source for the
        // (destination track, plugin) link into the mailbox so adapters with
        // a sidechain bus receive the detector signal. No link means zero
        // frames, which every adapter treats as silence (no behavior change).
        let gate = self.sidechain_gate.load(Ordering::Acquire);
        if gate.is_null() {
            shared.sidechain_frames.store(0, Ordering::Relaxed);
        } else {
            let mut metadata = [0u64; 6];
            let copied = unsafe {
                crate::sidechain_manager::hirari_sidechain_engine_copy_link(
                    gate.cast_const(),
                    self.sidechain_track.load(Ordering::Relaxed),
                    self.sidechain_plugin.load(Ordering::Relaxed),
                    shared.sidechain_left.as_mut_ptr(),
                    shared.sidechain_right.as_mut_ptr(),
                    submit_frames,
                    metadata.as_mut_ptr(),
                )
            };
            let frames = copied.min(submit_frames);
            if frames > 0 {
                let level = f32::from_bits(metadata[1] as u32);
                if level != 1.0 && level.is_finite() {
                    for index in 0..frames as usize {
                        shared.sidechain_left[index] *= level;
                        shared.sidechain_right[index] *= level;
                    }
                }
            }
            shared
                .sidechain_frames
                .store(frames, Ordering::Release);
        }
        for channel in 0..submit_channels as usize {
            shared.input.samples[channel][..submit_frames as usize].copy_from_slice(
                &audio.pending_input[channel][..submit_frames as usize],
            );
        }
        for index in 0..audio.pending_midi_count as usize {
            shared.midi[index] = audio.pending_midi[index];
        }
        shared
            .midi_events
            .store(audio.pending_midi_count, Ordering::Relaxed);
        shared
            .parameter_changes
            .store(audio.pending_parameter_count, Ordering::Release);
        shared.output_midi_events.store(0, Ordering::Relaxed);
        shared.output_midi_dropped.store(0, Ordering::Relaxed);
        audio.sequence = audio.sequence.wrapping_add(1);
        audio.last_submitted = audio.sequence;
        audio.pending_age_frames = 0;
        audio.overrun_reported_for_sequence = false;
        shared
            .request_sequence
            .store(audio.sequence, Ordering::Release);
        if produced {
            produced_flag
        } else {
            0
        }
    }

    pub(in crate::plugin_sandbox_host) fn output_midi_count(&self) -> u32 {
        let Some(_access) = self.acquire_audio_access() else {
            return 0;
        };
        self.audio_state().output_midi_count
    }

    pub(in crate::plugin_sandbox_host) fn copy_output_midi(
        &self,
        index: u32,
        sample_offset: &mut u64,
        articulation_id: &mut u8,
        data: &mut [u8],
    ) -> u32 {
        let Some(_access) = self.acquire_audio_access() else {
            return 0;
        };
        let audio = self.audio_state();
        if index >= audio.output_midi_count {
            return 0;
        }
        let event = &audio.output_midi[index as usize];
        let size = event.size as usize;
        if size == 0 || size > MAX_MIDI_PAYLOAD_BYTES || data.len() < size {
            return 0;
        }
        *sample_offset = event.sample_offset;
        *articulation_id = event.articulation_id;
        data[..size].copy_from_slice(&event.data[..size]);
        event.size
    }

    pub(in crate::plugin_sandbox_host) fn take_last_process_overrun(&self) -> bool {
        self.last_process_overrun.swap(false, Ordering::AcqRel)
    }

    pub(in crate::plugin_sandbox_host) fn take_completed_sequence_overrun(&self) -> bool {
        let Some(_access) = self.acquire_audio_access() else {
            return false;
        };
        let audio = self.audio_state_mut();
        let value = audio.completed_sequence_had_overrun;
        audio.completed_sequence_had_overrun = false;
        value
    }

    pub(in crate::plugin_sandbox_host) fn take_dropped_output_midi(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe {
            (*shared)
                .output_midi_dropped
                .swap(0, Ordering::AcqRel)
        }
    }

    pub(in crate::plugin_sandbox_host) fn take_mailbox_overruns(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe { (*shared).mailbox_overruns.swap(0, Ordering::AcqRel) }
    }

    pub(in crate::plugin_sandbox_host) fn take_input_midi_truncations(&self) -> u32 {
        let Some(access) = self.acquire_shared_access() else { return 0 };
        let shared = access.ptr;
        // SAFETY: Telemetry counter read is valid while the mapping is owned.
        unsafe {
            (*shared)
                .input_midi_truncations
                .swap(0, Ordering::AcqRel)
        }
    }

    pub(in crate::plugin_sandbox_host) fn record_input_midi_rejection(&self) {
        if let Some(access) = self.acquire_shared_access() {
            // SAFETY: Telemetry counter read is valid while the mapping is owned.
            unsafe {
                (*access.ptr)
                    .input_midi_truncations
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

                    pub(in crate::plugin_sandbox_host) fn is_midi_instrument(&self) -> bool {
        let Some(access) = self.acquire_shared_access() else { return false };
        let shared = access.ptr;
        // SAFETY: Telemetry read is valid while the mapping is owned.
        unsafe { (*shared).midi_instrument.load(Ordering::Acquire) != 0 }
    }

    pub(in crate::plugin_sandbox_host) fn latency_samples(&self) -> u32 {
        self.latency_samples.load(Ordering::Acquire)
    }

    pub(in crate::plugin_sandbox_host) fn plugin_path(&self) -> String {
        lock_control(&self.control).plugin_path.clone()
    }

    pub(in crate::plugin_sandbox_host) fn set_sidechain_context(&self, gate: *mut c_void, track: u32, plugin: u32) {
        self.sidechain_gate.store(gate, Ordering::Release);
        self.sidechain_track.store(track, Ordering::Relaxed);
        self.sidechain_plugin.store(plugin, Ordering::Relaxed);
    }

    pub(in crate::plugin_sandbox_host) fn set_state(&self, data: &[u8]) -> bool {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        self.last_state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Release);
        self.set_state_locked(&mut control, &mut state, data)
    }

    pub(in crate::plugin_sandbox_host) fn get_state(&self) -> Vec<u8> {
        let mut control = lock_control(&self.control);
        let mut state = lock_control(&self.state);
        let _transition = TransitionGuard::enter(&self.state_transition);
        self.last_state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Release);
        self.get_state_locked(&mut control, &mut state)
    }

    pub(in crate::plugin_sandbox_host) fn state_error_code(&self) -> u8 {
        self.last_state_error.load(Ordering::Acquire)
    }

    pub(in crate::plugin_sandbox_host) fn wait_for_audio_calls(&self) -> bool {
        for _ in 0..1000 {
            if self.process_calls.load(Ordering::SeqCst) == 0
                && self.shared_users.load(Ordering::SeqCst) == 0
            {
                return true;
            }
            usleep(100);
        }
        false
    }

    pub(in crate::plugin_sandbox_host) fn shared_ptr(&self) -> Option<*mut SharedAudioBlock> {
        let shared = self.shared();
        if shared.is_null() {
            None
        } else {
            Some(shared)
        }
    }

    pub(in crate::plugin_sandbox_host) fn set_state_locked(
        &self,
        control: &mut Control,
        state: &mut StateControl,
        data: &[u8],
    ) -> bool {
        if !self.is_alive() {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return false;
        }
        if data.len() > MAX_STATE_BYTES {
            self.last_state_error
                .store(STATE_ERROR_OVERSIZE, Ordering::Release);
            return false;
        }
        if !self.wait_for_audio_calls() {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return false;
        }
        // Do not create an exclusive reference to the shared block until all
        // audio calls that could access it have drained. `state_transition`
        // is already set by the caller, so new audio calls will leave before
        // dereferencing the mapping while this control operation proceeds.
        let Some(shared_ptr) = self.shared_ptr() else {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return false;
        };
        // SAFETY: `control` serializes unmapping, and the transition gate plus
        // the completed drain above excludes concurrent host audio access.
        let shared = unsafe { &mut *shared_ptr };
        let completed = shared.state_completed_sequence.load(Ordering::Acquire);
        if shared.state_request_sequence.load(Ordering::Acquire) != completed {
            self.last_state_error
                .store(STATE_ERROR_BUSY, Ordering::Release);
            return false;
        }
        let context = state.generation;
        if !context.valid() {
            self.last_state_error
                .store(STATE_ERROR_VERSION, Ordering::Release);
            return false;
        }
        shared.state[..data.len()].copy_from_slice(data);
        shared
            .state_size
            .store(data.len() as u32, Ordering::Relaxed);
        shared
            .state_version
            .store(STATE_PROTOCOL_VERSION, Ordering::Relaxed);
        shared
            .state_checksum
            .store(state_checksum(data), Ordering::Relaxed);
        shared
            .state_project_generation
            .store(context.project, Ordering::Relaxed);
        shared
            .state_plugin_generation
            .store(context.plugin, Ordering::Relaxed);
        shared
            .state_audio_generation
            .store(context.audio, Ordering::Relaxed);
        shared
            .state_generation
            .store(context.state, Ordering::Relaxed);
        shared.state_mode.store(2, Ordering::Relaxed);
        shared.state_result.store(0, Ordering::Relaxed);
        shared
            .state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Relaxed);
        state.sequence = state.sequence.wrapping_add(1);
        shared
            .state_request_sequence
            .store(state.sequence, Ordering::Release);
        let ok = self.wait_for_state(control, shared, state.sequence, false);
        let generation_ok = ok
            && shared.state_project_generation.load(Ordering::Acquire) == context.project
            && shared.state_plugin_generation.load(Ordering::Acquire) == context.plugin
            && shared.state_audio_generation.load(Ordering::Acquire) == context.audio
            && shared.state_generation.load(Ordering::Acquire) == context.state;
        let checksum_ok = generation_ok
            && shared.state_error.load(Ordering::Acquire) == STATE_ERROR_NONE
            && shared.state_checksum.load(Ordering::Acquire) == state_checksum(data);
        if checksum_ok {
            state.cache.clear();
            state.cache.extend_from_slice(data);
        }
        if !checksum_ok {
            let worker_error = shared.state_error.load(Ordering::Acquire);
            self.last_state_error.store(
                if worker_error == STATE_ERROR_NONE {
                    STATE_ERROR_TIMEOUT
                } else {
                    worker_error
                },
                Ordering::Release,
            );
        }
        checksum_ok
    }

    pub(in crate::plugin_sandbox_host) fn get_state_locked(&self, control: &mut Control, state: &mut StateControl) -> Vec<u8> {
        if !self.is_alive() {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return state.cache.clone();
        }
        if !self.wait_for_audio_calls() {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return state.cache.clone();
        }
        // See `set_state_locked`: `state_transition` prevents new audio calls
        // from touching shared memory, and existing calls must drain before
        // this exclusive reference is formed.
        let Some(shared_ptr) = self.shared_ptr() else {
            self.last_state_error
                .store(STATE_ERROR_UNAVAILABLE, Ordering::Release);
            return state.cache.clone();
        };
        // SAFETY: `control` serializes unmapping, and the transition gate plus
        // the completed drain above excludes concurrent host audio access.
        let shared = unsafe { &mut *shared_ptr };
        let completed = shared.state_completed_sequence.load(Ordering::Acquire);
        if shared.state_request_sequence.load(Ordering::Acquire) != completed {
            self.last_state_error
                .store(STATE_ERROR_BUSY, Ordering::Release);
            return state.cache.clone();
        }
        shared.state_size.store(0, Ordering::Relaxed);
        shared.state_version.store(0, Ordering::Relaxed);
        shared.state_mode.store(1, Ordering::Relaxed);
        shared.state_result.store(0, Ordering::Relaxed);
        shared
            .state_error
            .store(STATE_ERROR_NONE_CODE, Ordering::Relaxed);
        state.sequence = state.sequence.wrapping_add(1);
        shared
            .state_request_sequence
            .store(state.sequence, Ordering::Release);
        if !self.wait_for_state(control, shared, state.sequence, true) {
            self.last_state_error
                .store(STATE_ERROR_TIMEOUT, Ordering::Release);
            return state.cache.clone();
        }
        let size = shared.state_size.load(Ordering::Acquire) as usize;
        if size > MAX_STATE_BYTES {
            self.last_state_error
                .store(STATE_ERROR_OVERSIZE, Ordering::Release);
            return state.cache.clone();
        }
        let state_vec = shared.state[..size].to_vec();
        let worker_error = shared.state_error.load(Ordering::Acquire);
        let validation_error = validate_state(
            &state_vec,
            shared.state_version.load(Ordering::Acquire),
            shared.state_checksum.load(Ordering::Acquire),
        );
        if validation_error != STATE_ERROR_NONE || worker_error != STATE_ERROR_NONE {
            self.last_state_error.store(
                if worker_error != STATE_ERROR_NONE {
                    worker_error
                } else {
                    validation_error
                },
                Ordering::Release,
            );
            return state.cache.clone();
        }
        state.cache = state_vec.clone();
        state_vec
    }

    pub(in crate::plugin_sandbox_host) fn wait_for_state(
        &self,
        control: &mut Control,
        shared: &SharedAudioBlock,
        sequence: u64,
        allow_empty: bool,
    ) -> bool {
        for _ in 0..5000 {
            if !self.is_alive() {
                return false;
            }
            if sequence_reached(
                shared.state_completed_sequence.load(Ordering::Acquire),
                sequence,
            ) {
                let result = shared.state_result.load(Ordering::Acquire) != 0;
                return result
                    && (allow_empty
                        || shared.state_size.load(Ordering::Acquire) as usize <= MAX_STATE_BYTES);
            }
            usleep(1000);
        }
        // The worker may still be blocked inside a vendor state callback. It
        // must leave the graph, but unmapping here would race the unwinding
        // control call, so only kill; the serialized teardown path reaps.
        self.failure
            .store(FAILURE_PROCESS_HUNG, Ordering::Release);
        self.process_alive.store(false, Ordering::SeqCst);
        #[cfg(unix)]
        if control.pid > 0 {
            unsafe {
                libc::kill(control.pid, libc::SIGKILL);
            }
        }
        false
    }
}
