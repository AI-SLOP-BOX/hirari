/// Execute one trusted extension command through the small Aura process
/// protocol. The executable receives a JSON object on stdin and must return a
/// JSON object on stdout. The runner is deliberately project-local, bounded,
/// and never available to sandboxed manifests.
pub fn invoke_command(
    root: impl AsRef<Path>,
    extension_id: &str,
    command_id: &str,
    payload: &serde_json::Value,
    timeout_ms: u64,
) -> Result<serde_json::Value, String> {
    if !valid_token(extension_id) || !valid_token(command_id) {
        return Err("extension command identity is invalid".into());
    }
    if !(1..=30_000).contains(&timeout_ms) {
        return Err("extension timeout must be within 1..=30000ms".into());
    }
    let (extensions, errors) = discover(root.as_ref());
    if !errors.is_empty() {
        return Err(format!("extension discovery failed: {}", errors.join("; ")));
    }
    let extension = extensions
        .into_iter()
        .find(|item| item.manifest.id == extension_id && item.enabled)
        .ok_or_else(|| "enabled extension is not installed under this root".to_owned())?;
    if extension.manifest.execution != "trusted"
        || !extension.manifest.permissions.iter().any(|permission| permission == "process_spawn")
    {
        return Err("extension execution requires trusted mode and process_spawn permission".into());
    }
    let command = extension
        .manifest
        .commands
        .iter()
        .find(|item| item.id == command_id)
        .ok_or_else(|| "extension command is not declared".to_owned())?;
    let registration = ExtensionCommandRegistration {
        extension_id: extension.manifest.id.clone(),
        command_id: command.id.clone(),
        qualified_id: format!("{extension_id}.{command_id}"),
        title: command.title.clone(),
        kind: command.kind.clone(),
        execution: extension.manifest.execution.clone(),
        permissions: extension.manifest.permissions.clone(),
        input_schema: command.input_schema.clone(),
        root: extension.root.clone(),
        entrypoint: extension.manifest.entrypoint.clone(),
        enabled: true,
    };
    validate_command_payload(&registration, payload)?;
    let entrypoint = registration.entrypoint.as_deref().ok_or_else(|| "trusted extension has no entrypoint".to_owned())?;
    let entrypoint_path = validate_entrypoint(&extension.root, entrypoint)?;

    let run_dir = root.as_ref().join(".aura").join("extension-runs");
    fs::create_dir_all(&run_dir).map_err(|error| format!("cannot create extension run directory: {error}"))?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let mut audit = ExtensionRunAudit::new(root.as_ref(), extension_id, command_id, &run_id, payload);
    let input_path = run_dir.join(format!("{run_id}.input"));
    let output_path = run_dir.join(format!("{run_id}.output"));
    let request = serde_json::json!({
        "protocol": "aura.extension.v1",
        "extension_id": extension_id,
        "command_id": command_id,
        "payload": payload,
    });
    {
        let mut file = fs::File::create(&input_path).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    let mut child = Command::new(&entrypoint_path)
        .current_dir(&extension.root)
        .stdin(fs::File::open(&input_path).map_err(|error| error.to_string())?)
        .stdout(fs::File::create(&output_path).map_err(|error| error.to_string())?)
        .stderr(Stdio::null())
        .env_clear()
        .env("AURA_EXTENSION_PROTOCOL", "aura.extension.v1")
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            let _ = fs::remove_file(&input_path);
            if !status.success() {
                let _ = fs::remove_file(&output_path);
                audit.finish("failed");
                return Err(format!("extension exited with status {status}"));
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&input_path);
            let _ = fs::remove_file(&output_path);
            audit.finish("timed_out");
            return Err("extension command timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let metadata = fs::metadata(&output_path).map_err(|error| error.to_string())?;
    if metadata.len() > 1_048_576 {
        let _ = fs::remove_file(&output_path);
        audit.finish("failed_output_limit");
        return Err("extension output exceeds the 1 MiB limit".into());
    }
    let output = fs::read_to_string(&output_path).map_err(|error| error.to_string())?;
    let _ = fs::remove_file(&output_path);
    let value: serde_json::Value = serde_json::from_str(&output).map_err(|error| {
        audit.finish("failed_invalid_json");
        format!("extension returned invalid JSON: {error}")
    })?;
    if !value.is_object() {
        audit.finish("failed_invalid_result");
        return Err("extension result must be a JSON object".into());
    }
    audit.finish("completed");
    Ok(value)
}

fn validate_entrypoint(root: &Path, entrypoint: &str) -> Result<PathBuf, String> {
    let relative = Path::new(entrypoint);
    if entrypoint.is_empty() || relative.is_absolute() || relative.components().any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) {
        return Err("extension entrypoint must be a relative path without traversal".into());
    }
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| format!("invalid extension entrypoint: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("extension entrypoint must be a regular non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("extension entrypoint is not executable".into());
        }
    }
    Ok(path)
}

