fn sync_sidecar_parent(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

const MAX_NATIVE_AUDIO_BLOCK: usize = 16_384;
// The shared sandbox mailbox has a separate, smaller wire contract. Keep it
// aligned with PluginSandboxProtocol::kMaxFrames rather than accidentally
// widening the Rust admission check beyond the IPC allocation.
const MAX_SANDBOX_AUDIO_BLOCK: usize = 8_192;

fn parse_plugin_instance_id(value: &str) -> Option<(u32, u32)> {
    // Keep the accepted persisted form strict without accepting arbitrary
    // path-like identifiers.
    let parts: Vec<_> = value.split(':').collect();
    if parts.len() == 4 && parts[0] == "track" && parts[2] == "slot" {
        Some((parts[1].parse().ok()?, parts[3].parse().ok()?))
    } else {
        None
    }
}

fn copy_file_atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> bool {
    if source == destination || !source.is_file() {
        return false;
    }
    let temporary = PathBuf::from(format!(
        "{}.copy-tmp-{}-{}",
        destination.display(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| {
        let mut input = std::fs::File::open(source)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        std::fs::rename(&temporary, destination)?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    sync_sidecar_parent(destination)
}

include!("aura_core_engine_midi_monitor.rs");
include!("aura_core_engine_midi_io.rs");
include!("aura_core_engine_comping.rs");
include!("aura_core_engine_transport.rs");
include!("aura_core_engine_control_room.rs");
include!("aura_core_engine_plugins.rs");
include!("aura_core_engine_automation.rs");
include!("aura_core_engine_preview.rs");
include!("aura_core_engine_position.rs");
include!("aura_core_engine_tracks.rs");
