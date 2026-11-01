//! History control for the engine facade.
//!
//! The undo/redo stacks, transactions, callback dispatch and counts are
//! already Rust-owned (`undo_transaction.rs`); the engine only owns the graph
//! resync that has to run after a structural edit is replayed. These entry
//! points are what the C++ facade shims call, so undo no longer round-trips
//! through engine wrappers just to reach the same Rust history.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;

use crate::log_buffer::hirari_log_buffer_post;
use crate::undo_transaction::{
    hirari_undo_manager_abort_and_invoke, hirari_undo_manager_apply_and_invoke,
    hirari_undo_manager_begin_transaction, hirari_undo_manager_clear,
    hirari_undo_manager_end_transaction, hirari_undo_manager_redo_count,
    hirari_undo_manager_top_name, hirari_undo_manager_transaction_active,
    hirari_undo_manager_undo_count,
};

const HISTORY_COMPONENT: u32 = 0xA001;
/// `Diagnostics::LogBuffer::kMaxLogLen - 1`: the ring's text field without the
/// terminator.
const MAX_LOG_MESSAGE: usize = 95;

extern "C" {
    pub(crate) fn hirari_engine_sync_structural_changes(host: *mut c_void);
}

fn post_history_log(log: *const c_void, prefix: &[u8], name: &[u8]) {
    let mut message = Vec::with_capacity(prefix.len() + name.len());
    message.extend_from_slice(prefix);
    message.extend_from_slice(name);
    let length = message.len().min(MAX_LOG_MESSAGE);
    unsafe { hirari_log_buffer_post(log, 0, HISTORY_COMPONENT, message.as_ptr(), length) };
}

fn top_name(undo: *const c_void, redo: bool) -> Vec<u8> {
    let required = unsafe { hirari_undo_manager_top_name(undo, redo, std::ptr::null_mut(), 0) };
    if required == 0 {
        return Vec::new();
    }
    let mut name = vec![0u8; required];
    let count = unsafe { hirari_undo_manager_top_name(undo, redo, name.as_mut_ptr(), required) };
    name.truncate(count.min(required));
    name
}

/// # Safety
/// `undo` and `log` must be the history and diagnostics addresses the engine
/// owns, and `host` the engine the callback resolves structural changes on.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_undo(
    undo: *mut c_void,
    log: *const c_void,
    host: *mut c_void,
) {
    let name = top_name(undo, false);
    if !unsafe { hirari_undo_manager_apply_and_invoke(undo, false) } {
        let message = b"UNDO | STACK_EMPTY";
        unsafe {
            hirari_log_buffer_post(
                log,
                1,
                HISTORY_COMPONENT,
                message.as_ptr(),
                message.len(),
            )
        };
        return;
    }
    post_history_log(log, b"UNDO | UNDONE | ", &name);
    unsafe { hirari_engine_sync_structural_changes(host) };
}

/// # Safety
/// Same contract as [`hirari_undo_edit_undo`], in the redo direction.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_redo(
    undo: *mut c_void,
    log: *const c_void,
    host: *mut c_void,
) {
    let name = top_name(undo, true);
    if !unsafe { hirari_undo_manager_apply_and_invoke(undo, true) } {
        let message = b"UNDO | REDO_STACK_EMPTY";
        unsafe {
            hirari_log_buffer_post(
                log,
                1,
                HISTORY_COMPONENT,
                message.as_ptr(),
                message.len(),
            )
        };
        return;
    }
    post_history_log(log, b"UNDO | REDONE | ", &name);
    unsafe { hirari_engine_sync_structural_changes(host) };
}

/// # Safety
/// `undo` must be the history address the engine owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_transaction_active(undo: *const c_void) -> bool {
    unsafe { hirari_undo_manager_transaction_active(undo) }
}

/// # Safety
/// `undo` must be the history address the engine owns; `name` must be valid
/// for `name_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_begin_transaction(
    undo: *mut c_void,
    name: *const u8,
    name_len: usize,
) {
    // Starting a new transaction rolls back an unfinished one, exactly as the
    // facade's manager did before replacing it.
    if unsafe { hirari_undo_manager_transaction_active(undo) } {
        let _ = unsafe { hirari_undo_manager_abort_and_invoke(undo) };
    }
    unsafe { hirari_undo_manager_begin_transaction(undo, name, name_len) };
}

/// # Safety
/// `undo` must be the history address the engine owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_end_transaction(undo: *mut c_void) -> bool {
    unsafe { hirari_undo_manager_end_transaction(undo) }
}

/// # Safety
/// `undo` must be the history address the engine owns, and `host` the engine
/// the callback resolves structural changes on.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_abort_transaction(
    undo: *mut c_void,
    host: *mut c_void,
) -> bool {
    if !unsafe { hirari_undo_manager_abort_and_invoke(undo) } {
        return false;
    }
    unsafe { hirari_engine_sync_structural_changes(host) };
    true
}

/// # Safety
/// `undo` must be the history address the engine owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_clear(undo: *mut c_void) {
    unsafe { hirari_undo_manager_clear(undo) };
}

/// # Safety
/// `undo` must be the history address the engine owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_undo_count(undo: *const c_void) -> u32 {
    unsafe { hirari_undo_manager_undo_count(undo) }.min(u32::MAX as usize) as u32
}

/// # Safety
/// `undo` must be the history address the engine owns.
#[no_mangle]
pub unsafe extern "C" fn hirari_undo_edit_redo_count(undo: *const c_void) -> u32 {
    unsafe { hirari_undo_manager_redo_count(undo) }.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::undo_transaction::{
        hirari_undo_manager_create, hirari_undo_manager_destroy, hirari_undo_manager_record,
        hirari_undo_manager_register_native_callback,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ABORTED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn mark(_context: *mut c_void) {
        ABORTED.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "C" fn forget(_context: *mut c_void) {}

    #[test]
    fn starting_a_transaction_rolls_back_an_unfinished_one() {
        unsafe {
            ABORTED.store(0, Ordering::SeqCst);
            let state = hirari_undo_manager_create(8, 300);
            let undo = hirari_undo_manager_register_native_callback(
                state,
                std::ptr::null_mut(),
                Some(mark),
                Some(forget),
            );
            let redo = hirari_undo_manager_register_native_callback(
                state,
                std::ptr::null_mut(),
                Some(mark),
                Some(forget),
            );
            assert!(hirari_undo_manager_begin_transaction(
                state,
                b"first".as_ptr(),
                5,
            ));
            assert!(hirari_undo_manager_record(
                state,
                b"edit".as_ptr(),
                4,
                undo,
                redo,
                0,
                false,
            ));

            hirari_undo_edit_begin_transaction(state, b"second".as_ptr(), 6);

            assert_eq!(ABORTED.load(Ordering::SeqCst), 1);
            assert!(hirari_undo_manager_transaction_active(state));
            hirari_undo_manager_destroy(state);
        }
    }
}
