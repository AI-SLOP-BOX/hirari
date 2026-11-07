//! Plugin sandbox maintenance and query policy.
//!
//! The track keeps its sandboxed processors because the audio callback drives
//! them. This module owns the control-side pass around them: walking the track
//! list, remembering the previous health of each sandbox, deciding which
//! failure/recovery/quarantine edges are worth an engine event, and asking the
//! engine to restart what can be restarted. It also answers the query surface
//! the engine used to forward by hand — slot paths, slot plugin state, the
//! stored-state and last-add failures, and the isolated block-render test
//! boundary — by walking the graph through the layout host operations and
//! reading each track's plug-in chain storage directly, so the engine only
//! supplies the tracks lock and the event ring its callback owns.
//!
//! Every entry point runs on the control thread. The observed-state map is
//! keyed by engine address, and the engine forgets its entry when it is
//! destroyed so a later engine that reuses the address starts clean.

use crate::effect_chain_host::{
    hirari_effect_chain_buffer_free, hirari_effect_chain_last_sandbox_failure,
    hirari_effect_chain_last_sandbox_failure_text, hirari_effect_chain_process_sandbox_block,
    hirari_effect_chain_process_sandbox_midi_block, hirari_effect_chain_sandbox_slot_count,
    hirari_effect_chain_sandbox_slot_index, hirari_effect_chain_sandbox_slot_path,
    hirari_effect_chain_sandbox_state_error, hirari_effect_chain_sandbox_state_error_text,
};
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::ptr;
use std::sync::{Mutex, OnceLock};

/// Mirror of `Track::SandboxStatus`, filled by the native status callback.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariPluginSandboxStatus {
    pub plugin_index: u32,
    pub alive: bool,
    pub can_retry: bool,
    pub failure: u8,
    pub dropped_output_midi: u32,
    pub mailbox_overruns: u32,
    pub input_midi_truncations: u32,
    pub recovery_mode: u32,
}

/// Visits one sandbox status; returning false stops the enumeration.
pub type StatusVisitorFn =
    unsafe extern "C" fn(*mut c_void, u32, *const HirariPluginSandboxStatus) -> bool;
/// Reports every track's sandbox statuses in engine order; the visitor
/// receives each status together with its track id.
pub type StatusesFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, Option<StatusVisitorFn>) -> bool;
/// Restarts one track's rejected sandboxes, then reports the resulting
/// statuses through the visitor; returns how many were recovered.
pub type MaintainFn =
    unsafe extern "C" fn(*mut c_void, u32, bool, *mut c_void, Option<StatusVisitorFn>) -> u32;
