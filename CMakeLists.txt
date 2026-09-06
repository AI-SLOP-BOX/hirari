cmake_minimum_required(VERSION 3.20)
project(AuraDAWBuild NONE)

# The production application is now a Rust/Slint workspace.  Keep a small,
# honest CMake entrypoint for IDEs and existing native build automation rather
# than resurrecting the deleted legacy C++ executable and stale Metal sources.
find_program(CARGO_EXECUTABLE cargo REQUIRED)
enable_testing()

set(AURA_MANIFEST "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml")
set(AURA_APP_SCRIPT "${CMAKE_CURRENT_SOURCE_DIR}/scripts/build_app.sh")
set(AURA_VERIFY_SCRIPT "${CMAKE_CURRENT_SOURCE_DIR}/scripts/verify_release_bundle.sh")
set(AURA_RELEASE_ARCH "" CACHE STRING
    "Expected release architecture (arm64, x86_64, or universal2)")
if(NOT AURA_RELEASE_ARCH)
    # Use the configured target architecture, not the machine running CMake.
    # This matters under Rosetta and for cross-compilation.
    if(CMAKE_OSX_ARCHITECTURES MATCHES "(^|;)arm64(;|$)" AND
       CMAKE_OSX_ARCHITECTURES MATCHES "(^|;)(x86_64|AMD64)(;|$)")
        set(AURA_RELEASE_ARCH universal2)
    elseif(CMAKE_OSX_ARCHITECTURES MATCHES "(^|;)arm64(;|$)")
        set(AURA_RELEASE_ARCH arm64)
    elseif(CMAKE_OSX_ARCHITECTURES MATCHES "(^|;)(x86_64|AMD64)(;|$)")
        set(AURA_RELEASE_ARCH x86_64)
    elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "arm64|aarch64")
        set(AURA_RELEASE_ARCH arm64)
    elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "x86_64|AMD64")
        set(AURA_RELEASE_ARCH x86_64)
    else()
        set(AURA_RELEASE_ARCH "")
    endif()
endif()

add_custom_target(aura-check
    COMMAND "${CARGO_EXECUTABLE}" check --workspace --manifest-path "${AURA_MANIFEST}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Checking Aura Rust workspace"
)

# Keep CTest useful for IDEs and CI runners.  The native project is primarily
# a Rust/Slint workspace, so the stable smoke gate is the workspace check.
add_test(NAME aura_workspace_check
    COMMAND "${CARGO_EXECUTABLE}" check --workspace --manifest-path "${AURA_MANIFEST}")
