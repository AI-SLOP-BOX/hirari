/// Public JSON boundary for read-only audit consumers such as the CLI,
/// Codex, and other automation clients.
pub fn audit_log_json(path: impl AsRef<Path>) -> Result<serde_json::Value, BridgeError> {
    let ledger = RequestLedger::open(path)?;
    serde_json::to_value(ledger.audit_log())
        .map_err(|error| BridgeError::new("audit_log_encode_failed", error.to_string()))
}

fn validate_ledger_ids(request_id: &str, transaction_id: Option<&str>) -> Result<(), BridgeError> {
    if !valid_token(request_id, 128) {
        return Err(BridgeError::new("invalid_request_id", "invalid request ID"));
    }
    if transaction_id.is_some_and(|value| !valid_token(value, 128)) {
        return Err(BridgeError::new(
            "invalid_transaction_id",
            "invalid transaction ID",
        ));
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Validates a command against the generations observed by the caller.
/// Commands originating from an older UI/CLI snapshot must be rejected before
/// they reach native mutation; callers can then refresh and present a new
/// diff instead of applying edits to the wrong project.
pub fn validate_for_generations(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, String> {
    let command = validate(document)?;
    if let Some(expected) = command.expected_generation {
        if expected != current_project_generation {
            return Err(format!(
                "stale_project_generation: expected {expected}, current {current_project_generation}"
            ));
        }
    }
    if let Some(expected) = command.expected_audio_generation {
        if expected != current_audio_generation {
            return Err(format!(
                "stale_audio_generation: expected {expected}, current {current_audio_generation}"
            ));
        }
    }
    Ok(command)
}

/// Apply-time validation is stricter than dry-run validation.  A mutation
/// without the generations from the snapshot it was derived from is
/// inherently unsafe: it can target a project that changed between diff and
/// apply.  Read-only inspection is the sole exception.
pub fn validate_for_apply(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, String> {
    let command = validate_for_generations(
        document,
        current_project_generation,
        current_audio_generation,
    )?;
    let read_only = command.actions.len() == 1
        && matches!(
            command.actions[0],
            CommandAction::ControlInspect
                | CommandAction::AnalyzeDynamics { .. }
                | CommandAction::AnalyzeMix { .. }
                | CommandAction::AnalyzeSilence { .. }
                | CommandAction::PreviewVocalPitchCorrection { .. }
                | CommandAction::ProjectSearch { .. }
                | CommandAction::OpenUtauNotes { .. }
                | CommandAction::DiffMixSnapshots { .. }
                | CommandAction::RecallMixSnapshot { .. }
                | CommandAction::ExtensionCatalog { .. }
                | CommandAction::ExtensionValidate { .. }
                | CommandAction::ProjectInspect
                | CommandAction::RenderTargetCatalog
                | CommandAction::PluginCatalog
                | CommandAction::PluginSearch { .. }
        );
    if !read_only && command.expected_generation.is_none() {
        return Err(
            "missing_project_generation: apply commands must include the snapshot generation"
                .into(),
        );
    }
    if !read_only && command.expected_audio_generation.is_none() {
        return Err("missing_audio_generation: apply commands must include the audio configuration generation".into());
    }
    Ok(command)
}

/// Structured counterpart used by new adapters. The string-returning
/// function above remains for compatibility with existing CLI callers.
pub fn validate_for_generations_diagnostic(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, BridgeError> {
    let command =
        validate(document).map_err(|message| BridgeError::new("invalid_command", message))?;
    if let Some(expected) = command.expected_generation {
        if expected != current_project_generation {
            return Err(BridgeError::new(
                "stale_project_generation",
                format!("expected {expected}, current {current_project_generation}"),
            )
            .retryable(true)
            .at_generation(current_project_generation));
        }
    }
    if let Some(expected) = command.expected_audio_generation {
        if expected != current_audio_generation {
            return Err(BridgeError::new(
                "stale_audio_generation",
                format!("expected {expected}, current {current_audio_generation}"),
            )
            .retryable(true)
            .at_generation(current_audio_generation));
        }
    }
    Ok(command)
}

pub fn validate_for_apply_diagnostic(
    document: CommandDocument,
    current_project_generation: u64,
    current_audio_generation: u64,
) -> Result<ValidatedCommand, BridgeError> {
    let command = validate_for_generations_diagnostic(
        document,
        current_project_generation,
        current_audio_generation,
    )?;
    let read_only = command.actions.len() == 1
        && matches!(
            command.actions[0],
            CommandAction::ControlInspect
                | CommandAction::AnalyzeDynamics { .. }
                | CommandAction::AnalyzeMix { .. }
                | CommandAction::AnalyzeSilence { .. }
                | CommandAction::PreviewVocalPitchCorrection { .. }
                | CommandAction::ProjectSearch { .. }
                | CommandAction::ExtensionCatalog { .. }
                | CommandAction::ExtensionValidate { .. }
                | CommandAction::ProjectInspect
                | CommandAction::RenderTargetCatalog
                | CommandAction::PluginCatalog
        );
    if !read_only && command.expected_generation.is_none() {
        return Err(BridgeError::new(
            "missing_project_generation",
            "apply commands must include the snapshot generation",
        )
        .retryable(true)
        .at_generation(current_project_generation));
    }
    if !read_only && command.expected_audio_generation.is_none() {
        return Err(BridgeError::new(
            "missing_audio_generation",
            "apply commands must include the audio configuration generation",
        )
        .retryable(true)
        .at_generation(current_audio_generation));
    }
    Ok(command)
}


