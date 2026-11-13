//! Control-command admission policy: target lookup, project/audio generation
//! recheck, queue-full reporting and the forensic journal ordering.
//!
//! The tracks a target resolves against, the generations a command is stamped
//! with, the command ring and the forensic journal are all stores the engine
//! holds in its session bag, so this module reads them from the host pointer it
//! is given and owns the admission decision itself: a command only reaches the
//! queue when its track still exists and both generations still match.
//!
//! Every entry point runs on the control thread.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::engine_rings::CommandRecord;
use crate::engine_session::{handle_of_host, H_ENGINE_BLOCK, H_ENGINE_RINGS, H_FORENSIC_JOURNAL};
use crate::track_session;

pub const COMMAND_ERROR_NONE: u8 = 0;
pub const COMMAND_ERROR_INVALID_TARGET: u8 = 1;
pub const COMMAND_ERROR_STALE_PROJECT_GENERATION: u8 = 2;
pub const COMMAND_ERROR_STALE_AUDIO_GENERATION: u8 = 3;
pub const COMMAND_ERROR_QUEUE_FULL: u8 = 4;

// The last error is per engine instance: a push rejected on one engine must
// not be reported by another engine's query.
static LAST_ERRORS: OnceLock<Mutex<HashMap<usize, u8>>> = OnceLock::new();

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

/// Reads the admission error recorded for `host` without a C++ accessor.
pub fn engine_last_command_error(host: usize) -> u8 {
    last_error(host as *mut c_void)
}

/// Whether the id still addresses a track in the engine's own table. The lookup
/// runs under the tracks lock, so a track being removed concurrently cannot be
/// admitted against.
fn track_exists(host: *mut c_void, track_id: u32) -> bool {
    let Some(guard) = track_session::GraphLock::acquire(host) else {
        return false;
    };
    !track_session::find(guard.host(), track_id).is_null()
}

/// The layout fingerprint the command was written against and the device
/// generation the audio callback stamps it with. A host with no block state has
/// no generations to compare, which is the unreadable case below.
fn generations(host: *mut c_void, project: &mut u64, audio: &mut u64) -> bool {
    let block = handle_of_host(host, H_ENGINE_BLOCK);
    if block.is_null() {
        return false;
    }
    *project = unsafe { crate::layout_gather::hirari_project_generation(host) };
    *audio = unsafe { crate::engine_block_host::hirari_engine_block_generation(block) };
    true
}

