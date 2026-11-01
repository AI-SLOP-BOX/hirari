    // Phase 9: waveform peak cache lives in Rust (region_audio.rs); this
    // translation unit no longer holds per-region peak state.

    // PDC is project/session state. It must not be shared through the legacy
    // process-wide compatibility singleton. Rust (native_pdc.rs) owns the
    // configuration, solving, thread ownership, and atomic publication; this
    // member is only the handle the control thread edits and reads from.
    void* m_pdcState = hirari_native_pdc_create();
    // Node limits checked before a value crosses into Rust. Rust keeps the
    // authoritative copies (native_pdc.rs: MAX_TRACKS, MASTER_ID); these hold
    // here so an out-of-range ID never reaches the FFI in the first place.
    static constexpr uint32_t kPdcMaxTracks = 512;
    // Rust owns history, callback IDs, grouping, coalescing and eviction.
    // This is the single handle the engine keeps and hands to Rust callers.
    void* m_undoState = hirari_undo_manager_create(128, 300);
    void* m_tempoState = hirari_tempo_map_create();
    // MPE channel/voice ownership lives in Rust (mpe.rs); the engine keeps the
    // singleton handle so project reset can clear it without a C++ adapter.
    void* m_mpeManager = hirari_mpe_manager_create();
    // Rust (routing_engine_core.rs) owns the graph, gain, feedback, send-PDC
    // and topology stores; this is the single opaque handle the engine keeps.
    void* m_routingEngine = hirari_routing_engine_create();
    // Rust owns the publication gate, writer lock and captured undo snapshot;
    // this is the single handle the engine keeps.
    void* m_sidechainEngine = hirari_sidechain_engine_create();
    // Rust (bus_audio.rs) owns the per-bus summing stages and the summing
    // system; the engine only keeps the handle.
    void* m_busSystem = hirari_bus_system_create();
    static constexpr size_t kMaxMacros = 128;
    void* m_macroControl = hirari_macro_mapping_create();
    void* m_midiLearnState = hirari_midi_learn_create();
    // Rust (forensic_journaler.rs) owns the journal and writer thread.
    void* m_forensicJournal = hirari_forensic_journaler_create();
    // Decoding and import-cache policy live in Rust; this only adapts the
    // interleaved decode result into the engine's AudioBuffer ownership.
    std::shared_ptr<AudioBuffer> importAudioFile(const std::string& path,
                                                 double* sampleRateOut = nullptr);

    // Engine Infrastructure

    // The Control Room model is implemented in Rust
    // (control_room_runtime.rs). This is only the handle its audio
    // callback renders with; C++ never interprets the state itself, so the
    // engine owns nothing but the lifetime.
    void* m_controlRoomState = hirari_control_room_state_create();
    // MIDI clock receive state lives in Rust (external_sync_runtime.rs).
    void* m_externalSync = hirari_external_sync_create();
    // Timeline markers and arranger parts are a Rust-owned document
    // (arrangement_document.rs); the engine only owns the handle.
    void* m_arrangementDocument = hirari_arrangement_document_create();
    // Phase 10: Sovereign Recovery
    // Manual health polling and the background recovery loop share the same
    // sandbox host objects. Rust serializes maintenance and keeps the
    // observed-health map (plugin_sandbox_maintenance.rs); the engine only
    // supplies the track/status/restart callbacks.
    // Region import and pitch analysis jobs live in Rust (region_jobs.rs):
    // validation, registry limits, worker threads, staleness checks and undo
    // capture are all owned there. These declarations are the storage
    // callbacks that module drives.

public:
    // Region edit host callbacks. Rust drives every region edit through these
    // entry points; see hirari_unified_engine_part_4z.inc.
    static bool regionHostFind(void* host, uint32_t trackId, uint32_t regionId,
                               HirariRegionState* out);
    static bool regionHostApply(void* host, const HirariRegionEditPayload* payload);
    static void* regionHostSnapshot(void* host, uint32_t trackId, uint32_t regionId);
    static void regionHostReleaseSnapshot(void* host, void* handle);
    static size_t regionHostWarpMarkers(void* host, uint32_t trackId, uint32_t regionId,
                                        HirariWarpMarker* out, size_t capacity);
    static size_t regionHostRangeEdits(void* host, uint32_t trackId, uint32_t regionId,
                                       HirariRegionRangeEdit* out, size_t capacity);
    static size_t regionHostListSyncGroup(void* host, uint32_t group,
                                          HirariRegionGroupMember* out, size_t capacity);
    static bool regionHostNoteEdit(void* host, uint32_t trackId, uint32_t regionId,
                                   void* context, HirariRegionNoteMutateFn mutate);
    static void* regionHostRetainAudio(void* host, uint32_t trackId, uint32_t regionId);
    static uint32_t regionHostAudioView(void* handle, const float** out, size_t capacity,
                                        uint64_t* samples);
    static void regionHostReleaseAudio(void* handle);
    static bool regionHostTrackToken(void* host, uint32_t trackId, uint64_t* generation,
                                     size_t* snapshot);
    static size_t regionHostListTracks(void* host, uint32_t* out, size_t capacity);
    static size_t regionHostListRegions(void* host, uint32_t trackId, uint32_t* out,
                                        size_t capacity);
    static void* regionHostTempoState(void* host);
    static void* regionHostUndoState(void* host);
    static double regionHostSampleRate(void* host);
    static size_t regionHostRegionPath(void* host, uint32_t trackId, uint32_t regionId,
                                       uint8_t* out, size_t capacity);
    static bool regionHostDecodeAudio(void* host, const uint8_t* path, size_t length,
                                      void** outHandle, double* outRate);
    static uint32_t regionHostInsertRegion(void* host, uint32_t trackId,
                                           size_t expectedTrack, const uint8_t* path,
                                           size_t pathLength, uint64_t startSample,
                                           double sourceSampleRate, double timelineSampleRate,
                                           uint64_t length, void* audioHandle);
    static size_t regionHostTrackIdentity(void* host, uint32_t trackId);
    static void* regionHostRetainTrack(void* host, uint32_t trackId);
    static void regionHostReleaseTrack(void* handle);
    static bool regionHostPitchBegin(void* trackHandle);
    static void regionHostPitchFinish(void* trackHandle);
    static void* regionHostNoteSnapshot(void* trackHandle, uint32_t regionId);
    static uint32_t regionHostClone(void* host, uint32_t trackId, uint32_t regionId,
                                    const HirariRegionState* state);
    static size_t regionHostCompRanges(void* host, uint32_t trackId, uint32_t regionId,
                                       HirariRegionCompRange* out, size_t capacity,
                                       bool* managed);

    // Automation edit host callbacks. Rust drives the curve edits; see
    // hirari_unified_engine_part_6b.inc.
    static size_t automationHostRead(void* host, uint32_t trackId, int32_t kind,
                                     uint32_t pluginIndex, uint32_t parameterId,
                                     Hirari::Core::ProjectAutomationPoint* out, size_t capacity,
                                     bool* targetFound);
    static bool automationHostApply(void* host, const HirariAutomationEditPayload* payload);

private:

};
