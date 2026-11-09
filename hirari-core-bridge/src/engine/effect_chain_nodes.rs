//! Chain nodes the Rust effect chain owns outright.
//!
//! Every insert slot is one of these: a built-in catalog processor (the Rust
//! native DSP behind `native_plugin_host`) or an isolated worker
//! (`plugin_sandbox_host::chain`). The C++ engine used to wrap both in a
//! `DSP::IProcessor` vtable and hand the pointer back through a sixteen-slot
//! adapter table; the chain now calls them directly.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::midi_buffer_ops::MidiView;
use crate::native_plugin_host::{
    hirari_native_plugin_host_create, hirari_native_plugin_host_destroy,
    hirari_native_plugin_host_latency_samples, hirari_native_plugin_host_parameter,
    hirari_native_plugin_host_parameter_count, hirari_native_plugin_host_prepare,
    hirari_native_plugin_host_process, hirari_native_plugin_host_reset,
    hirari_native_plugin_host_set_parameter, hirari_native_plugin_host_state_restore,
    hirari_native_plugin_host_state_save, hirari_native_plugin_host_tail_samples,
};
use crate::plugin_sandbox_host::chain::SandboxNode;
use crate::processor_state::{hirari_processor_state_decode, hirari_processor_state_encode};

/// Chain-entry kind for an isolated worker; built-in kinds are catalog ids.
pub const SANDBOX_KIND: u32 = u32::MAX;
/// Chain metric ids, kept aligned with the C++ diagnostic pollers.
pub const METRIC_WATCHDOG_TRIPS: u32 = 0;
pub const METRIC_NON_FINITE_SAMPLES: u32 = 1;
pub const METRIC_TAIL_SAMPLES: u32 = 2;

const BYTES_PER_STATE_ENVELOPE: usize = 16;

/// One writable audio block: a channel pointer table plus its own dimensions.
#[derive(Clone, Copy)]
pub struct BlockAudio {
    planes: *const *mut f32,
    channels: u32,
    samples: u32,
}

impl BlockAudio {
    pub fn new(planes: *const *mut f32, channels: u32, samples: u32) -> Self {
        Self {
            planes,
            channels,
            samples,
        }
    }

    pub fn plane(&self, index: usize) -> *mut f32 {
        if self.planes.is_null() || index >= self.channels as usize {
            return std::ptr::null_mut();
        }
        unsafe { *self.planes.add(index) }
    }

    fn slice(&self, index: usize, frames: usize) -> Option<&mut [f32]> {
        let pointer = self.plane(index);
        if pointer.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts_mut(pointer, frames) })
    }

    pub fn pointer_array(&self) -> *mut *mut f32 {
        self.planes.cast_mut()
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Zeroes every channel for the block's own sample count.
    pub fn clear(&self) {
        for index in 0..self.channels as usize {
            if let Some(plane) = self.slice(index, self.samples as usize) {
                plane.fill(0.0);
            }
        }
    }
}

enum NodeKind {
    /// Built-in catalog processor: the opaque Rust native host handle.
    Internal(*mut c_void),
    Sandbox(Box<SandboxNode>),
}

pub struct ChainNode {
    kind: NodeKind,
    /// Wet/dry send amount. Read by the realtime traversal for parallel
    /// branches and persisted in the host state envelope.
    mix: AtomicU32,
    /// Bypass as last restored from persisted state. Chain-level bypass drives
    /// the traversal; this mirror only round-trips through a project save.
    restored_bypassed: AtomicBool,
    sidechain_bus_id: AtomicU32,
}

// The raw native handle is only dereferenced through the Rust host functions,
// which are themselves the realtime boundary; sandbox nodes already are Sync.
unsafe impl Send for ChainNode {}
unsafe impl Sync for ChainNode {}

impl Drop for ChainNode {
    fn drop(&mut self) {
        if let NodeKind::Internal(host) = self.kind {
            if !host.is_null() {
                unsafe { hirari_native_plugin_host_destroy(host) };
            }
        }
    }
}

