use aura_core_bridge::AuraCore;
use slint::ComponentHandle;
use std::rc::Rc;

use crate::slint_ui::{AppWindow, TransportActions};

pub fn install(ui: &AppWindow, core: Rc<AuraCore>) {
    let weak = ui.as_weak();
    ui.global::<TransportActions>().on_set_bpm({
        let weak = weak.clone();
        let core = core.clone();
        move |bpm| {
            let ok = core.set_tempo(bpm);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("TEMPO: {:.2} BPM", bpm.clamp(20.0, 300.0)).into()
                } else {
                    "TEMPO REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_set_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |beat, bpm, ramp| {
            let ok = core.set_tempo_event(beat as f64, bpm as f64, ramp);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("TEMPO EVENT {:.3} BEAT / {:.2} BPM", beat, bpm).into()
                } else {
                    "TEMPO EVENT REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_remove_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |beat| {
            let ok = core.remove_tempo_event(beat as f64);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("REMOVED TEMPO EVENT @ {:.2} BEAT", beat).into()
                } else {
                    "TEMPO EVENT REMOVE REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_set_snap({
        let weak = weak.clone();
        move |value| {
            let division = match value.as_str() {
                "1/1" => 1.0,
                "1/4" => 4.0,
                "1/8" => 8.0,
                "1/8T" => 12.0,
                "1/16" => 16.0,
                "1/16T" => 24.0,
                "1/32" => 32.0,
                "1/64" => 64.0,
                "Adaptive" => 16.0,
                _ => return,
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_snap_division(division);
                ui.set_last_action(format!("SNAP: {}", value).into());
            }
        }
    });
    ui.global::<TransportActions>().on_move_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |from, to| {
            let ok = core.move_tempo_event(from as f64, to as f64);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("MOVED TEMPO EVENT {:.2} → {:.2} BEAT", from, to).into()
                } else {
                    "TEMPO EVENT MOVE REJECTED".into()
                });
            }
        }
    });
}
