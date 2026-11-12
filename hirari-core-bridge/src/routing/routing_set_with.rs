//! Routing edit payload setters, split out of routing_edit_host.rs.
use super::*;

pub(in crate::routing_edit_host) fn valid_route_gain(gain: f32, enabled: bool) -> bool {
    gain.is_finite() && (0.0..=2.0).contains(&gain) && !(enabled && gain <= 0.0)
}

/// `set_route`: the enabled flag is the whole edit. Undo restores the prior
/// route gain as well so removing and re-adding an edge is lossless.
pub(in crate::routing_edit_host) fn set_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES || dest_id >= MAX_ROUTING_NODES || source_id == dest_id {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if before.route_enabled == enabled {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_ROUTE, source_id, dest_id, 1.0, enabled, false),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if after.route_enabled != enabled {
        return false;
    }
    let before_enabled = before.route_enabled;
    record_undo(
        ops,
        host,
        b"Audio Route\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE,
            source_id,
            dest_id,
            before.route_gain,
            before_enabled,
            false,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE,
            source_id,
            dest_id,
            if enabled { 1.0 } else { before.route_gain },
            enabled,
            false,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

pub(in crate::routing_edit_host) fn set_route_gain_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !valid_route_gain(gain, enabled)
    {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if before.route_enabled == enabled && (!enabled || (before.route_gain - gain).abs() < 1.0e-6) {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_ROUTE_GAIN, source_id, dest_id, gain, enabled, false),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if after.route_enabled != enabled {
        return false;
    }
    record_undo(
        ops,
        host,
        b"Audio Route Gain\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE_GAIN,
            source_id,
            dest_id,
            before.route_gain,
            before.route_enabled,
            false,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_ROUTE_GAIN,
            source_id,
            dest_id,
            gain,
            enabled,
            false,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

pub(in crate::routing_edit_host) fn set_send_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    pre_fader: bool,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !valid_route_gain(gain, enabled)
    {
        return false;
    }
    if enabled {
        if track_state(host, source_id).is_none() {
            return false;
        }
        let Some(destination) = track_state(host, dest_id) else {
            return false;
        };
        // Sends resolve to buses only; a track-to-track edge is an insert.
        if !destination.is_bus {
            return false;
        }
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    let before_enabled = before.send_gain > 0.0;
    if before_enabled == enabled
        && (!enabled
            || ((before.send_gain - gain).abs() < 1.0e-6 && before.send_pre_fader == pre_fader))
    {
        return false;
    }
    if !apply_route_payload(
        ops,
        host,
        &route_payload(HIRARI_ROUTING_EDIT_SEND, source_id, dest_id, gain, enabled, pre_fader),
    ) {
        return false;
    }
    let Some(after) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if (after.send_gain > 0.0) != enabled {
        return false;
    }
    record_undo(
        ops,
        host,
        b"Audio Send\0",
        &[route_payload(
            HIRARI_ROUTING_EDIT_SEND,
            source_id,
            dest_id,
            before.send_gain,
            before_enabled,
            before.send_pre_fader,
        )],
        &[route_payload(
            HIRARI_ROUTING_EDIT_SEND,
            source_id,
            dest_id,
            gain,
            enabled,
            pre_fader,
        )],
        false,
        false,
    );
    publish_routes(ops, host);
    true
}

pub(in crate::routing_edit_host) fn set_feedback_route_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    gain: f32,
    enabled: bool,
) -> bool {
    if source_id >= MAX_ROUTING_NODES
        || dest_id >= MAX_ROUTING_NODES
        || source_id == dest_id
        || !gain.is_finite()
    {
        return false;
    }
    let Some(before) = query_edge(ops, host, source_id, dest_id) else {
        return false;
    };
    if enabled {
        if !apply_feedback(ops, host, source_id, dest_id, gain) {
            return false;
        }
    } else {
        if before.feedback_gain < 0.0 {
            return false;
        }
        apply_feedback(ops, host, source_id, dest_id, -1.0);
    }
    mark_layout_changed(ops, host);
    record_undo(
        ops,
        host,
        b"Feedback Route\0",
        &[HirariRoutingEditPayload {
            op: HIRARI_ROUTING_EDIT_FEEDBACK,
            source_id,
            dest_id,
            gain: before.feedback_gain,
            ..Default::default()
        }],
        &[HirariRoutingEditPayload {
            op: HIRARI_ROUTING_EDIT_FEEDBACK,
            source_id,
            dest_id,
            gain: if enabled { gain.clamp(0.0, 2.0) } else { -1.0 },
            ..Default::default()
        }],
        false,
        false,
    );
    true
}

/// True when `target` is reachable from `from` following sidechain edges
/// (source -> destination). Mirrors the C++ depth-first walk.
pub(in crate::routing_edit_host) fn sidechain_reaches(routes: &[HirariRoutingSidechainEntry], from: u32, target: u32) -> bool {
    let mut pending = vec![from];
    let mut visited: Vec<u32> = Vec::new();
    while let Some(node) = pending.pop() {
        if node == target {
            return true;
        }
        if visited.contains(&node) {
            continue;
        }
        visited.push(node);
        for route in routes {
            if route.source_id == node {
                pending.push(route.dest_id);
            }
        }
    }
    false
}

pub(in crate::routing_edit_host) fn set_sidechain_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    source_id: u32,
    dest_id: u32,
    plugin_index: u32,
    tap_point: u32,
    enabled: bool,
    record: bool,
) -> bool {
    if source_id == dest_id || tap_point > 2 {
        return false;
    }
    let Some(routes) = list_sidechain(ops, host) else {
        return false;
    };
    let previous = routes
        .iter()
        .find(|route| route.dest_id == dest_id && route.plugin_index == plugin_index)
        .copied();
    let had_previous = previous.is_some();
    let previous_source = previous.map_or(0, |route| route.source_id);
    let previous_tap = previous.map_or(2, |route| route.tap_point);
    let edge_used_by_another = |source: u32| {
        routes.iter().any(|route| {
            route.source_id == source
                && route.dest_id == dest_id
                && route.plugin_index != plugin_index
        })
    };

    if !enabled {
        let removed = remove_sidechain(ops, host, dest_id, plugin_index);
        if had_previous && !edge_used_by_another(previous_source) {
            remove_dependency(ops, host, previous_source, dest_id);
        }
        if !removed {
            return false;
        }
        build_graph(ops, host);
        mark_layout_changed(ops, host);
        if record {
            record_undo(
                ops,
                host,
                b"Sidechain Disconnect\0",
                &[sidechain_payload(
                    previous_source,
                    dest_id,
                    plugin_index,
                    previous_tap,
                    true,
                )],
                &[sidechain_payload(
                    previous_source,
                    dest_id,
                    plugin_index,
                    previous_tap,
                    false,
                )],
                false,
                false,
            );
        }
        return true;
    }

    // Sidechain dependencies are directed source -> destination. Adding a
    // link is rejected when the destination can already reach the source;
    // this prevents A->B->A cycles before the native graph is published.
    if sidechain_reaches(&routes, dest_id, source_id) {
        return false;
    }
    let Some(context) = sidechain_context(ops, host, source_id, dest_id) else {
        return false;
    };
    if !context.source_found || !context.destination_found {
        return false;
    }
    let new_edge_used =
        edge_used_by_another(source_id) || (had_previous && previous_source == source_id);
    let removed_previous_dependency =
        had_previous && previous_source != source_id && !edge_used_by_another(previous_source);
    if removed_previous_dependency {
        remove_dependency(ops, host, previous_source, dest_id);
    }
    let restore_previous_dependency = |ops: &HirariRoutingHostOps, host: *mut c_void| {
        if removed_previous_dependency {
            add_dependency(ops, host, previous_source, dest_id);
            build_graph(ops, host);
        }
    };
    if !add_dependency(ops, host, source_id, dest_id) || !build_graph(ops, host) {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    }
    let silent = !context.source_prepared;
    let registered = match ops.register_sidechain {
        Some(register) => unsafe {
            register(
                host,
                dest_id,
                plugin_index,
                source_id,
                &context,
                tap_point,
                silent,
            )
        },
        None => register_sidechain_native(
            ops,
            host,
            dest_id,
            plugin_index,
            source_id,
            &context,
            tap_point,
            silent,
        ),
    };
    if !registered {
        if !new_edge_used {
            remove_dependency(ops, host, source_id, dest_id);
        }
        restore_previous_dependency(ops, host);
        return false;
    }
    if let Some(routes) = sidechain_routes_store(ops, host) {
        unsafe {
            crate::sidechain_route_store::hirari_sidechain_routes_set(
                routes,
                source_id,
                dest_id,
                plugin_index,
                tap_point,
            )
        };
    }
    mark_layout_changed(ops, host);
    if record {
        let undo = if had_previous {
            sidechain_payload(previous_source, dest_id, plugin_index, previous_tap, true)
        } else {
            sidechain_payload(0, dest_id, plugin_index, 0, false)
        };
        record_undo(
            ops,
            host,
            b"Sidechain Connect\0",
            &[undo],
            &[sidechain_payload(
                source_id,
                dest_id,
                plugin_index,
                tap_point,
                true,
            )],
            false,
            false,
        );
    }
    true
}

pub(in crate::routing_edit_host) fn set_track_armed_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    armed: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    // An already-correct value is a valid idempotent application.
    if track.armed == armed {
        return true;
    }
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_ARM, armed, 0) {
        return false;
    }
    if record {
        record_undo(
            ops,
            host,
            b"Record Arm\0",
            &[track_payload(HIRARI_ROUTING_EDIT_ARM, track_id, !armed, 0)],
            &[track_payload(HIRARI_ROUTING_EDIT_ARM, track_id, armed, 0)],
            false,
            false,
        );
    }
    true
}

