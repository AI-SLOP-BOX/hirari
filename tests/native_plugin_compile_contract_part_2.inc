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
        std::filesystem::remove(project, cleanup);
    }

    std::error_code offlineCleanup;

    {
        // Freeze playback is a real native path: publish happens off the
        // audio thread, while Track::process only copies from the immutable
        // bounded snapshot and bypasses the live region/effect graph.
        auto frozen = std::make_shared<Hirari::Core::AudioBuffer>(2, 8);
        for (uint32_t index = 0; index < 8; ++index) {
            frozen->getWritePointer(0)[index] = static_cast<float>(index + 1);
            frozen->getWritePointer(1)[index] = static_cast<float>(100 + index);
        }
        auto freezeTrack = std::make_shared<Hirari::Core::Engine::Track>(79, "Frozen", Hirari::Core::Engine::Track::Audio);
        freezeTrack->prepareToPlay(48'000.0, 8);
        if (!freezeTrack->publishFrozenAudio(frozen, 8, 48'000.0, 11, 13) ||
            !freezeTrack->isFrozen() || freezeTrack->frozenProjectGeneration() != 11 ||
            freezeTrack->frozenAudioGeneration() != 13) return 65;
        Hirari::Core::AudioBuffer rendered(2, 8);
        rendered.clear();
        freezeTrack->process(rendered, 8, 0);
        if (rendered.getReadPointer(0)[0] != 1.0f ||
            rendered.getReadPointer(0)[7] != 8.0f ||
            rendered.getReadPointer(1)[0] != 100.0f ||
            rendered.getReadPointer(1)[7] != 107.0f) return 66;
        freezeTrack->clearFrozenAudio();
        if (freezeTrack->isFrozen()) return 67;
        auto renderedTrack = std::make_shared<Hirari::Core::Engine::Track>(80, "Rendered Freeze", Hirari::Core::Engine::Track::Audio);
        Hirari::Core::Engine::Region sourceRegion{};
        sourceRegion.id = 1;
        sourceRegion.start = 0;
        sourceRegion.len = 8;
        sourceRegion.muted = false;
        sourceRegion.name = "freeze-source";
        sourceRegion.audio = frozen;
        renderedTrack->addRegion(sourceRegion);
        if (!Hirari::Core::Engine::TrackFreezeManager::freezeTrack(renderedTrack, 8, 48'000) ||
            !renderedTrack->isFrozen()) return 68;
        Hirari::Core::Engine::TrackFreezeManager::unfreezeTrack(renderedTrack);
        renderedTrack->collectRetiredFrozenAudio();
        if (renderedTrack->isFrozen()) return 69;
    }

