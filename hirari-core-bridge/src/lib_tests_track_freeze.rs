// Track freeze. The engine only pins a track and reports its block size, so
// these run against a live engine track to cover what the Rust freeze host now
// owns: the offline render and its publication, the disk artifact and its
// restore, project-end sizing, the cache path, admission limits and the
// retention of replaced renders.
static FREEZE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TrackFreezeEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: cxx::UniquePtr<crate::ffi::AudioEngine>,
    track: u32,
    dir: std::path::PathBuf,
}

impl TrackFreezeEngine {
    fn new() -> Self {
        let guard = native_engine_test_guard();
        let engine = crate::ffi::new_audio_engine();
        assert!(!engine.is_null());
        let reference = engine.as_ref().unwrap();
        let track = reference.add_track(0);
        assert_ne!(track, 0);
        reference.clear_undo_history();
        let unique = FREEZE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hirari-track-freeze-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            _guard: guard,
            engine,
            track,
            dir,
        }
    }

    fn engine(&self) -> &crate::ffi::AudioEngine {
        self.engine.as_ref().unwrap()
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.engine())
    }

    fn block_size(&self) -> u32 {
        self.engine().get_block_size()
    }

    fn session(&self) -> crate::track_session::TrackSession {
        crate::track_session::TrackSession::open(self.host(), self.track)
            .expect("the engine's track is addressable")
    }

    /// The freeze store the engine's track publishes into.
    fn frozen(&self) -> *mut std::ffi::c_void {
        self.session().frozen()
    }

    fn is_frozen(&self) -> bool {
        unsafe { crate::track_freeze_host::hirari_track_is_frozen(self.host(), self.track) }
    }

    fn total_samples(&self) -> u64 {
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_total_samples(self.frozen()) }
    }

    fn sample_rate(&self) -> u32 {
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_sample_rate(self.frozen()) }
    }

    fn retained(&self) -> usize {
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_retained_count(self.frozen()) }
    }

    fn cache_path(&self) -> Vec<u8> {
        let length =
            unsafe { crate::track_frozen_store::hirari_track_frozen_store_path_len(self.frozen()) };
        let mut path = vec![0u8; length];
        if length != 0 {
            let written = unsafe {
                crate::track_frozen_store::hirari_track_frozen_store_path_copy(
                    self.frozen(),
                    path.as_mut_ptr(),
                    length,
                )
            };
            assert_eq!(written, length);
        }
        path
    }

    /// A path inside this fixture's own directory, so artifacts from parallel
    /// targets cannot collide.
    fn artifact(&self, name: &str) -> std::path::PathBuf {
        let unique = FREEZE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.dir.join(format!("{name}-{unique}.wav"))
    }

    /// The rendered planes the audio thread is meant to copy.
    fn current_sample(&self, channel: u32, index: u32) -> f32 {
        let current =
            unsafe { crate::track_frozen_store::hirari_track_frozen_store_current(self.frozen()) };
        assert!(!current.is_null());
        unsafe {
            *crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(
                current.cast_mut(),
                channel,
                0,
            )
                .add(index as usize)
        }
    }
}

impl Drop for TrackFreezeEngine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn freeze(host: *mut std::ffi::c_void, track: u32, total: u64, rate: u32) -> bool {
    unsafe { crate::track_freeze_host::hirari_track_freeze(host, track, total, rate) }
}

fn storage(channels: u32, samples: u32) -> *mut std::ffi::c_void {
    let handle = crate::audio_buffer_ops::hirari_audio_buffer_storage_create();
    assert_eq!(
        unsafe {
            crate::audio_buffer_ops::hirari_audio_buffer_storage_reserve_buffer(
                handle, channels, samples,
            )
        },
        0
    );
    assert!(unsafe {
        crate::audio_buffer_ops::hirari_audio_buffer_storage_resize(handle, channels, samples)
    });
    handle
}

#[test]
fn a_freeze_replaces_the_live_graph_with_a_render_of_the_requested_length() {
    let fixture = TrackFreezeEngine::new();
    assert!(!fixture.is_frozen());
    assert!(freeze(fixture.host(), fixture.track, 4_096, 48_000));
    assert!(fixture.is_frozen());
    assert_eq!(fixture.total_samples(), 4_096);
    assert_eq!(fixture.sample_rate(), 48_000);
    // The store keeps exactly the render it published, so one freeze leaks
    // nothing back to the caller.
    assert_eq!(fixture.retained(), 1);
    assert!(fixture.cache_path().is_empty());

    assert!(unsafe { crate::track_freeze_host::hirari_track_unfreeze(fixture.host(), fixture.track) });
    assert!(!fixture.is_frozen());
    assert_eq!(fixture.total_samples(), 0);
    // Clearing leaves the render retained until the control side collects it
    // after the callback has been quiesced.
    assert_eq!(fixture.retained(), 1);
    unsafe {
        crate::track_frozen_store::hirari_track_frozen_store_retire(fixture.frozen());
    }
    assert_eq!(fixture.retained(), 0);
}