pub(in crate::routing_edit_host) fn set_phase_invert_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    inverted: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if track.phase_inverted == inverted {
        return true;
    }
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_PHASE, inverted, 0) {
        return false;
    }
    if record {
        record_undo(
            ops,
            host,
            b"Phase Invert\0",
            &[track_payload(
                HIRARI_ROUTING_EDIT_PHASE,
                track_id,
                !inverted,
                0,
            )],
            &[track_payload(
                HIRARI_ROUTING_EDIT_PHASE,
                track_id,
                inverted,
                0,
            )],
            false,
            false,
        );
    }
    true
}

const MAX_TRACK_DELAY_SAMPLES: u32 = 8192;

pub(in crate::routing_edit_host) fn set_track_delay_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    samples: u32,
    record: bool,
) -> bool {
    if samples > MAX_TRACK_DELAY_SAMPLES {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples) {
        return false;
    }
    if record {
        let mut name = format!("Track Delay {track_id}").into_bytes();
        name.push(0);
        // `perform` replays the redo payload through this module and records
        // with coalescing, matching the previous performAction call.
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_DELAY,
                track_id,
                false,
                track.delay_samples,
            )],
            &[track_payload(
                HIRARI_ROUTING_EDIT_DELAY,
                track_id,
                false,
                samples,
            )],
            true,
            true,
        );
        // The redo replay is allowed to be a no-op; keep the live value
        // explicit so coalescing cannot leave it stale.
        apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_DELAY, false, samples);
    }
    true
}

