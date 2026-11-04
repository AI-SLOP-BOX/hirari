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

