use aura_core_bridge::AuraCore;
use slint::ComponentHandle;
use std::rc::Rc;

use crate::slint_ui::{AppWindow, StackActions};

/// Track Stack controls share the Core's persisted stack registry.  The first
/// stack is the default quick-start surface; the CLI can address additional
/// stack IDs without creating a second UI-only representation.
pub fn install(ui: &AppWindow, core: Rc<AuraCore>) {
    let weak = ui.as_weak();
    ui.global::<StackActions>().on_create_from_selected({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id| {
            let ok = track_id >= 0
                && core.upsert_track_stack(1, "Stack 1", &[track_id as u32], 1.0, false);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    if ok {
                        "TRACK STACK CREATED"
                    } else {
                        "TRACK STACK CREATE REJECTED"
                    }
                    .into(),
                );
            }
        }
    });
    ui.global::<StackActions>().on_add_selected({
        let weak = weak.clone();
        let core = core.clone();
        move |track_id| {
            let ok = track_id >= 0 && core.add_track_stack_member(1, track_id as u32);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    if ok {
                        "TRACK ADDED TO STACK"
                    } else {
                        "STACK MEMBER REJECTED"
                    }
                    .into(),
                );
            }
        }
    });
    ui.global::<StackActions>().on_delete_stack({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let ok = core.delete_track_stack(1);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    if ok {
                        "TRACK STACK DELETED"
                    } else {
                        "STACK DELETE REJECTED"
                    }
                    .into(),
                );
            }
        }
    });
    ui.global::<StackActions>().on_gain_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |gain| {
            let ok = core.set_track_stack_gain(1, gain);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    if ok {
                        format!("STACK LEVEL {:.0}%", gain * 100.0)
                    } else {
                        "STACK LEVEL REJECTED".to_owned()
                    }
                    .into(),
                );
            }
        }
    });
    ui.global::<StackActions>().on_collapsed_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |collapsed| {
            let ok = core.set_track_stack_collapsed(1, collapsed);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    if ok {
                        if collapsed {
                            "STACK COLLAPSED"
                        } else {
                            "STACK EXPANDED"
                        }
                    } else {
                        "STACK STATE REJECTED"
                    }
                    .into(),
                );
            }
        }
    });
}
