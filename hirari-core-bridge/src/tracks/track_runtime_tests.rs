//! Tests split out of track_runtime.rs.

    use super::*;

    #[test]
    fn a_null_runtime_refuses_every_entry() {
        unsafe {
            assert!(!hirari_track_process_block(ptr::null_mut(), ptr::null_mut(), 8, 0, 1));
            assert_eq!(hirari_track_runtime_transport_bpm(ptr::null_mut()), 120.0);
            assert_eq!(hirari_track_runtime_eq(ptr::null_mut(), 0), 1.0);
            assert_eq!(hirari_track_runtime_pdc_requested(ptr::null_mut()), 0);
            assert!(!hirari_track_runtime_is_muted(ptr::null_mut()));
            hirari_track_runtime_destroy(ptr::null_mut());
            hirari_track_runtime_reset_for_offline_render(ptr::null_mut());
        }
    }
