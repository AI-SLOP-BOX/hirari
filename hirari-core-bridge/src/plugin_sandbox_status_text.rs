use std::ffi::{c_char, CStr};

/// Status text for a plug-in sandbox failure code. These strings are part of
/// the native status surface: diagnostics, logs and project-hydration checks
/// match them verbatim, so the table lives beside the protocol constants.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_failure_name(code: u8) -> *const c_char {
    let name: &'static CStr = match code {
        0 => c"none",
        1 => c"missing-helper",
        2 => c"invalid-plugin-path",
        3 => c"spawn-failed",
        4 => c"handshake-timeout",
        5 => c"plugin-load-failed",
        6 => c"plugin-abi-invalid",
        7 => c"plugin-instance-failed",
        8 => c"security-setup-failed",
        9 => c"plugin-format-unsupported",
        10 => c"plugin-quarantined",
        11 => c"process-exited",
        12 => c"process-hung",
        13 => c"process-failed",
        14 => c"unsupported-platform",
        _ => c"unknown-failure",
    };
    name.as_ptr()
}

/// Status text for the state-mailbox error code.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_state_error_name(code: u8) -> *const c_char {
    let name: &'static CStr = match code {
        0 => c"none",
        1 => c"state-oversize",
        2 => c"state-version-unsupported",
        3 => c"state-checksum-mismatch",
        4 => c"state-plugin-rejected",
        5 => c"state-unavailable",
        6 => c"state-busy",
        7 => c"state-timeout",
        _ => c"state-unknown-error",
    };
    name.as_ptr()
}

/// Failure codes that make a sandbox worker unusable. Mirrors
/// `PluginSandboxHost::Failure`.
const FAILURE_PLUGIN_QUARANTINED: u8 = 10;
const FAILURE_PROCESS_EXITED: u8 = 11;
const FAILURE_PROCESS_HUNG: u8 = 12;
const FAILURE_PROCESS_FAILED: u8 = 13;

/// Late blocks in a row after which the processor is quarantined.
const CONSECUTIVE_OVERRUN_LIMIT: u32 = 8;

/// Bit 0: latch the processor failure state.
const OUTCOME_LATCH_FAILURE: u32 = 1;
/// Bit 1: drop the input MIDI because no mailbox can deliver its output.
const OUTCOME_CLEAR_MIDI: u32 = 2;

/// Realtime classification for `ProcessSandboxProcessor::processBlock`.
///
/// A missing block is normal: it is the first half of the mailbox pipeline, so
/// `produced == false` alone never fails the processor. Only explicit worker
/// failures and a run of late blocks latch the failure state. When the mailbox
/// can no longer deliver output, input MIDI must not survive into a recovery
/// block.
#[no_mangle]
pub extern "C" fn hirari_plugin_sandbox_block_outcome(
    produced: bool,
    overruns: u32,
    failure_code: u8,
    failed: bool,
) -> u32 {
    let mut outcome = 0;
    if !failed
        && (overruns >= CONSECUTIVE_OVERRUN_LIMIT
            || (!produced && failure_code == FAILURE_PROCESS_FAILED))
    {
        outcome |= OUTCOME_LATCH_FAILURE;
    }
    if !produced
        && matches!(
            failure_code,
            FAILURE_PLUGIN_QUARANTINED
                | FAILURE_PROCESS_EXITED
                | FAILURE_PROCESS_HUNG
                | FAILURE_PROCESS_FAILED
        )
    {
        outcome |= OUTCOME_CLEAR_MIDI;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_mailbox_block_is_not_a_failure() {
        assert_eq!(hirari_plugin_sandbox_block_outcome(false, 0, 0, false), 0);
    }

    #[test]
    fn overrun_limit_latches_only_at_the_threshold() {
        assert_eq!(hirari_plugin_sandbox_block_outcome(true, 7, 0, false), 0);
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(true, 8, 0, false),
            OUTCOME_LATCH_FAILURE
        );
        assert_eq!(hirari_plugin_sandbox_block_outcome(true, 9, 0, true), 0);
    }

    #[test]
    fn process_failure_without_output_latches_and_drops_midi() {
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(false, 0, FAILURE_PROCESS_FAILED, false),
            OUTCOME_LATCH_FAILURE | OUTCOME_CLEAR_MIDI
        );
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(true, 0, FAILURE_PROCESS_FAILED, false),
            0
        );
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(false, 0, FAILURE_PROCESS_FAILED, true),
            OUTCOME_CLEAR_MIDI
        );
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(false, 0, FAILURE_PLUGIN_QUARANTINED, false),
            OUTCOME_CLEAR_MIDI
        );
        assert_eq!(
            hirari_plugin_sandbox_block_outcome(false, 0, FAILURE_PROCESS_HUNG, true),
            OUTCOME_CLEAR_MIDI
        );
    }

    #[test]
    fn every_failure_code_has_a_distinct_name() {
        let expected = [
            "none",
            "missing-helper",
            "invalid-plugin-path",
            "spawn-failed",
            "handshake-timeout",
            "plugin-load-failed",
            "plugin-abi-invalid",
            "plugin-instance-failed",
            "security-setup-failed",
            "plugin-format-unsupported",
            "plugin-quarantined",
            "process-exited",
            "process-hung",
            "process-failed",
            "unsupported-platform",
        ];
        for (code, name) in expected.iter().enumerate() {
            let actual = unsafe { CStr::from_ptr(hirari_plugin_sandbox_failure_name(code as u8)) };
            assert_eq!(actual.to_str().unwrap(), *name);
        }
        let unknown = unsafe { CStr::from_ptr(hirari_plugin_sandbox_failure_name(200)) };
        assert_eq!(unknown.to_str().unwrap(), "unknown-failure");
    }

    #[test]
    fn every_state_error_code_has_a_name() {
        let expected = [
            "none",
            "state-oversize",
            "state-version-unsupported",
            "state-checksum-mismatch",
            "state-plugin-rejected",
            "state-unavailable",
            "state-busy",
            "state-timeout",
        ];
        for (code, name) in expected.iter().enumerate() {
            let actual =
                unsafe { CStr::from_ptr(hirari_plugin_sandbox_state_error_name(code as u8)) };
            assert_eq!(actual.to_str().unwrap(), *name);
        }
        let unknown = unsafe { CStr::from_ptr(hirari_plugin_sandbox_state_error_name(200)) };
        assert_eq!(unknown.to_str().unwrap(), "state-unknown-error");
    }
}
