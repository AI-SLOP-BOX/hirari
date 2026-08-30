use aura_core_bridge::AuraCore;
use slint::ComponentHandle;
use std::rc::Rc;

use crate::slint_ui::{AppWindow, TransportActions};

fn normalize_cycle(start: f32, end: f32) -> Option<(f32, f32)> {
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    let start = start.max(0.0);
    Some((start, end.max(start + 0.25)))
}

/// Transport-only action bindings. Keeping these callbacks together gives
/// playback, cycle and scrub one ownership boundary instead of mixing them
/// with track, editor and plugin mutations in `slint_ui::run`.
pub fn install(ui: &AppWindow, core: Rc<AuraCore>) {
    let weak = ui.as_weak();
    ui.global::<TransportActions>().on_toggle_play({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            let playing = !core.is_playing();
            let accepted = core.try_set_playing(playing);
            if let Some(ui) = weak.upgrade() {
                ui.set_is_ply(if accepted { playing } else { core.is_playing() });
                ui.set_last_action(if !accepted {
                    "TRANSPORT: AUDIO DEVICE NOT READY".into()
                } else if playing {
                    "TRANSPORT: PLAY".into()
                } else {
                    "TRANSPORT: STOP".into()
                });
            }
        }
    });

    ui.global::<TransportActions>().on_toggle_loop({
        let weak = weak.clone();
        let core = core.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let enabled = !ui.get_loop_active();
                ui.set_loop_active(enabled);
                core.set_loop(enabled);
                ui.set_last_action(if enabled { "LOOP: ON" } else { "LOOP: OFF" }.into());
            }
        }
    });

    ui.global::<TransportActions>().on_set_loop_enabled({
        let weak = weak.clone();
        let core = core.clone();
        move |enabled| {
            core.set_loop(enabled);
            if let Some(ui) = weak.upgrade() {
                ui.set_loop_active(enabled);
                ui.set_last_action(if enabled { "LOOP: ON" } else { "LOOP: OFF" }.into());
            }
        }
    });

    ui.global::<TransportActions>().on_scrub({
        let core = core.clone();
        move |beat| {
            let samples = core.beats_to_samples(beat.max(0.0) as f64);
            core.set_playhead(samples);
        }
    });

    ui.global::<TransportActions>().on_loop_changed({
        let weak = weak.clone();
        let core = core.clone();
        move |start, end| {
            let Some((start, end)) = normalize_cycle(start, end) else {
                return;
            };
            let start_sample = core.beats_to_samples(start as f64);
            let end_sample = core.beats_to_samples(end as f64);
            let cycle_result = core.set_cycle_range_diagnostic_json(start_sample, end_sample, true);
            let cycle_ok = serde_json::from_str::<serde_json::Value>(&cycle_result)
                .ok()
                .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
            if start_sample >= end_sample || !cycle_ok {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("CYCLE: REJECTED BY AUDIO ENGINE".into());
                }
                return;
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_loop_st(start);
                ui.set_loop_ed(end);
                ui.set_last_action(format!("CYCLE: {:.2}–{:.2} BEATS", start, end).into());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::normalize_cycle;

    #[test]
    fn cycle_range_is_clamped_and_keeps_minimum_length() {
        assert_eq!(normalize_cycle(-2.0, -1.0), Some((0.0, 0.25)));
        assert_eq!(normalize_cycle(4.0, 8.0), Some((4.0, 8.0)));
    }

    #[test]
    fn cycle_range_rejects_non_finite_input() {
        assert_eq!(normalize_cycle(f32::NAN, 1.0), None);
        assert_eq!(normalize_cycle(1.0, f32::INFINITY), None);
    }
}