const MAX_TRACK_VOLUME: f32 = 2.0;

/// Mix edits record even when the value already matches: the previous engine
/// pushed an undo entry per call and control surfaces rely on that cadence.
pub(in crate::routing_edit_host) fn set_track_volume_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    value: f32,
    record: bool,
) -> bool {
    if !value.is_finite() || !(0.0..=MAX_TRACK_VOLUME).contains(&value) {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_mix(host, track_id, HIRARI_ROUTING_EDIT_VOLUME, value) {
        return false;
    }
    if record {
        let mut name = format!("Track Volume {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_mix_payload(
                HIRARI_ROUTING_EDIT_VOLUME,
                track_id,
                track.volume,
            )],
            &[track_mix_payload(
                HIRARI_ROUTING_EDIT_VOLUME,
                track_id,
                value,
            )],
            false,
            false,
        );
    }
    true
}

pub(in crate::routing_edit_host) fn set_track_pan_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    value: f32,
    record: bool,
) -> bool {
    if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
        return false;
    }
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_mix(host, track_id, HIRARI_ROUTING_EDIT_PAN, value) {
        return false;
    }
    if record {
        let mut name = format!("Track Pan {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_mix_payload(HIRARI_ROUTING_EDIT_PAN, track_id, track.pan)],
            &[track_mix_payload(HIRARI_ROUTING_EDIT_PAN, track_id, value)],
            false,
            false,
        );
    }
    true
}

pub(in crate::routing_edit_host) fn set_track_mute_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    muted: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_MUTE, muted, 0) {
        return false;
    }
    if record {
        let mut name = format!("Track Mute {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_MUTE,
                track_id,
                track.muted,
                0,
            )],
            &[track_payload(HIRARI_ROUTING_EDIT_MUTE, track_id, muted, 0)],
            false,
            false,
        );
    }
    true
}

pub(in crate::routing_edit_host) fn set_track_solo_with(
    ops: &HirariRoutingHostOps,
    host: *mut c_void,
    track_id: u32,
    solo: bool,
    record: bool,
) -> bool {
    let Some(track) = track_state(host, track_id) else {
        return false;
    };
    if !apply_track_scalar(host, track_id, HIRARI_ROUTING_EDIT_SOLO, solo, 0) {
        return false;
    }
    if record {
        let mut name = format!("Track Solo {track_id}").into_bytes();
        name.push(0);
        record_undo(
            ops,
            host,
            &name,
            &[track_payload(
                HIRARI_ROUTING_EDIT_SOLO,
                track_id,
                track.solo,
                0,
            )],
            &[track_payload(HIRARI_ROUTING_EDIT_SOLO, track_id, solo, 0)],
            false,
            false,
        );
    }
    true
}
