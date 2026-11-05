//! The engine's session bag.
//!
//! `HirariUnifiedEngine` used to declare one member per Rust object and create
//! and destroy them itself, which meant the C++ translation units owned the
//! construction order, the teardown order and the leaked duplicates a hand
//! written list always drifts into. This module owns all of that instead: the
//! engine holds one handle, and every object the graph, the callback and the
//! control surface read is a slot in this bag.
//!
//! The engine hands its own address to the bag when it creates it, so Rust
//! reading a `void* host` callback argument can reach the stores behind that
//! engine without the engine translating the request. The binding is dropped
//! with the bag, which is what makes a recycled engine address safe.

use crate::engine_block_host::{
    hirari_engine_block_create, hirari_engine_block_destroy, HirariEngineBlockHandles,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};

/// The largest block the device host may deliver, and the capacity the input
/// store is sized for.
const MAX_BLOCK_FRAMES: usize = 16_384;
/// The default device format the stores are created against before the first
/// `prepareToPlay`.
const DEFAULT_SAMPLE_RATE: f64 = 44_100.0;
/// The undo history depth and its coalescing window, in milliseconds.
const UNDO_MAX_DEPTH: usize = 128;
const UNDO_COALESCE_MS: u64 = 300;

pub const H_TRACK_TABLE: u32 = 0;
pub const H_INPUT_BLOCKS: u32 = 1;
pub const H_METRONOME: u32 = 2;
pub const H_ENGINE_RINGS: u32 = 3;
pub const H_MIDI_PLAYBACK_CLOCK: u32 = 4;
pub const H_MPE_STATE: u32 = 5;
pub const H_PREVIEW_SYNTH: u32 = 6;
pub const H_PREVIEW_SAMPLER: u32 = 7;
pub const H_METER: u32 = 8;
pub const H_MASTER_LIMITER: u32 = 9;
pub const H_ENGINE_BLOCK: u32 = 10;
pub const H_MIXER_TELEMETRY: u32 = 11;
pub const H_BOUNCE_SESSION: u32 = 12;
pub const H_SIDECHAIN_ROUTES: u32 = 13;
pub const H_MIDI_SNAPSHOT_STORE: u32 = 14;
pub const H_PDC: u32 = 15;
pub const H_UNDO: u32 = 16;
pub const H_TEMPO: u32 = 17;
pub const H_MPE_MANAGER: u32 = 18;
pub const H_ROUTING: u32 = 19;
pub const H_SIDECHAIN_ENGINE: u32 = 20;
pub const H_BUS_SYSTEM: u32 = 21;
pub const H_MACRO_CONTROL: u32 = 22;
pub const H_MIDI_LEARN: u32 = 23;
pub const H_FORENSIC_JOURNAL: u32 = 24;
pub const H_CONTROL_ROOM: u32 = 25;
pub const H_EXTERNAL_SYNC: u32 = 26;
pub const H_ARRANGEMENT_DOCUMENT: u32 = 27;
pub const H_LAYOUT_REVISION: u32 = 28;
/// The address of the engine's own tracks mutex, borrowed rather than owned: it
/// is a C++ member, so the bag never releases it. A bag built without an engine
/// — a test fixture — carries null, which its sessions read as no lock to take.
pub const H_TRACKS_MUTEX: u32 = 29;
const HANDLE_COUNT: usize = 30;

/// The bags built so far, keyed by the engine host that built them. Rust code
/// driven from a `void* host` callback argument reads a store through this map;
/// an entry exists exactly as long as the engine that created the bag does.
/// Addresses are stored rather than pointers because the map is process-wide
/// state and only the engine's own lifetime makes either side valid.
fn hosts() -> &'static Mutex<HashMap<usize, usize>> {
    static HOSTS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bind_host(host: *mut c_void, session: *mut EngineSession) {
    if host.is_null() || session.is_null() {
        return;
    }
    let Ok(mut table) = hosts().lock() else { return };
    table.insert(host as usize, session as usize);
}