impl ChainNode {
    /// A built-in catalog processor. Returns none when the catalog path cannot
    /// produce a host, which the caller reports as a failed insert.
    pub fn internal(path: &[u8]) -> Option<Box<Self>> {
        let host = unsafe { hirari_native_plugin_host_create(path.as_ptr(), path.len()) };
        if host.is_null() {
            return None;
        }
        Some(Box::new(Self {
            kind: NodeKind::Internal(host),
            mix: AtomicU32::new(1.0f32.to_bits()),
            restored_bypassed: AtomicBool::new(false),
            sidechain_bus_id: AtomicU32::new(0),
        }))
    }

    pub fn sandbox(node: Box<SandboxNode>) -> Box<Self> {
        Box::new(Self {
            kind: NodeKind::Sandbox(node),
            mix: AtomicU32::new(1.0f32.to_bits()),
            restored_bypassed: AtomicBool::new(false),
            sidechain_bus_id: AtomicU32::new(0),
        })
    }

    pub fn sandbox_node(&self) -> Option<&SandboxNode> {
        match &self.kind {
            NodeKind::Sandbox(node) => Some(node),
            NodeKind::Internal(_) => None,
        }
    }

    pub fn is_sandbox(&self) -> bool {
        self.sandbox_node().is_some()
    }

    pub fn mix(&self) -> f32 {
        f32::from_bits(self.mix.load(Ordering::Relaxed))
    }

    fn set_mix(&self, mix: f32) {
        self.mix.store(mix.to_bits(), Ordering::Relaxed);
    }

