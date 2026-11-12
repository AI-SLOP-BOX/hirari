//! Chain-owned isolated plugin nodes (the sandbox bridge).
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::{
    MAX_FRAMES, MAX_MIDI_PAYLOAD_BYTES, SandboxHost, new_sandbox_host,
};
use crate::effect_chain_nodes::BlockAudio;
use crate::midi_buffer_ops::MidiView;
use crate::plugin_sandbox_status_text::{
    hirari_plugin_sandbox_failure_name, hirari_plugin_sandbox_state_error_name,
};

/// State IPC is generation-protected. A newly created node must still be
    /// able to save/restore before the project manager publishes its first full
    /// generation tuple, so each node starts from a unique bootstrap context.
    static GENERATION_SEED: AtomicU64 = AtomicU64::new(0);

    pub struct SandboxNode {
        host: Box<SandboxHost>,
        /// Control-plane lifecycle serialization. `process` deliberately never
        /// takes it: audio callbacks must not wait for worker control.
        lifecycle: Mutex<()>,
        /// Discovery-probe cache: parameter metadata from the first probe
        /// for this node's plugin, cloned for UI/automation reads.
        parameters: Mutex<Option<Vec<crate::plugin_parameters::PluginParameter>>>,
    }

    impl Drop for SandboxNode {
        fn drop(&mut self) {
            self.host.stop();
        }
    }

    fn lock(mutation: &Mutex<()>) -> MutexGuard<'_, ()> {
        mutation.lock().unwrap_or_else(|error| error.into_inner())
    }

    impl SandboxNode {
        pub fn new(plugin_path: &[u8], sample_rate: f64, max_block_size: u32) -> Box<Self> {
            let path = String::from_utf8_lossy(plugin_path).into_owned();
            let host = new_sandbox_host(&path, sample_rate, max_block_size, 2, "auto");
            let instance = GENERATION_SEED.fetch_add(1, Ordering::Relaxed) + 1;
            host.set_generation_context(1, instance, 1, 1);
            Box::new(Self {
                host,
                lifecycle: Mutex::new(()),
                parameters: Mutex::new(None),
            })
        }

        pub fn start(&self) -> bool {
            let _guard = lock(&self.lifecycle);
            self.host.start()
        }

        pub fn stop(&self) {
            let _guard = lock(&self.lifecycle);
            self.host.stop();
        }

        pub fn is_alive(&self) -> bool {
            self.host.is_alive()
        }

        pub fn failure_code(&self) -> u8 {
            self.host.failure()
        }

        pub fn recovery_failed(&self) -> bool {
            self.host.recovery_failed()
        }

        pub fn failure_text(&self) -> *const c_char {
            hirari_plugin_sandbox_failure_name(self.host.failure())
        }

        pub fn poll_health(&self) -> bool {
            let _guard = lock(&self.lifecycle);
            self.host.poll_health() && !self.host.recovery_failed()
        }

        pub fn restart(&self, force: bool) -> bool {
            let _guard = lock(&self.lifecycle);
            self.host.restart(force)
        }

        pub fn auto_restart(&self) -> bool {
            let _guard = lock(&self.lifecycle);
            self.host.auto_restart()
        }

        pub fn can_retry(&self) -> bool {
            self.host.can_retry()
        }

        pub fn can_auto_retry(&self) -> bool {
            self.host.can_auto_retry()
        }

        /// A worker block size is not plugin latency, so latency stays zero and
        /// PDC never inserts a fabricated one-block delay.
        pub fn prepare(&self, sample_rate: f64, block_size: u32) {
            let _guard = lock(&self.lifecycle);
            let _ = self.host.prepare(sample_rate, block_size);
        }

        /// Clears the recovery latch, the overrun streak and the quarantine edge
        /// without touching the worker's DSP state or taking lifecycle locks.
        pub fn reset(&self) {
            self.host.reset_recovery();
        }

        pub fn set_parameter(&self, parameter_id: u32, value: f32, sample_offset: u32) {
            let value = f64::from(value);
            if !value.is_finite() {
                return;
            }
            // Keep the normalized snapshot even when the bounded realtime queue
            // is temporarily full; lifecycle recovery replays it before audio
            // resumes. CLAP and VST3 ids are sparse, so the queue is keyed by
            // native id rather than treated as an array index.
            let _ = self.host.remember_parameter(parameter_id, value);
            let _ = self
                .host
                .enqueue_parameter_change(parameter_id, value, sample_offset);
        }

        pub fn parameter(&self, parameter_id: u32) -> f32 {
            let mut value = 0.0_f64;
            if self.host.parameter_value(parameter_id, &mut value) && value.is_finite() {
                (value.clamp(0.0, 1.0)) as f32
            } else {
                0.0_f32
            }
        }

        pub fn is_midi_instrument(&self) -> bool {
            self.host.is_midi_instrument()
        }

        pub fn latency_samples(&self) -> u32 {
            self.host.latency_samples()
        }

        fn discovered_parameters(&self) -> Vec<crate::plugin_parameters::PluginParameter> {
            if let Some(cached) = self.parameters.lock().ok().and_then(|guard| guard.clone()) {
                return cached;
            }
            let path = self.host.plugin_path();
            let probed = crate::plugin_catalog::probe_worker_parameters(&path)
                .map(|json| super::parse_discovered_parameters(&json))
                .unwrap_or_default();
            // Seed the value cache with the probe defaults so control-plane
            // reads answer like a built-in host would, before any edit.
            for parameter in &probed {
                if parameter.max > parameter.min {
                    let default = (parameter.default - parameter.min)
                        / (parameter.max - parameter.min);
                    let _ =
                        self.host.remember_parameter(parameter.id, default.clamp(0.0, 1.0));
                }
            }
            if let Ok(mut guard) = self.parameters.lock() {
                let _ = guard.insert(probed.clone());
            }
            probed
        }

        pub fn parameter_count(&self) -> u32 {
            self.discovered_parameters().len() as u32
        }

        pub fn parameter_name(&self, parameter_id: u32) -> String {
            self.discovered_parameters()
                .into_iter()
                .find(|parameter| parameter.id == parameter_id)
                .map(|parameter| parameter.name)
                .unwrap_or_default()
        }

        pub fn set_sidechain_context(&self, gate: *mut c_void, track: u32, plugin: u32) {
            self.host.set_sidechain_context(gate, track, plugin);
        }

        /// One realtime block. A false return tells the chain traversal to keep
        /// the block silent; MIDI the worker already consumed is cleared.
        pub fn process(&self, audio: &BlockAudio, midi: &mut MidiView) -> bool {
            let frames = audio.samples();
            let channels = audio.channels();
            let mut produced = false;
            if self.host.is_alive() {
                self.host.reset_staged_midi();
                for index in 0..midi.len() {
                    if let Some((sample_offset, articulation_id, data)) = midi.event(index) {
                        let _ = self
                            .host
                            .stage_input_midi(sample_offset, articulation_id, data, frames);
                    }
                }
                let left = audio.plane(0);
                let right = audio.plane(1);
                let blocked = (channels > 0 && left.is_null())
                    || (channels > 1 && right.is_null());
                if !blocked {
                    let left_slice = if channels > 0 {
                        unsafe { std::slice::from_raw_parts_mut(left, frames as usize) }
                    } else {
                        &mut []
                    };
                    let right_slice = if channels > 1 {
                        unsafe { std::slice::from_raw_parts_mut(right, frames as usize) }
                    } else {
                        &mut []
                    };
                    let flags = self.host.process(left_slice, right_slice, channels, frames);
                    if (flags & 2) != 0 {
                        midi.clear();
                    } else if (flags & 1) != 0 {
                        produced = true;
                        // Collect the worker's output MIDI into the caller's
                        // buffer. The input events are already present, so one
                        // sort at the boundary restores monotonic offsets.
                        let count = self.host.output_midi_count();
                        let mut payload = [0_u8; MAX_MIDI_PAYLOAD_BYTES];
                        for index in 0..count {
                            let mut sample_offset = 0_u64;
                            let mut articulation_id = 0_u8;
                            let size = self.host.copy_output_midi(
                                index,
                                &mut sample_offset,
                                &mut articulation_id,
                                &mut payload,
                            ) as usize;
                            if size > 0 && sample_offset < u64::from(frames) {
                                midi.add(sample_offset, &payload[..size], articulation_id);
                            }
                        }
                        midi.sort();
                    }
                }
            }
            // Overrun streak, failure latch and MIDI invalidation are classified
            // in the host; only the resulting flags cross back here.
            let deadline_missed = self.host.take_last_process_overrun();
            let completed_late = self.host.take_completed_sequence_overrun();
            let outcome = self.host.observe_block(deadline_missed, completed_late, produced);
            if !produced {
                audio.clear();
            }
            if !produced && (outcome & 2) != 0 {
                midi.clear();
            }
            produced && !self.host.recovery_failed()
        }

        pub fn record_input_midi_rejection(&self) {
            self.host.record_input_midi_rejection();
        }

        pub fn take_dropped_output_midi(&self) -> u32 {
            self.host.take_dropped_output_midi()
        }

        pub fn take_mailbox_overruns(&self) -> u32 {
            self.host.take_mailbox_overruns()
        }

        pub fn take_input_midi_truncations(&self) -> u32 {
            self.host.take_input_midi_truncations()
        }

        /// 0 clears the block, 1 quarantines the worker.
        pub fn recovery_mode(&self) -> u32 {
            u32::from(self.host.recovery_failed())
        }

        pub fn state_error_code(&self) -> u8 {
            self.host.state_error_code()
        }

        pub fn state_error_text(&self) -> *const c_char {
            hirari_plugin_sandbox_state_error_name(self.host.state_error_code())
        }

        pub fn max_frames() -> u32 {
            MAX_FRAMES as u32
        }

        pub fn max_payload_bytes() -> usize {
            MAX_MIDI_PAYLOAD_BYTES
        }

        pub fn state(&self) -> Vec<u8> {
            let _guard = lock(&self.lifecycle);
            self.host.get_state()
        }

        pub fn restore_state(&self, data: &[u8]) -> bool {
            let _guard = lock(&self.lifecycle);
            let restored = self.host.set_state(data);
            if !restored {
                self.host.latch_recovery_failure();
            }
            restored
        }
    }
