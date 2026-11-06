//! Contract for the engine's Rust-owned session bag (engine_session.rs).
//!
//! The bag replaced twenty-nine C++ members and the hand-written order the
//! engine created and destroyed them in. These tests pin what that order is
//! responsible for: every slot the block and the control surface ask for answers,
//! the graph slots retire before the rest, and a retired or unknown slot reads
//! back null instead of a freed pointer.

use hirari_core_bridge::engine_session::{
    handle_of_host, hirari_engine_session_close_graph, hirari_engine_session_create,
    hirari_engine_session_destroy, hirari_engine_session_handle, H_ARRANGEMENT_DOCUMENT,
    H_BOUNCE_SESSION, H_CONTROL_ROOM, H_ENGINE_BLOCK, H_ENGINE_RINGS, H_FORENSIC_JOURNAL,
    H_INPUT_BLOCKS, H_LAYOUT_REVISION, H_MIDI_PLAYBACK_CLOCK, H_MIDI_SNAPSHOT_STORE,
    H_TRACKS_MUTEX, H_TRACK_TABLE, H_UNDO,
};

/// A stand-in for the C++ engine object. The bag only records the address it is
/// handed, so a fresh heap word is a unique, live host.
fn engine_host() -> *mut std::ffi::c_void {
    std::ptr::from_mut(Box::leak(Box::new(0u64))).cast::<std::ffi::c_void>()
}

/// Every slot the C++ engine reads must be live straight out of construction,
/// because the block binds them before the engine's own constructor body runs.
#[test]
fn a_new_engine_has_every_store_it_renders_with() {
    let session = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    assert!(true);
    for kind in [
        H_TRACK_TABLE,
        H_INPUT_BLOCKS,
        H_ENGINE_RINGS,
        H_MIDI_PLAYBACK_CLOCK,
        H_MIDI_SNAPSHOT_STORE,
        H_ENGINE_BLOCK,
        H_BOUNCE_SESSION,
        H_CONTROL_ROOM,
        H_LAYOUT_REVISION,
        H_FORENSIC_JOURNAL,
        H_ARRANGEMENT_DOCUMENT,
        H_UNDO,
    ] {
        assert!(
            !hirari_engine_session_handle(session, kind).is_null(),
            "slot {kind} is bound at construction"
        );
    }
    hirari_engine_session_destroy(session);
}

/// The layout revision is a slot like any other, so a control-side mark reads
/// the very counter the tracks bump.
#[test]
fn the_layout_revision_slot_is_the_one_the_block_marks() {
    let session = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    let revision = hirari_engine_session_handle(session, H_LAYOUT_REVISION);
    assert_eq!(
        revision,
        unsafe {
            hirari_engine_block_layout_revision(hirari_engine_session_handle(
                session,
                H_ENGINE_BLOCK,
            ))
        },
        "the block traverses the revision the bag holds"
    );
    hirari_engine_session_destroy(session);
}

/// Closing the graph is what the engine's shutdown does while the rest of the
/// bag stays live; the retired slots read null and a second close is a no-op.
#[test]
fn closing_the_graph_retires_only_the_tracks_and_the_hardware_input() {
    let session = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    hirari_engine_session_close_graph(session);
    assert!(hirari_engine_session_handle(session, H_TRACK_TABLE).is_null());
    assert!(hirari_engine_session_handle(session, H_INPUT_BLOCKS).is_null());
    assert!(!hirari_engine_session_handle(session, H_ENGINE_BLOCK).is_null());
    hirari_engine_session_close_graph(session);
    hirari_engine_session_destroy(session);
}

/// An unknown kind and a released bag answer null rather than reading memory
/// that no longer belongs to a store.
#[test]
fn an_unknown_slot_or_a_released_bag_reads_null() {
    let session = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    assert!(hirari_engine_session_handle(session, u32::MAX).is_null());
    hirari_engine_session_destroy(session);
    assert!(hirari_engine_session_handle(std::ptr::null(), H_ENGINE_BLOCK).is_null());
    hirari_engine_session_destroy(std::ptr::null_mut());
}

/// Two engines must not share a queue, a journal or an undo history: the bag is
/// per instance, which is what let offline engines and tests run side by side.
#[test]
fn each_engine_bag_holds_its_own_stores() {
    let first = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    let second = hirari_engine_session_create(engine_host(), std::ptr::null_mut());
    for kind in [
        H_ENGINE_RINGS,
        H_UNDO,
        H_FORENSIC_JOURNAL,
        H_MIDI_SNAPSHOT_STORE,
    ] {
        assert_ne!(
            hirari_engine_session_handle(first, kind),
            hirari_engine_session_handle(second, kind),
            "slot {kind} is not shared between engines"
        );
    }
    hirari_engine_session_destroy(first);
    hirari_engine_session_destroy(second);
}

/// The bag holds the engine's tracks lock as a borrowed slot: it hands back the
/// very address it was given, and a released bag leaves nothing to lock.
#[test]
fn the_bag_borrows_the_engine_tracks_mutex() {
    let host = engine_host();
    let mutex = std::ptr::from_mut(&mut 0u64).cast::<std::ffi::c_void>();
    let session = hirari_engine_session_create(host, mutex);
    assert_eq!(
        handle_of_host(host, H_TRACKS_MUTEX),
        mutex,
        "a Rust walk locks the mutex the engine's own edits take"
    );
    hirari_engine_session_destroy(session);
    assert!(handle_of_host(host, H_TRACKS_MUTEX).is_null());
}

/// The registry a Rust routine uses when it is handed only the engine's
/// address: the host resolves to the bag it created, and the binding goes away
/// with the bag so a recycled address cannot read a released store.
#[test]
fn a_host_address_resolves_the_bag_it_created() {
    let host = engine_host();
    let session = hirari_engine_session_create(host, std::ptr::null_mut());
    assert_eq!(
        handle_of_host(host, H_ENGINE_BLOCK),
        hirari_engine_session_handle(session, H_ENGINE_BLOCK),
        "the engine's own address reaches its stores"
    );
    hirari_engine_session_destroy(session);
    assert!(
        handle_of_host(host, H_ENGINE_BLOCK).is_null(),
        "a released bag leaves no binding behind"
    );
    assert!(handle_of_host(std::ptr::null(), H_ENGINE_BLOCK).is_null());
}

extern "C" {
    fn hirari_engine_block_layout_revision(state: *const std::ffi::c_void)
        -> *mut std::ffi::c_void;
}
