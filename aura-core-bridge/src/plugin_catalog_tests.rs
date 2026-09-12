#[cfg(test)]
mod tests {
    use super::{
        binary_identity, content_fingerprint, instrument_profiles, normalized, recommended_plugins,
    };

    #[test]
    fn aliases_ignore_spaces_and_punctuation() {
        assert_eq!(normalized("Surge XT"), "surgext");
        assert_eq!(normalized("Vital.vst3"), "vitalvst3");
    }

    #[test]
    fn catalog_ids_include_format_and_binary_identity() {
        let a = content_fingerprint("/plugins/vital.clap");
        let b = content_fingerprint("/plugins/vital.vst3");
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn third_party_instrument_profiles_expose_midi_automation_and_state_contract() {
        let profiles = instrument_profiles();
        assert_eq!(profiles.len(), 2);
        for profile in profiles {
            assert!(profile.midi_input);
            assert_eq!(profile.midi_channels, 16);
            assert!(profile.parameter_automation);
            assert!(profile.state_save_restore);
            assert!(profile.formats.contains(&"clap".to_owned()));
            assert_eq!(profile.verification_status, "template_unverified");
        }
    }

    #[test]
    fn recommendations_are_stable_and_point_to_catalog_entries() {
        let recommendations = recommended_plugins();
        let entries = recommendations.as_array().unwrap();
        assert!(entries.len() >= 5);
        assert!(entries.iter().all(|entry| entry["id"].as_str().is_some()
            && entry["use"].as_str().is_some()
            && entry["reason"].as_str().is_some()));
    }

    #[test]
    fn binary_identity_changes_when_plugin_contents_change() {
        let root =
            std::env::temp_dir().join(format!("aura-plugin-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
        let binary = root.join("Contents/MacOS/plugin");
        std::fs::write(&binary, b"version-a").unwrap();
        let first = binary_identity(&root);
        std::fs::write(&binary, b"version-b-updated").unwrap();
        let second = binary_identity(&root);
        assert_ne!(first.0, second.0);
        assert_ne!(first.1, second.1);
        assert!(first.0.len() == 64 && second.0.len() == 64);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod collection_tests {
    use super::{PluginCollectionManager, DEFAULT_PLUGIN_COLLECTION_ID};

    #[test]
    fn default_collection_is_immutable_and_rebuilt_on_rescan() {
        let mut manager = PluginCollectionManager::new(["plug.b", "plug.a"]);
        assert!(manager.validate());
        assert_eq!(
            manager
                .active_entries(false)
                .iter()
                .map(|entry| entry.plugin_id.as_str())
                .collect::<Vec<_>>(),
            vec!["plug.a", "plug.b"]
        );
        assert!(!manager.rename(DEFAULT_PLUGIN_COLLECTION_ID, "Other"));
        assert!(!manager.delete(DEFAULT_PLUGIN_COLLECTION_ID));
        manager.rescan(["plug.c", "plug.a"]);
        assert_eq!(
            manager
                .active_entries(false)
                .iter()
                .map(|entry| entry.plugin_id.as_str())
                .collect::<Vec<_>>(),
            vec!["plug.a", "plug.c"]
        );
        assert!(manager.validate());
    }

    #[test]
    fn user_collection_supports_folders_copy_activation_and_unavailable_cleanup() {
        let mut manager = PluginCollectionManager::new(["synth", "eq", "compressor"]);
        let favorites = manager.create("Favorites", false).unwrap();
        assert!(manager.add_plugin(&favorites, "synth", &["Instruments".into()]));
        assert!(manager.add_plugin(&favorites, "eq", &["Mix".into(), "EQ".into()]));
        assert!(!manager.add_plugin(&favorites, "missing", &[]));
        assert!(manager.activate(&favorites));
        assert_eq!(manager.active_entries(false).len(), 2);

        let copied = manager.copy_collection(&favorites, "Studio A").unwrap();
        assert!(manager.activate(&copied));
        manager.rescan(["synth", "compressor"]);
        assert_eq!(manager.active_entries(true).len(), 2);
        assert_eq!(manager.active_entries(false).len(), 1);
        assert_eq!(manager.remove_unavailable_from_user_collections(), 2);
        assert_eq!(manager.active_entries(true).len(), 1);
        assert!(manager.validate());
    }

    #[test]
    fn deleting_active_user_collection_falls_back_to_default() {
        let mut manager = PluginCollectionManager::new(["limiter"]);
        let id = manager.create("Mastering", true).unwrap();
        assert!(manager.activate(&id));
        assert!(manager.delete(&id));
        assert_eq!(manager.active_id, DEFAULT_PLUGIN_COLLECTION_ID);
        assert!(manager.validate());
    }
}

#[cfg(test)]
mod manager_registry_tests {
    use super::*;

    fn plugin(id: &str, bitness: u8) -> PluginInspection {
        PluginInspection {
            id: id.into(),
            name: id.into(),
            vendor: "Vendor".into(),
            category: "Fx".into(),
            format: "VST3".into(),
            version: "1.0".into(),
            path: format!("/plugins/{id}.vst3"),
            bitness,
            supports_f64: true,
            asio_guard: true,
            sidechain_inputs: 1,
            latency_samples: 64,
            hidden: false,
            block_reason: None,
        }
    }

    #[test]
    fn failed_scan_enters_blocklist_and_successful_rescan_reactivates_64_bit() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("unstable", 64), false));
        assert_eq!(registry.blocklist().len(), 1);
        assert!(!registry.reactivate("unstable", false));
        assert!(registry.reactivate("unstable", true));
        assert_eq!(registry.available(None, false).len(), 1);
        assert!(registry.validate());
    }

    #[test]
    fn unsupported_32_bit_plugin_cannot_be_reactivated() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("legacy", 32), true));
        assert_eq!(
            registry.plugins[0].block_reason,
            Some(PluginBlockReason::Unsupported32Bit)
        );
        assert!(!registry.reactivate("legacy", true));
    }

    #[test]
    fn hidden_and_project_filters_affect_browser_but_not_report() {
        let mut registry = PluginManagerRegistry::default();
        assert!(registry.record_scan(plugin("eq", 64), true));
        assert!(registry.record_scan(plugin("compressor", 64), true));
        assert!(registry.set_hidden("eq", true));
        let used = BTreeSet::from(["compressor".to_owned()]);
        assert_eq!(registry.available(Some(&used), true)[0].id, "compressor");
        let report = registry.diagnostic_report("macOS test host").unwrap();
        assert!(report.contains("eq | Vendor"));
        assert!(report.contains("compressor | Vendor"));
    }
}
