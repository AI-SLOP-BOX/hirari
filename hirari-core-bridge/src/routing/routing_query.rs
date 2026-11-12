//! Routing graph query + apply helpers, split out of routing_edit_host.rs.
use super::*;

pub(in crate::routing_edit_host) fn sidechain_routes_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.sidechain_routes_store?;
    Some(unsafe { access(host) })
}

/// Removes one link from both Rust stores that describe it: the sidechain gate
/// holds the live audio routing, the route table the saved model.
pub(in crate::routing_edit_host) fn remove_sidechain(ops: &HirariRoutingHostOps, host: *mut c_void, dest: u32, plugin: u32) -> bool {
    let gate_removed = match ops.sidechain_engine_store {
        Some(access) => unsafe {
            crate::sidechain_manager::hirari_sidechain_engine_remove_link(
                access(host),
                dest,
                plugin,
            )
        },
        None => false,
    };
    let Some(routes) = sidechain_routes_store(ops, host) else {
        return gate_removed;
    };
    let route_removed = unsafe {
        crate::sidechain_route_store::hirari_sidechain_routes_remove_link(routes, dest, plugin)
    };
    gate_removed || route_removed
}

pub(in crate::routing_edit_host) fn query_edge(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingEdgeState> {
    let routing = routing_store(ops, host)?;
    Some(unsafe {
        HirariRoutingEdgeState {
            route_enabled: crate::routing_engine_core::hirari_routing_engine_has_connection(
                routing, source_id, dest_id,
            ),
            route_gain: crate::routing_engine_core::hirari_routing_engine_connection_gain(
                routing, source_id, dest_id,
            ),
            send_gain: crate::routing_engine_core::hirari_routing_engine_send_gain(
                routing, source_id, dest_id,
            ),
            send_pre_fader: crate::routing_engine_core::hirari_routing_engine_send_pre_fader(
                routing, source_id, dest_id,
            ),
            feedback_gain: crate::routing_engine_core::hirari_routing_engine_feedback_gain(
                routing, source_id, dest_id,
            ),
        }
    })
}

/// One track's routing-relevant state, read from the stores the track owns:
/// the fader, pan, solo and record-arm live in its channel strip, mute, phase
/// and the manual timing offset in its runtime, and the bus flag is the
/// engine's own track type.
pub(in crate::routing_edit_host) fn track_state(host: *mut c_void, track_id: u32) -> Option<HirariRoutingTrackState> {
    let session = TrackSession::open(host, track_id)?;
    let strip = session.strip();
    let runtime = session.runtime();
    Some(HirariRoutingTrackState {
        armed: unsafe { hirari_channel_strip_is_record_armed(strip) },
        phase_inverted: unsafe { hirari_track_runtime_is_phase_inverted(runtime) },
        is_bus: session.is_bus(),
        delay_samples: unsafe { hirari_track_runtime_track_delay_samples(runtime) },
        volume: unsafe { hirari_channel_strip_gain(strip) },
        pan: unsafe { hirari_channel_strip_pan(strip) },
        muted: unsafe { hirari_track_runtime_is_muted(runtime) },
        solo: unsafe { hirari_channel_strip_is_solo(strip) },
    })
}

/// Applies one track-state mutation. A gate change — mute, solo, phase — drops
/// the frozen render because it changes what the track outputs; record-arm only
/// bumps the layout revision since nothing audible moves; any other op is the
/// manual timing offset, which bumps the layout only when the runtime accepted
/// a different value than it held.
pub(in crate::routing_edit_host) fn apply_track_scalar(
    host: *mut c_void,
    track_id: u32,
    op: i32,
    enabled: bool,
    samples: u32,
) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let (strip, runtime) = (session.strip(), session.runtime());
    match op {
        HIRARI_ROUTING_EDIT_ARM => {
            session.commit_layout(unsafe { hirari_channel_strip_set_record_armed(strip, enabled) })
        }
        HIRARI_ROUTING_EDIT_PHASE => {
            session.invalidate_frozen();
            unsafe { hirari_track_runtime_set_phase_invert(runtime, enabled as u8) };
            session.mark_layout();
            true
        }
        HIRARI_ROUTING_EDIT_MUTE => {
            session.invalidate_frozen();
            unsafe { hirari_track_runtime_set_muted(runtime, enabled as u8) };
            session.mark_layout();
            true
        }
        HIRARI_ROUTING_EDIT_SOLO => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_solo(strip, enabled) };
            session.mark_layout();
            true
        }
        _ => {
            let previous = unsafe { hirari_track_runtime_track_delay_samples(runtime) };
            unsafe {
                hirari_track_runtime_set_delays(
                    runtime,
                    hirari_track_runtime_pdc_requested(runtime),
                    samples,
                )
            };
            // The runtime bounds the offset to its shared delay line, so the
            // accepted value is what decides whether the graph has to move.
            session.commit_layout(
                unsafe { hirari_track_runtime_track_delay_samples(runtime) } != previous,
            );
            true
        }
    }
}

