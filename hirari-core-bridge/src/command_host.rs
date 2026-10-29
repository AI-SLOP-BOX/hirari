//! Control-command admission policy: target lookup, project/audio generation
//! recheck, queue-full reporting and the forensic journal ordering.
//!
//! The engine keeps the lock-free queue and the consumer that applies commands
//! in the audio callback. This module decides whether a command may enter that
//! queue at all, and which error the caller sees when it may not.
//!
//! Every entry point runs on the control thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

pub const COMMAND_ERROR_NONE: u8 = 0;
pub const COMMAND_ERROR_INVALID_TARGET: u8 = 1;
pub const COMMAND_ERROR_STALE_PROJECT_GENERATION: u8 = 2;
pub const COMMAND_ERROR_STALE_AUDIO_GENERATION: u8 = 3;
pub const COMMAND_ERROR_QUEUE_FULL: u8 = 4;

/// Report whether a track id still addresses a live track.
pub type TrackExistsFn = unsafe extern "C" fn(*mut c_void, u32) -> bool;
/// Write the current project and audio-configuration generations.
pub type GenerationsFn = unsafe extern "C" fn(*mut c_void, *mut u64, *mut u64) -> bool;
/// Enqueue a command stamped with the audio generation it was admitted for.
pub type EnqueueFn = unsafe extern "C" fn(*mut c_void, u32, u32, f32, u64, u64) -> bool;
/// Append the command to the forensic journal.
pub type JournalFn = unsafe extern "C" fn(*mut c_void, u32, u32, f32, u64);
/// Surface a full command queue in the diagnostics log.
pub type LogQueueFullFn = unsafe extern "C" fn(*mut c_void, u32);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariCommandHostOps {
    pub track_exists: Option<TrackExistsFn>,
    pub generations: Option<GenerationsFn>,
    pub enqueue: Option<EnqueueFn>,
    pub journal: Option<JournalFn>,
    pub log_queue_full: Option<LogQueueFullFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariCommandHostOps {}
unsafe impl Sync for HirariCommandHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariCommandHostOps>>> = OnceLock::new();
// The last error is per engine instance: a push rejected on one engine must
// not be reported by another engine's query.
static LAST_ERRORS: OnceLock<Mutex<HashMap<usize, u8>>> = OnceLock::new();

fn host_ops() -> Option<HirariCommandHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_command_host_install(ops: *const HirariCommandHostOps) {
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

fn publish_error(host: *mut c_void, error: u8) {
    let errors = LAST_ERRORS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut errors) = errors.lock() {
        errors.insert(host as usize, error);
    }
}

fn last_error(host: *mut c_void) -> u8 {
    let errors = LAST_ERRORS.get_or_init(|| Mutex::new(HashMap::new()));
    errors
        .lock()
        .ok()
        .and_then(|errors| errors.get(&(host as usize)).copied())
        .unwrap_or(COMMAND_ERROR_NONE)
}

fn journal(ops: &HirariCommandHostOps, host: *mut c_void, entry: (u32, u32, f32, u64)) {
    if let Some(journal) = ops.journal {
        unsafe { journal(host, entry.0, entry.1, entry.2, entry.3) };
    }
}

#[allow(clippy::too_many_arguments)]
fn push_impl(
    ops: &HirariCommandHostOps,
    host: *mut c_void,
    command_type: u32,
    track_id: u32,
    value: f32,
    timestamp: u64,
    expected_project_generation: u64,
    expected_audio_generation: u64,
) -> bool {
    let (Some(track_exists), Some(generations), Some(enqueue)) =
        (ops.track_exists, ops.generations, ops.enqueue)
    else {
        return false;
    };
    let entry = (command_type, track_id, value, timestamp);
    // A stale id must never fall through to the queue: the consumer would
    // resolve it against a different project after a reload.
    if !unsafe { track_exists(host, track_id) } {
        publish_error(host, COMMAND_ERROR_INVALID_TARGET);
        journal(ops, host, entry);
        return false;
    }
    let mut project_generation = 0u64;
    let mut audio_generation = 0u64;
    // An unreadable generation is not "generation zero": admitting it could
    // apply an old command to a rebuilt project.
    if !unsafe { generations(host, &mut project_generation, &mut audio_generation) } {
        publish_error(host, COMMAND_ERROR_STALE_PROJECT_GENERATION);
        journal(ops, host, entry);
        return false;
    }
    if project_generation != expected_project_generation {
        publish_error(host, COMMAND_ERROR_STALE_PROJECT_GENERATION);
        journal(ops, host, entry);
        return false;
    }
    if audio_generation != expected_audio_generation {
        publish_error(host, COMMAND_ERROR_STALE_AUDIO_GENERATION);
        journal(ops, host, entry);
        return false;
    }
    // A full queue means the audio thread cannot observe the command, so it is
    // a real failure rather than a best-effort enqueue.
    if !unsafe {
        enqueue(
            host,
            command_type,
            track_id,
            value,
            timestamp,
            expected_audio_generation,
        )
    } {
        publish_error(host, COMMAND_ERROR_QUEUE_FULL);
        journal(ops, host, entry);
        if let Some(log) = ops.log_queue_full {
            unsafe { log(host, track_id) };
        }
        return false;
    }
    publish_error(host, COMMAND_ERROR_NONE);
    journal(ops, host, entry);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_engine_push_command(
    host: *mut c_void,
    command_type: u32,
    track_id: u32,
    value: f32,
    timestamp: u64,
    expected_project_generation: u64,
    expected_audio_generation: u64,
) -> bool {
    match host_ops() {
        Some(ops) => push_impl(
            &ops,
            host,
            command_type,
            track_id,
            value,
            timestamp,
            expected_project_generation,
            expected_audio_generation,
        ),
        None => false,
    }
}

#[no_mangle]
pub extern "C" fn hirari_engine_last_command_error(host: *mut c_void) -> u8 {
    last_error(host)
}

/// Commands the audio callback applies to its published track directly.
pub const AUDIO_COMMAND_NONE: u32 = 0;
pub const AUDIO_COMMAND_VOLUME: u32 = 1;
pub const AUDIO_COMMAND_PAN: u32 = 2;
pub const AUDIO_COMMAND_MUTE: u32 = 3;
pub const AUDIO_COMMAND_SOLO: u32 = 4;

// Mirrors `Hirari::Core::CommandType` from src/core/bridge_types.hpp.
const COMMAND_TYPE_SET_VOLUME: u32 = 0;
const COMMAND_TYPE_SET_PAN: u32 = 1;
const COMMAND_TYPE_SET_MUTE: u32 = 2;
const COMMAND_TYPE_SET_SOLO: u32 = 3;

/// Sanitises a queued command for the audio callback.
///
/// Volume and pan are clamped only for finite values; a non-finite value is
/// dropped exactly as the previous inline C++ checks did. `out_value` receives
/// the value the caller should apply, and the returned id selects the setter.
#[no_mangle]
pub unsafe extern "C" fn hirari_engine_audio_command(
    command_type: u32,
    value: f32,
    out_value: *mut f32,
) -> u32 {
    let Some(output) = (unsafe { out_value.as_mut() }) else {
        return AUDIO_COMMAND_NONE;
    };
    *output = value;
    match command_type {
        COMMAND_TYPE_SET_VOLUME if value.is_finite() => {
            *output = value.clamp(0.0, 2.0);
            AUDIO_COMMAND_VOLUME
        }
        COMMAND_TYPE_SET_PAN if value.is_finite() => {
            *output = value.clamp(-1.0, 1.0);
            AUDIO_COMMAND_PAN
        }
        COMMAND_TYPE_SET_MUTE => AUDIO_COMMAND_MUTE,
        COMMAND_TYPE_SET_SOLO => AUDIO_COMMAND_SOLO,
        _ => AUDIO_COMMAND_NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static ENQUEUE_OK: Cell<bool> = const { Cell::new(true) };
    }

    // The last-command error table is process state, so these tests must not
    // overlap.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn log(entry: impl Into<String>) {
        LOG.with(|log| log.borrow_mut().push(entry.into()));
    }

    fn take_log() -> Vec<String> {
        LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }

    fn error_for(host: *mut c_void) -> u8 {
        last_error(host)
    }

    unsafe extern "C" fn fake_track_exists(_host: *mut c_void, track_id: u32) -> bool {
        log(format!("exists {track_id}"));
        track_id != 99
    }

    unsafe extern "C" fn fake_generations(
        _host: *mut c_void,
        project: *mut u64,
        audio: *mut u64,
    ) -> bool {
        log("generations".to_string());
        unsafe {
            *project = 7;
            *audio = 3;
        }
        true
    }

    unsafe extern "C" fn unreadable_generations(
        _host: *mut c_void,
        _project: *mut u64,
        _audio: *mut u64,
    ) -> bool {
        log("generations".to_string());
        false
    }

    unsafe extern "C" fn fake_enqueue(
        _host: *mut c_void,
        command_type: u32,
        track_id: u32,
        value: f32,
        timestamp: u64,
        audio_generation: u64,
    ) -> bool {
        log(format!(
            "enqueue {command_type} {track_id} {value} {timestamp} {audio_generation}"
        ));
        ENQUEUE_OK.with(|ok| ok.get())
    }

    unsafe extern "C" fn fake_journal(
        _host: *mut c_void,
        command_type: u32,
        track_id: u32,
        value: f32,
        timestamp: u64,
    ) {
        log(format!("journal {command_type} {track_id} {value} {timestamp}"));
    }

    unsafe extern "C" fn fake_log_queue_full(_host: *mut c_void, track_id: u32) {
        log(format!("queue_full {track_id}"));
    }

    fn fake_ops() -> HirariCommandHostOps {
        HirariCommandHostOps {
            track_exists: Some(fake_track_exists),
            generations: Some(fake_generations),
            enqueue: Some(fake_enqueue),
            journal: Some(fake_journal),
            log_queue_full: Some(fake_log_queue_full),
        }
    }

    #[test]
    fn push_rejects_an_unknown_target_before_reading_generations() {
        let _serial = serial();
        let ops = fake_ops();
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 0, 99, 1.0, 10, 7, 3));
        assert_eq!(take_log(), vec!["exists 99", "journal 0 99 1 10"]);
        assert_eq!(error_for(8_usize as *mut c_void), COMMAND_ERROR_INVALID_TARGET);
    }

    #[test]
    fn push_rejects_a_stale_project_generation_before_the_audio_generation() {
        let _serial = serial();
        let ops = fake_ops();
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 0, 5, 1.0, 10, 6, 3));
        assert_eq!(
            take_log(),
            vec!["exists 5", "generations", "journal 0 5 1 10"]
        );
        assert_eq!(
            error_for(8_usize as *mut c_void),
            COMMAND_ERROR_STALE_PROJECT_GENERATION
        );
    }

    #[test]
    fn push_rejects_a_stale_audio_generation() {
        let _serial = serial();
        let ops = fake_ops();
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 0, 5, 1.0, 10, 7, 2));
        assert_eq!(
            take_log(),
            vec!["exists 5", "generations", "journal 0 5 1 10"]
        );
        assert_eq!(
            error_for(8_usize as *mut c_void),
            COMMAND_ERROR_STALE_AUDIO_GENERATION
        );
    }

    #[test]
    fn push_reports_a_full_queue_after_the_journal_entry() {
        let _serial = serial();
        ENQUEUE_OK.with(|ok| ok.set(false));
        let ops = fake_ops();
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 2, 5, 0.5, 10, 7, 3));
        assert_eq!(
            take_log(),
            vec![
                "exists 5",
                "generations",
                "enqueue 2 5 0.5 10 3",
                "journal 2 5 0.5 10",
                "queue_full 5",
            ]
        );
        assert_eq!(error_for(8_usize as *mut c_void), COMMAND_ERROR_QUEUE_FULL);
        ENQUEUE_OK.with(|ok| ok.set(true));
    }

    #[test]
    fn push_stamps_the_accepted_command_and_journals_it() {
        let _serial = serial();
        let ops = fake_ops();
        assert!(push_impl(&ops, 8_usize as *mut c_void, 3, 5, 1.0, 10, 7, 3));
        assert_eq!(
            take_log(),
            vec![
                "exists 5",
                "generations",
                "enqueue 3 5 1 10 3",
                "journal 3 5 1 10",
            ]
        );
        assert_eq!(error_for(8_usize as *mut c_void), COMMAND_ERROR_NONE);
    }

    #[test]
    fn unreadable_generations_fail_closed_as_a_stale_project() {
        let _serial = serial();
        let mut ops = fake_ops();
        ops.generations = Some(unreadable_generations);
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 0, 5, 1.0, 10, 0, 0));
        assert_eq!(
            take_log(),
            vec!["exists 5", "generations", "journal 0 5 1 10"]
        );
        assert_eq!(
            error_for(8_usize as *mut c_void),
            COMMAND_ERROR_STALE_PROJECT_GENERATION
        );
    }

    #[test]
    fn missing_ops_fail_closed_without_touching_the_error() {
        let _serial = serial();
        let ops = HirariCommandHostOps::default();
        assert!(!push_impl(&ops, 8_usize as *mut c_void, 0, 5, 1.0, 10, 7, 3));
        assert!(take_log().is_empty());
        assert_eq!(error_for(8_usize as *mut c_void), COMMAND_ERROR_NONE);
    }
}

