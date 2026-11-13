//! Structural change publishers for the unified engine, split out of
//! engine_native_core.rs.
use super::*;

impl HirariUnifiedEngine {
    pub fn mark_project_layout_changed(&self) {
        let block = self.block();
        unsafe {
            crate::engine_track::hirari_layout_revision_mark(
                crate::engine_block_host::hirari_engine_block_layout_revision(block),
            );
        }
    }

    /// One ordered control walk, owned by Rust: the bus slots the graph
    /// occupies, the routing rebuild, the snapshot publication and the solved
    /// PDC offsets.
    pub fn sync_structural_changes(&self) {
        let _lock = self.lock_tracks();
        let block = self.block();
        unsafe {
            crate::engine_track_table::hirari_engine_sync_structure(
                self.track_table(),
                self.handle(H_BUS_SYSTEM),
                self.handle(H_ROUTING),
                self.handle(H_PDC),
                crate::engine_block_host::hirari_engine_block_generation(block),
            );
        }
        log_post(0, 0x1, "Structural Sync: Topology Rebuilt.");
    }

    /// Renders one bounded block into a wrapped buffer view.
    ///
    /// # Safety
    /// `output` must address `size` writable frames behind `offset` per
    /// channel, as produced by the audio-buffer wrap.
    pub(crate) unsafe fn process_block(&self, output: *mut c_void, offset: u32, size: u32) {
        let _denormal = DenormalGuard::enter();
        let block = self.block();
        let mut frames: u32 = 0;
        unsafe {
            if crate::engine_block_host::hirari_engine_block_begin(
                block, output, offset, size, &mut frames,
            ) == 0
            {
                return;
            }
            crate::engine_block_host::hirari_engine_block_route(block, output, offset, frames);
            crate::engine_block_host::hirari_engine_block_finish(block, output, offset, frames);
        }
    }

