    // The ARUA container now lives in the Rust bridge, so the contract drives
    // the same entrypoints the engine calls to keep the cxx marshalling and
    // link surface covered. The full rejection matrix (duplicate track ids,
    // orphan regions, corruption and oversized sparse input) lives beside the
    // codec in
    // `native_project::tests::rejects_duplicate_tracks_orphan_regions_corruption_and_oversize`.
    {
        const auto project =
            std::filesystem::temp_directory_path() / "hirari-region-id-contract.bin";
        Hirari::Core::Bridge::NativeProjectState state;
        state.version = 28;
        state.sampleRate = 48'000;
        state.bpm = 120.0;
        Hirari::Core::Bridge::NativeTrackState track;
        track.id = 7;
        track.volume = 1.0f;
        track.pan = 0.0f;
        Hirari::Core::Bridge::NativeRegionState region;
        region.id = 9001;
        region.trackId = 7;
        region.samplePosition = 0;
        region.sampleLength = 128;
        region.filePath = "contract.wav";
        // The version-28 decoder requires a usable warp ratio and a positive
        // loop count; the C++ `RegionState` defaults supplied both.
        region.warpRatio = 1.0;
        region.loopCount = 1;
        state.tracks.push_back(std::move(track));
        state.regions.push_back(std::move(region));
        if (!Hirari::Core::Bridge::native_project_save(project.string(), state)) return 5;
        const auto restored = Hirari::Core::Bridge::native_project_load(project.string());
        if (!restored.valid || restored.regions.size() != 1 || restored.regions[0].id != 9001) {
            return 6;
        }
        std::error_code cleanup;
        std::filesystem::remove(project, cleanup);
    }

    std::error_code offlineCleanup;

    {
        // Freeze playback is a real native path: publish happens off the
        // audio thread, while Track::process only copies from the immutable
        // bounded snapshot and bypasses the live region/effect graph.
        auto frozen = Hirari::Core::Engine::makeSharedAudioStorage(2, 8);
        for (uint32_t index = 0; index < 8; ++index) {
            hirari_audio_buffer_storage_write_pointer(frozen.get(), 0)[index] =
                static_cast<float>(index + 1);
            hirari_audio_buffer_storage_write_pointer(frozen.get(), 1)[index] =
                static_cast<float>(100 + index);
        }
        auto freezeTrack = std::make_shared<Hirari::Core::Engine::Track>(79, "Frozen", Hirari::Core::Engine::Track::Audio);
        freezeTrack->prepareToPlay(48'000.0, 8);
        if (!freezeTrack->publishFrozenAudio(
                Hirari::Core::Engine::SharedAudioStorage(frozen), 8, 48'000.0, 11, 13) ||
            !freezeTrack->isFrozen() || freezeTrack->frozenProjectGeneration() != 11 ||
            freezeTrack->frozenAudioGeneration() != 13) return 65;
        void* rendered = hirari_audio_buffer_storage_create();
        if (hirari_audio_buffer_storage_reserve_buffer(rendered, 2, 8) != 0 ||
            !hirari_audio_buffer_storage_resize(rendered, 2, 8)) {
            hirari_audio_buffer_storage_destroy(rendered);
            return 66;
        }
        hirari_audio_buffer_storage_clear(rendered, 0, 8);
        freezeTrack->process(rendered, 8, 0);
        const bool renderedMatches =
            hirari_audio_buffer_storage_read_pointer(rendered, 0, 0)[0] == 1.0f &&
            hirari_audio_buffer_storage_read_pointer(rendered, 0, 0)[7] == 8.0f &&
            hirari_audio_buffer_storage_read_pointer(rendered, 1, 0)[0] == 100.0f &&
            hirari_audio_buffer_storage_read_pointer(rendered, 1, 0)[7] == 107.0f;
        hirari_audio_buffer_storage_destroy(rendered);
        if (!renderedMatches) return 66;
        freezeTrack->clearFrozenAudio();
        if (freezeTrack->isFrozen()) return 67;
        auto renderedTrack = std::make_shared<Hirari::Core::Engine::Track>(80, "Rendered Freeze", Hirari::Core::Engine::Track::Audio);
        HirariRegionState sourceState{};
        sourceState.len = 8;
        auto* sourceDraft = hirari_region_store_draft_new(
            1, nullptr, 0, reinterpret_cast<const uint8_t*>("freeze-source"),
            sizeof("freeze-source") - 1, &sourceState);
        if (!hirari_region_store_draft_add(
                sourceDraft, renderedTrack.get(),
                new Hirari::Core::Engine::SharedAudioStorage(frozen))) return 68;
        if (!Hirari::Core::Engine::TrackFreezeManager::freezeTrack(renderedTrack, 8, 48'000) ||
            !renderedTrack->isFrozen()) return 68;
        Hirari::Core::Engine::TrackFreezeManager::unfreezeTrack(renderedTrack);
        renderedTrack->collectRetiredFrozenAudio();
        if (renderedTrack->isFrozen()) return 69;
    }

