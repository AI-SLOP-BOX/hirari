//! Engine-side analysis reads that the deleted C++ analysis accessors used to
//! serve. Every store behind them — the telemetry double buffer, the master
//! meter core, the track table, the region store and the bounce session — is
//! Rust-owned, so these reads and the offline-target writes go straight to
//! those stores through the engine's store-handle getters instead of a C++
//! field-by-field copy across the bridge.

use crate::analysis_views;
use crate::audio_driver_host;
use crate::bounce_driver;
use crate::channel_strip;
use crate::engine_block_host::{
    HirariEngineTelemetry, hirari_engine_block_track_table, hirari_engine_telemetry_active_index,
    hirari_engine_telemetry_snapshot,
};
use crate::engine_track_table::{self, HirariTrackSnapshot};
use crate::ffi::{AudioEngine, BridgeLoudness, HirariUnifiedEngine};
use crate::harmonic_context;
use crate::layout_gather;
use crate::master_meter;
use crate::region_store;
use crate::tempo;
use std::ffi::c_void;

fn block_handle(core: &HirariUnifiedEngine) -> *const c_void {
    core.engine_block_state() as *const c_void
}

/// The deleted C++ accessors each kept their own retry budget against the
/// seqlock-style publication, so the reads preserve them per entry point.
fn snapshot_with_retries(core: &HirariUnifiedEngine, attempts: usize) -> Option<HirariEngineTelemetry> {
    let state = block_handle(core);
    for _ in 0..attempts {
        let index = unsafe { hirari_engine_telemetry_active_index(state) };
        let mut telemetry = HirariEngineTelemetry::default();
        if unsafe { hirari_engine_telemetry_snapshot(state, index, &mut telemetry) } {
            return Some(telemetry);
        }
    }
    None
}

/// The active block's 512-bin spectrum, or empty while the audio callback
/// holds the publication.
pub fn spectrum(core: &HirariUnifiedEngine) -> Vec<f32> {
    snapshot_with_retries(core, 1).map_or_else(Vec::new, |telemetry| telemetry.spectrum.to_vec())
}

/// Per-track peaks from the active telemetry block, retrying up to `attempts`
/// times before reporting none.
pub fn track_peaks_l(core: &HirariUnifiedEngine) -> Vec<f32> {
    peaks(core, true)
}

pub fn track_peaks_r(core: &HirariUnifiedEngine) -> Vec<f32> {
    peaks(core, false)
}

fn peaks(core: &HirariUnifiedEngine, left: bool) -> Vec<f32> {
    // The deleted C++ forwarders retried four times before giving up.
    for _ in 0..4 {
        let Some(telemetry) = snapshot_with_retries(core, 1) else {
            continue;
        };
        let count = (telemetry.count as usize).min(telemetry.peaks_left.len());
        let samples = if left {
            &telemetry.peaks_left[..count]
        } else {
            &telemetry.peaks_right[..count]
        };
        return samples.to_vec();
    }
    Vec::new()
}

/// The master meter's analysis view. The deleted C++ `MeterData` never
/// assigned its `correlation` field, so this read still reports 0.0 — UI
/// advice depends on that established behavior.
pub fn master_meter_data(core: &HirariUnifiedEngine) -> BridgeLoudness {
    let state = core.meter_state() as *const c_void;
    if state.is_null() {
        return BridgeLoudness {
            integrated: 0.0,
            short_term: 0.0,
            true_peak_l: 0.0,
            true_peak_r: 0.0,
            correlation: 0.0,
        };
    }
    let mut stats = [-70.0f32, -70.0, -70.0, -100.0];
    unsafe { master_meter::hirari_master_meter_analysis_stats(state, stats.as_mut_ptr()) };
    BridgeLoudness {
        integrated: stats[2],
        short_term: stats[0],
        true_peak_l: unsafe { master_meter::hirari_master_meter_get(state, 2) },
        true_peak_r: unsafe { master_meter::hirari_master_meter_get(state, 3) },
        correlation: 0.0,
    }
}

pub fn intelligence_dashboard_json(core: &HirariUnifiedEngine) -> String {
    let meter = master_meter_data(core);
    analysis_views::analysis_dashboard_json(
        meter.short_term,
        meter.integrated,
        meter.true_peak_l,
        meter.true_peak_r,
        meter.correlation,
    )
}

/// Sections over the published graph's region spans, rendered through the
/// pure presentation transforms in analysis_views.rs.
pub fn song_structure_json(engine: &AudioEngine, core: &HirariUnifiedEngine) -> String {
    let table = unsafe { hirari_engine_block_track_table(block_handle(core)) };
    let mut published = HirariTrackSnapshot::default();
    unsafe { engine_track_table::hirari_engine_track_snapshot(table, &mut published) };
    let tracks = if published.active_tracks.is_null() {
        &[]
    } else {
        unsafe {
            std::slice::from_raw_parts(published.active_tracks, published.track_count as usize)
        }
    };
    let mut region_starts = Vec::new();
    let mut max_len = 0u64;
    for track in tracks {
        if track.is_null() {
            continue;
        }
        let count = region_store::hirari_region_store_count(*track);
        for index in 0..count {
            let mut start = 0u64;
            let mut length = 0u64;
            if !unsafe {
                region_store::hirari_region_store_span_at(
                    *track,
                    index,
                    &mut start,
                    &mut length,
                    std::ptr::null_mut(),
                )
            } {
                continue;
            }
            region_starts.push(start);
            max_len = max_len.max(start.saturating_add(length));
        }
    }
    let sample_rate = core.get_sample_rate();
    if region_starts.is_empty() {
        region_starts.push(0);
        max_len = sample_rate as u64 * 30;
    }
    let mut tension = 0.5f32;
    let mut valence = 0.5f32;
    unsafe {
        harmonic_context::hirari_harmonic_context_get(
            &mut tension,
            &mut valence,
            std::ptr::null_mut(),
        )
    };
    let bpm = unsafe { tempo::hirari_tempo_map_get_current_bpm(engine.tempo_state() as *const c_void) };
    let sections = analysis_views::analysis_structure_sections(
        &region_starts,
        max_len,
        bpm as f64,
        sample_rate,
        tension,
        valence,
    );
    let mut kinds = Vec::with_capacity(sections.len());
    let mut starts = Vec::with_capacity(sections.len());
    let mut ends = Vec::with_capacity(sections.len());
    let mut energies = Vec::with_capacity(sections.len());
    let mut flows = Vec::with_capacity(sections.len());
    for section in sections {
        kinds.push(section.section_type);
        starts.push(section.start_sample);
        ends.push(section.end_sample);
        energies.push(section.energy_level);
        flows.push(section.narrative_flow_score);
    }
    analysis_views::analysis_song_structure_json(&kinds, &starts, &ends, &energies, &flows)
}

