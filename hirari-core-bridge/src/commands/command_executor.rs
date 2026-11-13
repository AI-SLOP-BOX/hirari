//! Native command execution kept behind the validated command boundary.
//!
//! This module is intentionally small and explicit.  Adding a new command
//! requires adding it to validation, the capability catalogue, and this
//! executor; an unknown action can never silently become a no-op.

use crate::bridge_error::BridgeError;
use crate::command_api::{CommandAction, MutationClass, ValidatedCommand};
use crate::HirariCore;
use serde_json::{json, Value};
#[path = "command_executor_dispatch.rs"]
mod dispatch;
use self::dispatch::execute_action;

#[derive(Debug, serde::Serialize)]
pub struct ExecutionReport {
    pub transaction: String,
    pub applied: usize,
    pub results: Vec<Value>,
    pub rolled_back: bool,
}

fn native_result(raw: String) -> Result<Value, BridgeError> {
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| BridgeError::new("invalid_native_result", error.to_string()))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    if let Some(error) = value.get("error") {
        if let Ok(error) = serde_json::from_value::<BridgeError>(error.clone()) {
            return Err(error);
        }
    }
    Err(BridgeError::new(
        value
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("native_command_rejected"),
        value
            .get("message")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("native command was rejected"),
    ))
}

fn search_snapshot(value: &Value, needle: &str, path: &str, hits: &mut Vec<Value>) {
    if hits.len() >= 512 {
        return;
    }
    match value {
        Value::String(text) if text.to_ascii_lowercase().contains(needle) => {
            hits.push(json!({"path": path, "value": text}));
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}/{key}");
                if key.to_ascii_lowercase().contains(needle) && hits.len() < 512 {
                    hits.push(json!({"path": child_path, "field": key, "value": child}));
                }
                search_snapshot(child, needle, &child_path, hits);
                if hits.len() >= 512 {
                    break;
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                search_snapshot(child, needle, &format!("{path}/{index}"), hits);
                if hits.len() >= 512 {
                    break;
                }
            }
        }
        _ => {}
    }
}

/// Apply a validated batch to an already-open native core.
///
/// The caller owns the project-wide transaction lock, request ledger, and
/// live-generation check.  This function only performs the native part and
/// compensates reversible mutations with the engine's undo stack if a later
/// action fails.  External side effects are intentionally accepted only as a
/// single-action transaction because they cannot be rolled back reliably.
pub fn execute(
    core: &HirariCore,
    command: &ValidatedCommand,
) -> Result<ExecutionReport, BridgeError> {
    if command.mutation_class == MutationClass::ExternalSideEffect && command.actions.len() != 1 {
        return Err(BridgeError::new(
            "external_action_must_be_isolated",
            "save and render side effects must be submitted as a single-action transaction",
        ));
    }

    let undo_before = core.undo_depth();
    let stack_snapshot_before = core.track_stacks_json();
    let marker_snapshot_before = core.markers_json();
    let extension_activation_before = command
        .actions
        .iter()
        .filter_map(|action| match action {
            CommandAction::ExtensionSetEnabled {
                root, extension_id, ..
            } => crate::extensions::enabled_state(root, extension_id)
                .map(|enabled| (root.clone(), extension_id.clone(), enabled)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut results = Vec::with_capacity(command.actions.len());
    for action in &command.actions {
        let result = execute_action(core, action);
        match result {
            Ok(value) => results.push(value),
            Err(error) => {
                let mut rolled_back = false;
                while core.undo_depth() > undo_before {
                    core.undo();
                    rolled_back = true;
                }
                let stack_rolled_back = core.restore_track_stacks_json(&stack_snapshot_before);
                let markers_rolled_back = core.restore_markers_json(&marker_snapshot_before);
                let mut extensions_rolled_back = true;
                for (root, extension_id, enabled) in &extension_activation_before {
                    if crate::extensions::set_enabled(root, extension_id, *enabled).is_err() {
                        extensions_rolled_back = false;
                    }
                }
                return Err(BridgeError::new(
                    "transaction_apply_failed",
                    format!("{}; compensating rollback attempted={} stack_restore={stack_rolled_back} marker_restore={markers_rolled_back} extension_restore={extensions_rolled_back}", error.message, rolled_back),
                ));
            }
        }
    }
    Ok(ExecutionReport {
        transaction: command.transaction.clone(),
        applied: results.len(),
        results,
        rolled_back: false,
    })
}
#[cfg(test)]
#[path = "command_executor_tests.rs"]
mod tests;
