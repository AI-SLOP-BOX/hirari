use glob::glob;

fn main() {
    let mut build = cxx_build::bridge("src/lib.rs");

    build.file("src/hirari_core_ref.cpp");
    build.file("src/analysis_bridge_accessors.cpp");
    println!("cargo:rerun-if-changed=src/analysis_bridge_accessors.cpp");
    if std::env::var_os("CARGO_FEATURE_DSP_DIFFERENTIAL_REFERENCE").is_some() {
        build.file("tests/dsp_reference/builtin_gain_reference.cpp");
        build.file("tests/dsp_reference/master_limiter_reference.cpp");
        build.file("tests/dsp_reference/master_output_reference.cpp");
        build.file("tests/dsp_reference/control_room_audio_reference.cpp");
        build.file("tests/dsp_reference/bus_audio_reference.cpp");
        build.file("tests/dsp_reference/sampler_reference.cpp");
        build.file("tests/dsp_reference/poly_sampler_reference.cpp");
        build.file("tests/dsp_reference/builtin_midi_reference.cpp");
        build.file("tests/dsp_reference/automation_reference.cpp");
        build.file("tests/dsp_reference/midi_buffer_reference.cpp");
        build.file("tests/dsp_reference/spectral_reference.cpp");
        build.file("tests/dsp_reference/region_gain_reference.cpp");
        build.file("tests/dsp_reference/region_resampler_reference.cpp");
        build.file("tests/dsp_reference/wsola_reference.cpp");
        build.file("tests/dsp_reference/region_warp_reference.cpp");
        build.file("tests/dsp_reference/audio_note_curve_reference.cpp");
        build.file("tests/dsp_reference/region_pitch_correction_reference.cpp");
        build.file("tests/dsp_reference/region_time_stretch_reference.cpp");
        build.file("tests/dsp_reference/analysis_reference.cpp");
        build.file("tests/dsp_reference/masking_analysis_reference.cpp");
        build.file("tests/dsp_reference/stereo_dsp_reference.cpp");
        build.file("tests/dsp_reference/track_channel_eq_reference.cpp");
        build.file("tests/dsp_reference/track_pdc_delay_reference.cpp");
        build.file("tests/dsp_reference/track_holographic_panner_reference.cpp");
        build.file("tests/dsp_reference/effect_chain_reference.cpp");
        build.file("tests/dsp_reference/preview_synth_reference.cpp");
        build.file("tests/dsp_reference/stem_splitter_reference.cpp");
        build.file("tests/dsp_reference/deesser_reference.cpp");
        build.file("tests/dsp_reference/transient_shaper_reference.cpp");
        build.file("tests/dsp_reference/chromaglow_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/sampler_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/builtin_gain_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/master_limiter_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/master_output_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/control_room_audio_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/bus_audio_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/poly_sampler_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/builtin_midi_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/automation_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/midi_buffer_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/spectral_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/region_gain_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/region_resampler_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/wsola_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/region_warp_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/audio_note_curve_reference.cpp");
        println!(
            "cargo:rerun-if-changed=tests/dsp_reference/region_pitch_correction_reference.cpp"
        );
        println!("cargo:rerun-if-changed=tests/dsp_reference/region_time_stretch_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/analysis_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/masking_analysis_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/stereo_dsp_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/track_channel_eq_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/track_pdc_delay_reference.cpp");
        println!(
            "cargo:rerun-if-changed=tests/dsp_reference/track_holographic_panner_reference.cpp"
        );
        println!("cargo:rerun-if-changed=tests/dsp_reference/effect_chain_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/preview_synth_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/stem_splitter_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/deesser_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/transient_shaper_reference.cpp");
        println!("cargo:rerun-if-changed=tests/dsp_reference/chromaglow_reference.cpp");
    }

    let target_arch =
        std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".to_string());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // --- INDUSTRIAL Dynamic File Discovery ---
    // The current engine translation units are the production Rust bridge's
    // C++ build. Discovering the source set with a glob is not enough: Cargo
    // drops per-file `rerun-if-changed` entries when a file is deleted, so a
    // source-set change (add/remove/rename of a .cpp) left a stale archive
    // behind for months (the original invalidation bug). Registering the
    // directory makes any mtime change inside the tree re-run the build
    // script, which re-globs and re-archives the full source set.
    let search_patterns = ["../src/core/*.cpp"];

    for pattern in &search_patterns {
        for path in glob(pattern)
            .expect("Failed to read glob pattern")
            .flatten()
        {
            build.file(&path);
        }
        println!("cargo:rerun-if-changed={}", pattern.replace("*.cpp", ""));
    }

    // cxx_build does not reliably discover headers included through nested
    // implementation fragments. Register them explicitly so changes to
    // routing, PDC, undo, engine contracts, and vendored DSP headers always
    // rebuild the bridge.
    for pattern in [
        "../src/core/**/*.hpp",
        "../src/core/**/*.inc",
        "../external/**/*.h",
        "../external/**/*.hpp",
    ] {
        for path in glob(pattern)
            .expect("failed to scan C++ dependency pattern")
            .flatten()
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    build
        .include("../src")
        .include("../src/core")
        .include("../external")
        .flag_if_supported("-std=c++20")
        .flag_if_supported("-fobjc-arc")
        .flag("-O3")
        .flag("-flto")
        .flag("-fomit-frame-pointer")
        .flag("-fstack-protector-strong")
        .flag("-DNDEBUG")
        .flag("-fvisibility=hidden")
        .flag("-fvisibility-inlines-hidden");

    // --- ARCHITECTURE-SPECIFIC SIMD OPTIMIZATION ---
    if target_arch == "aarch64"
        && target_os == "macos"
        && std::env::var_os("HIRARI_ENABLE_APPLE_M1").is_some()
    {
        // Keep cross-compilation and non-Apple ARM builds portable. The
        // machine-specific tuning is opt-in for deployments known to target M1.
        build.flag("-mcpu=apple-m1");
    } else if target_arch == "x86_64" && std::env::var_os("HIRARI_ENABLE_AVX2").is_some() {
        // AVX2/FMA are opt-in: compiling them unconditionally makes the binary
        // crash with SIGILL on otherwise valid x86_64 machines.
        build.flag("-mavx2").flag("-mfma");
    }

    if target_os == "macos" {
        build.file("src/metal_audio_kernel.mm");
        build.file("../src/dsp/spatial/metal_audio_kernel.mm");
        build.file("../src/platform/macos/coreaudio_device.mm");
        build.file("../src/core/engine/video_system_bridge.mm");
    }

    // The plugin sandbox host lives in its own cxx bridge so its generated
    // header is self-contained (it pulls in no engine headers), which lets the
    // header-only C++ facade include it from plugin code without cycles.
    let mut sandbox_build = cxx_build::bridge("src/plugin_sandbox_host.rs");
    sandbox_build
        .include("../src")
        .include("../src/core")
        .include("../external")
        .flag_if_supported("-std=c++20")
        .flag_if_supported("-fobjc-arc")
        .flag("-O3")
        .flag("-flto")
        .flag("-fomit-frame-pointer")
        .flag("-fstack-protector-strong")
        .flag("-DNDEBUG")
        .flag("-fvisibility=hidden")
        .flag("-fvisibility-inlines-hidden");
    if target_arch == "aarch64"
        && target_os == "macos"
        && std::env::var_os("HIRARI_ENABLE_APPLE_M1").is_some()
    {
        sandbox_build.flag("-mcpu=apple-m1");
    } else if target_arch == "x86_64" && std::env::var_os("HIRARI_ENABLE_AVX2").is_some() {
        sandbox_build.flag("-mavx2").flag("-mfma");
    }
    sandbox_build.compile("hirari-core-bridge-sandbox");

    build.compile("hirari-core-bridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/plugin_sandbox_host.rs");
    println!("cargo:rerun-if-changed=src/hirari_core_ref.cpp");
    println!("cargo:rerun-if-changed=src/metal_audio_kernel.mm");
    // The spatial Metal kernel is a separate translation unit from the bridge
    // copy above. It was missing here, so editing it (or its header) silently
    // linked the previous object and left the process racing on the singleton
    // with no rebuild to pick up the fix.
    println!("cargo:rerun-if-changed=../src/dsp/spatial/metal_audio_kernel.mm");
    println!("cargo:rerun-if-changed=../src/dsp/spatial/metal_audio_kernel.hpp");
    println!("cargo:rerun-if-changed=../src/core/driver/mac_audio_driver_host.hpp");
    println!("cargo:rerun-if-changed=../src/core/driver/mac_midi_device_host.hpp");
    println!("cargo:rerun-if-changed=hirari-core-bridge/src/mac_audio_driver.rs");
    println!("cargo:rerun-if-changed=hirari-core-bridge/src/mac_midi_host.rs");
    println!("cargo:rerun-if-changed=../src/platform/audio_device.hpp");
    println!("cargo:rerun-if-changed=../src/platform/macos/coreaudio_device.mm");
    // The C++ translation units include these headers, so changes to the
    // bridge-facing engine path must invalidate the native build as well.
    println!("cargo:rerun-if-changed=../src/core/audio_engine.hpp");
    println!("cargo:rerun-if-changed=../src/core/hirari_unified_engine.hpp");
    println!("cargo:rerun-if-changed=../src/core/engine/track.hpp");
    println!("cargo:rerun-if-changed=../src/core/io/audio_decoder.hpp");
    println!("cargo:rerun-if-changed=../src/core/engine/undo_transaction_manager.hpp");
    println!("cargo:rerun-if-changed=../src/core/engine/video_system_bridge.hpp");
    println!("cargo:rerun-if-changed=../src/core/engine/video_system_bridge.mm");
    println!("cargo:rerun-if-changed=../src/core/engine/video_system.hpp");
    println!("cargo:rerun-if-changed=../src/dsp/mixing/channel_strip.hpp");

    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=Metal");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=CoreMIDI");
        println!("cargo:rustc-link-lib=framework=AudioToolbox");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
    }

    if target_os != "macos" {
        // Placeholders for non-mac acceleration frameworks (e.g. CUDA/OpenCL)
        println!(
            "cargo:warning=HIRARI | NON-MAC DETECTED: Falling back to AVX/SSE optimized CPU kernels."
        );
    }
}
