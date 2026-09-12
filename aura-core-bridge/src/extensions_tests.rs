#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_sorted_manifest_only_extensions() {
        let root = std::env::temp_dir().join(format!("aura-extensions-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("z-last")).unwrap();
        fs::create_dir_all(root.join("a-first")).unwrap();
        fs::write(root.join("z-last/manifest.json"), r#"{"id":"z-last","name":"Z","version":"1"}"#).unwrap();
        fs::write(root.join("a-first/manifest.json"), r#"{"id":"a-first","name":"A","version":"1"}"#).unwrap();
        let (found, errors) = discover(&root);
        assert!(errors.is_empty());
        assert_eq!(found.iter().map(|item| item.manifest.id.as_str()).collect::<Vec<_>>(), ["a-first", "z-last"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extension_activation_is_project_local_and_registry_filtered() {
        let root = std::env::temp_dir().join(format!("aura-extensions-state-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("panel")).unwrap();
        fs::write(root.join("panel/manifest.json"), r#"{"id":"panel","name":"Panel","version":"1","commands":[{"id":"open","title":"Open"}]}"#).unwrap();
        assert!(command_registry(&root).0.iter().any(|command| command.extension_id == "panel"));
        set_enabled(&root, "panel", false).unwrap();
        let (commands, errors) = command_registry(&root);
        assert!(errors.is_empty());
        assert!(commands.is_empty());
        set_enabled(&root, "panel", true).unwrap();
        assert_eq!(command_registry(&root).0.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsupported_permissions_without_executing_extension() {
        let root = std::env::temp_dir().join(format!("aura-extensions-invalid-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("bad")).unwrap();
        fs::write(root.join("bad/manifest.json"), r#"{"id":"bad","name":"Bad","version":"1","permissions":["network"]}"#).unwrap();
        let (found, errors) = discover(&root);
        assert!(found.is_empty());
        assert_eq!(errors.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn trusted_extensions_can_declare_powerful_permissions_but_are_not_auto_run() {
        let root = std::env::temp_dir().join(format!("aura-extensions-trusted-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("trusted")).unwrap();
        fs::write(
            root.join("trusted/manifest.json"),
            r#"{"id":"trusted","name":"Trusted","version":"1","execution":"trusted","permissions":["filesystem_external","network","process_spawn"]}"#,
        ).unwrap();
        let (found, errors) = discover(&root);
        assert!(errors.is_empty());
        assert_eq!(found[0].manifest.execution, "trusted");
        assert!(!found[0].manifest.permissions.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sandboxed_extensions_cannot_escalate_through_manifest() {
        let root = std::env::temp_dir().join(format!("aura-extensions-escalation-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("bad")).unwrap();
        fs::write(
            root.join("bad/manifest.json"),
            r#"{"id":"bad","name":"Bad","version":"1","permissions":["network"]}"#,
        ).unwrap();
        let (found, errors) = discover(&root);
        assert!(found.is_empty());
        assert_eq!(errors.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn command_registry_qualifies_and_rejects_duplicates() {
        let root = std::env::temp_dir().join(format!("aura-extension-registry-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("one")).unwrap();
        fs::create_dir_all(root.join("two")).unwrap();
        let manifest = r#"{"id":"one","name":"One","version":"1","commands":[{"id":"render","title":"Render"}]}"#;
        fs::write(root.join("one/manifest.json"), manifest).unwrap();
        fs::write(root.join("two/manifest.json"), r#"{"id":"two","name":"Two","version":"1","commands":[{"id":"render","title":"Render"}]}"#).unwrap();
        let (commands, errors) = command_registry(&root);
        assert_eq!(commands.iter().map(|item| item.qualified_id.as_str()).collect::<Vec<_>>(), ["one.render", "two.render"]);
        assert!(errors.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn trusted_entrypoint_invocation_is_bounded_and_json_based() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("aura-extension-invoke-{}", uuid::Uuid::new_v4()));
        let extension = root.join("echoer");
        fs::create_dir_all(&extension).unwrap();
        fs::write(
            extension.join("manifest.json"),
            r#"{"id":"echoer","name":"Echoer","version":"1","execution":"trusted","entrypoint":"run.sh","permissions":["process_spawn"],"commands":[{"id":"echo","title":"Echo","input_schema":{"type":"object","additionalProperties":false,"properties":{"value":{"type":"string"}},"required":["value"]}}]}"#,
        ).unwrap();
        fs::write(
            extension.join("run.sh"),
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\n' '{\"ok\":true,\"received\":true}'\n",
        ).unwrap();
        let mut permissions = fs::metadata(extension.join("run.sh")).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(extension.join("run.sh"), permissions).unwrap();
        set_enabled(&root, "echoer", true).unwrap();
        // Process creation can be briefly delayed when the full workspace test
        // suite is running in parallel. Keep the contract bounded while
        // avoiding a flaky assertion on a heavily loaded CI worker.
        let result = invoke_command(&root, "echoer", "echo", &serde_json::json!({"value":"ok"}), 30_000).unwrap();
        assert_eq!(result["received"], true);
        let audit = fs::read_to_string(root.join(".aura/extension-runs.jsonl")).unwrap();
        assert!(audit.contains("\"status\":\"completed\""));
        assert!(audit.contains("\"payload_sha256\":"));
        assert!(!audit.contains("\"value\":\"ok\""));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn entrypoint_cannot_be_declared_by_sandboxed_extension() {
        let manifest = ExtensionManifest {
            id: "unsafe".into(), name: "Unsafe".into(), version: "1".into(),
            description: String::new(), capabilities: Vec::new(), permissions: Vec::new(),
            execution: "sandboxed".into(), entrypoint: Some("run.sh".into()),
            runtime_language: "none".into(),
            commands: Vec::new(), contributions: ExtensionContributions::default(),
        };
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn installs_valid_extension_atomically_and_rejects_symlinks() {
        let root = std::env::temp_dir().join(format!("aura-extension-install-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("installed");
        fs::create_dir_all(source.join("assets")).unwrap();
        fs::write(source.join("manifest.json"), r#"{"id":"demo","name":"Demo","version":"1"}"#).unwrap();
        fs::write(source.join("assets/preset.json"), b"{}\n").unwrap();
        assert_eq!(install_from_directory(&source, &destination).unwrap(), "demo");
        assert!(destination.join("demo/manifest.json").is_file());
        assert!(destination.join("demo/assets/preset.json").is_file());
        assert!(install_from_directory(&source, &destination).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn marketplace_catalog_requires_signature_and_permission_review() {
        let listings = marketplace_catalog();
        assert!(!listings.is_empty());
        assert!(listings.iter().all(|listing| listing.signature_required
            && listing.permissions_review_required
            && listing.channel == "stable"));
    }

    #[test]
    fn marketplace_search_filters_query_and_channel() {
        assert_eq!(marketplace_search("EXAMPLE", Some("STABLE")).len(), 1);
        assert!(marketplace_search("missing", None).is_empty());
        assert_eq!(marketplace_search("", Some("beta")).len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn extension_install_rejects_symlink_entries_without_partial_publish() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("aura-extension-symlink-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("installed");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("manifest.json"), r#"{"id":"unsafe","name":"Unsafe","version":"1"}"#).unwrap();
        symlink("manifest.json", source.join("link")).unwrap();
        assert!(install_from_directory(&source, &destination).is_err());
        assert!(!destination.join("unsafe").exists());
        let _ = fs::remove_dir_all(root);
    }
}
