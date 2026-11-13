//! AudioEngine extension methods — VCA and a long tail of C-bridge
//! operations, split into this sibling crate module.
use super::*;

fn take_float_buffer(buffer: *mut c_void) -> Vec<f32> {
    if buffer.is_null() {
        return Vec::new();
    }
    let (data, count) = unsafe {
        (
            hirari_float_buffer_data(buffer),
            hirari_float_buffer_len(buffer),
        )
    };
    let mut samples = Vec::new();
    if !data.is_null() && count != 0 {
        samples.extend_from_slice(unsafe { std::slice::from_raw_parts(data, count) });
    }
    unsafe { hirari_float_buffer_destroy(buffer) };
    samples
}

impl crate::AudioEngine {
    pub fn add_vca_group(&self, group_id: u32, gain: f32) -> bool {
        if !hirari_vca_add_group(group_id, gain) {
            return false;
        }
        hirari_vca_resolve_hierarchy();
        true
    }

    pub fn set_vca_group_gain(&self, group_id: u32, gain: f32) -> bool {
        if !hirari_vca_add_group(group_id, gain) {
            return false;
        }
        hirari_vca_resolve_hierarchy();
        true
    }

    pub fn get_vca_snapshot_json(&self) -> String {
        let required = unsafe { hirari_vca_snapshot_json(std::ptr::null_mut(), 0) };
        if required == 0 {
            return "[]".to_string();
        }
        let mut buffer = vec![0u8; required];
        unsafe { hirari_vca_snapshot_json(buffer.as_mut_ptr(), required) };
        let end = buffer.iter().position(|&b| b == 0).unwrap_or(required);
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    }

    pub fn split_region(&self, tid: u32, rid: u32, split: f64) -> bool {
        self.split_region_with_right_id(tid, rid, split) != 0
    }

    pub fn split_region_with_right_id(&self, tid: u32, rid: u32, split: f64) -> u32 {
        unsafe { region_edit_host::hirari_region_split(host(self), tid, rid, split) }
    }

    pub fn get_region_waveform(&self, tid: u32, rid: u32) -> Vec<f32> {
        take_float_buffer(unsafe { hirari_region_waveform(host(self), tid, rid, 128) })
    }

    /// Background layout snapshots. The session, its cancellation flag and the
    /// revisions around the gather all live in `layout_gather`; C++ contributes
    /// only the engine host and the ops table it reads through.
    pub fn request_project_layout_snapshot(&self) -> u64 {
        layout_gather::request_layout_snapshot(core_mut(self))
    }

    pub fn poll_project_layout_snapshot(&self, request_id: u64) -> String {
        layout_gather::poll_layout_snapshot(core_mut(self) as *const _, request_id)
    }

    pub fn cancel_project_layout_snapshot(&self, request_id: u64) -> bool {
        layout_gather::cancel_layout_snapshot(core_mut(self) as *const _, request_id)
    }

    /// The live project layout as JSON, gathered without a C++ wrapper.
    pub fn project_layout_json(&self) -> String {
        layout_gather::project_layout_json(core_mut(self) as *const _)
    }

    pub fn get_region_audio_interleaved_range(
        &self,
        tid: u32,
        rid: u32,
        start_frame: u64,
        frames: u32,
        channels: u32,
    ) -> Vec<f32> {
        take_float_buffer(unsafe {
            hirari_region_audio_interleaved_range(
                host(self),
                tid,
                rid,
                start_frame,
                frames,
                channels,
            )
        })
    }

    pub fn maintain_sandboxed_plugins(&self, auto_restart: bool) -> u32 {
        unsafe { plugin_sandbox_maintenance::hirari_plugin_sandbox_maintain(host(self), auto_restart) }
    }

    pub fn set_track_input_monitor_channels(
        &self,
        tid: u32,
        enabled: bool,
        left: u32,
        right: u32,
    ) -> bool {
        unsafe {
            control_edit_host::hirari_control_edit_set_input_monitor(
                host(self),
                tid,
                enabled,
                left,
                right,
            )
        }
    }

    pub fn set_track_input_monitor(&self, tid: u32, enabled: bool) -> bool {
        self.set_track_input_monitor_channels(tid, enabled, 0, 1)
    }