#[test]
fn admission_rejects_empty_oversized_and_unknown_requests() {
    let fixture = TrackFreezeEngine::new();
    let host = fixture.host();
    assert!(!freeze(host, fixture.track, 0, 48_000));
    assert!(!freeze(host, fixture.track, 4_096, 0));
    assert!(!freeze(
        host,
        fixture.track,
        crate::track_freeze_host::MAX_FREEZE_SAMPLES + 1,
        48_000,
    ));
    assert!(!freeze(host, 9_999, 4_096, 48_000));
    assert!(!fixture.is_frozen());
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_freeze_to_file(
            host,
            fixture.track,
            std::ptr::null(),
            0,
            4_096,
            48_000,
        )
    });
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_restore_freeze(
            host,
            fixture.track,
            b"/tmp/missing.wav".as_ptr(),
            16,
            4_096,
            48_000,
        )
    });
    assert_eq!(fixture.retained(), 0);
}

#[test]
fn project_end_freeze_uses_the_larger_of_last_region_and_two_blocks() {
    let fixture = TrackFreezeEngine::new();
    // A freshly installed region table is empty, so the two-block round trip
    // decides the length.
    let mut project_end = u64::MAX;
    assert!(unsafe {
        crate::region_store::hirari_region_store_project_end(
            fixture.session().track(),
            &mut project_end,
        )
    });
    assert_eq!(project_end, 0);
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_freeze_to_project_end(
            fixture.host(),
            fixture.track,
            44_100,
        )
    });
    assert_eq!(
        fixture.total_samples(),
        u64::from(fixture.block_size().max(1)) * 2
    );
    assert_eq!(fixture.sample_rate(), 44_100);

    // A region that reaches past the fallback wins, because freezing to project
    // end is meant to cover the whole take.
    let source = fixture.artifact("project-end");
    write_test_tone_wav(&source, 48_000, 4_800);
    assert!(fixture
        .engine()
        .add_region(fixture.track, source.to_str().unwrap(), 0.0));
    let mut region_end = 0_u64;
    assert!(unsafe {
        crate::region_store::hirari_region_store_project_end(
            fixture.session().track(),
            &mut region_end,
        )
    });
    assert!(region_end > u64::from(fixture.block_size()) * 2);
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_unfreeze(fixture.host(), fixture.track)
    });
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_freeze_to_project_end(
            fixture.host(),
            fixture.track,
            44_100,
        )
    });
    assert_eq!(fixture.total_samples(), region_end);
}

#[test]
fn freezing_a_file_region_renders_its_audio_into_the_snapshot() {
    // The offline loop walks the track's real graph, so the published snapshot
    // carries the take rather than the silence an unprepared render would give.
    let fixture = TrackFreezeEngine::new();
    let source = fixture.artifact("take");
    write_test_tone_wav(&source, 48_000, 4_800);
    assert!(fixture
        .engine()
        .add_region(fixture.track, source.to_str().unwrap(), 0.0));
    assert!(freeze(fixture.host(), fixture.track, 4_800, 48_000));
    let peak = (0..4_800_u32).fold(0.0f32, |peak, index| {
        peak.max(fixture.current_sample(0, index).abs())
            .max(fixture.current_sample(1, index).abs())
    });
    assert!(peak > 0.01, "the freeze render must be audible (peak {peak})");
}

#[test]
fn a_disk_freeze_writes_an_artifact_and_restore_reads_that_artifact_back() {
    let fixture = TrackFreezeEngine::new();
    let host = fixture.host();
    let artifact = fixture.artifact("freeze");
    let path = artifact.to_string_lossy().into_owned();
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_freeze_to_file(
            host,
            fixture.track,
            path.as_ptr().cast::<u8>(),
            path.len(),
            2_048,
            48_000,
        )
    });
    let written = std::fs::read(&artifact).expect("the freeze writes its artifact");
    assert_eq!(&written[0..4], b"RIFF");
    assert_eq!(&written[8..12], b"WAVE");
    // Two float32 channels of the rendered length, plus the WAV header.
    assert!(written.len() >= 44 + 2_048 * 2 * std::mem::size_of::<f32>());
    assert_eq!(fixture.cache_path(), path.as_bytes());
    assert!(fixture.is_frozen());

    // Restoring re-publishes the bytes from disk rather than re-rendering, so
    // the recorded path survives and the same length is visible to the callback.
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_restore_freeze(
            host,
            fixture.track,
            path.as_ptr().cast::<u8>(),
            path.len(),
            2_048,
            48_000,
        )
    });
    assert!(fixture.is_frozen());
    assert_eq!(fixture.total_samples(), 2_048);
    assert_eq!(fixture.cache_path(), path.as_bytes());

    // A reader-only artifact at the wrong rate is refused and leaves the
    // previous freeze untouched.
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_restore_freeze(
            host,
            fixture.track,
            path.as_ptr().cast::<u8>(),
            path.len(),
            2_048,
            44_100,
        )
    });
    assert_eq!(fixture.sample_rate(), 48_000);
}

