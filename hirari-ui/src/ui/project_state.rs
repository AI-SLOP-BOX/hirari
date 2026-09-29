use hirari_core_bridge::HirariCore;
use slint::VecModel;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::slint_ui::Z_Track;

static STEP_SEQUENCER_STATE_REVISION: AtomicU64 = AtomicU64::new(1);
static UI_ROUTING_STATE_REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn mark_ui_routing_changed() {
    UI_ROUTING_STATE_REVISION.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn mark_step_sequencer_patterns_changed() {
    STEP_SEQUENCER_STATE_REVISION.fetch_add(1, Ordering::Relaxed);
}

fn absorb_bytes(fingerprint: &mut u64, bytes: &[u8]) {
    for byte in (bytes.len() as u64)
        .to_le_bytes()
        .into_iter()
        .chain(bytes.iter().copied())
    {
        *fingerprint ^= u64::from(byte);
        *fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
    }
}

/// Fingerprints persisted session state without including changing runtime
/// telemetry such as CPU load, callback counts, or device health.
pub(crate) fn save_fingerprint(
    core: &HirariCore,
    _tracks: &VecModel<Z_Track>,
    _step_sequencer_patterns_json: &str,
) -> Option<u64> {
    // Autosave only needs an in-process dirty token here. Exact command
    // generation checks remain content-based; polling that content hash every
    // five seconds forced an O(project size) native scan on the UI thread.
    let project_state_revision = core.project_state_revision().to_le_bytes();
    // Stable FNV-1a combines O(1) Core and UI mutation tokens; it is not an
    // integrity check.
    let mut fingerprint = 0xcbf2_9ce4_8422_2325u64;
    absorb_bytes(&mut fingerprint, b"project-state-revision");
    absorb_bytes(&mut fingerprint, &project_state_revision);
    absorb_bytes(&mut fingerprint, b"ui-routing-revision");
    absorb_bytes(
        &mut fingerprint,
        &UI_ROUTING_STATE_REVISION
            .load(Ordering::Relaxed)
            .to_le_bytes(),
    );
    absorb_bytes(&mut fingerprint, b"step-sequencer-revision");
    absorb_bytes(
        &mut fingerprint,
        &STEP_SEQUENCER_STATE_REVISION
            .load(Ordering::Relaxed)
            .to_le_bytes(),
    );
    Some(fingerprint)
}
