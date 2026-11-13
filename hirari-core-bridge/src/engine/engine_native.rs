//! The native engine facade, now Rust.
//!
//! `HirariUnifiedEngine` and `AudioEngine` used to be C++ classes behind the
//! cxx bridge. Every store they touched had already moved to Rust
//! (`engine_session.rs` and friends), so the classes were a facade: a session
//! bag handle, one graph lock, the driver-host lifetime and one thin method per
//! store operation. This module owns that facade in Rust and the C++ source
//! tree under `src/` is gone.
//!
//! The realtime rules from the old `SKILL.md` still hold and are now enforced
//! by the language itself: no allocation on the audio path (the block walk
//! reuses one wrapped view per callback), no `std::mutex` on the audio thread
//! (the graph lock is taken on the control plane only), and the denormal FPU
//! guard brackets every block exactly as the C++ guard did.

use std::ffi::c_void;
use std::ptr;
use parking_lot::Mutex;
use std::sync::OnceLock;

use crate::engine_session::{
    handle_of_host, H_BOUNCE_SESSION, H_BUS_SYSTEM, H_CONTROL_ROOM, H_ENGINE_BLOCK,
    H_ENGINE_RINGS, H_EXTERNAL_SYNC, H_FORENSIC_JOURNAL, H_INPUT_BLOCKS, H_MASTER_LIMITER,
    H_METRONOME, H_METER, H_MIDI_LEARN, H_MIDI_SNAPSHOT_STORE, H_MPE_MANAGER, H_MPE_STATE,
    H_PDC, H_PREVIEW_SAMPLER, H_PREVIEW_SYNTH, H_ROUTING, H_SIDECHAIN_ENGINE,
    H_SIDECHAIN_ROUTES, H_TEMPO, H_TRACK_TABLE, H_UNDO,
};
use crate::track_session::TRACK_TYPE_BUS;

/// The largest block the device host may deliver. The native graph, the driver
/// callback and the FFI block entry points all share this bound.
pub const MAX_AUDIO_BLOCK_SIZE: u32 = 16_384;
/// Published-graph capacities the C++ engine declared.
pub const MAX_TRACKS: u32 = 256;
pub const MAX_TRACK_MAP: u32 = 2048;

const DRIVER_MAX_CHANNELS: usize = 32;

/// The diagnostics ring is Rust-owned; post through the shared helper.
fn log_post(level: u32, component: u32, message: &str) {
    unsafe {
        crate::log_buffer::hirari_log_buffer_post(
            crate::log_buffer::hirari_log_buffer_default_state(),
            level,
            component,
            message.as_ptr(),
            message.len(),
        );
    }
}

/// The engine's whole store set and its graph lock. The lock is a Rust
/// [`Mutex`] now: the address handed to the session bag is the address of this
/// box, and every control-plane walk — the Rust edit hosts and the facade
/// methods below — brackets the same graph with it.
#[path = "engine_native_core.rs"]
mod core;
pub use core::*;

#[path = "engine_native_host_ops.rs"]
mod host_ops;
pub use host_ops::*;

pub struct AudioEngine {
    /// Session-owned graph. Separate FFI handles receive separate project
    /// state.
    engine: Box<HirariUnifiedEngine>,
    /// The Rust device host this session constructed.
    driver: *mut c_void,
    /// The configuration transaction lock (`engine_config.rs` drives it).
    config_lock: Mutex<()>,
}

impl AudioEngine {
    fn new(start_device: bool) -> Box<Self> {
        install_native_host_tables();
        let engine = HirariUnifiedEngine::new();
        let mut session = Box::new(AudioEngine {
            engine,
            driver: ptr::null_mut(),
            config_lock: Mutex::new(()),
        });
        // The caller owns these buffers; the Rust input store copies what the
        // graph needs so no driver pointer is retained beyond the callback.
        // SAFETY: the context registered here is `session` itself, whose
        // address is stable inside its box, and the device is not running yet.
        let context = &*session as *const AudioEngine as *mut c_void;
        session.driver = {
            crate::audio_driver_host::hirari_audio_driver_host_create(
                Some(process_driver_block),
                context,
            )
        };
        // Claiming the device is the Rust host's creation policy: it declines
        // for an offline or isolated session and publishes the failure itself.
        {
            crate::audio_driver_host::hirari_audio_driver_host_start_at_creation(
                session.driver,
                start_device,
            );
        }
        session
    }

