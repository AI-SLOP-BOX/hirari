int main() {
    // The built-in compressor and the sandbox supervisor used to be exercised
    // here through the deleted C++ plugin host. Both are Rust-owned and covered
    // by compressor_state_round_trips_and_resyncs_the_mirror,
    // parameters_are_mirrored_and_flushed_at_the_block_boundary and
    // traversal_runs_the_published_nodes.

    // Transactional undo is Rust-owned (hirari-core-bridge/src/undo_transaction.rs).
    // The contract drives the raw FFI so native callbacks and abort rollback
    // stay covered without any C++ history wrapper.
    void* engineUndo = hirari_undo_manager_create(128, 300);
    if (!engineUndo) return 18;
    int engineTransactionValue = 0;
    struct UndoDelta {
        int* value;
        int delta;
    };
    const auto registerDelta = [](void* state, int* value, int delta) -> uint64_t {
        auto* context = new UndoDelta{value, delta};
        const uint64_t id = hirari_undo_manager_register_native_callback(
            state, context,
            [](void* raw) { auto* d = static_cast<UndoDelta*>(raw); *d->value += d->delta; },
            [](void* raw) { delete static_cast<UndoDelta*>(raw); });
        if (!id) delete context;
        return id;
    };
    const auto perform = [&](const char* name, int delta) {
        const std::string label(name);
        return hirari_undo_manager_commit(
            engineUndo, reinterpret_cast<const uint8_t*>(label.data()), label.size(),
            registerDelta(engineUndo, &engineTransactionValue, -delta),
            registerDelta(engineUndo, &engineTransactionValue, delta), true, true);
    };

    void* finiteContract = hirari_audio_buffer_storage_create();
    if (hirari_audio_buffer_storage_reserve_buffer(finiteContract, 1, 3) != 0 ||
        !hirari_audio_buffer_storage_resize(finiteContract, 1, 3)) return 24;
    hirari_audio_buffer_storage_write_pointer(finiteContract, 0)[0] = 1.0f;
    hirari_audio_buffer_storage_write_pointer(finiteContract, 0)[1] =
        std::numeric_limits<float>::quiet_NaN();
    hirari_audio_buffer_storage_write_pointer(finiteContract, 0)[2] =
        std::numeric_limits<float>::infinity();
    const uint32_t sanitized = hirari_audio_buffer_storage_sanitize(finiteContract);
    const float* finitePlane = hirari_audio_buffer_storage_read_pointer(finiteContract, 0, 0);
    const bool planesFinite =
        std::isfinite(finitePlane[1]) && std::isfinite(finitePlane[2]);
    hirari_audio_buffer_storage_destroy(finiteContract);
    if (sanitized != 2 || !planesFinite) return 24;

    const char* partial = "engine-partial-operation";
    (void)hirari_undo_manager_begin_transaction(
        engineUndo, reinterpret_cast<const uint8_t*>(partial), std::string(partial).size());
    (void)perform("first", 2);
    (void)perform("second", 3);
    if (engineTransactionValue != 5 || !hirari_undo_manager_transaction_active(engineUndo))
        return 18;
    if (!hirari_undo_manager_abort_and_invoke(engineUndo) || engineTransactionValue != 0 ||
        hirari_undo_manager_transaction_active(engineUndo) ||
        hirari_undo_manager_undo_count(engineUndo) != 0) return 19;
    const char* commit = "engine-commit";
    (void)hirari_undo_manager_begin_transaction(
        engineUndo, reinterpret_cast<const uint8_t*>(commit), std::string(commit).size());
    (void)perform("first", 2);
    (void)perform("second", 3);
    if (!hirari_undo_manager_end_transaction(engineUndo) ||
        hirari_undo_manager_undo_count(engineUndo) != 1) return 20;
    (void)hirari_undo_manager_apply(engineUndo, false);
    if (engineTransactionValue != 0 || hirari_undo_manager_redo_count(engineUndo) != 1)
        return 21;
    (void)hirari_undo_manager_apply(engineUndo, true);
    if (engineTransactionValue != 5) return 22;
    hirari_undo_manager_clear(engineUndo);
    hirari_undo_manager_destroy(engineUndo);

    // A failed DB publication must not remove the last destination. Using a
    // directory as the destination forces rename to fail while making the
    // preservation invariant directly observable.
    void* projectDb = hirari_project_db_create();
    const auto* dbCategory = reinterpret_cast<const uint8_t*>("contract");
    const auto* dbKey = reinterpret_cast<const uint8_t*>("publication");
    const auto* dbValue = reinterpret_cast<const uint8_t*>("preserve-last-good");
    (void)hirari_project_db_record(
        projectDb, static_cast<uint64_t>(std::chrono::system_clock::now().time_since_epoch().count()),
        dbCategory, 8, dbKey, 11, dbValue, 18);
    const auto dbDestination = std::filesystem::temp_directory_path() / "hirari-project-db-contract-destination";
    std::error_code dbCleanup;
    std::filesystem::remove_all(dbDestination, dbCleanup);
    std::filesystem::create_directory(dbDestination, dbCleanup);
    const std::string dbDestinationText = dbDestination.string();
    const bool dbFlushed = hirari_project_db_flush(
        projectDb, reinterpret_cast<const uint8_t*>(dbDestinationText.data()),
        dbDestinationText.size());
    hirari_project_db_destroy(projectDb);
    if (dbCleanup || dbFlushed ||
        !std::filesystem::is_directory(dbDestination)) return 65;
    std::filesystem::remove_all(dbDestination, dbCleanup);



    // Payloads beyond the bounded SysEx/MIDI2 envelope must be observable at
    // ingress rather than silently becoming a truncated channel-voice event.
    void* midiIngress = hirari_midi_buffer_create();
    const uint8_t oversizedMidi[257] = {};
    hirari_midi_buffer_add(midiIngress, 0, oversizedMidi, sizeof(oversizedMidi), 0);
    const bool ingressReports =
        hirari_midi_buffer_overflowed(midiIngress) &&
        hirari_midi_buffer_take_oversize(midiIngress) == 1 &&
        hirari_midi_buffer_take_extended(midiIngress) == 1 &&
        hirari_midi_buffer_event_count(midiIngress) == 0;
    hirari_midi_buffer_destroy(midiIngress);
    if (!ingressReports) return 15;