    pub fn move_plugin(&self, tid: u32, from_index: u32, to_index: u32) -> bool {
        if !unsafe {
            plugin_edit_host::hirari_plugin_edit_move(host(self), tid, from_index, to_index)
        } {
            return false;
        }
        unsafe { hirari_engine_sync_structural_changes(host(self)) };
        true
    }

    pub fn handle_midi_cc(&self, channel: u8, cc: u8, value: u8) {
        unsafe {
            crate::midi_learn::hirari_midi_learn_handle_cc(
                learn_state(self),
                channel,
                cc,
                value,
            )
        };
    }

    pub fn handle_midi_cc14(&self, channel: u8, controller: u16, value: u16) {
        unsafe {
            crate::midi_learn::hirari_midi_learn_handle_cc14(
                learn_state(self),
                channel,
                controller,
                value,
            )
        };
    }

    pub fn set_preview_synth_engine(&self, engine: u32) {
        unsafe { crate::preview_synth::hirari_preview_synth_set_engine(preview_state(self), engine) };
    }

    pub fn set_sidechain_link(
        &self,
        source_id: u32,
        dest_id: u32,
        plugin_index: u32,
        tap_point: u32,
        enabled: bool,
    ) -> bool {
        unsafe {
            hirari_routing_edit_set_sidechain_link(
                host(self),
                source_id,
                dest_id,
                plugin_index,
                tap_point,
                enabled,
            )
        }
    }
}

// Simple getters read the inner engine through `get_core`; the AudioEngine
// forwarders for them are gone. Transport reads/writes go to
// `engine_transport`, which drives the Rust-owned engine-block store directly.
impl crate::AudioEngine {
    pub fn is_loop_enabled(&self) -> bool {
        crate::engine_transport::loop_enabled(self.get_core())
    }

    pub fn get_master_gain(&self) -> f32 {
        self.get_core().get_master_gain()
    }

    pub fn get_track_volume(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_volume(self.get_core(), tid)
    }
}