fn unbind_host(host: *mut c_void, session: *const EngineSession) {
    if host.is_null() {
        return;
    }
    // Only drop the binding this bag owns: an engine released after a later one
    // took over the same address must not retire the newcomer.
    let Ok(mut table) = hosts().lock() else { return };
    if table.get(&(host as usize)) == Some(&(session as usize)) {
        table.remove(&(host as usize));
    }
}

/// The store behind one slot of the bag the given engine host created. Null for
/// an unknown host, an unknown kind and a slot the bag has already retired.
pub fn handle_of_host(host: *const c_void, kind: u32) -> *mut c_void {
    let Ok(table) = hosts().lock() else {
        return ptr::null_mut();
    };
    let Some(session) = table.get(&(host as usize)).copied() else {
        return ptr::null_mut();
    };
    drop(table);
    hirari_engine_session_handle(session as *const c_void, kind)
}

extern "C" {
    fn hirari_engine_track_table_create() -> *mut c_void;
    fn hirari_engine_track_table_destroy(table: *mut c_void);
    fn hirari_input_blocks_create(capacity_frames: usize) -> *mut c_void;
    fn hirari_input_blocks_destroy(blocks: *mut c_void);
    fn hirari_metronome_create(sample_rate: f64) -> *mut c_void;
    fn hirari_metronome_destroy(state: *mut c_void);
    fn hirari_engine_rings_create() -> *mut c_void;
    fn hirari_engine_rings_destroy(rings: *mut c_void);
    fn hirari_midi_playback_clock_create() -> *mut c_void;
    fn hirari_midi_playback_clock_destroy(clock: *mut c_void);
    fn hirari_mpe_state_create() -> *mut c_void;
    fn hirari_mpe_state_free(state: *mut c_void);
    fn hirari_preview_synth_create() -> *mut c_void;
    fn hirari_preview_synth_destroy(state: *mut c_void);
    fn hirari_preview_sampler_create() -> *mut c_void;
    fn hirari_preview_sampler_destroy(state: *mut c_void);
    fn hirari_master_meter_create(sample_rate: f64) -> *mut c_void;
    fn hirari_master_meter_destroy(state: *mut c_void);
    fn hirari_master_limiter_create(sample_rate: f64) -> *mut c_void;
    fn hirari_master_limiter_destroy(state: *mut c_void);
    fn hirari_mixer_telemetry_create() -> *mut c_void;
    fn hirari_mixer_telemetry_destroy(state: *mut c_void);
    fn hirari_bounce_session_create() -> *mut c_void;
    fn hirari_bounce_session_destroy(session: *mut c_void);
    fn hirari_sidechain_routes_create() -> *mut c_void;
    fn hirari_sidechain_routes_destroy(state: *mut c_void);
    fn hirari_midi_snapshot_store_create() -> *mut c_void;
    fn hirari_midi_snapshot_store_destroy(store: *mut c_void);
    fn hirari_native_pdc_create() -> *mut c_void;
    fn hirari_native_pdc_destroy(state: *mut c_void);
    fn hirari_native_pdc_bind_control_thread(state: *const c_void) -> bool;
    fn hirari_undo_manager_create(max_depth: usize, coalesce_window_ms: u64) -> *mut c_void;
    fn hirari_undo_manager_destroy(state: *mut c_void);
    fn hirari_tempo_map_create() -> *mut c_void;
    fn hirari_tempo_map_destroy(state: *mut c_void);
    fn hirari_mpe_manager_create() -> *mut c_void;
    fn hirari_mpe_manager_destroy(state: *mut c_void);
    fn hirari_routing_engine_create() -> *mut c_void;
    fn hirari_routing_engine_destroy(state: *mut c_void);
    fn hirari_sidechain_engine_create() -> *mut c_void;
    fn hirari_sidechain_engine_destroy(state: *mut c_void);
    fn hirari_bus_system_create() -> *mut c_void;
    fn hirari_bus_system_destroy(state: *mut c_void);
    fn hirari_macro_mapping_create() -> *mut c_void;
    fn hirari_macro_mapping_destroy(state: *mut c_void);
    fn hirari_midi_learn_create() -> *mut c_void;
    fn hirari_midi_learn_destroy(state: *mut c_void);
    fn hirari_forensic_journaler_create() -> *mut c_void;
    fn hirari_forensic_journaler_destroy(state: *mut c_void);
    fn hirari_forensic_journaler_stop(state: *mut c_void);
    fn hirari_control_room_state_create() -> *mut c_void;
    fn hirari_control_room_state_destroy(state: *mut c_void);
    fn hirari_external_sync_create() -> *mut c_void;
    fn hirari_external_sync_destroy(state: *mut c_void);
    fn hirari_arrangement_document_create() -> *mut c_void;
    fn hirari_arrangement_document_destroy(state: *mut c_void);
    fn hirari_layout_revision_create() -> *mut c_void;
    fn hirari_layout_revision_release(state: *mut c_void);
}

