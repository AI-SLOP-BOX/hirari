//! Low-frequency AI, correlation and engine-event telemetry.

use crate::slint_ui::*;
use crate::ui::sync::replace_track;
use aura_core_bridge::ffi::BridgeEvent;
use aura_core_bridge::AuraCore;
use slint::Model;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static ASYNC_WAVEFORM_REQUESTS: OnceLock<Mutex<HashMap<u64, (u32, u32)>>> = OnceLock::new();

fn waveform_requests() -> &'static Mutex<HashMap<u64, (u32, u32)>> {
    ASYNC_WAVEFORM_REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn update_analysis_telemetry(
    ui: &AppWindow,
    core: &AuraCore,
    tracks: &slint::VecModel<Z_Track>,
    waveform_due: bool,
    ev_buffer: &mut Vec<BridgeEvent>,
) {
    // --- INDUSTRIAL TELEMETRY SYNC ---
    // Completed waveform decodes are consumed here, never on the UI event
    // handler that opened the project. A request ID is mapped back to its
    // track/clip only after the worker has published a complete peak vector.
    let completed_requests: Vec<(u64, u32, u32)> = waveform_requests()
        .lock()
        .map(|requests| {
            requests
                .iter()
                .map(|(id, &(track, clip))| (*id, track, clip))
                .collect()
        })
        .unwrap_or_default();
    for (request, track_id, clip_id) in completed_requests {
        if core.region_waveform_pending(request) {
            continue;
        }
        let waveform = core.poll_region_waveform(request);
        if !waveform.is_empty() {
            for index in 0..tracks.row_count() {
                let Some(mut track) = tracks.row_data(index) else {
                    continue;
                };
                if track.id as u32 != track_id {
                    continue;
                }
                let mut clips: Vec<Z_Clip> = track.clips.iter().collect();
                if let Some(clip) = clips.iter_mut().find(|clip| clip.id as u32 == clip_id) {
                    clip.points = slint::ModelRc::new(slint::VecModel::from(waveform.clone()));
                    track.clips = slint::ModelRc::new(slint::VecModel::from(clips));
                    replace_track(tracks, index, track);
                }
                break;
            }
        }
        if let Ok(mut requests) = waveform_requests().lock() {
            requests.remove(&request);
        }
    }

    if waveform_due {
        // Queue missing clip peaks. The worker performs decode/peak extraction;
        // this loop only schedules bounded requests and remains responsive.
        for track_index in 0..tracks.row_count() {
            let Some(track) = tracks.row_data(track_index) else {
                continue;
            };
            for clip in track.clips.iter() {
                if clip.points.row_count() != 0 {
                    continue;
                }
                let already_queued = waveform_requests()
                    .lock()
                    .map(|requests| {
                        requests.values().any(|&(track_id, clip_id)| {
                            track_id == track.id as u32 && clip_id == clip.id as u32
                        })
                    })
                    .unwrap_or(true);
                if already_queued {
                    continue;
                }
                let request = core.queue_region_waveform(track.id as u32, clip.id as u32);
                if request != 0 {
                    if let Ok(mut requests) = waveform_requests().lock() {
                        requests.insert(request, (track.id as u32, clip.id as u32));
                    }
                }
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
            // The track model can be replaced by a project/recovery callback
            // between row_count() and this pass.  Treat a vanished row as a
            // normal synchronization race instead of crashing the UI thread.
            let Some(mut trk) = tracks.row_data(i) else {
                continue;
            };
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