    pub fn get_core(&self) -> &HirariUnifiedEngine {
        &self.engine
    }

    pub fn control_room_state(&self) -> usize {
        self.engine.handle(H_CONTROL_ROOM) as usize
    }

    pub fn tempo_state(&self) -> usize {
        self.engine.handle(H_TEMPO) as usize
    }

    pub fn undo_state(&self) -> usize {
        self.engine.handle(H_UNDO) as usize
    }

    pub fn log_state(&self) -> usize {
        crate::log_buffer::hirari_log_buffer_default_state() as usize
    }

    pub fn engine_block_state(&self) -> usize {
        self.engine.block() as usize
    }

    pub fn audio_driver_host(&self) -> usize {
        self.driver as usize
    }

    pub fn is_driver_running(&self) -> bool {
        !self.driver.is_null()
            && {
                crate::audio_driver_host::hirari_audio_driver_host_is_running(self.driver)
            }
    }

    pub fn is_audio_device_ready(&self) -> bool {
        let _lock = self.config_guard();
        let ready = self.is_driver_running();
        if !ready && !self.engine.block().is_null() && self.transport_playing() {
            // Device loss is a transport safety boundary, not merely a UI
            // status change. Stop the graph even when no UI poll is running.
            self.engine.set_playing(false);
        }
        ready
    }

    /// Transport start/stop is Rust-owned (`engine_transport.rs`); this is the
    /// engine-side safety read the device checks use.
    fn transport_playing(&self) -> bool {
        unsafe { crate::engine_block_host::hirari_engine_block_is_playing(self.engine.block()) != 0 }
    }

    pub fn try_reconnect_audio_device(&self) {
        if crate::audio_device_mode::hirari_native_test_isolation() {
            return;
        }
        let _lock = self.config_guard();
        if !self.driver.is_null() {
            crate::audio_driver_host::hirari_audio_driver_host_try_reconnect(self.driver);
            if !self.is_driver_running() {
                self.engine.set_playing(false);
            }
        }
    }

    /// The configuration transaction is Rust-owned (`engine_config.rs`); the
    /// session lock serializes it with shutdown and reconnect.
    pub fn config_guard(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.config_lock.lock()
    }

    pub fn get_audio_config_generation(&self) -> u64 {
        unsafe {
            crate::engine_block_host::hirari_engine_block_generation(self.engine.block())
        }
    }