pub struct EngineSession {
    /// The engine that built this bag, so the host binding can be dropped with
    /// it. Null for a bag no engine owns, such as one a test built directly.
    host: *mut c_void,
    handles: [*mut c_void; HANDLE_COUNT],
}

impl EngineSession {
    /// Builds the bag in the order the stores depend on: every store the block
    /// traverses exists before the block binds them, and the PDC control-thread
    /// binding is taken before any graph edit can reach it. `tracks_mutex` is the
    /// engine's own lock, borrowed so a Rust walk brackets the same graph the
    /// engine's remaining C++ edits do.
    fn new(tracks_mutex: *mut c_void) -> Self {
        let mut handles: [*mut c_void; HANDLE_COUNT] = [ptr::null_mut(); HANDLE_COUNT];
        handles[H_TRACKS_MUTEX as usize] = tracks_mutex;
        handles[H_TRACK_TABLE as usize] = unsafe { hirari_engine_track_table_create() };
        handles[H_INPUT_BLOCKS as usize] = unsafe { hirari_input_blocks_create(MAX_BLOCK_FRAMES) };
        handles[H_METRONOME as usize] = unsafe { hirari_metronome_create(DEFAULT_SAMPLE_RATE) };
        handles[H_ENGINE_RINGS as usize] = unsafe { hirari_engine_rings_create() };
        handles[H_MIDI_PLAYBACK_CLOCK as usize] = unsafe { hirari_midi_playback_clock_create() };
        handles[H_MIDI_SNAPSHOT_STORE as usize] = unsafe { hirari_midi_snapshot_store_create() };
        handles[H_MPE_STATE as usize] = unsafe { hirari_mpe_state_create() };
        handles[H_PREVIEW_SYNTH as usize] = unsafe { hirari_preview_synth_create() };
        handles[H_PREVIEW_SAMPLER as usize] = unsafe { hirari_preview_sampler_create() };
        handles[H_METER as usize] = unsafe { hirari_master_meter_create(DEFAULT_SAMPLE_RATE) };
        handles[H_MASTER_LIMITER as usize] =
            unsafe { hirari_master_limiter_create(DEFAULT_SAMPLE_RATE) };
        handles[H_MIXER_TELEMETRY as usize] = unsafe { hirari_mixer_telemetry_create() };
        handles[H_BOUNCE_SESSION as usize] = unsafe { hirari_bounce_session_create() };
        handles[H_SIDECHAIN_ROUTES as usize] = unsafe { hirari_sidechain_routes_create() };
        handles[H_PDC as usize] = unsafe { hirari_native_pdc_create() };
        // Binding is advisory: an engine that cannot claim the control thread
        // still renders, exactly as the ignored C++ status did.
        let _ = unsafe { hirari_native_pdc_bind_control_thread(handles[H_PDC as usize]) };
        handles[H_UNDO as usize] =
            unsafe { hirari_undo_manager_create(UNDO_MAX_DEPTH, UNDO_COALESCE_MS) };
        handles[H_TEMPO as usize] = unsafe { hirari_tempo_map_create() };
        handles[H_MPE_MANAGER as usize] = unsafe { hirari_mpe_manager_create() };
        handles[H_ROUTING as usize] = unsafe { hirari_routing_engine_create() };
        handles[H_SIDECHAIN_ENGINE as usize] = unsafe { hirari_sidechain_engine_create() };
        handles[H_BUS_SYSTEM as usize] = unsafe { hirari_bus_system_create() };
        handles[H_MACRO_CONTROL as usize] = unsafe { hirari_macro_mapping_create() };
        handles[H_MIDI_LEARN as usize] = unsafe { hirari_midi_learn_create() };
        handles[H_FORENSIC_JOURNAL as usize] = unsafe { hirari_forensic_journaler_create() };
        handles[H_CONTROL_ROOM as usize] = unsafe { hirari_control_room_state_create() };
        handles[H_EXTERNAL_SYNC as usize] = unsafe { hirari_external_sync_create() };
        handles[H_ARRANGEMENT_DOCUMENT as usize] = unsafe { hirari_arrangement_document_create() };
        handles[H_LAYOUT_REVISION as usize] = unsafe { hirari_layout_revision_create() };
        handles[H_ENGINE_BLOCK as usize] = unsafe {
            hirari_engine_block_create(HirariEngineBlockHandles {
                routing: handles[H_ROUTING as usize],
                track_table: handles[H_TRACK_TABLE as usize],
                bounce: handles[H_BOUNCE_SESSION as usize],
                input_blocks: handles[H_INPUT_BLOCKS as usize],
                control_room: handles[H_CONTROL_ROOM as usize],
                mixer_telemetry: handles[H_MIXER_TELEMETRY as usize],
                metronome: handles[H_METRONOME as usize],
                master_limiter: handles[H_MASTER_LIMITER as usize],
                meter: handles[H_METER as usize],
                tempo: handles[H_TEMPO as usize],
                bus_system: handles[H_BUS_SYSTEM as usize],
                sidechain: handles[H_SIDECHAIN_ENGINE as usize],
                rings: handles[H_ENGINE_RINGS as usize],
                midi_clock: handles[H_MIDI_PLAYBACK_CLOCK as usize],
                midi_snapshots: handles[H_MIDI_SNAPSHOT_STORE as usize],
                preview_synth: handles[H_PREVIEW_SYNTH as usize],
                preview_sampler: handles[H_PREVIEW_SAMPLER as usize],
                mpe: handles[H_MPE_STATE as usize],
                macro_control: handles[H_MACRO_CONTROL as usize],
                layout_revision: handles[H_LAYOUT_REVISION as usize],
            })
        };
        EngineSession {
            host: ptr::null_mut(),
            handles,
        }
    }