// Read-only engine getters. The AudioEngine forwarders for these are gone:
// Rust reaches the same inner engine through `get_core`.
impl crate::AudioEngine {
    pub fn cycle_start(&self) -> u64 {
        crate::engine_transport::cycle_start(self.get_core())
    }
    pub fn cycle_end(&self) -> u64 {
        crate::engine_transport::cycle_end(self.get_core())
    }
    pub fn is_metronome_enabled(&self) -> bool {
        unsafe { crate::metronome_renderer::hirari_metronome_is_enabled(metronome(self)) }
    }
    pub fn set_metronome_enabled(&self, enabled: bool) {
        unsafe { crate::metronome_renderer::hirari_metronome_set_enabled(metronome(self), enabled) };
        self.mark_project_state_changed();
    }
    pub fn get_playhead(&self) -> u64 {
        crate::engine_transport::playhead(self.get_core())
    }
    pub fn samples_to_beats(&self, samples: u64) -> f64 {
        unsafe {
            crate::tempo::hirari_tempo_map_samples_to_beats(
                self.tempo_state() as *const c_void,
                samples,
                self.get_core().get_sample_rate(),
            )
        }
    }
    pub fn beats_to_samples(&self, beats: f64) -> u64 {
        unsafe {
            crate::tempo::hirari_tempo_map_beats_to_samples(
                self.tempo_state() as *const c_void,
                beats,
                self.get_core().get_sample_rate(),
            )
        }
    }
    pub fn get_routing_snapshot_json(&self) -> String {
        unsafe {
            let json = crate::routing_engine_core::hirari_routing_engine_connections_json(
                routing_engine(self),
            );
            if json.is_null() {
                return "[]".to_owned();
            }
            let text = std::ffi::CStr::from_ptr(json).to_string_lossy().into_owned();
            crate::routing_engine_core::hirari_routing_engine_connections_json_free(json);
            text
        }
    }
    pub fn midi_clock_ticks(&self) -> u64 {
        self.get_core().midi_clock_ticks()
    }
    pub fn midi_clock_last_tick(&self) -> u64 {
        self.get_core().midi_clock_last_tick()
    }
    pub fn midi_clock_rate(&self) -> f64 {
        self.get_core().midi_clock_rate()
    }
    pub fn bus_track_exists(&self, bus_track_id: u32) -> bool {
        self.get_core().bus_track_exists(bus_track_id)
    }
    pub fn get_track_delay_samples(&self, tid: u32) -> u32 {
        crate::engine_analysis_reads::track_delay_samples(self.get_core(), tid)
    }
    pub fn get_latency_ms(&self) -> f32 {
        crate::engine_analysis_reads::latency_ms(self.get_core())
    }
    pub fn low_latency_mode(&self) -> bool {
        unsafe { crate::native_pdc::hirari_native_pdc_low_latency_mode(pdc_state(self)) }
    }
    pub fn set_low_latency_mode(&self, active: bool) -> bool {
        unsafe { crate::native_pdc::hirari_native_pdc_set_low_latency_mode(pdc_state(self), active) }
    }
    pub fn get_tempo(&self) -> f32 {
        unsafe { crate::tempo::hirari_tempo_map_get_current_bpm(self.tempo_state() as *const c_void) }
    }
    pub fn trigger_preview_sample(&self) {
        unsafe { crate::preview_sampler::hirari_preview_sampler_trigger(preview_sampler(self)) }
    }
    pub fn set_preview_sample(&self, samples: &[f32], source_rate: f64) {
        crate::engine_transport::set_preview_sample(self.get_core(), samples, source_rate);
    }
    pub fn non_finite_plugin_samples(&self) -> u64 {
        crate::engine_analysis_reads::non_finite_plugin_samples(self.get_core())
    }
    pub fn get_sandbox_plugin_state_error(&self, track_id: u32, sandbox_index: u32) -> u8 {
        crate::plugin_sandbox_maintenance::sandbox_state_error(
            host(self),
            track_id,
            sandbox_index,
        )
        .0
    }
    pub fn get_plugin_parameter(&self, tid: u32, plugin_index: u32, parameter_id: u32) -> f32 {
        crate::engine_analysis_reads::plugin_parameter(self.get_core(), tid, plugin_index, parameter_id)
    }
    pub fn get_plugin_parameter_count(&self, tid: u32, plugin_index: u32) -> u32 {
        crate::engine_analysis_reads::plugin_parameter_count(self.get_core(), tid, plugin_index)
    }
    pub fn get_plugin_bypass(&self, tid: u32, plugin_index: u32) -> bool {
        crate::engine_analysis_reads::plugin_bypass(self.get_core(), tid, plugin_index)
    }
    pub fn get_track_latency_ms(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_latency_ms(self.get_core(), tid)
    }
    pub fn get_track_pdc_compensation_ms(&self, tid: u32) -> f32 {
        crate::engine_analysis_reads::track_pdc_compensation_ms(self.get_core(), tid)
    }
    pub fn has_sidechain_link(&self, source_id: u32, dest_id: u32, plugin_index: u32) -> bool {
        self.get_core()
            .has_sidechain_link(source_id, dest_id, plugin_index)
    }
    pub fn get_bounce_progress(&self) -> f32 {
        self.get_core().get_bounce_progress()
    }
    pub fn get_bounce_state(&self) -> u32 {
        self.get_core().get_bounce_state()
    }
    pub fn audio_range_overflowed(&self) -> bool {
        crate::engine_transport::range_overflowed(self.get_core())
    }
    pub fn get_video_frame_revision(&self) -> u64 {
        crate::video_system::video_service().frame_revision()
    }
    pub fn get_track_correlation(&self, tid: u32) -> f32 {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::get_track_correlation(host, tid)
    }
    pub fn assign_track_to_vca(&self, track_id: u32, group_id: u32) -> bool {
        crate::engine_transport::assign_track_to_vca(self.get_core(), track_id, group_id)
    }
    pub fn new_project(&self) {
        pinned(self).new_project()
    }
    pub fn take_watchdog_trips(&self) -> u32 {
        crate::engine_analysis_reads::watchdog_trips(self.get_core())
    }
    pub fn retry_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        crate::engine_analysis_reads::retry_sandboxed_plugin(self.get_core(), track_id, sandbox_index)
    }
    pub fn restart_sandboxed_plugin(&self, track_id: u32, sandbox_index: u32) -> bool {
        crate::engine_analysis_reads::restart_sandboxed_plugin(
            self.get_core(),
            track_id,
            sandbox_index,
        )
    }
    pub fn set_plugin_bypass(&self, tid: u32, plugin_index: u32, bypassed: bool) -> bool {
        pinned(self).set_plugin_bypass(tid, plugin_index, bypassed)
    }
    pub fn get_plugin_parameter_name(
        &self,
        track_id: u32,
        plugin_index: u32,
        parameter_id: u32,
    ) -> String {
        crate::engine_analysis_reads::plugin_parameter_name(
            self.get_core(),
            track_id,
            plugin_index,
            parameter_id,
        )
    }
    pub fn cancel_bounce(&self) -> bool {
        pinned(self).cancel_bounce()
    }
    pub fn pause_bounce(&self) -> bool {
        pinned(self).pause_bounce()
    }
    pub fn resume_bounce(&self) -> bool {
        pinned(self).resume_bounce()
    }
    pub fn execute_vocal_remover(&self, tid: u32) -> bool {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::execute_vocal_remover(host, tid)
    }
    pub fn set_articulation_map(&self, tid: u32, map_hash: u32) -> bool {
        pinned(self).set_articulation_map(tid, map_hash)
    }
    pub fn set_track_eq(
        &self,
        tid: u32,
        low_boost_db: f32,
        low_cut_db: f32,
        high_boost_db: f32,
        high_cut_db: f32,
    ) -> bool {
        crate::engine_analysis_reads::set_track_eq(
            self.get_core(),
            tid,
            low_boost_db,
            low_cut_db,
            high_boost_db,
            high_cut_db,
        )
    }
    pub fn execute_auto_arrangement(&self) -> bool {
        pinned(self).execute_auto_arrangement()
    }
    pub fn set_project_scale(&self, root: i32, scale_type: i32) -> bool {
        pinned(self).set_project_scale(root, scale_type)
    }
    pub fn append_region_processing(
        &self,
        tid: u32,
        rid: u32,
        step_id: u32,
        operation: &str,
        parameter: f32,
        enabled: bool,
    ) -> bool {
        crate::region_processing_ops::append_region_processing(
            host(self),
            tid,
            rid,
            step_id,
            operation,
            parameter,
            enabled,
        )
    }
    pub fn clear_region_processing(&self, tid: u32, rid: u32) -> bool {
        crate::region_processing_ops::clear_region_processing(host(self), tid, rid)
    }
    pub fn set_automation_record_mode(&self, mode: u32) -> bool {
        let host = crate::engine_event_ops::host_from_core(self.get_core());
        crate::engine_event_ops::set_automation_record_mode(host, mode)
    }
    pub fn request_video_frame(&self, seconds: f64) -> bool {
        crate::video_system::video_service().request_frame(seconds)
    }
    pub fn mark_project_layout_changed(&self) {
        self.get_core().mark_project_layout_changed()
    }
    pub fn mark_project_state_changed(&self) {
        pinned(self).mark_project_state_changed()
    }
    pub fn set_loop(&self, enabled: bool) {
        crate::engine_transport::set_loop(self.get_core(), enabled);
        self.mark_project_state_changed();
    }
    pub fn set_cycle_range(&self, start_sample: u64, end_sample: u64, enabled: bool) -> bool {
        if !crate::engine_transport::set_cycle_range(self.get_core(), start_sample, end_sample, enabled) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn set_playhead(&self, pos: u64) {
        crate::engine_transport::set_playhead(self.get_core(), pos)
    }
    pub fn set_test_tone(&self, enabled: bool) {
        crate::engine_transport::set_test_tone(self.get_core(), enabled)
    }
    pub fn set_master_gain(&self, value: f32) -> bool {
        if !pinned(self).set_master_gain(value) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn set_offline_render_target(&self, tid: u32) -> bool {
        crate::engine_analysis_reads::set_offline_target(self.get_core(), tid)
    }
    pub fn clear_offline_render_target(&self) {
        crate::engine_analysis_reads::clear_offline_target(self.get_core());
    }
    pub fn set_offline_render_tail_seconds(&self, seconds: f32) {
        crate::engine_analysis_reads::set_offline_tail_seconds(self.get_core(), seconds);
    }
    pub fn set_offline_render_options(&self, pre_fader: bool, include_inserts: bool) {
        crate::engine_analysis_reads::set_offline_options(
            self.get_core(),
            pre_fader,
            include_inserts,
        );
    }
    pub fn get_project_layout_revision(&self) -> u64 {
        layout_gather::project_layout_revision(core_mut(self) as *const _)
    }

    pub fn get_project_state_revision(&self) -> u64 {
        layout_gather::project_state_revision(core_mut(self) as *const _)
    }
    pub fn is_playing(&self) -> bool {
        crate::engine_transport::is_playing(self.get_core())
    }
    pub fn set_tempo(&self, bpm: f32) -> bool {
        let core = self.get_core();
        if !core.set_tempo(bpm) {
            return false;
        }
        self.mark_project_state_changed();
        true
    }
    pub fn midi_clock_tick(&self, timestamp: u64) {
        self.get_core().midi_clock_tick(timestamp)
    }
    pub fn set_midi_clock_rate(&self, bpm: f64) {
        self.get_core().set_midi_clock_rate(bpm)
    }
    pub fn get_sample_rate(&self) -> f64 {
        self.get_core().get_sample_rate()
    }
    pub fn set_region_warp_markers(
        &self,
        tid: u32,
        rid: u32,
        source_samples: &[u64],
        timeline_samples: &[u64],
        transient_flags: &[u8],
    ) -> bool {
        // The slice agreement checks, marker cap and marker layout live in
        // region_edit_host; this only hands it the engine host handle.
        unsafe {
            crate::region_edit_host::hirari_region_edit_set_warp_markers(
                host(self),
                tid,
                rid,
                source_samples.as_ptr(),
                source_samples.len(),
                timeline_samples.as_ptr(),
                timeline_samples.len(),
                transient_flags.as_ptr(),
                transient_flags.len(),
            )
        }
    }
    pub fn get_sandbox_statuses(&self) -> Vec<u32> {
        plugin_sandbox_maintenance::sandbox_statuses_packed(host(self) as usize)
    }
    pub fn read_wav_diagnostic_json(&self, path: &str, format: u32) -> String {
        unsafe {
            let raw = crate::wav_diagnostic::hirari_wav_diagnostic_json_for_path(
                path.as_ptr(),
                path.len(),
                format == 1,
            );
            if raw.is_null() {
                return String::new();
            }
            let data = crate::wav_diagnostic::hirari_wav_diagnostic_json_data(raw);
            let size = crate::wav_diagnostic::hirari_wav_diagnostic_json_size(raw);
            let text = if data.is_null() {
                String::new()
            } else {
                String::from_utf8_lossy(std::slice::from_raw_parts(data, size)).into_owned()
            };
            crate::wav_diagnostic::hirari_wav_diagnostic_json_free(raw);
            text
        }
    }
    pub fn poll_region_waveform(&self, request: u64) -> Vec<f32> {
        unsafe extern "C" fn collect(context: *mut c_void, samples: *const f32, count: usize) {
            if samples.is_null() || count == 0 {
                return;
            }
            // SAFETY: the poll visitor only runs for the duration of the call
            // and the context is the local vector below.
            let output = unsafe { &mut *(context as *mut Vec<f32>) };
            output.extend_from_slice(unsafe { std::slice::from_raw_parts(samples, count) });
        }
        let mut result = Vec::new();
        if request == 0 {
            return result;
        }
        unsafe {
            crate::async_snapshots::hirari_waveform_snapshot_poll(
                request,
                &mut result as *mut Vec<f32> as *mut c_void,
                Some(collect),
            );
        }
        result
    }
    pub fn region_waveform_pending(&self, request: u64) -> bool {
        request != 0 && unsafe { crate::async_snapshots::hirari_waveform_snapshot_pending(request) }
    }
}