/// Applies one fader or pan edit, dropping the frozen render first because the
/// mix the track renders changes with it.
pub(in crate::routing_edit_host) fn apply_track_mix(host: *mut c_void, track_id: u32, op: i32, value: f32) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let strip = session.strip();
    match op {
        HIRARI_ROUTING_EDIT_VOLUME => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_gain(strip, value) };
        }
        HIRARI_ROUTING_EDIT_PAN => {
            session.invalidate_frozen();
            unsafe { hirari_channel_strip_set_pan(strip, value) };
        }
        _ => return false,
    }
    session.mark_layout();
    true
}

pub(in crate::routing_edit_host) fn list_sidechain(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
) -> Option<Vec<HirariRoutingSidechainEntry>> {
    let routes = sidechain_routes_store(ops, host)?;
    let count = unsafe { crate::sidechain_route_store::hirari_sidechain_routes_count(routes) };
    let mut buffer = vec![
        crate::sidechain_route_store::HirariSidechainRoute {
            source_id: 0,
            destination_id: 0,
            plugin_index: 0,
            tap_point: 0,
        };
        count
    ];
    if count > 0 {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_copy(
                routes,
                buffer.as_mut_ptr(),
                count,
            );
        }
    }
    Some(
        buffer
            .into_iter()
            .map(|route| HirariRoutingSidechainEntry {
                source_id: route.source_id,
                dest_id: route.destination_id,
                plugin_index: route.plugin_index,
                tap_point: route.tap_point,
            })
            .collect(),
    )
}

pub(in crate::routing_edit_host) fn sidechain_engine_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.sidechain_engine_store?;
    Some(unsafe { access(host) })
}

pub(in crate::routing_edit_host) fn engine_block_store(ops: &HirariRoutingHostOps, host: *mut c_void) -> Option<*mut c_void> {
    let access = ops.engine_block_store?;
    let state = unsafe { access(host) };
    (!state.is_null()).then_some(state)
}

/// The sidechain registration inputs, read from the state the engine actually
/// owns: the device format recorded by the last graph preparation, the
/// generation the buffers were published at, and the source's own work buffer
/// for the "prepared at this block size" check. The Rust sidechain storage
/// holds 4096 frames, so the request is clamped before any source is tapped.
pub(in crate::routing_edit_host) fn sidechain_context_native(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingSidechainContext> {
    let engine_block = engine_block_store(ops, host)?;
    let block_size =
        unsafe { crate::engine_block_host::hirari_engine_block_device_block_frames(engine_block) }
            .min(4096);
    let sample_rate = unsafe {
        crate::engine_block_host::hirari_engine_block_device_sample_rate(engine_block)
    }
    .max(1.0) as u32;
    let generation =
        unsafe { crate::engine_block_host::hirari_engine_block_generation(engine_block) }.max(1);
    let (source_found, source_prepared) = match TrackSession::open(host, source_id) {
        Some(session) => {
            let tapped = unsafe {
                crate::audio_buffer_ops::hirari_audio_buffer_storage_samples(session.work_buffer())
            } >= block_size;
            (true, tapped)
        }
        None => (false, false),
    };
    let destination_found = TrackSession::open(host, dest_id).is_some();
    Some(HirariRoutingSidechainContext {
        block_size,
        sample_rate,
        generation,
        source_found,
        destination_found,
        source_prepared: source_prepared && destination_found,
    })
}

pub(in crate::routing_edit_host) fn sidechain_context(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> Option<HirariRoutingSidechainContext> {
    match ops.sidechain_context {
        Some(query) => {
            let mut context = HirariRoutingSidechainContext::default();
            unsafe { query(host, source_id, dest_id, &mut context) }.then_some(context)
        }
        None => sidechain_context_native(ops, host, source_id, dest_id),
    }
}

/// Registers one sidechain tap with the gate. A source whose work buffer has
/// not reached the current block size is registered silent, so the plug-in can
/// gate on the model without reading unprepared audio; a prepared source taps
/// its own work buffer storage.
pub(in crate::routing_edit_host) fn register_sidechain_native(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    dest_id: u32,
    plugin_index: u32,
    source_id: u32,
    context: &HirariRoutingSidechainContext,
    tap_point: u32,
    silent: bool,
) -> bool {
    let Some(gate) = sidechain_engine_store(ops, host) else {
        return false;
    };
    if silent {
        return unsafe {
            crate::sidechain_manager::hirari_sidechain_engine_register_silent_link(
                gate,
                dest_id,
                plugin_index,
                source_id,
                context.block_size,
                context.sample_rate,
                context.generation,
                1.0,
                tap_point as u8,
            )
        };
    }
    let Some(session) = TrackSession::open(host, source_id) else {
        return false;
    };
    let work = session.work_buffer();
    let channels = unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_channels(work) };
    let left = unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(work, 0, 0) };
    let right = if channels > 1 {
        unsafe { crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(work, 1, 0) }
    } else {
        left
    };
    // Registration copies one prepared block into Rust-owned storage;
    // supplying the frame count is part of that contract.
    unsafe {
        crate::sidechain_manager::hirari_sidechain_engine_register_link(
            gate,
            dest_id,
            plugin_index,
            source_id,
            left,
            right,
            context.block_size,
            context.sample_rate,
            context.generation,
            1.0,
            tap_point as u8,
        )
    }
}

