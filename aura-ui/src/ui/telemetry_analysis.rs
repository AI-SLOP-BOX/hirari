//! Low-frequency AI, correlation and engine-event telemetry.

use crate::slint_ui::*;
use crate::ui::sync::replace_track;
use aura_core_bridge::ffi::BridgeEvent;
use aura_core_bridge::AuraCore;
use slint::Model;

pub(crate) fn update_analysis_telemetry(
    ui: &AppWindow,
    core: &AuraCore,
    tracks: &slint::VecModel<Z_Track>,
    waveform_due: bool,
    ev_buffer: &mut Vec<BridgeEvent>,
) {
    // --- INDUSTRIAL TELEMETRY SYNC ---
    if waveform_due {
        // Keep clip previews synchronized with decoded engine data.
        for track_index in 0..tracks.row_count() {
            let Some(mut track) = tracks.row_data(track_index) else {
                continue;
            };
            let mut clips: Vec<Z_Clip> = track.clips.iter().collect();
            let mut changed = false;
            for clip in &mut clips {
                let waveform = core.get_region_waveform(track.id as u32, clip.id as u32);
                if !waveform.is_empty() {
                    clip.points = slint::ModelRc::new(slint::VecModel::from(waveform));
                    changed = true;
                }
            }
            if changed {
                track.clips = slint::ModelRc::new(slint::VecModel::from(clips));
                replace_track(tracks, track_index, track);
            }
        }

        // 1. AI Mixing Advice
        let advice = core.get_ai_advice();
        let advice_rc: Vec<slint::SharedString> = advice.into_iter().map(|s| s.into()).collect();
        ui.set_ai_advice(slint::ModelRc::new(slint::VecModel::from(advice_rc)));

        // 2. Intelligence Dashboard (JSON)
        let json = core.get_intelligence_dashboard_json();
        if !json.is_empty() {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&json) {
                if let Some(adv) = data["advice"].as_str() {
                    ui.set_ai_advice_detail(slint::SharedString::from(adv));
                }
            }
        }

        // 3. Track Correlation
        for i in 0..tracks.row_count() {
            let mut trk = tracks.row_data(i).unwrap();
            trk.correlation = core.get_track_correlation(trk.id as u32);
            replace_track(tracks, i, trk);
        }

        // 4. Spectral Masking Heatmap
        let clashes = core.get_clashing_frequencies();
        let mut heatmap = vec![0.0f32; 100];
        for c in clashes {
            let band_idx = ((c.frequency - 100.0) / 5.0).clamp(0.0, 99.0) as usize;
            heatmap[band_idx] = (heatmap[band_idx] + c.severity).min(1.0);
        }
        ui.set_mask_heatmap(slint::ModelRc::new(slint::VecModel::from(heatmap)));

        // 5. Engine Event Polling (Queue draining - Zero Allocation Pass)
        core.poll_events_into(ev_buffer);
        for ev in ev_buffer.iter() {
            match ev.event_type {
                1 => ui
                    .set_last_action(format!("ENGINE EVENT: CLIP ON TRACK {}", ev.track_id).into()),
                2 => ui.set_last_action(
                    format!(
                        "ENGINE EVENT: AUTOMATION WRITE {} -> {}",
                        ev.track_id, ev.value
                    )
                    .into(),
                ),
                _ => {}
            }
        }
    }
}