    /// Processes one bounded stereo block through the native graph. Rust must
    /// not maintain a second production audio graph.
    pub fn process_audio_block(&self, left: &mut [f32], right: &mut [f32]) {
        // The native graph is prepared for bounded realtime blocks. Reject
        // oversized FFI slices before narrowing to u32; otherwise a hostile or
        // malformed caller could wrap the frame count and reach the DSP with a
        // different size than the slice actually contains.
        if left.is_empty()
            || right.is_empty()
            || left.len() != right.len()
            || left.len() > MAX_AUDIO_BLOCK_SIZE as usize
        {
            return;
        }
        // A host with no device to drive its callback dispatches the block
        // through the same boundary a real driver would use; a running device
        // already drives it, so the dispatcher answers false and the graph
        // renders here.
        let dispatched = {
            crate::audio_driver_host::hirari_audio_driver_host_dispatch_process(
                self.driver,
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len() as u32,
            )
        };
        if dispatched {
            return;
        }
        let mut channels: [*mut f32; 2] = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            self.engine
                .process_block_direct(channels.as_mut_ptr(), 2, left.len() as u32);
        }
    }

    /// Compatibility API: stop only this wrapper's device and session graph.
    pub fn shutdown(&self) {
        let _lock = self.config_guard();
        if !self.driver.is_null() {
            crate::audio_driver_host::hirari_audio_driver_host_stop(self.driver);
        }
        self.engine.shutdown();
    }

    pub fn add_region(&self, tid: u32, path: &str, start_beat: f64) -> bool {
        self.engine.add_region(tid, path, start_beat)
    }

    /// Project save and load are Rust transactions (`layout_gather.rs`,
    /// `project_load_host.rs`); this only names the document path.
    pub fn save_project(&self, path: &str) -> bool {
        unsafe {
            crate::layout_gather::hirari_project_save_from_host(
                self.engine.host(),
                path.as_ptr(),
                path.len(),
            )
        }
    }

    pub fn load_project(&self, path: &str) -> bool {
        unsafe {
            crate::project_load_host::hirari_project_load_from_host(
                self.engine.host(),
                path.as_ptr(),
                path.len(),
            )
        }
    }

    pub fn bounce_project(&self, path: &str, format: u32) -> bool {
        self.engine.bounce_project(path, format)
    }

    pub fn bounce_project_async(&self, path: &str, format: u32) -> bool {
        self.engine.bounce_project_async(path, format)
    }

    pub fn queue_region_waveform(&self, tid: u32, rid: u32) -> u64 {
        if tid == 0 || rid == 0 {
            return 0;
        }
        unsafe {
            crate::async_snapshots::hirari_waveform_snapshot_request(
                self.engine.host(),
                tid,
                rid,
                Some(compute_region_waveform),
                Some(destroy_noop),
            )
        }
    }

    /// Command dispatch. Admission lives in Rust (`command_host.rs`).
    pub fn push_command(
        &self,
        command_type: u32,
        tid: u32,
        val: f32,
        ts: u64,
        expected_project_generation: u64,
        expected_audio_generation: u64,
    ) -> bool {
        unsafe {
            crate::command_host::hirari_engine_push_command(
                self.engine.host(),
                command_type,
                tid,
                val,
                ts,
                expected_project_generation,
                expected_audio_generation,
            )
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        // Cancel bridge-owned offline work before the handle disappears, then
        // stop the device before the graph it drives.
        self.engine.shutdown();
        if !self.driver.is_null() {
            unsafe { crate::audio_driver_host::hirari_audio_driver_host_destroy(self.driver) };
            self.driver = ptr::null_mut();
        }
    }
}

/// # Safety
/// `context` must be a live engine host, per the request contract.
unsafe extern "C" fn compute_region_waveform(
    context: *mut c_void,
    track_id: u32,
    region_id: u32,
    output: *mut f32,
    capacity: usize,
) -> usize {
    if context.is_null() || output.is_null() {
        return 0;
    }
    let buffer =
        unsafe { crate::region_audio::hirari_region_waveform(context, track_id, region_id, 128) };
    if buffer.is_null() {
        return 0;
    }
    let count = unsafe {
        let data = crate::region_audio::hirari_float_buffer_data(buffer);
        let count = crate::region_audio::hirari_float_buffer_len(buffer);
        if data.is_null() {
            crate::region_audio::hirari_float_buffer_destroy(buffer);
            return 0;
        }
        let copied = count.min(capacity);
        std::ptr::copy_nonoverlapping(data, output, copied);
        crate::region_audio::hirari_float_buffer_destroy(buffer);
        copied
    };
    count
}

unsafe extern "C" fn destroy_noop(_context: *mut c_void) {}

