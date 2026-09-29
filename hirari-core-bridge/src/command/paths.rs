
/// Resolve a command-provided path without allowing traversal or symlink
/// escape from the project root. Nonexistent output files are checked using
/// their canonical parent, so this is safe for save and bounce destinations.
pub fn validate_project_path(project_root: &Path, requested: &str) -> Result<PathBuf, BridgeError> {
    validate_command_path(project_root, requested, PathPolicy::ProjectOnly)
}

/// Resolve a command path according to an explicit user-selected policy.
/// `ProjectOnly` is the default safe policy. `Unrestricted` is intentionally
/// opt-in for trusted local automation and preserves canonicalization and
/// basic path validation while allowing external paths and symlinks.
pub fn validate_command_path(
    project_root: &Path,
    requested: &str,
    policy: PathPolicy,
) -> Result<PathBuf, BridgeError> {
    if requested.trim().is_empty() || requested.len() > 4096 {
        return Err(BridgeError::new(
            "invalid_path",
            "path must be 1..=4096 characters",
        ));
    }
    let root = project_root
        .canonicalize()
        .map_err(|error| BridgeError::new("project_root_unavailable", error.to_string()))?;
    let candidate = Path::new(requested);
    if policy == PathPolicy::Unrestricted {
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            project_root.join(candidate)
        };
        return if joined.exists() {
            joined
                .canonicalize()
                .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))
        } else {
            let parent = joined.parent().unwrap_or_else(|| Path::new("."));
            let parent = parent
                .canonicalize()
                .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))?;
            Ok(parent.join(joined.file_name().unwrap_or_default()))
        };
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BridgeError::new(
            "path_traversal",
            "parent-directory components are not allowed",
        ));
    }
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let mut symlink_cursor = if candidate.is_absolute() {
        PathBuf::new()
    } else {
        root.clone()
    };
    for component in candidate.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        symlink_cursor.push(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&symlink_cursor) {
            if metadata.file_type().is_symlink() {
                return Err(BridgeError::new(
                    "symlink_path",
                    "symlink components are not allowed for command output paths",
                ));
            }
        }
    }
    let check_path = if joined.exists() {
        joined.canonicalize()
    } else {
        joined
            .parent()
            .unwrap_or(&root)
            .canonicalize()
            .map(|parent| parent.join(joined.file_name().unwrap_or_default()))
    }
    .map_err(|error| BridgeError::new("path_unavailable", error.to_string()))?;
    if !check_path.starts_with(&root) {
        return Err(BridgeError::new(
            "path_outside_project",
            "path must remain inside the project root",
        ));
    }
    let relative = check_path.strip_prefix(&root).unwrap_or(Path::new(""));
    let mut cursor = root.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        cursor.push(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&cursor) {
            if metadata.file_type().is_symlink() {
                return Err(BridgeError::new(
                    "symlink_path",
                    "symlink components are not allowed for command output paths",
                ));
            }
        }
    }
    Ok(check_path)
}

pub fn validate_project_output_path(
    project_root: &Path,
    requested: &str,
    allow_overwrite: bool,
) -> Result<PathBuf, BridgeError> {
    let path = validate_project_path(project_root, requested)?;
    if path.exists() && !allow_overwrite {
        return Err(BridgeError::new(
            "overwrite_confirmation_required",
            "existing output requires explicit overwrite confirmation",
        ));
    }
    Ok(path)
}

/// Validate every filesystem path carried by a command before any native
/// action is started. Keeping this at the command boundary prevents one
/// action in a multi-action transaction from bypassing the same root policy
/// used by the other actions.
pub fn validate_command_action_paths(
    actions: &[CommandAction],
    permission: Permission,
    project_root: &Path,
) -> Result<(), BridgeError> {
    let policy = if permission == Permission::Unrestricted {
        PathPolicy::Unrestricted
    } else {
        PathPolicy::ProjectOnly
    };
    for action in actions {
        if let CommandAction::InsertPluginPath { path, .. } = action {
            if permission != Permission::Unrestricted
                && !crate::plugin_catalog::is_admitted_path(path)
            {
                return Err(BridgeError::new(
                    "plugin_path_not_admitted",
                    "plugin path must refer to a discovered installed plugin",
                )
                .object(path));
            }
        }
        let paths: Vec<&str> = match action {
            CommandAction::ProjectLoad { path }
            | CommandAction::SaveProject { path }
            | CommandAction::BounceProject { path, .. } => vec![path],
            CommandAction::BounceStems { output_dir, .. } => vec![output_dir],
            CommandAction::RecordCommit {
                project_path: Some(path),
                ..
            } => vec![path],
            CommandAction::ExtensionCatalog { root } => vec![root],
            CommandAction::ExtensionValidate { root, .. } => vec![root],
            CommandAction::ExtensionInvoke { root, .. } => vec![root],
            CommandAction::ExtensionSetEnabled { root, .. } => vec![root],
            CommandAction::OpenUtauImport {
                source_path,
                rendered_audio_path,
                ..
            } => vec![source_path, rendered_audio_path],
            CommandAction::OpenUtauNotes { source_path } => vec![source_path],
            CommandAction::OpenUtauImportMidi { source_path, .. } => vec![source_path],
            CommandAction::FreezeTrack {
                path: Some(path), ..
            } => vec![path],
            CommandAction::AddAudioRegion { path, .. }
            | CommandAction::ReplaceRegionAudio { path, .. } => vec![path],
            _ => Vec::new(),
        };
        for requested in paths {
            validate_command_path(project_root, requested, policy)?;
        }
    }
    Ok(())
}

/// Persistent request ledger for at-least-once transports.
///
/// A request is durably marked `prepared` before native mutation starts. This
/// is deliberately fail-closed: if the process dies after mutating the
/// project but before publishing the result, a retry sees an in-flight entry
/// and is refused instead of applying the mutation twice. Recovery tooling can
/// inspect that entry and decide whether to reconcile the project snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerState {
    Prepared,
    Applying,
    Committed,
    Failed,
}