/// The 10-element UI health snapshot, index contract 0..9 unchanged from the
/// deleted C++ accessor.
pub fn runtime_telemetry(engine: &AudioEngine, core: &HirariUnifiedEngine) -> Vec<f32> {
    let state = block_handle(core);
    let mut telemetry = HirariEngineTelemetry::default();
    // The block publish is a seqlock-style handoff from the audio callback;
    // the deleted C++ copied with up to eight retries and reported the zeros
    // it started with if none succeeded.
    for _ in 0..8 {
        let index = unsafe { hirari_engine_telemetry_active_index(state) };
        if unsafe { hirari_engine_telemetry_snapshot(state, index, &mut telemetry) } {
            break;
        }
    }
    let peak = |samples: &[f32]| -> f32 {
        samples
            .iter()
            .filter(|sample| sample.is_finite())
            .fold(0.0f32, |best, sample| best.max(sample.abs()))
    };
    let device_running =
        audio_driver_host::hirari_audio_driver_host_is_running(engine.audio_driver_host() as *mut c_void);
    vec![
        telemetry.dsp_load,
        peak(&telemetry.peaks_left),
        peak(&telemetry.peaks_right),
        telemetry.correlation,
        telemetry.active_voices as f32,
        telemetry.count as f32,
        (telemetry.version & 0x00ff_ffff) as f32,
        f32::from(device_running),
        f32::from(core.is_playing()),
        core.get_playhead() as f32,
    ]
}

fn bounce_session(core: &HirariUnifiedEngine) -> *mut c_void {
    core.bounce_session_state() as *mut c_void
}

/// The track's fader, or -1.0 when the id is not in the graph — the same
/// fallback the deleted C++ getter returned.
pub fn track_volume(core: &HirariUnifiedEngine, tid: u32) -> f32 {
    with_track_storage(core, tid, 0, |storage| {
        storage.map_or(-1.0, |strip| unsafe {
            channel_strip::hirari_channel_strip_gain(strip)
        })
    })
}

/// Activates an offline render target. Zero is the master; any other id is
/// validated against the current graph first, so a stale CLI target cannot
/// silently render an unrelated output.
pub fn set_offline_target(core: &HirariUnifiedEngine, tid: u32) -> bool {
    if tid != 0 && !with_track_storage(core, tid, 0, |storage| storage.is_some()) {
        return false;
    }
    unsafe { bounce_driver::hirari_bounce_set_offline_target(bounce_session(core), tid) };
    true
}

pub fn clear_offline_target(core: &HirariUnifiedEngine) {
    unsafe { bounce_driver::hirari_bounce_clear_offline_target(bounce_session(core)) };
}

/// Bounds and non-finite rejection are enforced by the caller; the session
/// stores the millisecond value the render plan consumes.
pub fn set_offline_tail_seconds(core: &HirariUnifiedEngine, seconds: f32) {
    unsafe { bounce_driver::hirari_bounce_set_tail_seconds(bounce_session(core), seconds) };
}

pub fn set_offline_options(core: &HirariUnifiedEngine, pre_fader: bool, include_inserts: bool) {
    unsafe {
        bounce_driver::hirari_bounce_set_render_options(
            bounce_session(core),
            u8::from(pre_fader),
            u8::from(include_inserts),
        )
    };
}

/// Takes the tracks lock through the layout host ops, finds the id's track and
/// hands one of its storages (kind 0 is the channel strip) to `read`.
fn with_track_storage<R>(
    core: &HirariUnifiedEngine,
    tid: u32,
    kind: u32,
    read: impl FnOnce(Option<*mut c_void>) -> R,
) -> R {
    let raw = layout_gather::ops_for(core) as *const layout_gather::HirariLayoutHostOps;
    if raw.is_null() {
        return read(None);
    }
    let ops = unsafe { &*raw };
    let host = core as *const HirariUnifiedEngine as *mut c_void;
    unsafe { (ops.begin)(host) };
    let found = (|| {
        let count = unsafe { (ops.track_count)(host) };
        for index in 0..count {
            let track = unsafe { (ops.track_handle)(host, index) };
            if track.is_null() || unsafe { (ops.track_id)(track) } != tid {
                continue;
            }
            return Some(unsafe { (ops.track_storage)(track, kind) });
        }
        None
    })();
    let result = read(found);
    unsafe { (ops.end)(host) };
    result
}