#[test]
fn a_failed_disk_freeze_leaves_no_orphan_artifact() {
    let fixture = TrackFreezeEngine::new();
    // A path in a directory that does not exist cannot be written, so the
    // render is abandoned before publication.
    let missing = fixture.dir.join("absent").join("take.wav");
    let path = missing.to_string_lossy().into_owned();
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_freeze_to_file(
            fixture.host(),
            fixture.track,
            path.as_ptr().cast::<u8>(),
            path.len(),
            2_048,
            48_000,
        )
    });
    assert!(!fixture.is_frozen());
    assert!(!missing.exists());
    assert_eq!(fixture.retained(), 0);
}

#[test]
fn the_cache_path_records_only_for_a_live_freeze() {
    let fixture = TrackFreezeEngine::new();
    let host = fixture.host();
    let path = b"cache/take.wav";
    // Nothing is published yet, so there is nothing to annotate.
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_set_freeze_cache_path(
            host,
            fixture.track,
            path.as_ptr(),
            path.len(),
        )
    });
    assert!(freeze(host, fixture.track, 512, 48_000));
    assert!(unsafe {
        crate::track_freeze_host::hirari_track_set_freeze_cache_path(
            host,
            fixture.track,
            path.as_ptr(),
            path.len(),
        )
    });
    assert_eq!(fixture.cache_path(), path.to_vec());
    assert!(!unsafe {
        crate::track_freeze_host::hirari_track_set_freeze_cache_path(host, fixture.track, path.as_ptr(), 0)
    });
    assert_eq!(fixture.cache_path(), path.to_vec());
}

#[test]
fn repeated_freezes_retire_down_to_the_current_render() {
    let fixture = TrackFreezeEngine::new();
    let host = fixture.host();
    for total in [512_u64, 1_024, 2_048] {
        assert!(freeze(host, fixture.track, total, 48_000));
        assert_eq!(fixture.total_samples(), total);
    }
    // Each freeze replaced the snapshot, so two superseded renders are waiting
    // alongside the current one until the control side collects them.
    assert_eq!(fixture.retained(), 3);
    unsafe {
        crate::track_frozen_store::hirari_track_frozen_store_retire(fixture.frozen());
    }
    assert_eq!(fixture.retained(), 1);
    assert!(fixture.is_frozen());
}

#[test]
fn the_audio_block_copies_the_published_snapshot() {
    // The contract the realtime path depends on: a snapshot published off the
    // audio thread is what `Track::process` copies, bypassing the live
    // region/effect graph entirely.
    let fixture = TrackFreezeEngine::new();
    // A session holds the engine's tracks lock, and every freeze entry below
    // takes its own, so only the handles are carried out of this scope.
    let (frozen, runtime) = {
        let session = fixture.session();
        (session.frozen(), session.runtime())
    };
    let rendered = storage(2, 8);
    for index in 0..8_u32 {
        unsafe {
            *crate::audio_buffer_ops::hirari_audio_buffer_storage_write_pointer(rendered, 0)
                .add(index as usize) = index as f32 + 1.0;
            *crate::audio_buffer_ops::hirari_audio_buffer_storage_write_pointer(rendered, 1)
                .add(index as usize) = 100.0 + index as f32;
        }
    }
    assert!(unsafe {
        crate::track_frozen_store::hirari_track_frozen_store_publish(
            frozen,
            rendered,
            2,
            8,
            8,
            48_000,
            11,
            13,
            std::ptr::null(),
            0,
        )
    });
    assert!(fixture.is_frozen());

    let block = storage(2, 8);
    unsafe {
        crate::audio_buffer_ops::hirari_audio_buffer_storage_clear(block, 0, 8);
        crate::track_runtime::hirari_track_process_block(runtime, block, 8, 0, 1);
    }
    let read = |channel: u32, index: u32| -> f32 {
        unsafe {
            *crate::audio_buffer_ops::hirari_audio_buffer_storage_read_pointer(block, channel, 0)
                .add(index as usize)
        }
    };
    assert_eq!(read(0, 0), 1.0);
    assert_eq!(read(0, 7), 8.0);
    assert_eq!(read(1, 0), 100.0);
    assert_eq!(read(1, 7), 107.0);
    // Playback past the snapshot is silent rather than a wrap-around read.
    unsafe {
        crate::track_runtime::hirari_track_process_block(runtime, block, 8, 8_000, 1);
    }
    assert_eq!(read(0, 0), 0.0);
    assert_eq!(read(1, 7), 0.0);

    unsafe {
        crate::audio_buffer_ops::hirari_audio_buffer_storage_destroy(block);
        crate::track_frozen_store::hirari_track_frozen_store_clear(frozen);
        assert!(!fixture.is_frozen());
        assert_eq!(
            crate::track_frozen_store::hirari_track_frozen_store_retained_count(frozen),
            1
        );
        crate::track_frozen_store::hirari_track_frozen_store_retire(frozen);
    }
    assert_eq!(
        unsafe { crate::track_frozen_store::hirari_track_frozen_store_retained_count(frozen) },
        0
    );
}
