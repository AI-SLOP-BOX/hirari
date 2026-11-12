//! Tests split out of region_time_jobs.rs.
    use super::*;
    use std::time::{Duration, Instant};

    struct FakeRegion {
        id: u32,
        state: HirariRegionState,
        markers: Vec<HirariWarpMarker>,
        samples: Vec<f32>,
    }

    struct FakeTrack {
        id: u32,
        generation: u64,
        snapshot: usize,
        regions: Vec<FakeRegion>,
    }

    #[derive(Default)]
    struct FakeInner {
        tracks: Vec<FakeTrack>,
        applied: Vec<(u32, u32, usize)>,
        undo_entries: Vec<(String, usize, usize)>,
    }

    struct FakeHost {
        inner: Mutex<FakeInner>,
        tempo: *mut c_void,
        sample_rate: f64,
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    impl FakeHost {
        fn inner(&self) -> std::sync::MutexGuard<'_, FakeInner> {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    fn host_of<'a>(host: *mut c_void) -> &'a FakeHost {
        unsafe { &*(host as *const FakeHost) }
    }

    unsafe extern "C" fn fake_find(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
        out: *mut HirariRegionState,
    ) -> bool {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id == region_id {
                    unsafe { out.write(region.state) };
                    return true;
                }
            }
        }
        false
    }

    fn payload_markers(payload: &HirariRegionEditPayload) -> Vec<HirariWarpMarker> {
        if payload.blob.is_null() || payload.blob_len == 0 {
            return Vec::new();
        }
        unsafe {
            std::slice::from_raw_parts(
                payload.blob.cast::<HirariWarpMarker>(),
                payload.blob_len / std::mem::size_of::<HirariWarpMarker>(),
            )
            .to_vec()
        }
    }

    unsafe extern "C" fn fake_apply(
        host: *mut c_void,
        payload: *const HirariRegionEditPayload,
    ) -> bool {
        let payload = unsafe { ptr::read(payload) };
        let host = host_of(host);
        let mut guard = host.inner();
        if payload.op != HIRARI_REGION_EDIT_MARKERS {
            return false;
        }
        let markers = payload_markers(&payload);
        let mut applied = None;
        for track in &mut guard.tracks {
            if track.id != payload.track_id {
                continue;
            }
            for region in &mut track.regions {
                if region.id == payload.region_id {
                    region.markers = markers;
                    applied = Some(region.markers.len());
                    break;
                }
            }
            break;
        }
        match applied {
            Some(count) => {
                guard.applied.push((payload.track_id, payload.region_id, count));
                true
            }
            None => false,
        }
    }

    unsafe extern "C" fn fake_record(
        _host: *mut c_void,
        name: *const std::ffi::c_char,
        undo: *const HirariRegionEditPayload,
        undo_count: usize,
        redo: *const HirariRegionEditPayload,
        redo_count: usize,
        _coalesce: bool,
        _perform: bool,
    ) -> bool {
        let name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let _ = unsafe { std::slice::from_raw_parts(undo, undo_count) };
        let _ = unsafe { std::slice::from_raw_parts(redo, redo_count) };
        let host = host_of(_host);
        host.inner().undo_entries.push((name, undo_count, redo_count));
        true
    }

    unsafe extern "C" fn fake_markers(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
        out: *mut HirariWarpMarker,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id != region_id {
                    continue;
                }
                let count = region.markers.len();
                if !out.is_null() && capacity >= count {
                    unsafe { ptr::copy_nonoverlapping(region.markers.as_ptr(), out, count) };
                }
                return count;
            }
        }
        0
    }

    struct FakeAudio {
        samples: Box<[f32]>,
    }

    unsafe extern "C" fn fake_retain_audio(
        host: *mut c_void,
        track_id: u32,
        region_id: u32,
    ) -> *mut c_void {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for region in &track.regions {
                if region.id == region_id {
                    let samples = region.samples.clone().into_boxed_slice();
                    return Box::into_raw(Box::new(FakeAudio { samples })).cast();
                }
            }
        }
        ptr::null_mut()
    }

    unsafe extern "C" fn fake_audio_view(
        handle: *mut c_void,
        out: *mut *const f32,
        capacity: usize,
        samples: *mut u64,
    ) -> u32 {
        if handle.is_null() {
            return 0;
        }
        let audio = unsafe { &*(handle as *const FakeAudio) };
        if !samples.is_null() {
            unsafe { *samples = audio.samples.len() as u64 };
        }
        if !out.is_null() && capacity >= 1 {
            unsafe { out.write(audio.samples.as_ptr()) };
        }
        1
    }

    unsafe extern "C" fn fake_release_audio(handle: *mut c_void) {
        if !handle.is_null() {
            drop(unsafe { Box::from_raw(handle as *mut FakeAudio) });
        }
    }

    unsafe extern "C" fn fake_track_token(
        host: *mut c_void,
        track_id: u32,
        generation: *mut u64,
        snapshot: *mut usize,
    ) -> bool {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            if !generation.is_null() {
                unsafe { *generation = track.generation };
            }
            if !snapshot.is_null() {
                unsafe { *snapshot = track.snapshot };
            }
            return true;
        }
        false
    }

    unsafe extern "C" fn fake_list_tracks(
        host: *mut c_void,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for (index, track) in guard.tracks.iter().enumerate() {
            if !out.is_null() && index < capacity {
                unsafe { out.add(index).write(track.id) };
            }
        }
        guard.tracks.len()
    }

    unsafe extern "C" fn fake_list_regions(
        host: *mut c_void,
        track_id: u32,
        out: *mut u32,
        capacity: usize,
    ) -> usize {
        let host = host_of(host);
        let guard = host.inner();
        for track in &guard.tracks {
            if track.id != track_id {
                continue;
            }
            for (index, region) in track.regions.iter().enumerate() {
                if !out.is_null() && index < capacity {
                    unsafe { out.add(index).write(region.id) };
                }
            }
            return track.regions.len();
        }
        0
    }

    unsafe extern "C" fn fake_tempo(host: *mut c_void) -> *mut c_void {
        host_of(host).tempo
    }

    unsafe extern "C" fn fake_undo_state(_host: *mut c_void) -> *mut c_void {
        ptr::null_mut()
    }

    unsafe extern "C" fn fake_sample_rate(host: *mut c_void) -> f64 {
        host_of(host).sample_rate
    }

    fn ops() -> HirariRegionHostOps {
        HirariRegionHostOps {
            find_region: Some(fake_find),
            apply_edit: Some(fake_apply),
            record_undo: Some(fake_record),
            warp_markers: Some(fake_markers),
            retain_audio: Some(fake_retain_audio),
            audio_view: Some(fake_audio_view),
            release_audio: Some(fake_release_audio),
            track_token: Some(fake_track_token),
            list_tracks: Some(fake_list_tracks),
            list_regions: Some(fake_list_regions),
            tempo_state: Some(fake_tempo),
            undo_state: Some(fake_undo_state),
            sample_rate: Some(fake_sample_rate),
            ..Default::default()
        }
    }

    /// Alternating loud/quiet blocks give the alignment and quantize kernels a
    /// stable envelope to follow.
    fn pattern(len: usize, period: usize) -> Vec<f32> {
        (0..len)
            .map(|index| if (index / period).is_multiple_of(2) { 0.1 } else { 0.8 })
            .collect()
    }

    fn state(start: u64, len: u64, group: u32) -> HirariRegionState {
        HirariRegionState {
            start,
            len,
            source_length: len,
            loop_count: 1,
            warp_ratio: 1.0,
            sync_group: group,
            source_sample_rate: 44_100.0,
            timeline_sample_rate: 44_100.0,
            ..Default::default()
        }
    }

    struct Fixture {
        host: *mut c_void,
        ops: HirariRegionHostOps,
    }

    impl Fixture {
        fn new(tracks: Vec<FakeTrack>) -> Self {
            let tempo = crate::tempo::hirari_tempo_map_create();
            unsafe {
                crate::tempo::hirari_tempo_map_clear(tempo, 44_100.0, 120.0);
            }
            Self {
                host: Box::into_raw(Box::new(FakeHost {
                    inner: Mutex::new(FakeInner {
                        tracks,
                        ..Default::default()
                    }),
                    tempo,
                    sample_rate: 44_100.0,
                }))
                .cast(),
                ops: ops(),
            }
        }

        fn inner(&self) -> std::sync::MutexGuard<'_, FakeInner> {
            host_of(self.host).inner()
        }

        fn wait(&self, registry: &'static Registry, key: u64) -> u8 {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let status = status_of(registry, self.host, key);
                if status == STATUS_READY || status == STATUS_FAILED || Instant::now() > deadline {
                    return status;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                crate::tempo::hirari_tempo_map_destroy(host_of(self.host).tempo);
            }
            // The host handle itself stays allocated: the process-global job
            // registries are keyed by host address, so a reused address could
            // expose a previous fixture's finished job.
        }
    }

    fn track(id: u32, regions: Vec<FakeRegion>) -> FakeTrack {
        FakeTrack {
            id,
            generation: 1,
            snapshot: id as usize * 1_000,
            regions,
        }
    }

    fn region(id: u32, state: HirariRegionState, samples: Vec<f32>) -> FakeRegion {
        FakeRegion {
            id,
            state,
            markers: Vec::new(),
            samples,
        }
    }

    #[test]
    fn alignment_applies_markers_to_every_target_and_records_one_undo_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![
                region(2, state(0, 8192, 0), pattern(8192, 512)),
                region(3, state(0, 8192, 0), pattern(8192, 512)),
            ],
        )]);
        let ops = fixture.ops;

        assert!(align_with(&ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(
            fixture.wait(alignment_registry(), region_key(1, 2)),
            STATUS_READY
        );
        assert!(align_finalize(&ops, fixture.host, 1, 2));

        let inner = fixture.inner();
        let applied = &inner.applied;
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, 1);
        assert_eq!(applied[0].1, 3);
        assert!(applied[0].2 >= 2);
        assert_eq!(inner.undo_entries.len(), 1);
        assert_eq!(inner.undo_entries[0].0, "Align Audio Regions to Reference");
        assert_eq!(inner.undo_entries[0].1, 1);
        assert_eq!(inner.undo_entries[0].2, 1);
        drop(inner);
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), STATUS_APPLIED);
    }

    #[test]
    fn alignment_rejects_targets_that_session_changed_before_finalize() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![
                region(2, state(0, 8192, 0), pattern(8192, 512)),
                region(3, state(0, 8192, 0), pattern(8192, 512)),
            ],
        )]);
        let ops = fixture.ops;

        assert!(align_with(&ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(
            fixture.wait(alignment_registry(), region_key(1, 2)),
            STATUS_READY
        );
        fixture.inner().tracks[0].generation += 1;

        assert!(!align_finalize(&ops, fixture.host, 1, 2));
        let inner = fixture.inner();
        assert!(inner.applied.is_empty());
        assert!(inner.undo_entries.is_empty());
        drop(inner);
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), STATUS_FAILED);
    }

    #[test]
    fn alignment_rejects_a_reference_that_already_has_markers() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mut reference = region(2, state(0, 8192, 0), pattern(8192, 512));
        reference.markers.push(HirariWarpMarker::default());
        let fixture = Fixture::new(vec![track(
            1,
            vec![reference, region(3, state(0, 8192, 0), pattern(8192, 512))],
        )]);

        assert!(!align_with(&fixture.ops, fixture.host, 1, 2, &[(1, 3)]));
        assert_eq!(status_of(alignment_registry(), fixture.host, region_key(1, 2)), 0);
    }

    #[test]
    fn quantize_marks_every_sync_group_member_and_records_one_grouped_entry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(0, 8192, 7), pattern(8192, 512))]),
        ]);
        let ops = fixture.ops;

        assert!(quantize_with(&ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(fixture.wait(quantize_registry(), region_key(1, 2)), STATUS_READY);
        assert!(quantize_finalize(&ops, fixture.host, 1, 2));

        let inner = fixture.inner();
        assert_eq!(inner.applied.len(), 2);
        assert!(inner.applied.iter().all(|(_, _, count)| *count >= 2));
        assert_eq!(inner.undo_entries.len(), 1);
        assert_eq!(inner.undo_entries[0].0, "Quantize Synchronized Audio Regions");
        assert_eq!(inner.undo_entries[0].1, 2);
        assert_eq!(inner.undo_entries[0].2, 2);
        drop(inner);
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), STATUS_APPLIED);
    }

    #[test]
    fn quantize_rejects_a_sync_group_member_at_a_different_position() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(512, 8192, 7), pattern(8192, 512))]),
        ]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), 0);
    }

    #[test]
    fn quantize_rejects_a_locked_sync_group_member() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mut member = region(3, state(0, 8192, 7), pattern(8192, 512));
        member.state.locked = true;
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![member]),
        ]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert!(fixture.inner().applied.is_empty());
    }

    #[test]
    fn quantize_ignores_group_members_without_audio() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![
            track(1, vec![region(2, state(0, 8192, 7), pattern(8192, 512))]),
            track(2, vec![region(3, state(0, 8192, 7), Vec::new())]),
        ]);
        let ops = fixture.ops;

        assert!(quantize_with(&ops, fixture.host, 1, 2, 1.0, 0.25, 0.0));
        assert_eq!(fixture.wait(quantize_registry(), region_key(1, 2)), STATUS_READY);
        assert!(quantize_finalize(&ops, fixture.host, 1, 2));
        assert_eq!(fixture.inner().applied.len(), 1);
    }

    #[test]
    fn quantize_rejects_settings_outside_the_shared_bounds() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fixture = Fixture::new(vec![track(
            1,
            vec![region(2, state(0, 8192, 7), pattern(8192, 512))],
        )]);

        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.5, 0.25, 0.0));
        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.3, 0.0));
        assert!(!quantize_with(&fixture.ops, fixture.host, 1, 2, 1.0, 0.25, 2.0));
        assert_eq!(status_of(quantize_registry(), fixture.host, region_key(1, 2)), 0);
    }