/// Posts one sandbox engine event. The native side stamps the current
/// playhead and copies the pre-formatted label.
pub type PublishEventFn =
    unsafe extern "C" fn(*mut c_void, u32, u32, f32, *const u8, usize);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariPluginSandboxHostOps {
    pub statuses: Option<StatusesFn>,
    pub maintain: Option<MaintainFn>,
    pub publish_event: Option<PublishEventFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariPluginSandboxHostOps {}
unsafe impl Sync for HirariPluginSandboxHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariPluginSandboxHostOps>>> = OnceLock::new();

#[derive(Clone, Copy, Default)]
struct Observed {
    alive: bool,
    failure: u8,
}

static OBSERVED: OnceLock<Mutex<Option<HashMap<usize, HashMap<u64, Observed>>>>> = OnceLock::new();

fn host_ops() -> Option<HirariPluginSandboxHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Reads one track's plug-in chain while the graph is locked, or `None` when
/// the engine has no such track.
fn with_track_chain<T>(
    host: *mut c_void,
    track_id: u32,
    read: impl FnOnce(*mut c_void) -> T,
) -> Option<T> {
    crate::layout_gather::with_track_storage(
        host.cast_const(),
        track_id,
        crate::layout_gather::CHAIN,
        read,
    )
}

/// Visits every track's plug-in chain in engine order, with the graph locked
/// for the whole walk so the slots stay where the callback left them.
fn each_track_chain(host: *mut c_void, visit: impl FnMut(u32, *mut c_void)) {
    crate::layout_gather::each_track_storage(
        host.cast_const(),
        crate::layout_gather::CHAIN,
        visit,
    );
}

/// The bundle path one sandbox slot is loading, as the chain stores it.
fn slot_path(chain: *const c_void, slot: usize) -> String {
    let length = unsafe { hirari_effect_chain_sandbox_slot_path(chain, slot, ptr::null_mut(), 0) };
    if length == 0 {
        return String::new();
    }
    let mut path = vec![0u8; length];
    if unsafe {
        hirari_effect_chain_sandbox_slot_path(chain, slot, path.as_mut_ptr(), length)
    } != length
    {
        return String::new();
    }
    String::from_utf8_lossy(&path).into_owned()
}

/// Text a chain FFI entry returns as a C string; a missing answer is empty.
fn c_text(value: *const std::os::raw::c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned()
}

/// Every sandbox status in engine order, or `None` when the host callbacks are
/// not installed.
fn snapshot_all(host: *mut c_void) -> Option<Vec<(u32, HirariPluginSandboxStatus)>> {
    let ops = host_ops()?;
    Some(collect_all_statuses(ops.statuses?, host))
}

/// Packs the engine's sandbox statuses into the wire format the UI decodes.
/// One snapshot is taken, so the per-sandbox drop counters are consumed once.
pub fn sandbox_statuses_packed(host: usize) -> Vec<u32> {
    // Mirrors Plugins::SandboxProtocol::kStatusHeaderV9.
    const STATUS_HEADER_V9: u32 = 0x4155_5209;
    let Some(statuses) = snapshot_all(host as *mut c_void) else {
        return vec![STATUS_HEADER_V9];
    };
    let mut packed = Vec::with_capacity(1 + statuses.len() * 9);
    packed.push(STATUS_HEADER_V9);
    for (track_id, status) in statuses {
        packed.extend_from_slice(&[
            track_id,
            status.plugin_index,
            status.alive as u32,
            status.can_retry as u32,
            u32::from(status.failure),
            status.dropped_output_midi,
            status.mailbox_overruns,
            status.input_midi_truncations,
            status.recovery_mode,
        ]);
    }
    packed
}

/// One sandbox slot as the engine ordered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxSlot {
    pub track_id: u32,
    pub slot: u32,
    pub plugin_index: u32,
    pub path: String,
}

/// Every sandbox slot in engine order; empty when the graph is unreachable.
pub fn sandbox_slots(host: *mut c_void) -> Vec<SandboxSlot> {
    let mut slots = Vec::new();
    each_track_chain(host, |track_id, chain| {
        let count = unsafe { hirari_effect_chain_sandbox_slot_count(chain.cast_const()) };
        for slot in 0..count {
            slots.push(SandboxSlot {
                track_id,
                slot: slot as u32,
                plugin_index: unsafe {
                    hirari_effect_chain_sandbox_slot_index(chain.cast_const(), slot)
                },
                path: slot_path(chain.cast_const(), slot),
            });
        }
    });
    slots
}

/// Bundle path of every sandboxed plugin, in engine order.
pub fn sandbox_plugin_paths(host: *mut c_void) -> Vec<String> {
    sandbox_slots(host).into_iter().map(|entry| entry.path).collect()
}

/// The plugin index bound to one sandbox slot.
fn slot_plugin_index(host: *mut c_void, track_id: u32, slot: u32) -> Option<u32> {
    sandbox_slots(host)
        .into_iter()
        .find(|entry| entry.track_id == track_id && entry.slot == slot)
        .map(|entry| entry.plugin_index)
}

/// Reads a sandbox slot's plugin state through the plugin-chain state op; the
/// slot keeps the engine's plugin index, so the state is the same bytes the
/// audio graph would persist.
pub fn sandbox_plugin_state(host: *mut c_void, track_id: u32, slot: u32) -> Vec<u8> {
    let Some(plugin_index) = slot_plugin_index(host, track_id, slot) else {
        return Vec::new();
    };
    let length = unsafe {
        crate::plugin_edit_host::hirari_plugin_edit_get_state(
            host,
            track_id,
            plugin_index,
            std::ptr::null_mut(),
            0,
        )
    };
    if length == 0 || length == usize::MAX {
        return Vec::new();
    }
    let mut state = vec![0u8; length];
    let written = unsafe {
        crate::plugin_edit_host::hirari_plugin_edit_get_state(
            host,
            track_id,
            plugin_index,
            state.as_mut_ptr(),
            length,
        )
    };
    state.truncate(written.min(length));
    state
}