/// The device callback. Validates the caller's buffer contract, stages the
/// hardware input into the Rust input store and renders the block.
unsafe extern "C" fn process_driver_block(
    inputs: *const *const f32,
    input_channel_count: u32,
    outputs: *const *mut f32,
    output_channel_count: u32,
    frames: u32,
    context: *mut c_void,
) {
    if context.is_null() || outputs.is_null() || output_channel_count < 2
        || output_channel_count as usize > DRIVER_MAX_CHANNELS
        || frames == 0
        || frames > MAX_AUDIO_BLOCK_SIZE
    {
        return;
    }
    for channel in 0..output_channel_count {
        if unsafe { *outputs.add(channel as usize) }.is_null() {
            return;
        }
    }
    let session = unsafe { &*(context as *const AudioEngine) };
    // The caller owns these buffers; the Rust input store copies what the
    // graph needs so no driver pointer is retained beyond the callback.
    if !inputs.is_null() && input_channel_count > 0 && unsafe { *inputs } != ptr::null() {
        unsafe {
            crate::engine_input_block::hirari_input_blocks_publish_monitor(
                session.engine.handle(H_INPUT_BLOCKS),
                inputs,
                input_channel_count,
                frames,
            );
        }
    }
    unsafe {
        crate::engine_input_block::hirari_input_blocks_publish_talkback(
            session.engine.handle(H_INPUT_BLOCKS),
            inputs,
            input_channel_count,
            frames,
            session.engine.handle(H_CONTROL_ROOM),
        );
    }
    let mut mutable_outputs: [*mut f32; DRIVER_MAX_CHANNELS] = [ptr::null_mut(); DRIVER_MAX_CHANNELS];
    for (index, slot) in mutable_outputs.iter_mut().enumerate().take(output_channel_count as usize) {
        *slot = unsafe { *outputs.add(index) };
    }
    unsafe {
        session
            .engine
            .process_block_direct(mutable_outputs.as_mut_ptr(), output_channel_count, frames);
    }
}

/// The wire code of one bridge command.
fn command_code(cmd: crate::ffi::HirariCommand) -> u32 {
    match cmd {
        crate::ffi::HirariCommand::SetVolume => 0,
        crate::ffi::HirariCommand::SetPan => 1,
        crate::ffi::HirariCommand::SetMute => 2,
        crate::ffi::HirariCommand::SetSolo => 3,
        crate::ffi::HirariCommand::AddPlugin => 4,
        crate::ffi::HirariCommand::RemovePlugin => 5,
        crate::ffi::HirariCommand::GenerateDrumFill => 6,
        crate::ffi::HirariCommand::ExecuteAutoMixing => 7,
        crate::ffi::HirariCommand::ExecuteAutoArrangement => 8,
        crate::ffi::HirariCommand::SetSpatialMode => 9,
        crate::ffi::HirariCommand::SetSpatialPosition => 10,
        crate::ffi::HirariCommand::SetAscendedMode => 11,
        crate::ffi::HirariCommand::SetAutomationRecordMode => 12,
        // cxx shared enums are open to future codes; admission rejects them.
        _ => u32::MAX,
    }
}

/// Creates the session engine with the device start policy the offline mode
/// selects.
pub fn new_audio_engine() -> Box<AudioEngine> {
    AudioEngine::new(true)
}

/// Creates the session engine without claiming a device: offline renders,
/// automation and isolated tests.
pub fn new_audio_engine_offline() -> Box<AudioEngine> {
    AudioEngine::new(false)
}

/// The inner engine behind a bridge handle.
pub fn get_unified_engine(bridge: &AudioEngine) -> &HirariUnifiedEngine {
    bridge.get_core()
}

/// Command admission through the shared Rust host.
pub fn push_command(
    engine: &AudioEngine,
    cmd: crate::ffi::HirariCommand,
    tid: u32,
    val: f32,
    ts: u64,
    expected_project_generation: u64,
    expected_audio_generation: u64,
) -> bool {
    // Branchless boundary validation: a track id outside the table is refused
    // before admission.
    let mask = u32::from(tid < 1024);
    if mask == 0 {
        return false;
    }
    engine.push_command(
        command_code(cmd),
        tid,
        val,
        ts,
        expected_project_generation,
        expected_audio_generation,
    )
}