    /// Takes the graph and the hardware input store out of the bag: the engine
    /// has joined its callbacks, so nothing else can reach them, and a later
    /// teardown must not touch the slots again.
    fn close_graph(&mut self) {
        for kind in [H_TRACK_TABLE, H_INPUT_BLOCKS] {
            let handle = std::mem::replace(&mut self.handles[kind as usize], ptr::null_mut());
            if kind == H_TRACK_TABLE {
                unsafe { hirari_engine_track_table_destroy(handle) };
            } else {
                unsafe { hirari_input_blocks_destroy(handle) };
            }
        }
    }

    /// Releases everything the bag still holds. The block goes first because it
    /// borrows every other slot, and the journal is stopped before it is freed
    /// because its writer thread reads the stores the later slots own.
    fn release(&mut self) {
        self.close_graph();
        let take = |session: &mut Self, kind: u32| -> *mut c_void {
            std::mem::replace(&mut session.handles[kind as usize], ptr::null_mut())
        };
        unsafe {
            hirari_engine_block_destroy(take(self, H_ENGINE_BLOCK));
            hirari_bounce_session_destroy(take(self, H_BOUNCE_SESSION));
            hirari_engine_rings_destroy(take(self, H_ENGINE_RINGS));
            hirari_midi_snapshot_store_destroy(take(self, H_MIDI_SNAPSHOT_STORE));
            hirari_midi_playback_clock_destroy(take(self, H_MIDI_PLAYBACK_CLOCK));
            hirari_control_room_state_destroy(take(self, H_CONTROL_ROOM));
            hirari_native_pdc_destroy(take(self, H_PDC));
            hirari_tempo_map_destroy(take(self, H_TEMPO));
            hirari_external_sync_destroy(take(self, H_EXTERNAL_SYNC));
            hirari_arrangement_document_destroy(take(self, H_ARRANGEMENT_DOCUMENT));
            hirari_metronome_destroy(take(self, H_METRONOME));
            hirari_mixer_telemetry_destroy(take(self, H_MIXER_TELEMETRY));
            hirari_macro_mapping_destroy(take(self, H_MACRO_CONTROL));
            hirari_midi_learn_destroy(take(self, H_MIDI_LEARN));
            hirari_preview_synth_destroy(take(self, H_PREVIEW_SYNTH));
            hirari_preview_sampler_destroy(take(self, H_PREVIEW_SAMPLER));
            hirari_mpe_state_free(take(self, H_MPE_STATE));
            hirari_routing_engine_destroy(take(self, H_ROUTING));
            hirari_sidechain_routes_destroy(take(self, H_SIDECHAIN_ROUTES));
            hirari_sidechain_engine_destroy(take(self, H_SIDECHAIN_ENGINE));
            hirari_mpe_manager_destroy(take(self, H_MPE_MANAGER));
            hirari_bus_system_destroy(take(self, H_BUS_SYSTEM));
            let journal = take(self, H_FORENSIC_JOURNAL);
            hirari_forensic_journaler_stop(journal);
            hirari_forensic_journaler_destroy(journal);
            hirari_master_limiter_destroy(take(self, H_MASTER_LIMITER));
            hirari_master_meter_destroy(take(self, H_METER));
            hirari_undo_manager_destroy(take(self, H_UNDO));
            hirari_layout_revision_release(take(self, H_LAYOUT_REVISION));
        }
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.release();
    }
}