/// Stores a sandbox slot's plugin state. Mirrors the engine's admission: an
/// oversized blob is refused before the graph is touched, and no undo entry is
/// recorded because hydration and recovery replay the preset themselves.
pub fn set_sandbox_plugin_state(
    host: *mut c_void,
    track_id: u32,
    slot: u32,
    state: &[u8],
) -> bool {
    if state.len() > MAX_SANDBOX_STATE_BYTES {
        return false;
    }
    let Some(plugin_index) = slot_plugin_index(host, track_id, slot) else {
        return false;
    };
    unsafe {
        crate::plugin_edit_host::hirari_plugin_edit_set_state(
            host,
            track_id,
            plugin_index,
            state.as_ptr(),
            state.len(),
        )
    }
}

/// Runs one isolated stereo block through a sandbox slot. The caller owns the
/// buffers; both must hold `frames` samples.
pub fn process_sandbox_block(
    host: *mut c_void,
    track_id: u32,
    slot: u32,
    left: &mut [f32],
    right: &mut [f32],
) -> bool {
    if left.is_empty() || left.len() != right.len() {
        return false;
    }
    let frames = left.len() as u32;
    with_track_chain(host, track_id, |chain| unsafe {
        hirari_effect_chain_process_sandbox_block(
            chain,
            slot as usize,
            left.as_mut_ptr(),
            right.as_mut_ptr(),
            frames,
        )
    })
    .unwrap_or(false)
}

/// Feeds one MIDI event through a sandbox slot and returns the event it
/// produced. An empty result means the sandbox produced nothing usable.
pub fn process_sandbox_midi_block(
    host: *mut c_void,
    track_id: u32,
    slot: u32,
    frames: u32,
    midi_data: &[u8],
) -> Vec<u8> {
    if midi_data.is_empty() {
        return Vec::new();
    }
    with_track_chain(host, track_id, |chain| {
        let mut length = 0_usize;
        let data = unsafe {
            hirari_effect_chain_process_sandbox_midi_block(
                chain,
                slot as usize,
                frames,
                midi_data.as_ptr(),
                midi_data.len(),
                &mut length,
            )
        };
        if data.is_null() {
            return Vec::new();
        }
        // The worker owns the payload until it is copied, and an event the
        // protocol cannot carry is reported as no event at all.
        let payload = if length > MAX_SANDBOX_MIDI_BYTES {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(data, length).to_vec() }
        };
        unsafe { hirari_effect_chain_buffer_free(data, length) };
        payload
    })
    .unwrap_or_default()
}

/// The last sandbox-add failure of one track. An unresolved track answers with
/// no failure and no text, which is what the previous native surface returned.
pub fn last_sandbox_failure(host: *mut c_void, track_id: u32) -> (u8, String) {
    with_track_chain(host, track_id, |chain| {
        (
            unsafe { hirari_effect_chain_last_sandbox_failure(chain.cast_const()) },
            c_text(unsafe {
                hirari_effect_chain_last_sandbox_failure_text(chain.cast_const())
            }),
        )
    })
    .unwrap_or((0, String::new()))
}

/// One sandbox slot's stored-state error. The engine's own fallback code is
/// reported when the track or slot is missing.
pub fn sandbox_state_error(host: *mut c_void, track_id: u32, slot: u32) -> (u8, String) {
    with_track_chain(host, track_id, |chain| {
        (
            unsafe { hirari_effect_chain_sandbox_state_error(chain.cast_const(), slot as usize) },
            c_text(unsafe {
                hirari_effect_chain_sandbox_state_error_text(chain.cast_const(), slot as usize)
            }),
        )
    })
    .unwrap_or((STATE_ERROR_UNAVAILABLE, String::new()))
}

/// Mirrors `SandboxProtocol::kStateErrorUnavailable`.
const STATE_ERROR_UNAVAILABLE: u8 = 5;
/// Mirrors `SandboxProtocol::kMaxStateBytes`.
const MAX_SANDBOX_STATE_BYTES: usize = 4 * 1024 * 1024;
/// Mirrors `SandboxProtocol::kMaxMidiPayloadBytes`.
const MAX_SANDBOX_MIDI_BYTES: usize = 256;

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_maintenance_host_install(    ops: *const HirariPluginSandboxHostOps,
) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { std::ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

/// Drops the observed-health state for one engine address.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_maintenance_forget(host: *mut c_void) {
    let slot = OBSERVED.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(&(host as usize));
        }
    }
}

