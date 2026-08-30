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
