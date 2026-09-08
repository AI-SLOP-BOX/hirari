use aura_core_bridge::AuraCore;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::slint_ui::{ui_error_message, AppWindow, MixerActions, UiErrorKind, Z_Fx, Z_Track};
use crate::ui::sync::replace_track;

/// Channel-strip mutations. All mixer actions update the engine first where
/// required, then publish the accepted value to the Slint model.
pub fn install(ui: &AppWindow, core: Rc<AuraCore>, tracks: Rc<VecModel<Z_Track>>) {
    let weak = ui.as_weak();
    ui.global::<MixerActions>().on_toggle_fx({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |track_id, fx_index| {
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                if track.id != track_id || fx_index < 0 {
                    continue;
                }
                let mut effects: Vec<Z_Fx> = track.fx.iter().collect();
                let Some(effect) = effects.get_mut(fx_index as usize) else {
                    return;
                };
                let requested_active = !effect.active;
                let name = effect.name.clone();
                if !core.set_plugin_bypass(track_id as u32, fx_index as u32, !requested_active) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Plugin,
                                &format!("FX {} {} could not be bypassed", track_id, name),
                            )
                            .into(),
                        );
                    }
                    return;
                }
                effect.active = requested_active;
                let active = effect.active;
                track.fx = slint::ModelRc::new(VecModel::from(effects));
                replace_track(&tracks, row, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        format!(
                            "FX {} {}: {}",
                            track_id,
                            name,
                            if active { "ON" } else { "OFF" }
                        )
                        .into(),
                    );
                }
                break;
            }
        }
    });
    ui.global::<MixerActions>().on_reset_peaks({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                let peak_count = ui.get_pks().row_count();
                ui.set_pks(slint::ModelRc::new(VecModel::from(vec![0.0; peak_count])));
                let hold_count = ui.get_pks_h().row_count();
                ui.set_pks_h(slint::ModelRc::new(VecModel::from(vec![0.0; hold_count])));
                ui.set_last_action("PEAK METERS RESET".into());
            }
        }
    });

    ui.global::<MixerActions>().on_fader_changed({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, val| {
            if !val.is_finite() {
                return;
            }
            let bounded = val.clamp(0.0, 2.0);
            if id == 999 {
                if core.set_master_gain(bounded) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_master_output_gain(bounded);
                        ui.set_last_action(format!("MASTER LEVEL {:.0}%", bounded * 100.0).into());
                    }
                }
                return;
            }
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.volume = bounded;
                    core.set_track_fader(id as u32, bounded);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_pan_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, val| {
            if !val.is_finite() {
                return;
            }
            let bounded = val.clamp(-1.0, 1.0);
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    core.set_track_pan(id as u32, bounded);
                    track.pan = bounded;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_monitor({
        let weak = ui.as_weak();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            let Some(target_row) = (0..tracks.row_count())
                .find(|row| tracks.row_data(*row).is_some_and(|track| track.id == id))
            else {
                return;
            };
            let currently_enabled = tracks
                .row_data(target_row)
                .is_some_and(|track| track.monitor);
            let enabled = !currently_enabled;
            if !core.set_track_input_monitor(id as u32, enabled) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(
                        ui_error_message(
                            UiErrorKind::Engine,
                            &format!("Track {} input monitor could not be changed", id),
                        )
                        .into(),
                    );
                }
                return;
            }
            // The engine has one physical input monitor focus. Reflect that
            // exclusivity in the model when enabling a different track.
            for row in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(row) else {
                    continue;
                };
                let next = enabled && track.id == id;
                if track.monitor != next {
                    track.monitor = next;
                    replace_track(&tracks, row, track);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(
                    format!("MONITOR {}: {}", id, if enabled { "ON" } else { "OFF" }).into(),
                );
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_mute({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.mute = !track.mute;
                    core.set_mute(id as u32, track.mute);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_solo({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.solo = !track.solo;
                    core.set_solo(id as u32, track.solo);
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
    ui.global::<MixerActions>().on_toggle_phase({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move |id| {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id != id {
                    continue;
                }
                let inverted = !track.phase_invert;
                if !core.set_phase_invert(id as u32, inverted) {
                    if let Some(ui) = weak.upgrade() {
                        ui.set_last_action(
                            ui_error_message(
                                UiErrorKind::Engine,
                                "phase invert failed: track not found",
                            )
                            .into(),
                        );
                    }
                    return;
                }
                track.phase_invert = inverted;
                replace_track(&tracks, i, track);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if inverted {
                        "PHASE: INVERTED".into()
                    } else {
                        "PHASE: NORMAL".into()
                    });
                }
                break;
            }
        }
    });
    ui.global::<MixerActions>().on_reset_all_mute_solo({
        let weak = weak.clone();
        let core = core.clone();
        let tracks = tracks.clone();
        move || {
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.mute || track.solo {
                    track.mute = false;
                    track.solo = false;
                    core.set_mute(track.id as u32, false);
                    core.set_solo(track.id as u32, false);
                    replace_track(&tracks, i, track);
                }
            }
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action("RESET MUTE/SOLO".into());
            }
        }
    });
    ui.global::<MixerActions>().on_delay_changed({
        let core = core.clone();
        let tracks = tracks.clone();
        move |id, milliseconds| {
            if !milliseconds.is_finite() {
                return;
            }
            let bounded = milliseconds.clamp(0.0, 200.0);
            let sample_rate = core.get_sample_rate();
            if !sample_rate.is_finite() || sample_rate <= 0.0 {
                return;
            }
            let samples = ((bounded as f64 * sample_rate / 1000.0).round() as u32).min(8192);
            if !core.set_track_delay_samples(id as u32, samples) {
                return;
            }
            let accepted_ms = samples as f32 * 1000.0 / sample_rate as f32;
            for i in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(i) else {
                    continue;
                };
                if track.id == id {
                    track.delay_ms = accepted_ms;
                    replace_track(&tracks, i, track);
                    break;
                }
            }
        }
    });
}
