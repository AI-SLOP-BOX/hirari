use crate::slint_ui::AppWindow;
use aura_core_bridge::AuraCore;

/// Production command-palette entry points for non-note MIDI editing.
/// Numeric defaults are intentionally conservative and can later be replaced
/// by the MIDI preferences model without changing the command contract.
pub fn handle_command(command: &str, core: &AuraCore, ui: &AppWindow) -> bool {
    match command.trim().to_ascii_uppercase().as_str() {
        "MIDI SWING" => {
            let ok = core.apply_midi_swing(0.5, 0.5);
            ui.set_last_action(if ok {
                "MIDI SWING APPLIED".into()
            } else {
                "MIDI SWING REJECTED".into()
            });
            true
        }
        "MIDI HUMANIZE" => {
            let ok = core.humanize_midi(0.02, 8, 1);
            ui.set_last_action(if ok {
                "MIDI HUMANIZE APPLIED".into()
            } else {
                "MIDI HUMANIZE REJECTED".into()
            });
            true
        }
        _ => false,
    }
}
