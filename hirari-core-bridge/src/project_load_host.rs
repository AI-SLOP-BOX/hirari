//! Project load is one control-plane transaction, and Rust runs it.
//!
//! The engine used to own the sequence: decode the document, build provisional
//! tracks, hydrate their chains and regions, prepare them, swap the graph, then
//! restore routing, buses, tempo, markers, arranger parts, sidechains and the
//! project scale before re-preparing the whole graph at the document's rate.
//! Every step of that already lived in Rust stores; the C++ body only walked
//! them in order and turned a decoded Rust document back into opaque handles.
//! Walking it here means the ordering, the failure bail-outs and the ownership
//! of a half-built track are one Rust function, and the engine keeps a single
//! relay.
//!
//! The transaction's atomicity promise is unchanged: everything above the
//! commit point runs while the previous project stays published, so a malformed
//! sidechain, an unloadable insert or a track that cannot process the new block
//! size leaves the open project intact.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::arrangement_document::{
    hirari_arrangement_document_reset_markers, hirari_arrangement_document_set_parts,
    hirari_arrangement_document_upsert_marker, HirariArrangementPartInput,
};
use crate::bus_audio::{hirari_bus_system_bus, hirari_bus_system_register};
use crate::engine_block_host::{
    hirari_engine_block_device_block_frames, hirari_engine_block_join_callbacks,
    hirari_engine_block_layout_revision, hirari_engine_block_set_project_scale,
};
use crate::engine_session::{
    handle_of_host, H_ARRANGEMENT_DOCUMENT, H_BUS_SYSTEM, H_CONTROL_ROOM, H_ENGINE_BLOCK,
    H_ENGINE_RINGS, H_HYDRATION_GUARD, H_PDC, H_ROUTING, H_SIDECHAIN_ENGINE, H_SIDECHAIN_ROUTES,
    H_TEMPO, H_TRACK_TABLE, H_UNDO,
};
use crate::engine_track::{
    hirari_layout_revision_mark, hirari_track_can_process, hirari_track_id, hirari_track_new,
    hirari_track_owner_release, hirari_track_prepare, hirari_track_resolve_bus,
    hirari_track_set_delay_samples, hirari_track_set_muted, hirari_track_set_pan,
    hirari_track_set_phase_inverted, hirari_track_set_record_armed, hirari_track_set_solo,
    hirari_track_set_spatial, hirari_track_set_volume, hirari_track_storage, hirari_track_type,
    STORAGE_AUTOMATION, STORAGE_CHAIN,
};
use crate::engine_track_table::{
    hirari_engine_reset_project, hirari_engine_track_count, hirari_engine_track_insert,
    hirari_engine_track_ptrs,
};
use crate::engine_transport::stop_at_host;
use crate::ffi::{NativeTrackHandle, NativeTrackStorages};
use crate::AudioEngine;
use crate::HirariUnifiedEngine;
use crate::id_host::hirari_ids_seed_from_project;
use crate::native_project::native_project_load;
use crate::project_plugin_hydration::native_project_hydrate_plugins;
use crate::region_hydration::native_project_hydrate_regions;
use crate::routing_edit_host::hirari_routing_edit_set_sidechain_link;
use crate::routing_engine_core::hirari_routing_engine_add_connection;
use crate::tempo::{
    hirari_tempo_map_add_tempo, hirari_tempo_map_clear, hirari_tempo_map_set_current_bpm,
};
use crate::track_session::{GraphLock, TRACK_TYPE_BUS};
use crate::undo_transaction::hirari_undo_manager_clear;

/// One engine's hydration slot, claimed for the length of a load. A second load
/// on the same engine is refused rather than allowed to interleave partial state.
struct HydrationGuard<'a>(&'a AtomicBool);

