use aura_core_bridge::AuraCore;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::slint_ui::{AppWindow, CommandActions, Z_Track};
use slint::{ComponentHandle, VecModel};

pub fn install(
    ui: &AppWindow,
    core: Rc<AuraCore>,
    tracks: Rc<VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    tone_started_ms: Arc<AtomicU64>,
    tone_baseline: Arc<AtomicU64>,
    render_started_ms: Arc<AtomicU64>,
) {
    let weak = ui.as_weak();
    ui.global::<CommandActions>()
        .on_palette_exec(move |raw_command| {
            let Some(ui) = weak.upgrade() else { return };
            let command = raw_command.trim();
            ui.set_last_action(format!("AI EXEC: {command}").into());
            // Template creation mutates both the native graph and the bound
            // Slint model. Defer it one event-loop turn so the originating
            // TouchArea finishes its layout pass before the model changes.
            // This avoids a native macOS frame teardown that used to leave
            // only the chrome visible after selecting a template.
            if command.starts_with("INIT_") && !ui_smoke_mode() {
                let deferred_command = command.to_owned();
                let deferred_ui = ui.clone_strong();
                let deferred_core = core.clone();
                let deferred_tracks = tracks.clone();
                let deferred_path = last_saved_path.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(1), move || {
                    crate::ui::templates::handle_command(
                        &deferred_command,
                        &deferred_core,
                        &deferred_ui,
                        &deferred_tracks,
                        &deferred_path,
                    );
                });
                return;
            }
            let handled = crate::ui::diagnostics::handle_command(
                command,
                &core,
                &ui,
                &tone_started_ms,
                &tone_baseline,
            ) || crate::ui::project_commands::handle_command(
                command,
                &core,
                &ui,
                &tracks,
                &last_saved_path,
            ) || crate::ui::midi_commands::handle_command(command, &core, &ui)
                || crate::ui::recording_commands::handle_command(command, &core, &ui, &tracks)
                || crate::ui::plugin_commands::handle_command(command, &core, &ui, &tracks)
                || crate::ui::special_commands::handle_command(
                    command,
                    &core,
                    &ui,
                    &tracks,
                    &render_started_ms,
                )
                || crate::ui::editing_commands::handle_command(
                    command,
                    &core,
                    &ui,
                    &tracks,
                    &last_saved_path,
                )
                || crate::ui::articulation_commands::handle_command(command, &core, &tracks)
                || crate::ui::templates::handle_command(
                    command,
                    &core,
                    &ui,
                    &tracks,
                    &last_saved_path,
                );
            if !handled {
                ui.set_last_action(format!("COMMAND NOT AVAILABLE: {command}").into());
            }
        });
}

// Keep the test-only environment hook out of production binaries entirely.
// Besides reducing attack surface, this lets release-bundle verification
// prove that debug smoke entry points were not linked into the shipped app.
#[cfg(debug_assertions)]
fn ui_smoke_mode() -> bool {
    std::env::var_os("AURA_UI_SMOKE").is_some()
}

#[cfg(not(debug_assertions))]
fn ui_smoke_mode() -> bool {
    false
}
