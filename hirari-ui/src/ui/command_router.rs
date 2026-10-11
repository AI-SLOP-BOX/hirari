use hirari_core_bridge::HirariCore;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::slint_ui::{AppWindow, CommandActions, MidiActions, Z_Track};
use slint::{ComponentHandle, Model, VecModel};

pub(crate) fn install(
    ui: &AppWindow,
    core: Rc<HirariCore>,
    tracks: Rc<VecModel<Z_Track>>,
    last_saved_path: Rc<RefCell<Option<String>>>,
    persisted_snapshot: Rc<Cell<Option<u64>>>,
    session_recovery_path: Rc<RefCell<Option<std::path::PathBuf>>>,
    tone_started_ms: Arc<AtomicU64>,
    tone_baseline: Arc<AtomicU64>,
    operation_gate: crate::ui::operation_gate::OperationGate,
    project_save_queue: Rc<crate::ui::project_save_queue::ProjectSaveQueue>,
    dawproject_import_queue: Arc<crate::ui::dawproject_import_queue::DawProjectImportQueue>,
) {
    let weak = ui.as_weak();
    // The palette handler is a `move` closure and captures `tracks`, so the
    // demo arrangement takes its own clone before that happens.
    let write_demo_tracks = tracks.clone();
    let write_demo_ui = ui.as_weak();
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
                let deferred_recovery_path = session_recovery_path.clone();
                let deferred_gate = operation_gate.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(1), move || {
                    crate::ui::templates::handle_command(
                        &deferred_command,
                        &deferred_core,
                        &deferred_ui,
                        &deferred_tracks,
                        &deferred_path,
                        &deferred_recovery_path,
                        &deferred_gate,
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
                &persisted_snapshot,
                &operation_gate,
                &project_save_queue,
                &dawproject_import_queue,
            ) || crate::ui::midi_commands::handle_command(command, &core, &ui)
                || crate::ui::recording_commands::handle_command(command, &core, &ui, &tracks)
                || crate::ui::plugin_commands::handle_command(command, &core, &ui, &tracks)
                || crate::ui::special_commands::handle_command(command, &core, &ui, &tracks)
                || crate::ui::editing_commands::handle_command(
                    command,
                    &core,
                    &ui,
                    &tracks,
                    &last_saved_path,
                    &operation_gate,
                )
                || crate::ui::articulation_commands::handle_command(command, &core, &tracks)
                || crate::ui::templates::handle_command(
                    command,
                    &core,
                    &ui,
                    &tracks,
                    &last_saved_path,
                    &session_recovery_path,
                    &operation_gate,
                );
            if !handled {
                ui.set_last_action(format!("COMMAND NOT AVAILABLE: {command}").into());
            }
        });

    // The demo arrangement is data, not layout: sixty add_note calls in the
    // Slint part made the toolbar file hit the repository's 600-line part
    // limit and blocked every other change to it. Cloned up front because the
    // palette handler below is a `move` closure and takes `tracks` with it.
    ui.global::<CommandActions>().on_write_demo(move || {
        if let Some(ui) = write_demo_ui.upgrade() {
            // `global::<T>()` hands back the global by value, so it has to be
            // bound before the arrangement helper can borrow it.
            let midi = ui.global::<MidiActions>();
            write_demo_arrangement(&midi, &write_demo_tracks);
        }
    });
}

/// Seeds the four-track demo arrangement.
fn write_demo_arrangement(midi: &MidiActions, tracks: &Rc<VecModel<Z_Track>>) {
    let seed = |track_index: usize, notes: &[(i32, f32, f32, i32)]| {
        let Some(track) = tracks.row_data(track_index) else {
            return;
        };
        for (pitch, beat, length, velocity) in notes {
            midi.invoke_add_note(track.id, *pitch, *beat, *length, *velocity);
        }
    };
    seed(
        0,
        &[
            (60, 0.0, 4.0, 100),
            (64, 4.0, 4.0, 96),
            (67, 8.0, 4.0, 104),
            (72, 12.0, 4.0, 100),
        ],
    );
    seed(
        1,
        &[
            (36, 0.0, 2.0, 108),
            (36, 4.0, 2.0, 104),
            (43, 8.0, 2.0, 106),
            (41, 12.0, 2.0, 104),
            (45, 16.0, 2.0, 106),
            (43, 20.0, 2.0, 104),
            (40, 24.0, 2.0, 106),
            (43, 25.0, 2.0, 104),
        ],
    );
    seed(
        2,
        &[
            (48, 0.0, 4.0, 86),
            (53, 4.0, 4.0, 86),
            (55, 8.0, 4.0, 86),
            (50, 12.0, 4.0, 86),
        ],
    );
    seed(
        3,
        &[
            (42, 13.0, 0.5, 76),
            (38, 14.0, 0.5, 110),
            (42, 15.0, 0.5, 76),
            (36, 16.0, 0.5, 118),
            (42, 17.0, 0.5, 76),
            (38, 18.0, 0.5, 110),
            (42, 19.0, 0.5, 76),
            (36, 20.0, 0.5, 118),
            (42, 21.0, 0.5, 76),
            (38, 22.0, 0.5, 110),
            (42, 23.0, 0.5, 76),
            (36, 24.0, 0.5, 118),
            (42, 25.0, 0.5, 76),
            (38, 26.0, 0.5, 110),
        ],
    );
}

// Keep the test-only environment hook out of production binaries entirely.
// Besides reducing attack surface, this lets release-bundle verification
// prove that debug smoke entry points were not linked into the shipped app.
#[cfg(debug_assertions)]
fn ui_smoke_mode() -> bool {
    std::env::var_os("HIRARI_UI_SMOKE").is_some()
}

#[cfg(not(debug_assertions))]
fn ui_smoke_mode() -> bool {
    false
}