fn journal(host: *mut c_void, entry: (u32, u32, f32, u64)) {
    let journaler = handle_of_host(host, H_FORENSIC_JOURNAL);
    if journaler.is_null() {
        return;
    }
    // SAFETY: the bag hands back the journaler it created and keeps it alive
    // for the engine.
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
    host: *mut c_void,
    command_type: u32,
    track_id: u32,
    value: f32,
    timestamp: u64,
    expected_project_generation: u64,
    expected_audio_generation: u64,
) -> bool {
    let rings = handle_of_host(host, H_ENGINE_RINGS);
    let entry = (command_type, track_id, value, timestamp);
    // A stale id must never fall through to the queue: the consumer would
    // resolve it against a different project after a reload.
    if !track_exists(host, track_id) {
        publish_error(host, COMMAND_ERROR_INVALID_TARGET);
        journal(host, entry);
        return false;
    }
    let mut project_generation = 0u64;
    let mut audio_generation = 0u64;
    // An unreadable generation is not "generation zero": admitting it could
    // apply an old command to a rebuilt project.
    if !generations(host, &mut project_generation, &mut audio_generation) {
        publish_error(host, COMMAND_ERROR_STALE_PROJECT_GENERATION);
        journal(host, entry);
        return false;
    }
    if project_generation != expected_project_generation {
        publish_error(host, COMMAND_ERROR_STALE_PROJECT_GENERATION);
        journal(host, entry);
        return false;
    }
    if audio_generation != expected_audio_generation {
        publish_error(host, COMMAND_ERROR_STALE_AUDIO_GENERATION);
        journal(host, entry);
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
    if rings.is_null()
        || !unsafe { crate::engine_rings::hirari_engine_command_ring_push(rings.cast(), &record) }
    {
        publish_error(host, COMMAND_ERROR_QUEUE_FULL);
        journal(host, entry);
        report_queue_full(track_id);
        return false;
    }
    publish_error(host, COMMAND_ERROR_NONE);
    journal(host, entry);
    true
}

/// # Safety
/// `host` must be an engine host pointer whose session bag is still live.
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
    push_impl(
        host,
        command_type,
        track_id,
        value,
        timestamp,
        expected_project_generation,
        expected_audio_generation,
    )
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
    use crate::engine_session::{
        hirari_engine_session_create, hirari_engine_session_destroy, hirari_engine_session_handle,
        H_ENGINE_BLOCK, H_ENGINE_RINGS, H_FORENSIC_JOURNAL, H_LAYOUT_REVISION, H_TRACK_TABLE,
    };
    use crate::engine_track::{hirari_track_new, TRACK_TYPE_AUDIO};
    use crate::engine_track_table::hirari_engine_track_insert;

    // The last-command error table and the diagnostic log ring are process
    // state, so these tests must not overlap.
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static JOURNAL_SEQUENCE: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn error_for(host: *mut c_void) -> u8 {
        last_error(host)
    }

    fn push(
        host: *mut c_void,
        command_type: u32,
        track_id: u32,
        value: f32,
        timestamp: u64,
        project: u64,
        audio: u64,
    ) -> bool {
        unsafe { hirari_engine_push_command(host, command_type, track_id, value, timestamp, project, audio) }
    }

    const JOURNAL_BYTES: usize = 24;

    /// A real session bag with one real track in its own table. The command
    /// ring, the engine block and the journaler are the slots the bag hands
    /// out, so the admission policy is tested against the stores it reads.
    struct TestHost {
        host: *mut c_void,
        session: *mut c_void,
        rings: *mut c_void,
        journaler: *mut c_void,
        block: *mut c_void,
        journal_path: String,
    }

    impl TestHost {
        fn new(track_id: u32) -> Self {
            let journal_path = std::env::temp_dir()
                .join(format!(
                    "hirari-command-journal-{}-{}",
                    std::process::id(),
                    JOURNAL_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ))
                .to_string_lossy()
                .into_owned();
            // The bag records this address as its engine, which is how the
            // admission below resolves the track, the generations and the
            // stores without a host table.
            let host: *mut c_void =
                std::ptr::from_mut(Box::leak(Box::new(0u64))).cast::<std::ffi::c_void>();
            let session = hirari_engine_session_create(host, std::ptr::null_mut());
            let revision = hirari_engine_session_handle(session, H_LAYOUT_REVISION);
            let mut track = std::ptr::null_mut();
            let owner = unsafe {
                hirari_track_new(
                    track_id,
                    b"Track\0".as_ptr(),
                    6,
                    TRACK_TYPE_AUDIO,
                    revision,
                    &mut track,
                )
            };
            assert!(unsafe {
                hirari_engine_track_insert(
                    hirari_engine_session_handle(session, H_TRACK_TABLE),
                    usize::MAX,
                    owner,
                )
            });
            let journaler = hirari_engine_session_handle(session, H_FORENSIC_JOURNAL);
            // SAFETY: every handle comes from the bag just built and the path
            // bytes outlive the call.
            unsafe {
                crate::forensic_journaler::hirari_forensic_journaler_start(
                    journaler,
                    journal_path.as_ptr(),
                    journal_path.len(),
                );
            }
            Self {
                host,
                session,
                rings: hirari_engine_session_handle(session, H_ENGINE_RINGS),
                journaler,
                block: hirari_engine_session_handle(session, H_ENGINE_BLOCK),
                journal_path,
            }
        }

        fn host(&self) -> *mut c_void {
            self.host
        }

        /// The layout fingerprint the engine would stamp a command with.
        fn project_generation(&self) -> u64 {
            unsafe { crate::layout_gather::hirari_project_generation(self.host) }
        }

        fn audio_generation(&self) -> u64 {
            unsafe { crate::engine_block_host::hirari_engine_block_generation(self.block) }
        }

        /// The audio callback moving to a new device configuration.
        fn bump_audio_generation(&self) -> u64 {
            unsafe { crate::engine_block_host::hirari_engine_block_bump_generation(self.block) }
        }

        fn push_to_ring(&self, record: &CommandRecord) -> bool {
            // SAFETY: `rings` is this engine's own ring handle.
            unsafe { crate::engine_rings::hirari_engine_command_ring_push(self.rings.cast(), record) }
        }

        fn drain_ring(&self) -> Vec<CommandRecord> {
            let mut drained = Vec::new();
            loop {
                let mut record = CommandRecord::default();
                // SAFETY: `rings` is this engine's own ring handle.
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
            // SAFETY: `journaler` is this engine's own journaler handle.
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
            // Releasing the bag stops and destroys the journaler, frees the
            // rings and retires the host binding, so the address cannot be read
            // by a later session.
            { hirari_engine_session_destroy(self.session) };
            let _ = std::fs::remove_file(&self.journal_path);
        }
    }

    #[test]
    fn push_rejects_an_unknown_target_before_the_generations_are_read() {
        let _serial = serial();
        let host = TestHost::new(5);
        // Both generations are wrong on purpose: a stale id is rejected before
        // either is compared, because the consumer would resolve it against a
        // reloaded project.
        assert!(!push(
            host.host(),
            0,
            99,
            1.0,
            10,
            host.project_generation() + 1,
            host.audio_generation() + 1
        ));
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 99, 1.0, 10)]);
        assert_eq!(error_for(host.host()), COMMAND_ERROR_INVALID_TARGET);
    }

    #[test]
    fn push_rejects_a_stale_project_generation_before_the_audio_generation() {
        let _serial = serial();
        let host = TestHost::new(5);
        let audio = host.audio_generation();
        assert!(!push(
            host.host(),
            0,
            5,
            1.0,
            10,
            host.project_generation() + 1,
            audio
        ));
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 5, 1.0, 10)]);
        assert_eq!(
            error_for(host.host()),
            COMMAND_ERROR_STALE_PROJECT_GENERATION
        );
    }

    #[test]
    fn push_rejects_a_command_the_audio_callback_has_outgrown() {
        let _serial = serial();
        let host = TestHost::new(5);
        let audio = host.audio_generation();
        assert_eq!(host.bump_audio_generation(), audio);
        assert!(!push(
            host.host(),
            0,
            5,
            1.0,
            10,
            host.project_generation(),
            audio
        ));
        assert!(host.drain_ring().is_empty());
        assert_eq!(host.journal_records(), vec![(0, 5, 1.0, 10)]);
        assert_eq!(error_for(host.host()), COMMAND_ERROR_STALE_AUDIO_GENERATION);
    }

    #[test]
    fn push_reports_a_full_queue_after_the_journal_entry() {
        let _serial = serial();
        let host = TestHost::new(5);
        host.clear_log();
        host.fill_ring();
        let queued = host.drain_ring().len();
        host.fill_ring();
        assert!(!push(
            host.host(),
            2,
            5,
            0.5,
            10,
            host.project_generation(),
            host.audio_generation()
        ));
        assert_eq!(host.drain_ring().len(), queued);
        assert_eq!(host.journal_records(), vec![(2, 5, 0.5, 10)]);
        assert!(host.queue_full_reported(5));
        assert_eq!(error_for(host.host()), COMMAND_ERROR_QUEUE_FULL);
    }

    #[test]
    fn push_stamps_the_accepted_command_and_journals_it() {
        let _serial = serial();
        let host = TestHost::new(5);
        let audio = host.audio_generation();
        assert!(push(
            host.host(),
            3,
            5,
            1.0,
            10,
            host.project_generation(),
            audio
        ));
        assert_eq!(
            host.drain_ring(),
            vec![CommandRecord {
                type_: 3,
                tid: 5,
                val: 1.0,
                ts: 10,
                expected_audio_generation: audio,
            }]
        );
        assert_eq!(host.journal_records(), vec![(3, 5, 1.0, 10)]);
        assert_eq!(error_for(host.host()), COMMAND_ERROR_NONE);
    }

    #[test]
    fn a_rejection_belongs_to_the_engine_that_saw_it() {
        let _serial = serial();
        let rejected = TestHost::new(5);
        let untouched = TestHost::new(5);
        assert!(!push(
            rejected.host(),
            0,
            99,
            1.0,
            10,
            rejected.project_generation(),
            rejected.audio_generation()
        ));
        assert_eq!(error_for(rejected.host()), COMMAND_ERROR_INVALID_TARGET);
        assert_eq!(
            error_for(untouched.host()),
            COMMAND_ERROR_NONE,
            "the other engine recorded nothing"
        );
        assert!(untouched.journal_records().is_empty());
    }

    #[test]
    fn a_host_with_no_session_bag_admits_nothing() {
        let _serial = serial();
        let host: *mut c_void =
            std::ptr::from_mut(Box::leak(Box::new(0u64))).cast::<std::ffi::c_void>();
        assert!(!push(host, 0, 5, 1.0, 10, 0, 0));
        assert_eq!(error_for(host), COMMAND_ERROR_INVALID_TARGET);
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
