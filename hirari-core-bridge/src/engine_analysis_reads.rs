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
use crate::dsp_utils::hirari_samples_to_milliseconds;
use crate::effect_chain_host;
use crate::engine_block_host::{
    HirariEngineTelemetry, hirari_engine_block_device_block_frames,
    hirari_engine_block_device_sample_rate, hirari_engine_block_master_limiter,
    hirari_engine_block_track_table, hirari_engine_telemetry_active_index,
    hirari_engine_telemetry_snapshot,
};
use crate::engine_track_table::{self, HirariTrackSnapshot};
use crate::ffi::{AudioEngine, BridgeLoudness, HirariUnifiedEngine};
use crate::harmonic_context;
use crate::layout_gather;
use crate::master_meter;
use crate::region_store;
use crate::tempo;
use crate::track_channel_eq;
use crate::track_runtime;
use crate::track_session::{self, HirariTrackSessionOps, TrackSession};
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
        f32::from(crate::engine_transport::is_playing(core)),
        crate::engine_transport::playhead(core) as f32,
    ]
}

fn bounce_session(core: &HirariUnifiedEngine) -> *mut c_void {
    core.bounce_session_state() as *mut c_void
}

/// The track's fader, or -1.0 when the id is not in the graph — the same
/// fallback the deleted C++ getter returned.
pub fn track_volume(core: &HirariUnifiedEngine, tid: u32) -> f32 {
    layout_gather::with_track_storage(host(core).cast_const(), tid, 0, |strip| unsafe {
        channel_strip::hirari_channel_strip_gain(strip)
    })
    .unwrap_or(-1.0)
}

/// Activates an offline render target. Zero is the master; any other id is
/// validated against the current graph first, so a stale CLI target cannot
/// silently render an unrelated output.
pub fn set_offline_target(core: &HirariUnifiedEngine, tid: u32) -> bool {
    if tid != 0
        && layout_gather::with_track_storage(host(core).cast_const(), tid, 0, |_| ()).is_none()
    {
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

/// The engine address a track session pins its lock against.
fn host(core: &HirariUnifiedEngine) -> *mut c_void {
    core as *const HirariUnifiedEngine as *mut c_void
}

/// A live track with the engine's tracks lock held, as the deleted
/// `queryLockedTrack` and `mutateLockedTrack` prologues resolved it.
fn session(core: &HirariUnifiedEngine, tid: u32) -> Option<TrackSession> {
    TrackSession::open(host(core), tid)
}

/// Releases the tracks lock when the walk leaves, including on a panic.
struct TracksLock<'a> {
    ops: &'a HirariTrackSessionOps,
    host: *mut c_void,
}

impl Drop for TracksLock<'_> {
    fn drop(&mut self) {
        self.ops.unlock_tracks(self.host);
    }
}

/// Visits every live track's effect chain with the tracks lock held, in graph
/// order — the walk the deleted C++ diagnostic polls performed over
/// `lockedTracks()`.
fn for_each_chain(core: &HirariUnifiedEngine, mut visit: impl FnMut(*mut c_void)) {
    let Some(ops) = track_session::engine_ops() else {
        return;
    };
    let table = unsafe { hirari_engine_block_track_table(block_handle(core)) };
    if table.is_null() {
        return;
    }
    let address = host(core);
    ops.lock_tracks(address);
    let _guard = TracksLock {
        ops: &ops,
        host: address,
    };
    let count = unsafe { engine_track_table::hirari_engine_track_count(table) };
    let pointers = unsafe { engine_track_table::hirari_engine_track_ptrs(table) };
    if pointers.is_null() {
        return;
    }
    // SAFETY: the table's buffer holds `count` entries while its lock is held.
    for &track in unsafe { std::slice::from_raw_parts(pointers, count) } {
        if track.is_null() {
            continue;
        }
        visit(ops.storage(track, track_session::STORAGE_CHAIN));
    }
}

/// One plug-in parameter, or 0.0 when the track is not in the graph.
pub fn plugin_parameter(
    core: &HirariUnifiedEngine,
    tid: u32,
    plugin_index: u32,
    parameter_id: u32,
) -> f32 {
    session(core, tid).map_or(0.0, |session| unsafe {
        effect_chain_host::hirari_effect_chain_parameter(
            session.chain() as *const c_void,
            plugin_index,
            parameter_id,
        )
    })
}

/// How many parameters one plug-in publishes, or 0 for a missing track.
pub fn plugin_parameter_count(core: &HirariUnifiedEngine, tid: u32, plugin_index: u32) -> u32 {
    session(core, tid).map_or(0, |session| unsafe {
        effect_chain_host::hirari_effect_chain_parameter_count(
            session.chain() as *const c_void,
            plugin_index,
        )
    })
}

/// Whether one insert is bypassed, or false for a missing track.
pub fn plugin_bypass(core: &HirariUnifiedEngine, tid: u32, plugin_index: u32) -> bool {
    session(core, tid).is_some_and(|session| unsafe {
        effect_chain_host::hirari_effect_chain_bypass(session.chain() as *const c_void, plugin_index)
    })
}