    pub fn prepare(&self, sample_rate: f64, block_size: u32) {
        match &self.kind {
            // The built-in host sizes its own scratch from the sample rate; the
            // engine's block was always ignored, so forwarding it would change
            // nothing but invite the opposite assumption.
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_prepare(*host, sample_rate, 0) },
            NodeKind::Sandbox(node) => node.prepare(sample_rate, block_size),
        }
    }

    pub fn reset(&self) {
        match &self.kind {
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_reset(*host) },
            NodeKind::Sandbox(node) => node.reset(),
        }
    }

    /// Runs one block. The audio block may be longer than the block the engine
    /// asked for and a built-in processor consumes the whole storage, exactly as
    /// the C++ virtuals did. A false return tells the traversal to silence the
    /// block; the bypass rule mirrors the C++ virtual that skipped processing
    /// while still counting the node as visited.
    pub fn process(&self, audio: &BlockAudio, bpm: f64, midi: *mut c_void) -> bool {
        match &self.kind {
            NodeKind::Internal(host) => {
                if self.restored_bypassed.load(Ordering::Relaxed) || host.is_null() {
                    return true;
                }
                unsafe {
                    hirari_native_plugin_host_process(
                        *host,
                        audio.pointer_array(),
                        audio.channels(),
                        audio.samples(),
                        bpm,
                    )
                }
                true
            }
            NodeKind::Sandbox(node) => node.process(audio, &mut MidiView::new(midi)),
        }
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        match &self.kind {
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_set_parameter(*host, id, value) },
            NodeKind::Sandbox(node) => node.set_parameter(id, value, 0),
        }
    }

    /// Sample-accurate automation. Built-in processors only accept block-start
    /// values (the C++ base virtual dropped later offsets); isolated workers
    /// carry the offset through the realtime mailbox.
    pub fn set_parameter_at_sample(&self, id: u32, value: f32, sample_offset: u32) {
        match &self.kind {
            NodeKind::Internal(host) => {
                if sample_offset == 0 {
                    unsafe { hirari_native_plugin_host_set_parameter(*host, id, value) };
                }
            }
            NodeKind::Sandbox(node) => node.set_parameter(id, value, sample_offset),
        }
    }

    pub fn parameter(&self, id: u32) -> f32 {
        match &self.kind {
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_parameter(*host, id) },
            NodeKind::Sandbox(node) => node.parameter(id),
        }
    }

    pub fn num_parameters(&self) -> u32 {
        match &self.kind {
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_parameter_count(*host) },
            NodeKind::Sandbox(_) => 0,
        }
    }

    pub fn latency_samples(&self) -> u32 {
        match &self.kind {
            NodeKind::Internal(host) => unsafe { hirari_native_plugin_host_latency_samples(*host) },
            NodeKind::Sandbox(node) => node.latency_samples(),
        }
    }

    pub fn is_midi_instrument(&self) -> bool {
        match &self.kind {
            NodeKind::Internal(_) => false,
            NodeKind::Sandbox(node) => node.is_midi_instrument(),
        }
    }

    pub fn metric(&self, metric: u32) -> u64 {
        match (&self.kind, metric) {
            (NodeKind::Internal(host), METRIC_TAIL_SAMPLES) => unsafe {
                u64::from(hirari_native_plugin_host_tail_samples(*host))
            },
            _ => 0,
        }
    }

    /// Host state envelope for a built-in with no serializer of its own.
    fn envelope(&self) -> Vec<u8> {
        let mut state = vec![0_u8; BYTES_PER_STATE_ENVELOPE];
        unsafe {
            hirari_processor_state_encode(
                self.restored_bypassed.load(Ordering::Relaxed),
                self.mix(),
                self.sidechain_bus_id.load(Ordering::Relaxed),
                state.as_mut_ptr(),
                state.len(),
            )
        };
        state
    }

    fn accept_envelope(&self, data: &[u8]) -> bool {
        let mut bypassed = 0_u8;
        let mut mix = 0.0_f32;
        let mut sidechain = 0_u32;
        if unsafe {
            hirari_processor_state_decode(
                data.as_ptr(),
                data.len(),
                &mut bypassed,
                &mut mix,
                &mut sidechain,
            )
        } {
            self.restored_bypassed.store(bypassed != 0, Ordering::Relaxed);
            self.set_mix(mix);
            self.sidechain_bus_id.store(sidechain, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub fn state(&self) -> Vec<u8> {
        match &self.kind {
            NodeKind::Sandbox(node) => node.state(),
            NodeKind::Internal(host) => {
                let mut buffer = vec![0_u8; 32];
                let written = unsafe {
                    hirari_native_plugin_host_state_save(
                        *host,
                        u8::from(self.restored_bypassed.load(Ordering::Relaxed)),
                        self.mix(),
                        self.sidechain_bus_id.load(Ordering::Relaxed),
                        buffer.as_mut_ptr(),
                        buffer.len(),
                    )
                };
                // Kinds without their own serializer fall back to the shared
                // host envelope, which stays authoritative for the catalog.
                if written == 0 {
                    return self.envelope();
                }
                buffer.truncate(written);
                buffer
            }
        }
    }

    pub fn restore_state(&self, data: &[u8]) -> bool {
        match &self.kind {
            NodeKind::Sandbox(node) => node.restore_state(data),
            NodeKind::Internal(host) => {
                let mut bypassed = 0_u8;
                let mut mix = 1.0_f32;
                let mut sidechain = 0_u32;
                let restored = unsafe {
                    hirari_native_plugin_host_state_restore(
                        *host,
                        data.as_ptr(),
                        data.len(),
                        &mut bypassed,
                        &mut mix,
                        &mut sidechain,
                    )
                };
                if restored {
                    self.restored_bypassed.store(bypassed != 0, Ordering::Relaxed);
                    self.set_mix(mix);
                    self.sidechain_bus_id.store(sidechain, Ordering::Relaxed);
                    return true;
                }
                self.accept_envelope(data)
            }
        }
    }

    /// GUI state stays plugin-owned and empty for both chain node kinds; the
    /// project container still round-trips the slot.
    pub fn gui_state(&self) -> Vec<u8> {
        Vec::new()
    }

    pub fn restore_gui_state(&self, _data: &[u8]) -> bool {
        true
    }
}
