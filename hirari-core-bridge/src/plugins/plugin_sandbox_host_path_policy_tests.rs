//! Tests split out of plugin_sandbox_host.rs.
    use super::supported_bundle_path;

    #[test]
    fn bundles_are_recognised_case_insensitively() {
        assert!(supported_bundle_path(b"/Library/Audio/Plug-Ins/VST3/Test.VST3"));
        assert!(supported_bundle_path(b"/Library/Audio/Plug-Ins/Components/Test.component"));
        assert!(supported_bundle_path(b"plugin.clap"));
        assert!(supported_bundle_path(b"builtin://passthrough"));
        assert!(supported_bundle_path(b"BUILTIN://Passthrough"));
    }

    #[test]
    fn unsupported_or_malformed_paths_are_rejected() {
        assert!(!supported_bundle_path(b""));
        assert!(!supported_bundle_path(b"plugin.dll"));
        assert!(!supported_bundle_path(b"plugin.vst3\0trailing"));
        assert!(supported_bundle_path(b".vst3"));
        assert!(!supported_bundle_path(&vec![b'a'; 4097]));
    }