fn session(state: *mut c_void) -> Option<&'static mut EngineSession> {
    if state.is_null() {
        return None;
    }
    Some(unsafe { &mut *(state as *mut EngineSession) })
}

/// Builds the engine's whole store set for one engine instance and records the
/// engine address that owns it, so a Rust host callback can reach these stores
/// without asking the engine to translate the request. `tracks_mutex` is the
/// address of the engine's own graph lock, or null for a bag with no engine.
#[no_mangle]
pub extern "C" fn hirari_engine_session_create(host: *mut c_void, tracks_mutex: *mut c_void) -> *mut c_void {
    let mut session = Box::new(EngineSession::new(tracks_mutex));
    session.host = host;
    let pointer = Box::into_raw(session);
    bind_host(host, pointer);
    pointer.cast()
}

/// The handle of one slot, or null for an unknown kind and for a slot this bag
/// has already retired.
#[no_mangle]
pub extern "C" fn hirari_engine_session_handle(state: *const c_void, kind: u32) -> *mut c_void {
    if state.is_null() {
        return ptr::null_mut();
    }
    let session = unsafe { &*(state as *const EngineSession) };
    usize::try_from(kind)
        .ok()
        .filter(|kind| *kind < HANDLE_COUNT)
        .map_or(ptr::null_mut(), |kind| session.handles[kind])
}

/// Retires the track table and the hardware input store after the engine has
/// joined its callbacks. Idempotent, because the slots go null with them.
#[no_mangle]
pub extern "C" fn hirari_engine_session_close_graph(state: *mut c_void) {
    if let Some(session) = session(state) {
        session.close_graph();
    }
}

/// Releases the whole bag, its host binding and the bag itself.
#[no_mangle]
pub extern "C" fn hirari_engine_session_destroy(state: *mut c_void) {
    if state.is_null() {
        return;
    }
    let session = state.cast::<EngineSession>();
    let host = unsafe { (*session).host };
    unbind_host(host, session);
    drop(unsafe { Box::from_raw(session) });
}
