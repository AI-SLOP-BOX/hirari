//! Control-command admission policy: target lookup, project/audio generation
//! recheck, queue-full reporting and the forensic journal ordering.
//!
//! The engine keeps the tracks the target lookup resolves against and the
//! consumer that applies commands in the audio callback. This module owns the
//! admission decision and writes the admitted command straight into the Rust
//! command ring, forensic journal and diagnostic log that the engine holds.
//!
//! Every entry point runs on the control thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::engine_rings::CommandRecord;

pub const COMMAND_ERROR_NONE: u8 = 0;
pub const COMMAND_ERROR_INVALID_TARGET: u8 = 1;
pub const COMMAND_ERROR_STALE_PROJECT_GENERATION: u8 = 2;
pub const COMMAND_ERROR_STALE_AUDIO_GENERATION: u8 = 3;
pub const COMMAND_ERROR_QUEUE_FULL: u8 = 4;

/// Report whether a track id still addresses a live track.
pub type TrackExistsFn = unsafe extern "C" fn(*mut c_void, u32) -> bool;
/// Write the current project and audio-configuration generations.
pub type GenerationsFn = unsafe extern "C" fn(*mut c_void, *mut u64, *mut u64) -> bool;
/// Hand over a Rust store the engine only holds.
pub type StoreFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariCommandHostOps {
    pub track_exists: Option<TrackExistsFn>,
    pub generations: Option<GenerationsFn>,
    pub command_rings: Option<StoreFn>,
    pub journaler: Option<StoreFn>,
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

#[no_mangle]
pub extern "C" fn hirari_engine_last_command_error(host: *mut c_void) -> u8 {
    last_error(host)
}

/// Reads the admission error recorded for `host` without a C++ accessor.
pub fn engine_last_command_error(host: usize) -> u8 {
    last_error(host as *mut c_void)
}

fn resolve(store: Option<StoreFn>, host: *mut c_void) -> *mut c_void {
    let Some(store) = store else {
        return std::ptr::null_mut();
    };
    unsafe { store(host) }
}

fn journal(ops: &HirariCommandHostOps, host: *mut c_void, entry: (u32, u32, f32, u64)) {
    let journaler = resolve(ops.journaler, host);
    if journaler.is_null() {
        return;
    }
    // SAFETY: the host hands back a journaler created by
    // `hirari_forensic_journaler_create` and keeps it alive for the engine.
    unsafe {
        crate::forensic_journaler::hirari_forensic_journaler_log(
            journaler,
            entry.0,
            entry.1,
            entry.2,
            entry.3,
        );
    }
}

/// The command queue being full is a reportable fault, not a rejected edit.
fn report_queue_full(track_id: u32) {
    const MESSAGE: &[u8] = b"COMMAND_QUEUE_FULL";
    // SAFETY: the default diagnostic ring is process-lifetime storage.
    unsafe {
        crate::log_buffer::hirari_log_buffer_post(
            crate::log_buffer::hirari_log_buffer_default_state(),
            1,
            track_id,
            MESSAGE.as_ptr(),
            MESSAGE.len(),
        );
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
    let (Some(track_exists), Some(generations)) = (ops.track_exists, ops.generations) else {
        return false;
    };
    let rings = resolve(ops.command_rings, host);
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
    let record = CommandRecord {
        type_: command_type,
        tid: track_id,
        val: value,
        ts: timestamp,
        expected_audio_generation,
    };
    // SAFETY: `rings` is either null or the engine's own ring handle, and the
    // record is a local value the ring copies out of.
    if rings.is_null() || !unsafe { crate::engine_rings::hirari_engine_command_ring_push(rings.cast(), &record) }
    {
        publish_error(host, COMMAND_ERROR_QUEUE_FULL);
        journal(ops, host, entry);
        report_queue_full(track_id);
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
    use std::cell::RefCell;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    // The last-command error table, the command ring and the journal file are
    // process state, so these tests must not overlap.
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static JOURNAL_SEQUENCE: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);

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

    const JOURNAL_BYTES: usize = 24;

    /// Real Rust ring and journaler handles behind a host pointer, so the
    /// admission policy is tested against the stores it now writes itself.
    struct TestHost {
        rings: *mut c_void,
        journaler: *mut c_void,
        journal_path: String,
    }

    impl TestHost {
        fn new() -> Self {
            let journal_path = std::env::temp_dir()
                .join(format!(
                    "hirari-command-journal-{}-{}",
                    std::process::id(),
                    JOURNAL_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ))
                .to_string_lossy()
                .into_owned();
            let rings: *mut c_void = crate::engine_rings::hirari_engine_rings_create().cast();
            let journaler = crate::forensic_journaler::hirari_forensic_journaler_create();
            // SAFETY: both handles come from the matching create functions and
            // the path bytes outlive the call.
            unsafe {
                crate::forensic_journaler::hirari_forensic_journaler_start(
                    journaler,
                    journal_path.as_ptr(),
                    journal_path.len(),
                );
            }
            Self {
                rings,
                journaler,
                journal_path,
            }
        }

        fn host(&mut self) -> *mut c_void {
            self as *mut Self as *mut c_void
        }

        fn push_to_ring(&self, record: &CommandRecord) -> bool {
            // SAFETY: `rings` is this host's own ring handle.
            unsafe { crate::engine_rings::hirari_engine_command_ring_push(self.rings.cast(), record) }
        }

        fn drain_ring(&self) -> Vec<CommandRecord> {
            let mut drained = Vec::new();
            loop {
                let mut record = CommandRecord::default();
                // SAFETY: `rings` is this host's own ring handle.
                let popped = unsafe {
                    crate::engine_rings::hirari_engine_command_ring_pop(
                        self.rings.cast(),
                        &mut record,
                    )
                };
                if !popped {
                    return drained;
                }
                drained.push(record);
            }
        }

        /// Flushes the writer thread and returns the records it persisted.
        fn journal_records(&self) -> Vec<(u32, u32, f32, u64)> {
            // SAFETY: `journaler` is this host's own journaler handle.
            unsafe { crate::forensic_journaler::hirari_forensic_journaler_stop(self.journaler) };
            let bytes = std::fs::read(&self.journal_path).unwrap_or_default();
            bytes
                .chunks(JOURNAL_BYTES)
                .filter(|chunk| chunk.len() == JOURNAL_BYTES)
                .map(|chunk| {
                    let kind = u32::from_ne_bytes(chunk[0..4].try_into().unwrap());
                    let track_id = u32::from_ne_bytes(chunk[4..8].try_into().unwrap());
                    let value = f32::from_ne_bytes(chunk[8..12].try_into().unwrap());
                    let timestamp = u64::from_ne_bytes(chunk[16..24].try_into().unwrap());
                    (kind, track_id, value, timestamp)
                })
                .collect()
        }

        fn fill_ring(&self) {
            let record = CommandRecord::default();
            while self.push_to_ring(&record) {}
        }

        fn pop_log_entry(&self) -> Option<crate::log_buffer::LogEntry> {
            let mut entry = crate::log_buffer::LogEntry::default();
            // SAFETY: the default diagnostic ring is process-lifetime storage.
            let popped = unsafe {
                crate::log_buffer::hirari_log_buffer_pop(
                    crate::log_buffer::hirari_log_buffer_default_state(),
                    &mut entry,
                )
            };
            popped.then_some(entry)
        }

        fn clear_log(&self) {
            while self.pop_log_entry().is_some() {}
        }

        fn queue_full_reported(&self, track_id: u32) -> bool {
            while let Some(entry) = self.pop_log_entry() {
                let text = std::str::from_utf8(&entry.message)
                    .unwrap_or_default()
                    .trim_end_matches('\0');
                if entry.component_id == track_id && text == "COMMAND_QUEUE_FULL" {
                    return true;
                }
            }
            false
        }
    }

    impl Drop for TestHost {
        fn drop(&mut self) {
            // SAFETY: each handle was boxed by its own create function.
            unsafe {
                crate::engine_rings::hirari_engine_rings_destroy(self.rings.cast());
                crate::forensic_journaler::hirari_forensic_journaler_destroy(self.journaler);
            }
            let _ = std::fs::remove_file(&self.journal_path);
        }
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

    unsafe extern "C" fn host_rings(host: *mut c_void) -> *mut c_void {
        // SAFETY: the host pointer is a live TestHost supplied by each test.
        unsafe { &*(host.cast::<TestHost>()) }.rings
    }

    unsafe extern "C" fn host_journaler(host: *mut c_void) -> *mut c_void {
        // SAFETY: the host pointer is a live TestHost supplied by each test.
        unsafe { &*(host.cast::<TestHost>()) }.journaler
    }

    fn fake_ops() -> HirariCommandHostOps {
        HirariCommandHostOps {
            track_exists: Some(fake_track_exists),
            generations: Some(fake_generations),
            command_rings: Some(host_rings),
            journaler: Some(host_journaler),
        }
    }

    #[test]
    fn push_rejects_an_unknown_target_before_reading_generations() {
        let _serial = serial();
        let mut host = TestHost::new();
        let ops = fake_ops();
        let pointer = host.host();
        assert!(!push_impl(&ops, pointer, 0, 99, 1.0, 10, 7, 3));
        assert_eq!(take_log(), vec!["exists 99"]);
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 99, 1.0, 10)]);
        assert_eq!(error_for(pointer), COMMAND_ERROR_INVALID_TARGET);
    }

    #[test]
    fn push_rejects_a_stale_project_generation_before_the_audio_generation() {
        let _serial = serial();
        let mut host = TestHost::new();
        let ops = fake_ops();
        let pointer = host.host();
        assert!(!push_impl(&ops, pointer, 0, 5, 1.0, 10, 6, 3));
        assert_eq!(take_log(), vec!["exists 5", "generations"]);
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 5, 1.0, 10)]);
        assert_eq!(
            error_for(pointer),
            COMMAND_ERROR_STALE_PROJECT_GENERATION
        );
    }

    #[test]
    fn push_rejects_a_stale_audio_generation() {
        let _serial = serial();
        let mut host = TestHost::new();
        let ops = fake_ops();
        let pointer = host.host();
        assert!(!push_impl(&ops, pointer, 0, 5, 1.0, 10, 7, 2));
        assert_eq!(take_log(), vec!["exists 5", "generations"]);
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 5, 1.0, 10)]);
        assert_eq!(error_for(pointer), COMMAND_ERROR_STALE_AUDIO_GENERATION);
    }

    #[test]
    fn push_reports_a_full_queue_after_the_journal_entry() {
        let _serial = serial();
        let mut host = TestHost::new();
        host.clear_log();
        host.fill_ring();
        let queued = host.drain_ring().len();
        host.fill_ring();
        let ops = fake_ops();
        let pointer = host.host();
        assert!(!push_impl(&ops, pointer, 2, 5, 0.5, 10, 7, 3));
        assert_eq!(take_log(), vec!["exists 5", "generations"]);
        assert_eq!(host.drain_ring().len(), queued);
        assert_eq!(host.journal_records(), vec![(2, 5, 0.5, 10)]);
        assert!(host.queue_full_reported(5));
        assert_eq!(error_for(pointer), COMMAND_ERROR_QUEUE_FULL);
    }

    #[test]
    fn push_stamps_the_accepted_command_and_journals_it() {
        let _serial = serial();
        let mut host = TestHost::new();
        let ops = fake_ops();
        let pointer = host.host();
        assert!(push_impl(&ops, pointer, 3, 5, 1.0, 10, 7, 3));
        assert_eq!(take_log(), vec!["exists 5", "generations"]);
        assert_eq!(
            host.drain_ring(),
            vec![CommandRecord {
                type_: 3,
                tid: 5,
                val: 1.0,
                ts: 10,
                expected_audio_generation: 3,
            }]
        );
        assert_eq!(host.journal_records(), vec![(3, 5, 1.0, 10)]);
        assert_eq!(error_for(pointer), COMMAND_ERROR_NONE);
    }

    #[test]
    fn unreadable_generations_fail_closed_as_a_stale_project() {
        let _serial = serial();
        let mut host = TestHost::new();
        let mut ops = fake_ops();
        ops.generations = Some(unreadable_generations);
        let pointer = host.host();
        assert!(!push_impl(&ops, pointer, 0, 5, 1.0, 10, 0, 0));
        assert_eq!(take_log(), vec!["exists 5", "generations"]);
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 5, 1.0, 10)]);
        assert_eq!(
            error_for(pointer),
            COMMAND_ERROR_STALE_PROJECT_GENERATION
        );
    }

    #[test]
    fn missing_ops_fail_closed_without_touching_the_error() {
        let _serial = serial();
        let mut host = TestHost::new();
        let pointer = host.host();
        let ops = HirariCommandHostOps::default();
        assert!(!push_impl(&ops, pointer, 0, 5, 1.0, 10, 7, 3));
        assert!(take_log().is_empty());
        assert_eq!(error_for(pointer), COMMAND_ERROR_NONE);
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