fn collect_all_statuses(
    read: StatusesFn,
    host: *mut c_void,
) -> Vec<(u32, HirariPluginSandboxStatus)> {
    unsafe extern "C" fn visit(
        context: *mut c_void,
        track_id: u32,
        status: *const HirariPluginSandboxStatus,
    ) -> bool {
        if context.is_null() || status.is_null() {
            return false;
        }
        unsafe { &mut *context.cast::<Vec<(u32, HirariPluginSandboxStatus)>>() }
            .push((track_id, unsafe { *status }));
        true
    }
    let mut statuses = Vec::new();
    unsafe {
        read(
            host,
            (&mut statuses as *mut Vec<(u32, HirariPluginSandboxStatus)>).cast(),
            Some(visit),
        )
    };
    statuses
}

fn maintain_track(
    maintain: MaintainFn,
    host: *mut c_void,
    track_id: u32,
    auto_restart: bool,
) -> (u32, Vec<HirariPluginSandboxStatus>) {
    unsafe extern "C" fn visit(
        context: *mut c_void,
        _track_id: u32,
        status: *const HirariPluginSandboxStatus,
    ) -> bool {
        if context.is_null() || status.is_null() {
            return false;
        }
        unsafe { &mut *context.cast::<Vec<HirariPluginSandboxStatus>>() }
            .push(unsafe { *status });
        true
    }
    let mut statuses = Vec::new();
    let recovered = unsafe {
        maintain(
            host,
            track_id,
            auto_restart,
            (&mut statuses as *mut Vec<HirariPluginSandboxStatus>).cast(),
            Some(visit),
        )
    };
    (recovered, statuses)
}

fn publish(
    publish_event: PublishEventFn,
    host: *mut c_void,
    event_type: u32,
    track_id: u32,
    plugin_index: u32,
    value: f32,
    label: &str,
) {
    let label = format!("{label} #{plugin_index}");
    unsafe {
        publish_event(
            host,
            event_type,
            track_id,
            value,
            label.as_ptr(),
            label.len(),
        )
    };
}

/// Runs one maintenance pass. Returns the number of sandboxes recovered while
/// polling, matching the native method. The previous health of every sandbox
/// is compared exactly as before: recovery and failure are exclusive, while
/// quarantine and dropped-MIDI edges are reported on their own.
///
/// # Safety
/// `host` must be the engine pointer the installed callbacks expect.
#[no_mangle]
pub unsafe extern "C" fn hirari_plugin_sandbox_maintain(
    host: *mut c_void,
    auto_restart: bool,
) -> u32 {
    let Some(ops) = host_ops() else { return 0 };
    let (Some(statuses), Some(maintain), Some(publish_event)) =
        (ops.statuses, ops.maintain, ops.publish_event)
    else {
        return 0;
    };
    let slot = OBSERVED.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else { return 0 };
    let observed = guard
        .get_or_insert_with(HashMap::new)
        .entry(host as usize)
        .or_default();

    // One read of every track in engine order supplies each track's "before"
    // snapshot; the per-track restart then reports its own "after" statuses.
    let mut before: Vec<(u32, Vec<HirariPluginSandboxStatus>)> = Vec::new();
    for (track_id, status) in collect_all_statuses(statuses, host) {
        match before.last_mut() {
            Some((id, statuses)) if *id == track_id => statuses.push(status),
            _ => before.push((track_id, vec![status])),
        }
    }

    let mut recovered = 0;
    for (track_id, statuses) in before {
        for status in statuses {
            let key = (u64::from(track_id) << 32) | u64::from(status.plugin_index);
            let entry = observed.entry(key).or_default();
            if (!entry.alive && status.alive)
                || (entry.failure != 0 && status.failure == 0 && status.alive)
            {
                publish(
                    publish_event,
                    host,
                    0x61,
                    track_id,
                    status.plugin_index,
                    1.0,
                    "PLUGIN_SANDBOX_RECOVERED",
                );
            } else if entry.alive && !status.alive {
                publish(
                    publish_event,
                    host,
                    0x60,
                    track_id,
                    status.plugin_index,
                    f32::from(status.failure),
                    "PLUGIN_SANDBOX_FAILED",
                );
            }
            if entry.failure != status.failure && status.failure != 0 {
                publish(
                    publish_event,
                    host,
                    0x63,
                    track_id,
                    status.plugin_index,
                    f32::from(status.failure),
                    "PLUGIN_SANDBOX_QUARANTINED",
                );
            }
            if status.dropped_output_midi > 0 {
                publish(
                    publish_event,
                    host,
                    0x62,
                    track_id,
                    status.plugin_index,
                    status.dropped_output_midi as f32,
                    "PLUGIN_SANDBOX_MIDI_DROPPED",
                );
            }
            entry.alive = status.alive;
            entry.failure = status.failure;
        }

        let (restarted, after) = maintain_track(maintain, host, track_id, auto_restart);
        recovered += restarted;

        for status in after {
            let key = (u64::from(track_id) << 32) | u64::from(status.plugin_index);
            let entry = observed.entry(key).or_default();
            if !entry.alive && status.alive {
                publish(
                    publish_event,
                    host,
                    0x61,
                    track_id,
                    status.plugin_index,
                    1.0,
                    "PLUGIN_SANDBOX_RECOVERED",
                );
            }
            entry.alive = status.alive;
            entry.failure = status.failure;
        }
    }
    recovered
}