#[cfg(test)]
mod audio_command_tests {
    use super::*;

    fn decide(command_type: u32, value: f32) -> (u32, f32) {
        let mut applied = 0.0f32;
        let op = unsafe { hirari_engine_audio_command(command_type, value, &mut applied) };
        (op, applied)
    }

    #[test]
    fn volume_and_pan_are_clamped() {
        assert_eq!(decide(0, 3.5), (AUDIO_COMMAND_VOLUME, 2.0));
        assert_eq!(decide(0, -1.0), (AUDIO_COMMAND_VOLUME, 0.0));
        assert_eq!(decide(1, -4.0), (AUDIO_COMMAND_PAN, -1.0));
        assert_eq!(decide(1, 0.25), (AUDIO_COMMAND_PAN, 0.25));
    }

    #[test]
    fn non_finite_volume_and_pan_are_dropped() {
        let (op, applied) = decide(0, f32::NAN);
        assert_eq!(op, AUDIO_COMMAND_NONE);
        assert!(applied.is_nan());
        assert_eq!(decide(1, f32::INFINITY).0, AUDIO_COMMAND_NONE);
    }

    #[test]
    fn mute_and_solo_pass_their_threshold_value() {
        assert_eq!(decide(2, 0.75), (AUDIO_COMMAND_MUTE, 0.75));
        assert_eq!(decide(3, 0.1), (AUDIO_COMMAND_SOLO, 0.1));
        assert_eq!(decide(2, f32::NAN).0, AUDIO_COMMAND_MUTE);
    }

    #[test]
    fn other_commands_and_missing_output_are_ignored() {
        assert_eq!(decide(4, 1.0), (AUDIO_COMMAND_NONE, 1.0));
        let op = unsafe {
            hirari_engine_audio_command(0, 1.0, core::ptr::null_mut())
        };
        assert_eq!(op, AUDIO_COMMAND_NONE);
    }
}