/// Display name for one plug-in parameter, read from the built-in effect table
/// the chain node's kind selects. An insert outside the chain has no kind to
/// name and a sandboxed worker keeps its labels inside the worker, so both stay
/// empty — which is what the deleted C++ stub answered for every insert.
pub fn plugin_parameter_name(
    core: &HirariUnifiedEngine,
    tid: u32,
    plugin_index: u32,
    parameter_id: u32,
) -> String {
    let Some(session) = session(core, tid) else {
        return String::new();
    };
    let chain = session.chain() as *const c_void;
    let count = unsafe { effect_chain_host::hirari_effect_chain_count(chain) };
    if plugin_index >= count {
        return String::new();
    }
    let kind = unsafe { effect_chain_host::hirari_effect_chain_node_kind(chain, plugin_index) };
    if kind == crate::effect_chain_nodes::SANDBOX_KIND {
        return String::new();
    }
    let name = crate::builtin_effect_names::hirari_builtin_effect_parameter_name(kind, parameter_id);
    if name.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// Drains every track's watchdog counter, summing the edges the audio callback
/// reported. The wrap is the deleted C++ accumulator's own.
pub fn watchdog_trips(core: &HirariUnifiedEngine) -> u32 {
    let mut trips = 0_u32;
    for_each_chain(core, |chain| {
        trips = trips.wrapping_add(unsafe {
            effect_chain_host::hirari_effect_chain_metric(chain as *const c_void, 0) as u32
        });
    });
    trips
}

/// Every track's non-finite sample count, keeping each chain's last measurement
/// while a control edit holds it.
pub fn non_finite_plugin_samples(core: &HirariUnifiedEngine) -> u64 {
    let mut samples = 0_u64;
    for_each_chain(core, |chain| {
        samples = samples.wrapping_add(unsafe {
            effect_chain_host::hirari_effect_chain_non_finite_samples(chain as *const c_void)
        });
    });
    samples
}

/// Restarts one quarantined worker. `clears_scan_fault` separates the explicit
/// user restart from the retry, which also clears the path's scan-cache fault.
/// A change only bumps the layout revision, leaving the frozen render valid.
fn sandbox_restart(core: &HirariUnifiedEngine, tid: u32, slot: u32, clears_scan_fault: bool) -> bool {
    session(core, tid).is_some_and(|session| {
        let changed = unsafe {
            effect_chain_host::hirari_effect_chain_sandbox_restart(
                session.chain(),
                usize::try_from(slot).unwrap_or(usize::MAX),
                true,
                clears_scan_fault,
            )
        };
        session.commit_layout(changed)
    })
}

pub fn restart_sandboxed_plugin(core: &HirariUnifiedEngine, tid: u32, slot: u32) -> bool {
    sandbox_restart(core, tid, slot, false)
}

pub fn retry_sandboxed_plugin(core: &HirariUnifiedEngine, tid: u32, slot: u32) -> bool {
    sandbox_restart(core, tid, slot, true)
}

/// Bands a track's channel EQ. The dB pairs fold into the runtime's gain
/// coefficients, and a missing track is reported as a rejection.
pub fn set_track_eq(
    core: &HirariUnifiedEngine,
    tid: u32,
    low_boost_db: f32,
    low_cut_db: f32,
    high_boost_db: f32,
    high_cut_db: f32,
) -> bool {
    let (low, high) = (
        track_channel_eq::hirari_track_eq_gain(low_boost_db, low_cut_db),
        track_channel_eq::hirari_track_eq_gain(high_boost_db, high_cut_db),
    );
    session(core, tid).is_some_and(|session| {
        unsafe { track_runtime::hirari_track_runtime_set_eq(session.runtime(), low, high) };
        true
    })
}

/// The engine's end-to-end latency: the device block plus the master limiter's
/// lookahead, in milliseconds at the rate the graph is prepared for.
pub fn latency_ms(core: &HirariUnifiedEngine) -> f32 {
    let state = block_handle(core);
    let limiter = unsafe { hirari_engine_block_master_limiter(state) };
    let samples = u64::from(unsafe { hirari_engine_block_device_block_frames(state) })
        + u64::from(unsafe { crate::master_limiter::hirari_master_limiter_latency(limiter) });
    unsafe {
        hirari_samples_to_milliseconds(samples, hirari_engine_block_device_sample_rate(state))
    }
}

/// A track's reported latency in milliseconds; zero when the graph has no such
/// track — the answer the deleted locked-query gave for a miss.
pub fn track_latency_ms(core: &HirariUnifiedEngine, tid: u32) -> f32 {
    let rate = unsafe { hirari_engine_block_device_sample_rate(block_handle(core)) };
    session(core, tid).map_or(0.0, |session| unsafe {
        hirari_samples_to_milliseconds(
            u64::from(crate::engine_track::hirari_track_latency_samples(session.track())),
            rate,
        )
    })
}

/// A track's plug-in delay compensation request in milliseconds.
pub fn track_pdc_compensation_ms(core: &HirariUnifiedEngine, tid: u32) -> f32 {
    let rate = unsafe { hirari_engine_block_device_sample_rate(block_handle(core)) };
    session(core, tid).map_or(0.0, |session| unsafe {
        hirari_samples_to_milliseconds(
            u64::from(crate::engine_track::hirari_track_pdc_requested(session.track())),
            rate,
        )
    })
}

/// A track's own manual delay in samples.
pub fn track_delay_samples(core: &HirariUnifiedEngine, tid: u32) -> u32 {
    session(core, tid).map_or(0, |session| unsafe {
        crate::engine_track::hirari_track_delay_samples(session.track())
    })
}
