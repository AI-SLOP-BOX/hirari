include!("validation_rules/track_and_mixer.rs");
include!("validation_rules/regions_and_midi.rs");
include!("validation_rules/project_and_extensions.rs");

pub fn validate(document: CommandDocument) -> Result<ValidatedCommand, String> {
    let transaction = document.transaction.trim().to_owned();
    if document.schema_version != 1 || document.command_version != 1 {
        return Err("unsupported command schema or version".into());
    }
    if !valid_token(&transaction, 128) {
        return Err("transaction must be 1..=128 characters".into());
    }
    if document.actions.is_empty() || document.actions.len() > 256 {
        return Err("actions must contain 1..=256 entries".into());
    }
    for action in &document.actions {
        validate_track_and_mixer_action(action)?;
        validate_regions_and_midi_action(action)?;
        validate_project_and_extensions_action(action)?;
    }
    if document.permission == Permission::ReadOnly
        && document.actions.iter().any(|action| {
            !matches!(
                action,
                CommandAction::ControlInspect
                    | CommandAction::InspectMidiNotes
                    | CommandAction::InspectChordTrack
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
                    | CommandAction::TrackFreezeStatus { .. }
                    | CommandAction::DiffMixSnapshots { .. }
                    | CommandAction::RecallMixSnapshot { .. }
            )
        })
    {
        return Err("read_only permission allows project inspection only".into());
    }
    let mutation_class = document
        .actions
        .iter()
        .map(mutation_class)
        .max_by_key(|class| match class {
            MutationClass::ReadOnly => 0,
            MutationClass::Reversible => 1,
            MutationClass::Irreversible => 2,
            MutationClass::ExternalSideEffect => 3,
        })
        .unwrap_or(MutationClass::ReadOnly);
    let destructive = document.actions.iter().any(is_destructive);
    if mutation_class == MutationClass::ExternalSideEffect
        && !matches!(
            document.permission,
            Permission::SystemWrite | Permission::Unrestricted
        )
    {
        return Err("external side effects require system_write permission".into());
    }
    Ok(ValidatedCommand {
        schema_version: document.schema_version,
        command_version: document.command_version,
        transaction,
        permission: document.permission,
        expected_generation: document.expected_generation,
        expected_audio_generation: document.expected_audio_generation,
        actions: document.actions,
        destructive,
        mutation_class,
    })
}