/// Backoff before retrying a failed worker spawn, or 0 when the failure is
/// terminal. Worker creation runs on the control thread; on macOS a worker
/// that just exited can briefly hold code-signature and IPC resources, so only
/// transport-level failures (spawn, handshake, early exit) are retried, with a
/// 25 ms doubling backoff. Plug-in ABI/load/instance failures stay terminal and
/// must never be hidden by a retry.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_transport_retry_delay(attempt: u32, failure: u8) -> u32 {
    if !matches!(failure, 3 | 4 | 11) || attempt >= 32 {
        return 0;
    }
    25u32 << attempt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static EVENTS: RefCell<Vec<(u32, u32, f32, String)>> = const { RefCell::new(Vec::new()) };
    }

    thread_local! {
        static SCRIPTS: RefCell<Vec<(u32, Vec<HirariPluginSandboxStatus>)>> =
            const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn statuses(
        _host: *mut c_void,
        context: *mut c_void,
        visitor: Option<StatusVisitorFn>,
    ) -> bool {
        let Some(visitor) = visitor else { return false };
        let next = SCRIPTS.with(|s| {
            let mut s = s.borrow_mut();
            if s.is_empty() {
                (0, Vec::new())
            } else {
                s.remove(0)
            }
        });
        for status in next.1 {
            if !unsafe { visitor(context, next.0, &status) } {
                break;
            }
        }
        true
    }

    unsafe extern "C" fn maintain(
        _host: *mut c_void,
        track_id: u32,
        _auto: bool,
        context: *mut c_void,
        visitor: Option<StatusVisitorFn>,
    ) -> u32 {
        let Some(visitor) = visitor else { return 0 };
        let next = SCRIPTS.with(|s| {
            let mut s = s.borrow_mut();
            if s.is_empty() {
                (0, Vec::new())
            } else {
                s.remove(0)
            }
        });
        for status in next.1 {
            if !unsafe { visitor(context, track_id, &status) } {
                break;
            }
        }
        0
    }

    unsafe extern "C" fn publish_event(
        _host: *mut c_void,
        event_type: u32,
        track_id: u32,
        value: f32,
        label: *const u8,
        length: usize,
    ) {
        let label = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(label, length) })
            .into_owned();
        EVENTS.with(|events| events.borrow_mut().push((event_type, track_id, value, label)));
    }

    /// The host table is process-wide and the engine installs its own copy,
    /// so these tests hand it back when they finish.
    struct HostOpsGuard(Option<HirariPluginSandboxHostOps>);

    impl Drop for HostOpsGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(ops) => unsafe { hirari_plugin_sandbox_maintenance_host_install(&ops) },
                None => unsafe { hirari_plugin_sandbox_maintenance_host_install(ptr::null()) },
            }
        }
    }

    fn install() -> HostOpsGuard {
        let previous = HostOpsGuard(host_ops());
        let ops = HirariPluginSandboxHostOps {
            statuses: Some(statuses),
            maintain: Some(maintain),
            publish_event: Some(publish_event),
        };
        unsafe { hirari_plugin_sandbox_maintenance_host_install(&ops) };
        previous
    }

    fn status(alive: bool, failure: u8, dropped: u32) -> HirariPluginSandboxStatus {
        HirariPluginSandboxStatus {
            plugin_index: 3,
            alive,
            can_retry: true,
            failure,
            dropped_output_midi: dropped,
            ..Default::default()
        }
    }

    #[test]
    fn reports_failure_quarantine_and_dropped_midi_edges() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _host_ops = install();
        EVENTS.with(|events| events.borrow_mut().clear());
        SCRIPTS.with(|s| {
            s.borrow_mut().clear();
            s.borrow_mut().push((7, vec![status(true, 0, 0)]));
            s.borrow_mut().push((7, vec![status(true, 0, 0)]));
            s.borrow_mut().push((7, vec![status(false, 2, 4)]));
            s.borrow_mut().push((7, vec![status(false, 2, 4)]));
        });
        let host = 0x1234usize as *mut c_void;
        unsafe {
            hirari_plugin_sandbox_maintenance_forget(host);
            assert_eq!(hirari_plugin_sandbox_maintain(host, false), 0);
            assert_eq!(hirari_plugin_sandbox_maintain(host, false), 0);
        }
        let events = EVENTS.with(|events| events.borrow().clone());
        assert_eq!(
            events.iter().map(|event| event.0).collect::<Vec<_>>(),
            vec![0x61, 0x60, 0x63, 0x62]
        );
        assert_eq!(events[1].3, "PLUGIN_SANDBOX_FAILED #3");
        unsafe { hirari_plugin_sandbox_maintenance_forget(host) };
    }

    #[test]
    fn reports_recovery_on_the_poll_after_restart() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _host_ops = install();
        EVENTS.with(|events| events.borrow_mut().clear());
        SCRIPTS.with(|s| {
            s.borrow_mut().clear();
            s.borrow_mut().push((7, vec![status(false, 5, 0)]));
            s.borrow_mut().push((7, vec![status(false, 5, 0)]));
            s.borrow_mut().push((7, vec![status(true, 0, 0)]));
            s.borrow_mut().push((7, vec![status(true, 0, 0)]));
        });
        let host = 0x5678usize as *mut c_void;
        unsafe {
            hirari_plugin_sandbox_maintenance_forget(host);
            hirari_plugin_sandbox_maintain(host, true);
            hirari_plugin_sandbox_maintain(host, true);
        }
        let events = EVENTS.with(|events| events.borrow().clone());
        assert_eq!(
            events.iter().map(|event| event.0).collect::<Vec<_>>(),
            vec![0x63, 0x61]
        );
        assert_eq!(events[1].3, "PLUGIN_SANDBOX_RECOVERED #3");
        unsafe { hirari_plugin_sandbox_maintenance_forget(host) };
    }

    #[test]
    fn packs_statuses_for_the_ui_protocol() {
        let _engine_guard = crate::tests::native_engine_test_guard();
        let _host_ops = install();
        SCRIPTS.with(|s| {
            s.borrow_mut().clear();
            s.borrow_mut().push((7, vec![status(true, 0, 3)]));
        });
        let packed = sandbox_statuses_packed(0x9abc);
        assert_eq!(packed[0], 0x4155_5209);
        assert_eq!(&packed[1..5], &[7, 3, 1, 1]);
        assert_eq!(packed[6], 3);
        assert_eq!(packed.len(), 10);
    }

    #[test]
    fn transport_retries_only_cover_transient_failures() {
        for code in 0..=255u8 {
            let delay = hirari_plugin_sandbox_transport_retry_delay(0, code);
            assert_eq!(delay != 0, matches!(code, 3 | 4 | 11));
        }
        assert_eq!(hirari_plugin_sandbox_transport_retry_delay(0, 3), 25);
        assert_eq!(hirari_plugin_sandbox_transport_retry_delay(4, 11), 400);
        assert_eq!(hirari_plugin_sandbox_transport_retry_delay(32, 3), 0);
    }
}
