//! Track control-plane policy: spatial-position validation and HRTF kernel
//! admission (non-empty, matching stereo taps, bounded length).
//!
//! The track is a Rust object and its panner is one of the storages it owns, so
//! a request needs no native callback: the engine's track session pins the id
//! under the tracks lock, [`crate::engine_track`] performs the write with the
//! edit protocol around it, and this module contributes only the two checks that
//! have to run before a store the realtime thread reads is touched.
//!
//! Every entry point runs on the control thread.

use std::ffi::c_void;

use crate::engine_track::{
    hirari_track_clear_hrtf_kernel as write_clear_hrtf,
    hirari_track_set_hrtf_kernel as write_hrtf_kernel, hirari_track_set_spatial,
};
use crate::track_session::TrackSession;

/// Mirrors the panner's kernel cap, so an oversized request is refused before
/// the track is looked up.
pub const MAX_HRTF_TAPS: usize = 128;

/// A non-finite coordinate would poison the panner's history, so it is rejected
/// here rather than clamped on the way in.
pub unsafe extern "C" fn hirari_track_set_spatial_position(
    host: *mut c_void,
    track_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> bool {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return false;
    }
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    unsafe { hirari_track_set_spatial(session.track(), x, y, z) };
    true
}

/// Installs one stereo HRTF kernel. The two taps have to describe the same
/// length and neither may be missing: a half kernel would steer the panner with
/// one ear's data.
pub unsafe extern "C" fn hirari_track_set_hrtf_kernel(
    host: *mut c_void,
    track_id: u32,
    left: *const f32,
    left_len: usize,
    right: *const f32,
    right_len: usize,
) -> bool {
    if left_len == 0
        || left_len != right_len
        || left_len > MAX_HRTF_TAPS
        || left.is_null()
        || right.is_null()
    {
        return false;
    }
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    unsafe { write_hrtf_kernel(session.track(), left, right, left_len as u32) }
}