    /// Renders one device callback, slicing an oversized buffer into bounded
    /// engine blocks. The slices reuse the caller's memory: no allocation on
    /// the audio path.
    ///
    /// # Safety
    /// `buffers` must address `num_channels` valid plane pointers covering
    /// `num_samples` frames.
    pub(crate) unsafe fn process_block_direct(&self, buffers: *mut *mut f32, num_channels: u32, num_samples: u32) {
        let block = self.block();
        let input_blocks = self.handle(H_INPUT_BLOCKS);
        unsafe {
            crate::engine_block_host::hirari_engine_block_callback_enter(block);
        }
        struct CallbackGuard(*mut c_void);
        impl Drop for CallbackGuard {
            fn drop(&mut self) {
                unsafe {
                    crate::engine_block_host::hirari_engine_block_callback_leave(self.0);
                }
            }
        }
        let _guard = CallbackGuard(block);

        let valid_input = !buffers.is_null()
            && num_channels >= 2
            && num_channels <= 32
            && num_samples > 0
            && num_samples <= MAX_AUDIO_BLOCK_SIZE
            && !unsafe { *buffers.add(0) }.is_null()
            && !unsafe { *buffers.add(1) }.is_null();
        if !valid_input {
            // The caller does not provide the actual allocation length. When
            // the declared block size is invalid, writing even a bounded
            // prefix could overrun a shorter external buffer. Fail closed
            // without touching memory whose extent cannot be proven here.
            unsafe {
                crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
            }
            return;
        }
        for channel in 0..num_channels {
            if unsafe { *buffers.add(channel as usize) }.is_null() {
                return;
            }
        }
        if unsafe { crate::engine_block_host::hirari_engine_block_is_shutdown(block) } != 0 {
            unsafe {
                let silence = crate::audio_buffer_ops::hirari_audio_buffer_storage_create();
                let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
                    silence, buffers, num_channels, num_samples,
                );
                crate::audio_buffer_ops::hirari_audio_buffer_storage_clear(
                    silence, 0, num_samples,
                );
                crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(silence);
                crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
            }
            return;
        }
        // The track work buffers and the sandbox protocol are prepared for
        // bounded engine blocks. A host/device callback may legally be larger
        // than that bound, so feed the same external buffer in bounded slices
        // rather than silently skipping every track whose prepared buffer is
        // smaller.
        let wrap = { crate::audio_buffer_ops::hirari_audio_buffer_storage_create() };
        unsafe {
            let _ = crate::audio_buffer_ops::hirari_audio_buffer_storage_wrap_channels(
                wrap, buffers, num_channels, num_samples,
            );
        }
        let mut offset: u32 = 0;
        while offset < num_samples {
            // Rust owns the engine block cap, the transport and the
            // cycle-boundary split, including the rewind a playhead that
            // reached the loop end needs.
            let plan = unsafe {
                crate::engine_block_host::hirari_engine_block_chunk_plan(
                    block,
                    num_samples - offset,
                )
            };
            if (plan >> 32) != 0 {
                continue;
            }
            let chunk = plan as u32;
            if chunk == 0 {
                continue;
            }
            unsafe { self.process_block(wrap, offset, chunk) };
            offset += chunk;
        }
        unsafe {
            crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(wrap);
            crate::engine_input_block::hirari_input_blocks_end_monitor(input_blocks);
            crate::engine_input_block::hirari_input_blocks_end_talkback(input_blocks);
        }
    }

    /// The whole device format transaction: validity gate, quiesce, format
    /// swap, graph preparation and the structural rebuild.
    pub fn prepare_to_play(&self, sample_rate: f64, block_size: u32) {
        // A failed device negotiation must not advance the audio configuration
        // generation or partially replace the currently valid preparation
        // state. The rate/block bounds live in Rust (audio_config_rules.rs).
        if !crate::audio_config_rules::hirari_audio_format_valid(sample_rate, block_size) {
            return;
        }
        // Close callback admission before waiting for in-flight callbacks.
        let block = self.block();
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_config_transition(block, 1);
        }
        struct TransitionGuard(*mut c_void);
        impl Drop for TransitionGuard {
            fn drop(&mut self) {
                unsafe {
                    crate::engine_block_host::hirari_engine_block_set_config_transition(
                        self.0, 0,
                    );
                }
            }
        }
        let _guard = TransitionGuard(block);
        unsafe {
            crate::engine_block_host::hirari_engine_block_join_callbacks(block);
        }
        {
            let _lock = self.lock_tracks();
            // Deferred Track objects are intentionally retained until
            // shutdown. prepareToPlay may race with a device callback that
            // still has the previous published snapshot, so transport state is
            // not a reclamation proof.
            unsafe {
                crate::engine_block_host::hirari_engine_block_set_device_format(
                    block, sample_rate, block_size,
                );
                crate::engine_block_host::hirari_engine_block_bump_generation(block);
                crate::engine_block_host::hirari_engine_block_clear_midi_events(block);
                crate::master_limiter::hirari_master_limiter_prepare(
                    self.handle(H_MASTER_LIMITER),
                    sample_rate,
                );
                crate::master_meter::hirari_master_meter_prepare(
                    self.handle(H_METER),
                    sample_rate,
                );
            }
            // One Rust walk over the track table: a freeze the new rate cannot
            // replay is dropped and retired with the layout marked, the
            // track's blocks and DSP retune, and a work block that reached the
            // new length re-offers its taps to the sidechain graph at this
            // generation. Status 2 is an impossible size and 3 an exhausted
            // allocation; the C++ facade threw across the bridge, so a Rust
            // panic reports the failed preparation the same way.
            let status = unsafe {
                crate::engine_block_host::hirari_engine_block_prepare_tracks(block)
            };
            if status == 2 {
                panic!("prepareToPlay: track block size exceeds the allocation limit");
            }
            if status == 3 {
                panic!("prepareToPlay: track buffer allocation failed");
            }
        }
        // sync_structural_changes takes the graph lock itself. Never call it
        // while the preparation lock is held.
        self.sync_structural_changes();
    }

    /// Starts or stops the transport. Stopping also reclaims retired region
    /// generations once the callback is quiescent.
    pub fn set_playing(&self, playing: bool) {
        let block = self.block();
        unsafe {
            crate::engine_block_host::hirari_engine_block_set_playing(
                block,
                u8::from(playing),
            );
        }
        if !playing {
            // The callback is quiescent once transport is stopped. Reclaim old
            // immutable region generations on the control thread only.
            unsafe {
                crate::engine_block_host::hirari_engine_block_join_callbacks(block);
            }
            let _lock = self.lock_tracks();
            let tracks = self.published_tracks_live_locked();
            for track in tracks {
                crate::region_store::hirari_region_store_trim_retired(track);
            }
        }
    }
}
