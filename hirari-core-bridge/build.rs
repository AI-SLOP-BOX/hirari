use glob::glob;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn read_spirv_words(path: &Path) -> Result<Vec<u32>, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read generated SPIR-V {}: {error}", path.display()))?;
    if bytes.len() < 4 || bytes.len() % 4 != 0 {
        return Err(format!(
            "invalid SPIR-V size for {}: {} bytes (must be a non-zero multiple of 4)",
            path.display(),
            bytes.len()
        ));
    }
    let words = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    if words.first().copied() != Some(0x0723_0203) {
        return Err(format!("invalid SPIR-V magic in {}", path.display()));
    }
    Ok(words)
}

fn write_spirv_header(out_dir: &Path, vertex: &Path, fragment: &Path) -> Result<(), String> {
    let vertex_words = read_spirv_words(vertex)?;
    let fragment_words = read_spirv_words(fragment)?;

    let mut header = String::from(
        "#pragma once\n#include <cstdint>\n\nnamespace Hirari::Graphics::VulkanShaders {\n",
    );
    for (name, words) in [("kVertex", &vertex_words), ("kFragment", &fragment_words)] {
        writeln!(header, "inline constexpr uint32_t {name}[] = {{")
            .map_err(|error| format!("cannot format Vulkan shader header: {error}"))?;
        for (index, word) in words.iter().enumerate() {
            if index % 8 == 0 {
                header.push_str("    ");
            }
            write!(header, "0x{word:08x}u, ")
                .map_err(|error| format!("cannot format Vulkan shader word: {error}"))?;
            if index % 8 == 7 {
                header.push('\n');
            }
        }
        header.push_str("\n};\n");
    }
    writeln!(
        header,
        "inline constexpr uint32_t kVertexWordCount = {}u;",
        vertex_words.len()
    )
    .map_err(|error| format!("cannot format Vulkan vertex count: {error}"))?;
    writeln!(
        header,
        "inline constexpr uint32_t kFragmentWordCount = {}u;",
        fragment_words.len()
    )
    .map_err(|error| format!("cannot format Vulkan fragment count: {error}"))?;
    header.push_str("}\n");
    fs::write(out_dir.join("hirari_vulkan_ui_spv.hpp"), header)
        .map_err(|error| format!("cannot write embedded Vulkan SPIR-V header: {error}"))?;
    Ok(())
}

fn shader_build_error(code: &str, message: impl std::fmt::Display) -> ! {
    panic!("HIRARI_SHADER_ERROR code={code}: {message}");
}

fn validate_vulkan_shaders(out_dir: &Path) {
    let shader_dir = Path::new("../src/graphics/shaders");
    let shaders = [
        ("vulkan_ui.vert", "vulkan_ui.vert.spv"),
        ("vulkan_ui.frag", "vulkan_ui.frag.spv"),
    ];
    let glslc = std::env::var_os("GLSLC").unwrap_or_else(|| "glslc".into());

    let mut outputs = Vec::with_capacity(shaders.len());
    for (source_name, output_name) in shaders {
        let source = shader_dir.join(source_name);
        let output = out_dir.join(output_name);
        let result = Command::new(&glslc)
            .arg("--target-env=vulkan1.2")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .status();
        match result {
            Ok(status) if status.success() => {
                println!("cargo:rerun-if-changed={}", source.display());
                outputs.push(output);
            }
            Ok(status) => shader_build_error(
                "GLSLC_FAILED",
                format!(
                    "backend=vulkan shader={} compiler={} failed with status {}; install/configure GLSLC or disable HIRARI_ENABLE_VULKAN",
                    source.display(), glslc.to_string_lossy(), status
                ),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                shader_build_error(
                    "GLSLC_UNAVAILABLE",
                    format!(
                        "backend=vulkan shader compiler={} was not found; install/configure GLSLC or disable HIRARI_ENABLE_VULKAN",
                        glslc.to_string_lossy()
                    ),
                );
            }
            Err(error) => shader_build_error(
                "GLSLC_UNAVAILABLE",
                format!(
                    "backend=vulkan shader={} compiler={} unavailable: {error}",
                    source.display(), glslc.to_string_lossy()
                ),
            ),
        }
    }
    if let Err(error) = write_spirv_header(out_dir, &outputs[0], &outputs[1]) {
        shader_build_error("SPIRV_INVALID", error);
    }
}