/// Build a deterministic declarative UI registry.  IDs are qualified exactly
/// like commands, and every menu contribution must point at a command owned by
/// the same extension.  No extension code is executed here.
pub fn ui_registry(root: impl AsRef<Path>) -> (Vec<ExtensionUiRegistration>, Vec<String>) {
    let (extensions, mut errors) = discover(root);
    let mut registrations = Vec::new();
    for extension in extensions {
        if !extension.enabled { continue; }
        let command_ids: std::collections::HashSet<_> = extension
            .manifest
            .commands
            .iter()
            .map(|command| command.id.as_str())
            .collect();
        for panel in extension.manifest.contributions.panels {
            registrations.push(ExtensionUiRegistration {
                extension_id: extension.manifest.id.clone(),
                kind: "panel".to_owned(),
                id: format!("{}.{}", extension.manifest.id, panel.id),
                title: panel.title,
                location: if panel.location.is_empty() { "right".to_owned() } else { panel.location },
                command_id: None,
                icon: (!panel.icon.is_empty()).then_some(panel.icon),
            });
        }
        for menu in extension.manifest.contributions.menus {
            if !command_ids.contains(menu.command_id.as_str()) {
                errors.push(format!("menu contribution references unknown command: {}.{}", extension.manifest.id, menu.command_id));
                continue;
            }
            registrations.push(ExtensionUiRegistration {
                extension_id: extension.manifest.id.clone(),
                kind: "menu".to_owned(),
                id: format!("{}.{}", extension.manifest.id, menu.id),
                title: menu.title,
                location: if menu.menu.is_empty() { "view".to_owned() } else { menu.menu },
                command_id: Some(format!("{}.{}", extension.manifest.id, menu.command_id)),
                icon: None,
            });
        }
    }
    registrations.sort_by(|left, right| left.id.cmp(&right.id));
    (registrations, errors)
}

/// Validate an extension payload against its declared, intentionally small
/// JSON-schema subset before any command is admitted to the host boundary.
pub fn validate_command_payload(
    command: &ExtensionCommandRegistration,
    payload: &serde_json::Value,
) -> Result<(), String> {
    const MAX_PAYLOAD_BYTES: usize = 1_048_576;
    if serde_json::to_vec(payload).map_err(|_| "extension payload is not serializable")?.len()
        > MAX_PAYLOAD_BYTES
    {
        return Err("extension payload exceeds the 1 MiB limit".into());
    }
    let Some(object) = payload.as_object() else {
        return Err("extension payload must be a JSON object".into());
    };
    let schema = &command.input_schema;
    if schema.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err("extension input schema must describe an object".into());
    }
    if schema
        .get("additionalProperties")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        let properties = schema.get("properties").and_then(serde_json::Value::as_object);
        if object
            .keys()
            .any(|key| properties.is_none_or(|fields| !fields.contains_key(key)))
        {
            return Err("extension payload contains an unknown field".into());
        }
    }
    if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
        for field in required.iter().filter_map(serde_json::Value::as_str) {
            if !object.contains_key(field) {
                return Err(format!("extension payload is missing required field {field}"));
            }
        }
    }
    if let Some(properties) = schema.get("properties").and_then(serde_json::Value::as_object) {
        for (name, definition) in properties {
            let Some(value) = object.get(name) else { continue };
            let Some(expected) = definition.get("type").and_then(serde_json::Value::as_str) else { continue };
            let valid = match expected {
                "string" => value.is_string(),
                "number" => value.as_f64().is_some_and(f64::is_finite),
                "integer" => value.as_i64().is_some(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                "null" => value.is_null(),
                _ => false,
            };
            if !valid {
                return Err(format!("extension field {name} has the wrong type"));
            }
            if let Some(values) = definition.get("enum").and_then(serde_json::Value::as_array) {
                if !values.iter().any(|candidate| candidate == value) {
                    return Err(format!("extension field {name} is not an allowed value"));
                }
            }
            if let Some(length) = value.as_str().map(str::len) {
                if definition.get("minLength").and_then(serde_json::Value::as_u64)
                    .is_some_and(|minimum| (length as u64) < minimum)
                    || definition.get("maxLength").and_then(serde_json::Value::as_u64)
                        .is_some_and(|maximum| (length as u64) > maximum)
                {
                    return Err(format!("extension field {name} has an invalid string length"));
                }
            }
            if let Some(length) = value.as_array().map(Vec::len) {
                if definition.get("minItems").and_then(serde_json::Value::as_u64)
                    .is_some_and(|minimum| (length as u64) < minimum)
                    || definition.get("maxItems").and_then(serde_json::Value::as_u64)
                        .is_some_and(|maximum| (length as u64) > maximum)
                {
                    return Err(format!("extension field {name} has an invalid array length"));
                }
            }
            if let Some(number) = value.as_f64() {
                if !number.is_finite()
                    || definition.get("minimum").and_then(serde_json::Value::as_f64)
                        .is_some_and(|minimum| number < minimum)
                    || definition.get("maximum").and_then(serde_json::Value::as_f64)
                        .is_some_and(|maximum| number > maximum)
                {
                    return Err(format!("extension field {name} is outside its allowed range"));
                }
            }
        }
    }
    Ok(())
}
