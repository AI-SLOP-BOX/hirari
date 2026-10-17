#[cfg(test)]
mod cli_render_probe_tests {
    use crate::project::ProjectDocument;
    use std::io::Write;

    fn temp_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hirari-cli-render-test-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ))
    }

    #[test]
    fn a_48khz_document_is_readable_by_the_same_loader_the_cli_uses() {
        // `hirari project init <proj> 48000` writes a valid document with
        // `sample_rate: 48000.0`. `render mix` and `render stems` both go
        // through `ProjectDocument::load`, which must accept it or the CLI
        // cannot render any project saved at a rate other than the engine's
        // hardcoded 44100.
        let path = temp_path("48k.hirari");
        let document =
            ProjectDocument::from_layout_json("Probe", 120.0, 48_000.0, "[]").expect("layout");
        document
            .save_atomic(path.to_str().unwrap())
            .expect("save 48k document");

        let loaded = ProjectDocument::load(path.to_str().unwrap());
        assert!(
            loaded.is_ok(),
            "a 48 kHz project document must load through ProjectDocument::load: {:?}",
            loaded.err()
        );
        assert_eq!(
            loaded.expect("load").sample_rate.round(),
            48_000.0,
            "the loaded document keeps its persisted sample rate"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_48khz_document_round_trips_without_the_layout_path_losing_rate() {
        let path = temp_path("48k-roundtrip.hirari");
        let mut document =
            ProjectDocument::from_layout_json("Probe", 120.0, 48_000.0, "[]").expect("layout");
        document.sample_rate = 48_000.0;
        document
            .save_atomic(path.to_str().unwrap())
            .expect("save");
        let bytes = std::fs::read(path.to_str().unwrap()).expect("read back");
        let mut handle = std::fs::File::create(path.to_str().unwrap()).expect("rewrite");
        handle.write_all(&bytes).expect("write");
        let reloaded = ProjectDocument::load(path.to_str().unwrap());
        assert!(
            reloaded.is_ok(),
            "round-trip load of a 48 kHz document: {:?}",
            reloaded.err()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_fresh_offline_core_can_be_reconfigured_to_a_projects_sample_rate() {
        // `AudioEngine::apply_config_locked` used to route every reconfigure
        // through the audio driver on macOS, and an offline/CLI/test engine
        // owns no device, so the reconfigure always failed and the void
        // `apply_config` wrapper discarded that failure. The engine then kept
        // its default 44100 while `load_project_v2` only compared rates, so
        // any project saved at another rate failed to load. Assert the
        // property that matters: reconfiguring an offline engine to a project
        // rate moves the engine to that rate.
        let core = crate::HirariCore::new_offline().expect("offline core initializes");
        let engine = core.engine.as_ref().expect("offline core owns an engine");

        assert!(
            !engine.is_playing(),
            "a fresh offline core must not be playing; a playing engine \
             rejects every reconfiguration and leaves the rate at its default"
        );
        let generation_before = engine.get_audio_config_generation();

        let applied = engine.try_apply_config(120.0, 48_000, engine.get_block_size());
        assert!(
            applied,
            "an offline engine with no audio device must accept a 48 kHz \
             reconfiguration, got rate {} block {}",
            engine.get_sample_rate(),
            engine.get_block_size()
        );
        assert!(
            engine.get_audio_config_generation() != generation_before,
            "a successful reconfiguration publishes a new audio config generation"
        );
        assert!(
            (engine.get_sample_rate() - 48_000.0).abs() < 0.5,
            "the offline engine reports its configured rate, got {}",
            engine.get_sample_rate()
        );
    }

    #[test]
    fn a_48khz_project_loads_into_an_offline_core() {
        let path = temp_path("48k-core.hirari");
        let document =
            ProjectDocument::from_layout_json("Probe", 120.0, 48_000.0, "[]").expect("layout");
        document
            .save_atomic(path.to_str().unwrap())
            .expect("save 48k document");

        let core = crate::HirariCore::new_offline().expect("offline core initializes");
        let loaded = core.load_project_v2(path.to_str().unwrap());
        assert!(
            loaded.is_ok(),
            "a 48 kHz project must load into a fresh offline core: {loaded:?}"
        );
        assert!(
            (core.get_sample_rate() - 48_000.0).abs() < 0.5,
            "after loading, the core runs at the project's rate, got {}",
            core.get_sample_rate()
        );

        let _ = std::fs::remove_file(path);
    }
}
