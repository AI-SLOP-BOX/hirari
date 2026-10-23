use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct LockEntry { user_id: Vec<u8>, last_heartbeat: Instant }
struct EditEntry { _user_id: Vec<u8>, _sequence: u64, _data: Vec<u8> }
#[derive(Default)]
struct HubState {
    user_sequences: HashMap<Vec<u8>, u64>,
    locks: HashMap<u32, LockEntry>,
    edit_log: Vec<EditEntry>,
}
struct CollaborationHub { state: Mutex<HubState> }

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 { Some(&[]) } else if ptr.is_null() { None } else { Some(std::slice::from_raw_parts(ptr, len)) }
}

#[no_mangle]
pub extern "C" fn hirari_collaboration_hub_create() -> *mut c_void {
    Box::into_raw(Box::new(CollaborationHub { state: Mutex::new(HubState::default()) })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_collaboration_hub_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<CollaborationHub>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_collaboration_hub_log_edit(
    state: *mut c_void, user: *const u8, user_len: usize, sequence: u64,
    data: *const u8, data_len: usize,
) -> bool {
    let Some(hub) = state.cast::<CollaborationHub>().as_ref() else { return false };
    let (Some(user), Some(data)) = (bytes(user, user_len), bytes(data, data_len)) else { return false };
    let Ok(mut state) = hub.state.lock() else { return false };
    let last = state.user_sequences.entry(user.to_vec()).or_insert(0);
    if sequence <= *last { return true; }
    *last = sequence;
    state.edit_log.push(EditEntry { _user_id: user.to_vec(), _sequence: sequence, _data: data.to_vec() });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_collaboration_hub_is_locked(
    state: *const c_void, track_id: u32, requester: *const u8, requester_len: usize,
) -> bool {
    let Some(hub) = state.cast::<CollaborationHub>().as_ref() else { return false };
    let Some(requester) = bytes(requester, requester_len) else { return false };
    let Ok(state) = hub.state.lock() else { return false };
    let Some(lock) = state.locks.get(&track_id) else { return false };
    if lock.last_heartbeat.elapsed().as_secs() > 5 { return false; }
    lock.user_id.as_slice() != requester
}

#[no_mangle]
pub unsafe extern "C" fn hirari_collaboration_hub_acquire(
    state: *mut c_void, track_id: u32, user: *const u8, user_len: usize,
) -> bool {
    let Some(hub) = state.cast::<CollaborationHub>().as_ref() else { return false };
    let Some(user) = bytes(user, user_len) else { return false };
    let Ok(mut state) = hub.state.lock() else { return false };
    state.locks.insert(track_id, LockEntry { user_id: user.to_vec(), last_heartbeat: Instant::now() });
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_collaboration_hub_refresh(
    state: *mut c_void, track_id: u32, user: *const u8, user_len: usize,
) -> bool {
    let Some(hub) = state.cast::<CollaborationHub>().as_ref() else { return false };
    let Some(user) = bytes(user, user_len) else { return false };
    let Ok(mut state) = hub.state.lock() else { return false };
    let stale = Instant::now().checked_sub(Duration::from_secs(10)).unwrap_or_else(Instant::now);
    let lock = state.locks.entry(track_id).or_insert(LockEntry { user_id: Vec::new(), last_heartbeat: stale });
    if lock.user_id == user { lock.last_heartbeat = Instant::now(); }
    true
}
