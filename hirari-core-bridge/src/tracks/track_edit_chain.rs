//! Chain snapshot readers for the track-edit lifecycle, split out of
//! track_edit_host.rs.
use super::*;

pub(crate) fn chain_state(chain: *mut c_void, index: u32) -> Vec<u8> {
    let mut length = 0usize;
    let data = unsafe { crate::effect_chain_host::hirari_effect_chain_processor_state(chain, index, &mut length) };
    if data.is_null() || length == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, length).to_vec() };
    unsafe { crate::effect_chain_host::hirari_effect_chain_buffer_free(data, length) };
    bytes
}

/// Reads one sandbox slot's path out of a chain.
pub(crate) fn chain_sandbox_path(chain: *mut c_void, slot: usize) -> Vec<u8> {
    let length =
        unsafe { crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(chain, slot, std::ptr::null_mut(), 0) };
    if length == 0 {
        return Vec::new();
    }
    let mut path = vec![0u8; length];
    let written = unsafe {
        crate::effect_chain_host::hirari_effect_chain_sandbox_slot_path(
            chain,
            slot,
            path.as_mut_ptr(),
            length,
        )
    };
    path.truncate(written.min(length));
    path
}

/// Copies the store state a duplicated track inherits.
///
/// The engine builds the new track and hands over its storages; the name with
/// the copy suffix, the fader, the mute and phase flags, the three automation
/// lists, the plugin lanes and the chain in order are then read and written
/// store-to-store here. A sandbox path the destination chain refuses is skipped
/// like the engine's own walk did, so one rejected worker never aborts the
/// duplicate.
///
/// # Safety
/// Every handle must be a live storage of its kind, and the two handle sets must
/// belong to tracks of the same kind so the chain rebuild selects the same DSP
/// variants.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn hirari_track_clone_state(
    source_strip: *mut c_void,
    source_runtime: *mut c_void,
    source_automation: *mut c_void,
    source_chain: *mut c_void,
    source_name: *mut c_void,
    dest_strip: *mut c_void,
    dest_runtime: *mut c_void,
    dest_automation: *mut c_void,
    dest_chain: *mut c_void,
    dest_name: *mut c_void,
    dest_is_bus: bool,
) -> bool {
    if source_strip.is_null()
        || source_automation.is_null()
        || source_chain.is_null()
        || dest_strip.is_null()
        || dest_automation.is_null()
        || dest_chain.is_null()
    {
        return false;
    }
    // A name that cannot carry the suffix aborts the duplicate before any other
    // storage is touched.
    if !crate::track_name_store::copy_with_suffix(
        source_name as *const c_void,
        dest_name,
        b" Copy",
    ) {
        return false;
    }
    unsafe {
        crate::channel_strip::hirari_channel_strip_set_gain(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_gain(source_strip),
        );
        crate::channel_strip::hirari_channel_strip_set_pan(
            dest_strip,
            crate::channel_strip::hirari_channel_strip_pan(source_strip),
        );
        crate::track_runtime::hirari_track_runtime_set_muted(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_muted(source_runtime) as u8,
        );
        crate::track_runtime::hirari_track_runtime_set_phase_invert(
            dest_runtime,
            crate::track_runtime::hirari_track_runtime_is_phase_inverted(source_runtime) as u8,
        );
        for kind in 0..3u32 {
            let mut count = 0usize;
            let points =
                crate::track_automation_store::hirari_track_automation_store_points(
                    source_automation, kind, &mut count,
                );
            crate::track_automation_store::hirari_track_automation_store_set_points(
                dest_automation, kind, points, count,
            );
        }
        let mut lane_count = 0usize;
        let lanes = crate::track_automation_store::hirari_track_automation_store_plugin_lane_views(
            source_automation,
            &mut lane_count,
        );
        if lane_count != 0 {
            crate::track_automation_store::hirari_track_automation_store_set_plugin_lanes(
                dest_automation, lanes, lane_count,
            );
        }
        let slot_count = crate::effect_chain_host::hirari_effect_chain_sandbox_slot_count(source_chain);
        let mut sandbox_slot = 0usize;
        for index in 0..crate::effect_chain_host::hirari_effect_chain_count(source_chain) {
            let kind = crate::effect_chain_host::hirari_effect_chain_node_kind(source_chain, index);
            let state = chain_state(source_chain, index);
            let bypassed = crate::effect_chain_host::hirari_effect_chain_bypass(source_chain, index);
            let added = if kind == crate::effect_chain_nodes::SANDBOX_KIND {
                let path_index = sandbox_slot;
                sandbox_slot += 1;
                if path_index >= slot_count {
                    false
                } else {
                    let path = chain_sandbox_path(source_chain, path_index);
                    !path.is_empty()
                        && crate::effect_chain_host::hirari_effect_chain_add_sandbox(
                            dest_chain,
                            path.as_ptr(),
                            path.len(),
                        )
                }
            } else {
                crate::effect_chain_host::hirari_effect_chain_add_builtin(
                    dest_chain, kind, dest_is_bus, false,
                )
            };
            if added && !state.is_empty() {
                let dest_index =
                    crate::effect_chain_host::hirari_effect_chain_count(dest_chain) - 1;
                crate::effect_chain_host::hirari_effect_chain_processor_state_restore(
                    dest_chain,
                    dest_index,
                    state.as_ptr(),
                    state.len(),
                );
                crate::effect_chain_host::hirari_effect_chain_set_bypass(
                    dest_chain,
                    dest_index,
                    bypassed,
                );
            }
        }
    }
    true
}