pub unsafe extern "C" fn hirari_track_clear_hrtf_kernel(host: *mut c_void, track_id: u32) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    unsafe { write_clear_hrtf(session.track()) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_track::{
        hirari_layout_revision_create, hirari_layout_revision_current, hirari_layout_revision_release,
        hirari_track_new, TRACK_TYPE_AUDIO,
    };
    use crate::engine_track_table::{
        hirari_engine_track_find, hirari_engine_track_insert, hirari_engine_track_table_create,
        hirari_engine_track_table_destroy,
    };
    use crate::track_session::{hirari_track_session_install, HirariTrackSessionOps};

    unsafe extern "C" fn no_lock(_host: *mut c_void) {}

    unsafe extern "C" fn find(host: *mut c_void, track_id: u32) -> *mut c_void {
        unsafe { hirari_engine_track_find(host, track_id) }
    }

    /// The session table is process-global and the engine installs its own copy
    /// at startup, so the pin these tests install is handed back to it after.
    struct SessionGuard(Option<HirariTrackSessionOps>);

    impl SessionGuard {
        fn install() -> Self {
            let previous = crate::track_session::engine_ops();
            let ops = HirariTrackSessionOps {
                lock: Some(no_lock),
                unlock: Some(no_lock),
                track: Some(find),
                ..HirariTrackSessionOps::default()
            };
            unsafe { hirari_track_session_install(&ops) };
            Self(previous)
        }
    }

    impl Drop for SessionGuard {
        fn drop(&mut self) {
            match self.0 {
                Some(ops) => unsafe { hirari_track_session_install(&ops) },
                None => unsafe { hirari_track_session_install(std::ptr::null()) },
            }
        }
    }

    /// One real track in the engine's own table, with the layout revision the
    /// track marks for every edit it applies. The host the session resolves
    /// through is the table itself.
    struct Engine {
        table: *mut c_void,
        revision: *mut c_void,
        track: *mut c_void,
    }

    impl Engine {
        fn new(id: u32) -> Self {
            let table = unsafe { hirari_engine_track_table_create() };
            let revision = unsafe { hirari_layout_revision_create() };
            let mut track = std::ptr::null_mut();
            let owner = unsafe {
                hirari_track_new(
                    id,
                    b"Track\0".as_ptr(),
                    6,
                    TRACK_TYPE_AUDIO,
                    revision,
                    &mut track,
                )
            };
            assert!(unsafe { hirari_engine_track_insert(table, usize::MAX, owner) });
            Self { table, revision, track }
        }

        /// How many times the track marked the layout: one edit marks it twice,
        /// the frozen render dropped before the write and the publication after.
        fn marks(&self) -> u64 {
            unsafe { hirari_layout_revision_current(self.revision) }
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe {
                // The table holds the only owning handle, so destroying it runs
                // the track's own storage teardown.
                hirari_engine_track_table_destroy(self.table);
                hirari_layout_revision_release(self.revision);
            }
        }
    }

    #[test]
    fn an_unresolved_track_is_refused() {
        let _guard = SessionGuard::install();
        let engine = Engine::new(5);
        let before = engine.marks();
        assert!(
            !unsafe { hirari_track_set_spatial_position(engine.table, 99, 1.0, 2.0, 3.0) },
            "a retired id cannot be written"
        );
        assert!(!unsafe { hirari_track_clear_hrtf_kernel(engine.table, 99) });
        assert_eq!(engine.marks(), before, "and nothing was marked");
    }

    #[test]
    fn spatial_position_rejects_non_finite_coordinates_before_the_lookup() {
        let _guard = SessionGuard::install();
        let engine = Engine::new(5);
        let before = engine.marks();
        assert!(!unsafe { hirari_track_set_spatial_position(engine.table, 5, f32::NAN, 0.0, 0.0) });
        assert!(
            !unsafe { hirari_track_set_spatial_position(engine.table, 5, 0.0, f32::INFINITY, 0.0) },
            "an infinite coordinate never reaches the panner"
        );
        assert!(!unsafe {
            hirari_track_set_spatial_position(engine.table, 5, 0.0, 0.0, f32::NEG_INFINITY)
        });
        assert_eq!(engine.marks(), before, "a rejected edit marks no layout");

        assert!(unsafe { hirari_track_set_spatial_position(engine.table, 5, 1.0, 2.0, 3.0) });
        assert_eq!(engine.marks(), before + 2, "and an accepted one is an edit");
    }

    #[test]
    fn hrtf_admits_only_a_complete_matching_kernel() {
        let _guard = SessionGuard::install();
        let engine = Engine::new(5);
        let before = engine.marks();
        let taps = vec![0.5f32; MAX_HRTF_TAPS + 1];
        assert!(
            !unsafe {
                hirari_track_set_hrtf_kernel(engine.table, 5, std::ptr::null(), 0, std::ptr::null(), 0)
            },
            "an empty kernel is not a kernel"
        );
        assert!(
            !unsafe { hirari_track_set_hrtf_kernel(engine.table, 5, taps.as_ptr(), 2, taps.as_ptr(), 3) },
            "the two taps have to describe the same length"
        );
        assert!(
            !unsafe {
                hirari_track_set_hrtf_kernel(
                    engine.table,
                    5,
                    taps.as_ptr(),
                    MAX_HRTF_TAPS + 1,
                    taps.as_ptr(),
                    MAX_HRTF_TAPS + 1,
                )
            },
            "and neither may exceed the panner cap"
        );
        assert_eq!(engine.marks(), before, "none of them reached the track");

        let left = [0.5f32, 0.25];
        let right = [0.75f32, 0.125];
        assert!(unsafe {
            hirari_track_set_hrtf_kernel(engine.table, 5, left.as_ptr(), 2, right.as_ptr(), 2)
        });
        assert_eq!(engine.marks(), before + 2, "a valid kernel is installed");

        assert!(unsafe { hirari_track_clear_hrtf_kernel(engine.table, 5) });
        assert_eq!(engine.marks(), before + 4, "and clearing it is an edit too");
    }

    #[test]
    fn a_kernel_the_panner_rejects_still_retires_the_frozen_render() {
        let _guard = SessionGuard::install();
        let engine = Engine::new(5);
        let before = engine.marks();
        let left = [f32::NAN, 0.25];
        let right = [0.75f32, 0.125];
        assert!(
            !unsafe {
                hirari_track_set_hrtf_kernel(engine.table, 5, left.as_ptr(), 2, right.as_ptr(), 2)
            },
            "a non-finite sample is refused by the store"
        );
        // The argument gate above refuses before the edit protocol starts; a
        // kernel that reaches the panner has already retired the frozen render,
        // which is one mark even when the panner then declines it.
        assert_eq!(engine.marks(), before + 1);
    }
}
