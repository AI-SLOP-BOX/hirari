//! Project-load plugin hydration.
//!
//! A saved document stores a track's inserts positionally: `pluginTypes` is the
//! chain in save order with `u32::MAX` marking a sandbox slot whose binary path
//! is consumed from a parallel array. Rebuilding it used to be a C++ walk that
//! converted each Rust array into a `std::vector` before forwarding it straight
//! back to the Rust chain and automation stores. The conversion was the only
//! thing the engine added, so the walk lives here now and the engine supplies
//! only the provisional track's own store handles.
//!
//! The per-edit protocol the old `Track` wrappers ran (drop the frozen render,
//! bump the layout revision) is deliberately absent: these tracks have never
//! rendered, and `loadProject` bumps the project revision once at its commit
//! point, after the replacement graph is published.

use std::ffi::c_void;

use crate::effect_chain_host::{
    hirari_effect_chain_add_builtin, hirari_effect_chain_add_sandbox, hirari_effect_chain_count,
    hirari_effect_chain_processor_gui_state_restore,
    hirari_effect_chain_processor_state_restore, hirari_effect_chain_set_bypass,
};
use crate::ffi::{NativeAutomationPoint, NativeProjectState, NativeTrackState, NativeTrackStorages, };
use crate::track_automation_store::{
    hirari_track_automation_store_set_plugin_lanes, hirari_track_automation_store_set_points,
    AutomationPoint, PluginLaneView,
};

/// `Track::Type::Bus`, which selects the bus-side DSP variants.
const TRACK_TYPE_BUS: u32 = 3;
/// The `pluginTypes` entry that marks a sandboxed insert.
const SANDBOX_MARKER: u32 = u32::MAX;

const STORE_VOLUME: u32 = 0;
const STORE_PAN: u32 = 1;
const STORE_TRACK_DELAY: u32 = 2;

fn owned_points(points: &[NativeAutomationPoint]) -> Vec<AutomationPoint> {
    points
        .iter()
        .map(|point| AutomationPoint {
            time: point.time,
            value: point.value,
            curve: point.curve,
        })
        .collect()
}

/// Installs the three track-level curves. An empty document curve leaves the
/// store's published empty generation alone, exactly as the loader did before.
fn hydrate_curves(store: *mut c_void, track: &NativeTrackState) {
    for (kind, curve) in [
        (STORE_VOLUME, &track.volumeAutomation),
        (STORE_PAN, &track.panAutomation),
        (STORE_TRACK_DELAY, &track.trackDelayAutomation),
    ] {
        if curve.is_empty() {
            continue;
        }
        let points = owned_points(curve);
        unsafe {
            hirari_track_automation_store_set_points(store, kind, points.as_ptr(), points.len());
        }
    }
}

/// Replaces every plugin parameter lane in one call, so the store publishes the
/// saved lane set as a single generation.
fn hydrate_plugin_lanes(store: *mut c_void, track: &NativeTrackState) {
    if track.pluginAutomation.is_empty() {
        return;
    }
    let lanes: Vec<Vec<AutomationPoint>> = track
        .pluginAutomation
        .iter()
        .map(|lane| owned_points(&lane.points))
        .collect();
    let views: Vec<PluginLaneView> = track
        .pluginAutomation
        .iter()
        .zip(lanes.iter())
        .map(|(lane, points)| PluginLaneView {
            processor_index: lane.pluginIndex,
            parameter_id: lane.parameterId,
            points: points.as_ptr(),
            point_count: points.len(),
        })
        .collect();
    unsafe {
        hirari_track_automation_store_set_plugin_lanes(store, views.as_ptr(), views.len());
    }
}

/// Creates the saved inserts in order. A sandbox marker consumes the next path
/// whether or not its binary loads, so the following insert keeps its position.
/// Returns false when the engine could not build a builtin the document names.
fn hydrate_chain(chain: *mut c_void, track: &NativeTrackState) -> bool {
    let mut path_index = 0usize;
    let add_sandbox = |index: usize| {
        if let Some(path) = track.sandboxedPluginPaths.get(index) {
            // An unavailable binary is quarantined by the chain host; the loader
            // keeps going with the hole in the chain, as it always has.
            unsafe { hirari_effect_chain_add_sandbox(chain, path.as_ptr(), path.len()) };
        }
    };
    for plugin_type in &track.pluginTypes {
        if *plugin_type == SANDBOX_MARKER {
            add_sandbox(path_index);
            path_index += 1;
            continue;
        }
        if !unsafe {
            hirari_effect_chain_add_builtin(chain, *plugin_type, track.trackType == TRACK_TYPE_BUS, false)
        } {
            return false;
        }
    }
    // Older files may list sandbox paths with no marker in the plugin array;
    // append them so their state still lands on an insert.
    while path_index < track.sandboxedPluginPaths.len() {
        add_sandbox(path_index);
        path_index += 1;
    }
    true
}

/// Applies the positional per-insert arrays. Each one stops at the live chain
/// length, because missing sandbox binaries intentionally leave a hole the saved
/// state cannot address.
fn hydrate_plugin_state(chain: *mut c_void, track: &NativeTrackState) {
    let slots = unsafe { hirari_effect_chain_count(chain) } as usize;
    for (index, state) in track.pluginStates.iter().enumerate().take(slots) {
        unsafe {
            hirari_effect_chain_processor_state_restore(
                chain,
                index as u32,
                state.bytes.as_ptr(),
                state.bytes.len(),
            );
        }
    }
    for (index, state) in track.pluginGuiStates.iter().enumerate().take(slots) {
        unsafe {
            hirari_effect_chain_processor_gui_state_restore(
                chain,
                index as u32,
                state.bytes.as_ptr(),
                state.bytes.len(),
            );
        }
    }
    for (index, bypassed) in track.pluginBypass.iter().enumerate().take(slots) {
        unsafe { hirari_effect_chain_set_bypass(chain, index as u32, *bypassed != 0) };
    }
}

pub fn native_project_hydrate_plugins(
    state: &NativeProjectState,
    tracks: &Vec<NativeTrackStorages>,
) -> bool {
    for track in &state.tracks {
        let Some(target) = tracks.iter().find(|entry| entry.id == track.id) else {
            continue;
        };
        let chain = target.chain as *mut c_void;
        let store = target.automation as *mut c_void;
        if chain.is_null() || store.is_null() {
            return false;
        }
        hydrate_curves(store, track);
        if !hydrate_chain(chain, track) {
            return false;
        }
        hydrate_plugin_lanes(store, track);
        hydrate_plugin_state(chain, track);
    }
    true
}

// The engine's own hydration path is exercised by the project-load tests in
// `lib_tests_plugin_hydration.rs`, which drive this through a real engine rather
// than a fake chain: the rejection rules, sandbox-slot consumption and positional
// bounds are properties of the live stores.