set_tests_properties(aura_workspace_check PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")

add_test(NAME aura_audio_device_transition
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_device_transition_contract.sh")
set_tests_properties(aura_audio_device_transition PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")

add_test(NAME aura_native_plugin_contract
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_native_plugin_compile_contract.sh")
set_tests_properties(aura_native_plugin_contract PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")

add_test(NAME aura_cli_history_render_e2e
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_cli_history_render_e2e.sh")
set_tests_properties(aura_cli_history_render_e2e PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}")

add_test(NAME aura_ui_core_integration
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_ui_integration.sh")
set_tests_properties(aura_ui_core_integration PROPERTIES
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    TIMEOUT 900)

add_custom_target(aura-test-native
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_native_plugin_compile_contract.sh"
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/test_pdc_graph.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Testing native plugin and PDC contracts"
)

add_custom_target(aura-test
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_NATIVE_TEST_ISOLATION=1
            RUST_TEST_THREADS=1
            CARGO_INCREMENTAL=0
            CARGO_PROFILE_TEST_DEBUG=0
            "${CARGO_EXECUTABLE}" test --workspace --manifest-path "${AURA_MANIFEST}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Testing Aura Rust workspace"
)
add_dependencies(aura-test aura-test-native)

add_custom_target(aura-test-fast
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_NATIVE_TEST_ISOLATION=1
            RUST_TEST_THREADS=1
            CARGO_INCREMENTAL=0
            CARGO_PROFILE_TEST_DEBUG=0
            "${CARGO_EXECUTABLE}" test --workspace --lib --manifest-path "${AURA_MANIFEST}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running fast Aura library tests (excludes integration/native lifecycle tests)"
)

add_custom_target(aura-test-ui-integration
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_ui_integration.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running non-vacuous UI/core workflow integration tests"
)

add_custom_target(aura-cli
    COMMAND "${CARGO_EXECUTABLE}" build -p aura-core-bridge --bin aura
            --manifest-path "${AURA_MANIFEST}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Building Aura declarative CLI"
)

add_custom_target(aura-test-sanitized
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_cpp_sanitizers.sh"
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_cpp_tsan.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running sanitized native contracts"
)

add_custom_target(aura-test-all)
add_dependencies(aura-test-all aura-test aura-test-native aura-test-sanitized aura-test-stress)
add_dependencies(aura-test-all aura-test-ui-integration)

add_custom_target(aura-audit-session-singletons
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/audit_session_singletons.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Auditing project-state singleton references"
)
add_dependencies(aura-test-all aura-audit-session-singletons)

add_custom_target(aura-audit-realtime-boundary
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/check_realtime_boundary.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Auditing audio-thread realtime boundaries"
)
add_dependencies(aura-test-all aura-audit-realtime-boundary)

add_custom_target(aura-audit-async-generations
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/audit_async_generation_gate.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Auditing asynchronous publication generation boundaries"
)
add_dependencies(aura-test-all aura-audit-async-generations)

add_custom_target(aura-test-stress
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_native_stress.sh"
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_parallel_worker_smoke.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running repeated native lifecycle stress contract"
)

add_custom_target(aura-test-project-soak
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_NATIVE_TEST_ISOLATION=1
            CARGO_INCREMENTAL=0
            "${CARGO_EXECUTABLE}" test -p aura-core-bridge --test render_workflow
            large_project_save_reload_render_cycles_preserve_track_graph
            --quiet --manifest-path "${AURA_MANIFEST}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running large-project save/reload/render soak"
)
add_dependencies(aura-test-all aura-test-project-soak)

add_custom_target(aura-test-recording-long
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_recording_stress.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running long-duration recording and finalized-frame stress contract"
)
add_dependencies(aura-test-stress aura-test-recording-long)

add_custom_target(aura-test-stress-long
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_STRESS_DURATION_SECONDS=3600
            AURA_REQUIRE_RESOURCE_METRICS=1
            /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_native_stress.sh"
    COMMAND "${CARGO_EXECUTABLE}" test -p aura-core-bridge --lib
            persistence::tests::repeated_atomic_save_load_keeps_latest_generation_and_no_temp_files
            --manifest-path "${AURA_MANIFEST}" -- --exact --nocapture
    COMMAND "${CARGO_EXECUTABLE}" test -p aura-core-bridge --lib
            offline::tests::stale_render_completion_cannot_publish_after_batch_cancellation
            --manifest-path "${AURA_MANIFEST}" -- --exact --nocapture
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running the long-duration native lifecycle stress contract"
)

add_custom_target(aura-test-device-transition
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_device_transition_contract.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Testing audio device fallback and reconfiguration transitions"
)
add_dependencies(aura-test-all aura-test-device-transition)

add_custom_target(aura-test-real-device-matrix
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_real_device_matrix.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Collecting PASS/FAIL/SKIPPED evidence for the local device and plugin matrix"
)

add_custom_target(aura-test-fuzz
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_input_fuzz_smoke.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running deterministic project/WAV/plugin-state fuzz smoke"
)

add_custom_target(aura-test-codecs
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_ffmpeg_codec_contract.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running direct FFmpeg MP3/FLAC publication contract"
)
add_dependencies(aura-test-all aura-test-codecs)

add_custom_target(aura-test-cli-e2e
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_cli_history_render_e2e.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running CLI generation, render, replay and history E2E"
)
add_dependencies(aura-test-all aura-test-cli-e2e)
add_dependencies(aura-test-all aura-test-fuzz)

add_custom_target(aura-test-vst3
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_REQUIRE_VST3=1
            /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_vst3_e2e_smoke.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running strict VST3 SDK and fixture E2E contract"
)

if(APPLE)
    add_custom_target(aura-test-plugin-matrix
        COMMAND "${CMAKE_COMMAND}" -E env
                AURA_RELEASE_MODE=1
                AURA_REQUIRE_VST3=1
                /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_vst3_e2e_smoke.sh"
        COMMAND "${CMAKE_COMMAND}" -E env
                AURA_RELEASE_MODE=1
                /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_au_e2e_smoke.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Running the strict external-plugin matrix (VST3 and AU)"
    )
else()
    add_custom_target(aura-test-plugin-matrix
        COMMAND "${CMAKE_COMMAND}" -E env
                AURA_RELEASE_MODE=1
                AURA_REQUIRE_VST3=1
                /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_vst3_e2e_smoke.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Running the strict external-plugin matrix (VST3)"
    )
endif()

if(APPLE)
    add_custom_target(aura-test-macos-device
        COMMAND "${CMAKE_COMMAND}" -E env AURA_REQUIRE_HARDWARE_DEVICE=1
                /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_macos_audio_device_contract.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Running the real CoreAudio initialize/start/stop/reconfigure contract"
    )

    add_custom_target(aura-test-au
        COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/run_au_e2e_smoke.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Running AU host E2E smoke on macOS"
    )
endif()

add_custom_target(aura-app
    COMMAND /bin/sh "${AURA_APP_SCRIPT}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Building packaging/Aura DAW.app"
)

add_custom_target(aura-verify-release
    COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/build_clap_fixture.sh"
    COMMAND "${CMAKE_COMMAND}" -E env
            AURA_RELEASE_MODE=1
            AURA_EXPECTED_ARCH=${AURA_RELEASE_ARCH}
            AURA_REQUIRE_ARCH=1
            AURA_REQUIRE_APP_SMOKE=1
            AURA_REQUIRE_PLUGIN_SMOKE=1
            AURA_REQUIRE_CODESIGN=1
            AURA_REQUIRE_HARDWARE_DEVICE=1
            /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/verify_release_bundle.sh"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    USES_TERMINAL
    COMMENT "Running strict release bundle verification"
)
add_dependencies(aura-verify-release aura-app)
add_dependencies(aura-verify-release aura-test-all)
if(APPLE)
    # A macOS release must prove the actual AU/VST3 paths and the CoreAudio
    # lifecycle, not only the packaged CLAP worker. Missing SDKs, fixtures, or
    # an unusable hardware output intentionally fail this gate.
    add_dependencies(aura-verify-release aura-test-plugin-matrix)
    add_dependencies(aura-verify-release aura-test-macos-device)
endif()

if(APPLE)
    add_custom_target(aura-verify
        COMMAND "${CMAKE_COMMAND}" -E env
                AURA_RELEASE_MODE=1
                AURA_EXPECTED_ARCH=${AURA_RELEASE_ARCH}
                AURA_REQUIRE_RELEASE_BUNDLE=1
                /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/verify_repository_health.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Verifying Aura release bundle"
    )
    # Keep the public verification entry point on the same strict gate as the
    # explicitly named release target.  Repository health is useful, but it
    # cannot prove that the packaged app, external-plugin matrix, CoreAudio
    # contract and codesign verification all passed.
    add_dependencies(aura-verify aura-verify-release)
else()
    add_custom_target(aura-verify
        COMMAND /bin/sh "${CMAKE_CURRENT_SOURCE_DIR}/scripts/verify_repository_health.sh"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        USES_TERMINAL
        COMMENT "Verifying Aura repository health"
    )
endif()
add_dependencies(aura-verify aura-test aura-test-native aura-test-sanitized aura-test-stress)

add_custom_target(aura ALL DEPENDS aura-check)