fn main() {
    let mut build = cxx_build::bridge("src/lib.rs");

    build.file("src/hirari_core_ref.cpp");
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
    let vulkan_enabled = std::env::var("HIRARI_ENABLE_VULKAN")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
        .unwrap_or(false);

    // --- INDUSTRIAL Dynamic File Discovery ---
    // The current engine/ translation units are an unreferenced mastering shim,
    // an unreferenced forensic shim, and a symbol-free track anchor. The
    // production Rust bridge owns the corresponding runtime code; do not pull
    // those stale C++ wrappers into every native build.
    let search_patterns = ["../src/core/*.cpp"];

    for pattern in &search_patterns {
        for path in glob(pattern)
            .expect("Failed to read glob pattern")
            .flatten()
        {
            build.file(&path);
            // Individual files are fine, but broad directory triggers are bloat
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    // cxx_build does not reliably discover headers included through nested
    // implementation fragments. Register them explicitly so changes to
    // routing, PDC, undo, engine contracts, and vendored DSP headers always
    // rebuild the bridge.
    for pattern in [
        "../src/core/**/*.hpp",
        "../src/core/**/*.inc",
        "../src/external/**/*.h",
        "../src/external/**/*.hpp",
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
        .include("../src/external")
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
        build.file("../src/core/driver/mac_audio_driver_host.mm");
        build.file("../src/core/driver/mac_midi_device_host.mm");
        build.file("../src/platform/macos/coreaudio_device.mm");
        build.file("../src/core/engine/video_system_bridge.mm");
    }

    if vulkan_enabled {
        let out_dir = PathBuf::from(
            std::env::var_os("OUT_DIR").expect("OUT_DIR is required for Vulkan shader validation"),
        );
        validate_vulkan_shaders(&out_dir);
        let vulkan_prefix = std::env::var_os("VULKAN_SDK")
            .map(PathBuf::from)
            .or_else(|| {
                [Path::new("/opt/homebrew"), Path::new("/usr/local")]
                    .iter()
                    .find(|prefix| prefix.join("include/vulkan/vulkan.h").is_file())
                    .map(|prefix| prefix.to_path_buf())
            });
        if let Some(prefix) = vulkan_prefix {
            let include_dir = prefix.join("include");
            if include_dir.join("vulkan/vulkan.h").is_file() {
                build.include(&include_dir);
                build.include(&out_dir);
                let lib_dir = prefix.join("lib");
                if lib_dir.is_dir() {
                    println!("cargo:rustc-link-search=native={}", lib_dir.display());
                }
            } else {
                shader_build_error(
                    "VULKAN_HEADERS_MISSING",
                    format!(
                        "HIRARI_ENABLE_VULKAN=1 but Vulkan headers were not found under {}",
                        prefix.display()
                    ),
                );
            }
        } else {
            shader_build_error(
                "VULKAN_SDK_MISSING",
                "HIRARI_ENABLE_VULKAN=1 but no Vulkan SDK was found; set VULKAN_SDK or install Vulkan headers",
            );
        }
        build
            .file("../src/graphics/platform/vulkan_kernel.cpp")
            .define("HIRARI_ENABLE_VULKAN", "1");
        if target_os == "macos" {
            build.file("../src/graphics/platform/vulkan_surface_macos.mm");
            println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_surface_macos.mm");
            println!("cargo:rustc-link-lib=framework=QuartzCore");
        } else if target_os == "windows" {
            build.file("../src/graphics/platform/vulkan_surface_windows.cpp");
            println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_surface_windows.cpp");
        } else if target_os == "linux" {
            build.file("../src/graphics/platform/vulkan_surface_linux.cpp");
            println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_surface_linux.cpp");
        }
        println!("cargo:rerun-if-env-changed=HIRARI_ENABLE_VULKAN");
        println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_kernel.cpp");
        println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_kernel.hpp");
        println!("cargo:rerun-if-changed=../src/graphics/platform/vulkan_surface.hpp");
        println!("cargo:rustc-link-lib=vulkan");
    }

    build.compile("hirari-core-bridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/hirari_core_ref.cpp");
    println!("cargo:rerun-if-changed=src/metal_audio_kernel.mm");
    println!("cargo:rerun-if-changed=../src/core/driver/mac_audio_driver_host.mm");
    println!("cargo:rerun-if-changed=../src/core/driver/mac_audio_driver_host.hpp");
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