impl<'a> HydrationGuard<'a> {
    fn claim(host: *mut c_void) -> Option<Self> {
        let flag = handle_of_host(host, H_HYDRATION_GUARD).cast::<AtomicBool>();
        if flag.is_null() {
            return None;
        }
        let flag = unsafe { &*flag };
        flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for HydrationGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A provisional track's owning handle, released if the load never reaches the
/// commit point. The commit hands every handle to the graph at once, so nothing
/// is freed after that.
struct ProvisionalTracks {
    owners: Vec<*mut c_void>,
}

impl ProvisionalTracks {
    fn take_all(&mut self) -> Vec<*mut c_void> {
        std::mem::take(&mut self.owners)
    }
}

impl Drop for ProvisionalTracks {
    fn drop(&mut self) {
        for &owner in &self.owners {
            if !owner.is_null() {
                unsafe { hirari_track_owner_release(owner) };
            }
        }
    }
}

/// # Safety
/// `host` is a live `HirariUnifiedEngine` address, `path` `path_len` readable
/// UTF-8 bytes. Null arguments are refused.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_load_from_host(
    host: *mut c_void,
    path: *const u8,
    path_len: usize,
) -> bool {
    if host.is_null() || path.is_null() {
        return false;
    }
    let bytes = std::slice::from_raw_parts(path, path_len);
    // The deleted C++ handed the bytes to a `rust::Str`, which aborts on
    // malformed UTF-8; a refusal is the same rejection with a quieter exit.
    match std::str::from_utf8(bytes) {
        Ok(text) => load(host, text),
        Err(_) => false,
    }
}

fn load(host: *mut c_void, path: &str) -> bool {
    let Some(_hydration) = HydrationGuard::claim(host) else {
        return false;
    };
    let mut state = native_project_load(path);
    if !state.valid {
        return false;
    }

    let block = handle_of_host(host, H_ENGINE_BLOCK);
    let table = handle_of_host(host, H_TRACK_TABLE);
    if block.is_null() || table.is_null() {
        return false;
    }
    let sample_rate = f64::from(state.sampleRate);
    let block_frames = unsafe { hirari_engine_block_device_block_frames(block as *const c_void) };

    // The stop is the transport's own: quiesce the callback, then release the
    // retired region generations no live block can be reading.
    stop_at_host(host);
    unsafe { hirari_engine_block_join_callbacks(block as *const c_void) };

    // The whole build runs under the graph lock, and the provisional handles are
    // released by their guard while that lock is still held.
    let Some(_lock) = GraphLock::acquire(host) else {
        return false;
    };
    let revision = unsafe { hirari_engine_block_layout_revision(block as *const c_void) };
    let mut provisional = ProvisionalTracks { owners: Vec::new() };
    let mut built: Vec<*mut c_void> = Vec::with_capacity(state.tracks.len());
    let mut plugin_targets: Vec<NativeTrackStorages> = Vec::with_capacity(state.tracks.len());
    for entry in &state.tracks {
        // The document keeps a track's name in the field the first containers
        // used for the insert label, and an unnamed track takes its kind.
        let name = match entry.pluginName.as_str() {
            "" if entry.trackType == TRACK_TYPE_BUS => "Bus",
            "" => "Track",
            other => other,
        };
        let mut track = ptr::null_mut();
        let owner = unsafe {
            hirari_track_new(
                entry.id,
                name.as_ptr(),
                name.len(),
                entry.trackType,
                revision,
                &mut track,
            )
        };
        if owner.is_null() {
            return false;
        }
        provisional.owners.push(owner);
        unsafe {
            hirari_track_set_volume(track, entry.volume);
            hirari_track_set_pan(track, entry.pan);
            hirari_track_set_muted(track, entry.muted);
            hirari_track_set_solo(track, entry.solo);
            hirari_track_set_phase_inverted(track, entry.phaseInvert);
            hirari_track_set_record_armed(track, entry.recordArmed);
            hirari_track_set_delay_samples(track, entry.trackDelaySamples);
            hirari_track_set_spatial(track, entry.pan3d_x, entry.pan3d_y, entry.pan3d_z);
        }
        built.push(track);
        plugin_targets.push(NativeTrackStorages {
            id: entry.id,
            trackType: entry.trackType,
            chain: unsafe { hirari_track_storage(track, STORAGE_CHAIN) } as usize,
            automation: unsafe { hirari_track_storage(track, STORAGE_AUTOMATION) } as usize,
        });
    }

    if !native_project_hydrate_plugins(&state, &plugin_targets) {
        return false;
    }

    let handles: Vec<NativeTrackHandle> = built
        .iter()
        .map(|&track| NativeTrackHandle {
            id: unsafe { hirari_track_id(track) },
            track: track as usize,
        })
        .collect();
    if !native_project_hydrate_regions(&mut state, &handles, sample_rate) {
        return false;
    }

    // Prepare before the commit point: an insert or buffer state the new format
    // cannot support must fail while the old project is still published.
    for &track in &built {
        if track.is_null() {
            return false;
        }
        let _ = unsafe { hirari_track_prepare(track, sample_rate, block_frames) };
        if !unsafe { hirari_track_can_process(track, block_frames) } {
            return false;
        }
    }

    unsafe {
        hirari_engine_reset_project(
            table,
            handle_of_host(host, H_ENGINE_RINGS),
            handle_of_host(host, H_PDC),
            handle_of_host(host, H_SIDECHAIN_ENGINE),
            handle_of_host(host, H_ROUTING),
            handle_of_host(host, H_BUS_SYSTEM),
            handle_of_host(host, H_CONTROL_ROOM),
            handle_of_host(host, H_SIDECHAIN_ROUTES),
        );
    }
    provisional.take_all().into_iter().for_each(|owner| {
        // The graph takes each handle unconditionally; an insert it refuses
        // would have to be released here, and it never refuses an append.
        unsafe { hirari_engine_track_insert(table, usize::MAX, owner) };
    });
    unsafe { hirari_layout_revision_mark(revision) };

    for route in &state.routes {
        unsafe {
            hirari_routing_engine_add_connection(
                handle_of_host(host, H_ROUTING),
                route.sourceId,
                route.destinationId,
                route.gain,
                route.send,
                route.preFader,
            );
        }
    }

    // A bus claims its slot only now that the graph it sums into exists.
    let bus_system = handle_of_host(host, H_BUS_SYSTEM);
    let published =
        unsafe { std::slice::from_raw_parts(hirari_engine_track_ptrs(table), hirari_engine_track_count(table)) };
    for &track in published {
        if unsafe { hirari_track_type(track) } != TRACK_TYPE_BUS {
            continue;
        }
        let bus_id = unsafe { hirari_track_id(track) };
        let resolved = if unsafe { hirari_bus_system_register(bus_system, bus_id) } {
            unsafe { hirari_bus_system_bus(bus_system, bus_id) }
        } else {
            ptr::null_mut()
        };
        unsafe { hirari_track_resolve_bus(track, resolved, true) };
    }
    drop(_lock);

    unsafe { hirari_ids_seed_from_project(host) };
    let tempo = handle_of_host(host, H_TEMPO);
    unsafe { hirari_tempo_map_set_current_bpm(tempo, state.bpm as f32) };
    unsafe { hirari_engine_block_set_project_scale(block, state.rootNote, state.scaleType) };

    // Markers and arranger parts are document content, not transient UI state.
    let arrangement = handle_of_host(host, H_ARRANGEMENT_DOCUMENT);
    unsafe { hirari_arrangement_document_reset_markers(arrangement) };
    for marker in &state.markers {
        unsafe {
            hirari_arrangement_document_upsert_marker(
                arrangement,
                marker.sample,
                marker.name.as_ptr(),
                marker.name.len(),
                marker.color,
            );
        }
    }
    let parts: Vec<HirariArrangementPartInput> = state
        .arrangerParts
        .iter()
        .map(|part| HirariArrangementPartInput {
            start: part.start,
            length: part.length,
            name: part.name.as_ptr(),
            name_len: part.name.len(),
            repeats: part.repeats,
        })
        .collect();
    let _ = unsafe {
        hirari_arrangement_document_set_parts(arrangement, parts.as_ptr(), parts.len())
    };

    let _ = unsafe { hirari_tempo_map_clear(tempo, sample_rate, state.bpm) };
    for event in &state.tempoEvents {
        let _ = unsafe {
            hirari_tempo_map_add_tempo(
                tempo,
                event.sample,
                event.bpm,
                sample_rate,
                event.ramp != 0,
            )
        };
    }

    // A project carries its own timeline rate, so the graph is re-prepared for
    // it: opening a 48 or 96 kHz document must not leave the engine running at
    // the previous device format.
    prepare_graph(host, sample_rate, block_frames);

    for sidechain in &state.sidechains {
        if !unsafe {
            hirari_routing_edit_set_sidechain_link(
                host,
                sidechain.sourceId,
                sidechain.destinationId,
                sidechain.pluginIndex,
                sidechain.tapPoint,
                true,
            )
        } {
            return false;
        }
    }

    // A loaded project starts a fresh history: undoing into the previous
    // document's commands would mutate the wrong graph.
    unsafe { hirari_undo_manager_clear(handle_of_host(host, H_UNDO)) };
    true
}

/// Re-prepares the graph at the document's format through the engine. The
/// allocation failures this raises are C++ exceptions, so the sequence stays
/// with the engine object that catches them; everything else it calls is Rust.
fn prepare_graph(host: *mut c_void, sample_rate: f64, block_frames: u32) {
    let core = host.cast::<crate::HirariUnifiedEngine>();
    if core.is_null() {
        return;
    }
    let core = unsafe { core::pin::Pin::new_unchecked(&mut *core) };
    core.prepare_to_play(sample_rate, block_frames);
}