pub(in crate::routing_edit_host) fn build_graph(ops: &HirariRoutingHostOps, host: *mut c_void) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    // The native host callback ignored the topology result, so a rebuild is
    // reported as done; cycle admission is decided before the dependency is
    // added.
    unsafe { crate::routing_engine_core::hirari_routing_engine_build(routing) };
    true
}

pub(in crate::routing_edit_host) fn mark_layout_changed(ops: &HirariRoutingHostOps, host: *mut c_void) {
    if let Some(mark) = ops.mark_layout_changed {
        unsafe { mark(host) };
    }
}

pub(in crate::routing_edit_host) fn add_dependency(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
) -> bool {
    match routing_store(ops, host) {
        Some(routing) => unsafe {
            crate::routing_engine_core::hirari_routing_engine_add_dependency(
                routing, source_id, dest_id,
            )
        },
        None => false,
    }
}

pub(in crate::routing_edit_host) fn remove_dependency(ops: &HirariRoutingHostOps, host: *mut c_void, source_id: u32, dest_id: u32) {
    if let Some(routing) = routing_store(ops, host) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_remove_dependency(
                routing, source_id, dest_id,
            );
        }
    }
}

/// Rebuilds the routing graph and republishes the derived track/PDC state.
pub(in crate::routing_edit_host) fn publish_routes(ops: &HirariRoutingHostOps, host: *mut c_void) {
    if let Some(routing) = routing_store(ops, host) {
        unsafe {
            crate::routing_engine_core::hirari_routing_engine_build(routing);
        }
    }
    if let Some(sync) = ops.sync_structural {
        unsafe { sync(host) };
    }
}

/// Applies one resolved route/send mutation to the Rust-owned graph. A route
/// without a saved gain is a unity route; a disabled entry removes the edge.
pub(in crate::routing_edit_host) fn apply_route_payload(ops: &HirariRoutingHostOps, host: *mut c_void, edit: &HirariRoutingEditPayload) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    unsafe {
        if edit.op == HIRARI_ROUTING_EDIT_SEND {
            if edit.enabled {
                crate::routing_engine_core::hirari_routing_engine_add_connection(
                    routing,
                    edit.source_id,
                    edit.dest_id,
                    edit.gain,
                    true,
                    edit.pre_fader,
                );
            } else {
                crate::routing_engine_core::hirari_routing_engine_remove_send(
                    routing, edit.source_id, edit.dest_id,
                );
            }
        } else if edit.enabled {
            crate::routing_engine_core::hirari_routing_engine_add_connection(
                routing,
                edit.source_id,
                edit.dest_id,
                if edit.gain > 0.0 { edit.gain } else { 1.0 },
                false,
                false,
            );
        } else {
            crate::routing_engine_core::hirari_routing_engine_remove_connection(
                routing, edit.source_id, edit.dest_id,
            );
        }
    }
    true
}

/// A negative feedback gain names the absent edge, so it removes it.
pub(in crate::routing_edit_host) fn apply_feedback(ops: &HirariRoutingHostOps, host: *mut c_void, source_id: u32, dest_id: u32, gain: f32) -> bool {
    let Some(routing) = routing_store(ops, host) else {
        return false;
    };
    unsafe {
        if gain < 0.0 {
            crate::routing_engine_core::hirari_routing_engine_remove_feedback(
                routing, source_id, dest_id,
            );
            return true;
        }
        crate::routing_engine_core::hirari_routing_engine_add_feedback(
            routing, source_id, dest_id, gain,
        )
    }
}
