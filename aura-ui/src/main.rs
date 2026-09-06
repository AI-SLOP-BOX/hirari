pub mod orchestrator;
pub mod slint_ui;
pub mod ui;

fn configure_plugin_worker() -> bool {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join("aura-plugin-host-worker"));
            candidates.push(
                parent
                    .join("../..")
                    .join("build-tools/aura-plugin-host-worker"),
            );
        }
    }
    candidates.push(std::path::PathBuf::from(
        "build-tools/aura-plugin-host-worker",
    ));
    if let Some(worker) = candidates
        .into_iter()
        .find(|path| is_executable_worker(path))
    {
        // Prefer the helper shipped beside the running app.  This avoids a
        // stale developer-shell variable silently selecting an older worker
        // after the app bundle has been updated.
        std::env::set_var("AURA_PLUGIN_HOST_BIN", worker);
        return true;
    }
    false
}

fn is_executable_worker(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn main() {
    let worker_configured = configure_plugin_worker();
    if std::env::var("AURA_HEADLESS").as_deref() == Ok("1") {
        if !worker_configured {
            eprintln!("AURA_HEADLESS_INIT_FAILED worker=missing");
            std::process::exit(1);
        }
        match aura_core_bridge::AuraCore::new() {
            Ok(core) => {
                let layout_valid =
                    serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
                        .is_ok_and(|value| value.is_array());
                let packaged_resources_present = std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
                    .and_then(|macos| macos.parent().map(std::path::Path::to_path_buf))
                    .is_some_and(|root| {
                        root.join("Resources").is_dir()
                            && root.join("Resources/aura-resources.manifest").is_file()
                    });
                // `cargo run` has no .app bundle, so use the checked-in UI
                // resources during developer/headless smoke runs. Packaged
                // builds still require the signed manifest above.
                let dev_resources_present = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("resources/fonts")
                    .is_dir();
                let resources_present = packaged_resources_present || dev_resources_present;
                let health = core.runtime_health_snapshot();
                let audio_generation = core.audio_config_generation();
                let sample_rate = core.get_sample_rate();
                let native_engine_ready = audio_generation > 0
                    && sample_rate.is_finite()
                    && (8_000.0..=384_000.0).contains(&sample_rate);
                let strict_audio =
                    std::env::var("AURA_REQUIRE_HARDWARE_DEVICE").as_deref() == Ok("1");
                if layout_valid
                    && resources_present
                    && native_engine_ready
                    && (!strict_audio || health.audio_device_ready)
                {
                    println!(
                        "AURA_HEADLESS_READY native_engine=ready bridge=ready project_layout=valid resources=ready audio_device_ready={} audio_driver={} audio_generation={} sample_rate={} runtime_health={}",
                        health.audio_device_ready,
                        health.audio_driver_status,
                        audio_generation,
                        sample_rate,
                        health.status_text()
                    );
                } else {
                    eprintln!(
                        "AURA_HEADLESS_INIT_FAILED layout_valid={} resources_present={} native_engine_ready={} audio_generation={} sample_rate={} audio_device_ready={} audio_driver={} strict_audio={}",
                        layout_valid, resources_present, native_engine_ready,
                        audio_generation, sample_rate, health.audio_device_ready,
                        health.audio_driver_status, strict_audio
                    );
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("AURA_HEADLESS_INIT_FAILED native_engine={error}");
                std::process::exit(1);
            }
        }
        return;
    }
    eprintln!("AURA | TRANSITIONING TO SLINT UI...");
    slint_ui::run();
}
