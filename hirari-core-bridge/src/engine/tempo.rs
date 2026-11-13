#[derive(Clone, Debug, PartialEq)]
pub struct TempoEvent {
    pub sample_pos: u64,
    pub bpm: f64,
    pub ramp: bool,
    pub world_beats: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeTempoMapEvent {
    sample_pos: u64,
    bpm: f64,
    ramp: bool,
    world_beats: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeTimeSignatureEvent {
    sample_pos: u64,
    numerator: u8,
    denominator: u8,
    beat: f64,
}

struct NativeTempoMap {
    events: arc_swap::ArcSwap<Vec<NativeTempoMapEvent>>,
    signatures: arc_swap::ArcSwap<Vec<NativeTimeSignatureEvent>>,
    writer: std::sync::Mutex<()>,
    sample_rate_bits: std::sync::atomic::AtomicU64,
    current_bpm_bits: std::sync::atomic::AtomicU32,
}

impl NativeTempoMap {
    fn new() -> Self {
        Self {
            events: arc_swap::ArcSwap::from_pointee(Vec::new()),
            signatures: arc_swap::ArcSwap::from_pointee(vec![NativeTimeSignatureEvent {
                sample_pos: 0,
                numerator: 4,
                denominator: 4,
                beat: 0.0,
            }]),
            writer: std::sync::Mutex::new(()),
            sample_rate_bits: std::sync::atomic::AtomicU64::new(48_000.0f64.to_bits()),
            current_bpm_bits: std::sync::atomic::AtomicU32::new(120.0f32.to_bits()),
        }
    }

    }

unsafe fn tempo_map_from_ptr<'a>(state: *const std::ffi::c_void) -> Option<&'a NativeTempoMap> {
    if state.is_null() {
        None
    } else {
        Some(unsafe { &*state.cast::<NativeTempoMap>() })
    }
}

#[no_mangle]
pub extern "C" fn hirari_tempo_map_create() -> *mut std::ffi::c_void {
    Box::into_raw(Box::new(NativeTempoMap::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_destroy(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<NativeTempoMap>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_add_tempo(
    state: *mut std::ffi::c_void,
    sample_pos: u64,
    bpm: f64,
    sample_rate: f64,
    ramp: bool,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = state.events.load_full();
    let mut candidate = vec![
        NativeTempoMapEvent {
            sample_pos: 0,
            bpm: 0.0,
            ramp: false,
            world_beats: 0.0,
        };
        current.len().saturating_add(1)
    ];
    let count = unsafe {
        hirari_tempo_add_event_at_sample(
            current.as_ptr().cast(),
            current.len(),
            candidate.as_mut_ptr().cast(),
            candidate.len(),
            sample_pos,
            bpm,
            ramp,
            sample_rate,
        )
    };
    if count == 0 {
        return false;
    }
    candidate.truncate(count);
    state
        .sample_rate_bits
        .store(sample_rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
    state.events.store(std::sync::Arc::new(candidate));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_replace_events_at_beats(
    state: *mut std::ffi::c_void,
    events: *const std::ffi::c_void,
    event_count: usize,
    sample_rate: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    if events.is_null() || event_count == 0 {
        return false;
    }
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut candidate = unsafe {
        std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count).to_vec()
    };
    if !unsafe {
        hirari_tempo_replace_events_at_beats(
            candidate.as_mut_ptr().cast(),
            event_count,
            sample_rate,
        )
    } {
        return false;
    }
    state
        .sample_rate_bits
        .store(sample_rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
    state.events.store(std::sync::Arc::new(candidate));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_remove_tempo(
    state: *mut std::ffi::c_void,
    sample_pos: u64,
    sample_rate: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = state.events.load_full();
    let mut candidate = vec![
        NativeTempoMapEvent {
            sample_pos: 0,
            bpm: 0.0,
            ramp: false,
            world_beats: 0.0,
        };
        current.len()
    ];
    let count = unsafe {
        hirari_tempo_remove_event_at_sample(
            current.as_ptr().cast(),
            current.len(),
            candidate.as_mut_ptr().cast(),
            candidate.len(),
            sample_pos,
            sample_rate,
        )
    };
    if count == 0 {
        return false;
    }
    candidate.truncate(count);
    state
        .sample_rate_bits
        .store(sample_rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
    state.events.store(std::sync::Arc::new(candidate));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_clear(
    state: *mut std::ffi::c_void,
    sample_rate: f64,
    initial_bpm: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    if !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !initial_bpm.is_finite()
        || initial_bpm <= 0.0
    {
        return false;
    }
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state
        .events
        .store(std::sync::Arc::new(vec![NativeTempoMapEvent {
            sample_pos: 0,
            bpm: initial_bpm,
            ramp: false,
            world_beats: 0.0,
        }]));
    state
        .sample_rate_bits
        .store(sample_rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_add_time_signature(
    state: *mut std::ffi::c_void,
    beat: f64,
    numerator: u8,
    denominator: u8,
    sample_rate: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let signatures = state.signatures.load_full();
    let tempos = state.events.load_full();
    let mut candidate = vec![
        NativeTimeSignatureEvent {
            sample_pos: 0,
            numerator: 0,
            denominator: 0,
            beat: 0.0,
        };
        signatures.len().saturating_add(1)
    ];
    let count = unsafe {
        hirari_time_signature_add(
            signatures.as_ptr().cast(),
            signatures.len(),
            candidate.as_mut_ptr().cast(),
            candidate.len(),
            tempos.as_ptr().cast(),
            tempos.len(),
            beat,
            numerator,
            denominator,
            sample_rate,
        )
    };
    if count == 0 {
        return false;
    }
    candidate.truncate(count);
    state.signatures.store(std::sync::Arc::new(candidate));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_remove_time_signature(
    state: *mut std::ffi::c_void,
    beat: f64,
    sample_rate: f64,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = state.signatures.load_full();
    let mut candidate = vec![
        NativeTimeSignatureEvent {
            sample_pos: 0,
            numerator: 0,
            denominator: 0,
            beat: 0.0,
        };
        current.len()
    ];
    let count = unsafe {
        hirari_time_signature_remove(
            current.as_ptr().cast(),
            current.len(),
            candidate.as_mut_ptr().cast(),
            candidate.len(),
            beat,
            sample_rate,
        )
    };
    if count == 0 {
        return false;
    }
    candidate.truncate(count);
    state.signatures.store(std::sync::Arc::new(candidate));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_clear_time_signatures(
    state: *mut std::ffi::c_void,
) -> bool {
    let Some(state) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    let _writer = state
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state
        .signatures
        .store(std::sync::Arc::new(vec![NativeTimeSignatureEvent {
            sample_pos: 0,
            numerator: 4,
            denominator: 4,
            beat: 0.0,
        }]));
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_get_event_count(state: *const std::ffi::c_void) -> usize {
    unsafe { tempo_map_from_ptr(state) }.map_or(0, |map| map.events.load().len())
}

/// Copies the tempo events in the order the project encoder expects.
pub(crate) unsafe fn project_tempo_events(
    state: *const std::ffi::c_void,
) -> Vec<(u64, f64, bool)> {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return Vec::new();
    };
    map.events
        .load()
        .iter()
        .map(|event| (event.sample_pos, event.bpm, event.ramp))
        .collect()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_copy_events(
    state: *const std::ffi::c_void,
    output: *mut std::ffi::c_void,
    capacity: usize,
) -> usize {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return 0;
    };
    let events = map.events.load();
    if output.is_null() || capacity < events.len() {
        return events.len();
    }
    unsafe {
        std::slice::from_raw_parts_mut(output.cast::<NativeTempoMapEvent>(), events.len())
            .copy_from_slice(&events);
    }
    events.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_get_event_at(
    state: *const std::ffi::c_void,
    sample_pos: u64,
    output: *mut std::ffi::c_void,
) -> bool {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    let events = map.events.load();
    let Some(event) = events.iter().find(|event| event.sample_pos == sample_pos) else {
        return false;
    };
    unsafe { output.cast::<NativeTempoMapEvent>().write(*event) };
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_get_signature_count(
    state: *const std::ffi::c_void,
) -> usize {
    unsafe { tempo_map_from_ptr(state) }.map_or(0, |map| map.signatures.load().len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_copy_signatures(
    state: *const std::ffi::c_void,
    output: *mut std::ffi::c_void,
    capacity: usize,
) -> usize {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return 0;
    };
    let signatures = map.signatures.load();
    if output.is_null() || capacity < signatures.len() {
        return signatures.len();
    }
    unsafe {
        std::slice::from_raw_parts_mut(output.cast::<NativeTimeSignatureEvent>(), signatures.len())
            .copy_from_slice(&signatures);
    }
    signatures.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_get_signature_at(
    state: *const std::ffi::c_void,
    beat: f64,
    output: *mut std::ffi::c_void,
) -> bool {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return false;
    };
    let signatures = map.signatures.load();
    unsafe {
        hirari_time_signature_find(signatures.as_ptr().cast(), signatures.len(), beat, output)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_samples_to_beats(
    state: *const std::ffi::c_void,
    samples: u64,
    sample_rate: f64,
) -> f64 {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return 0.0;
    };
    let events = map.events.load();
    unsafe {
        hirari_tempo_samples_to_beats(events.as_ptr().cast(), events.len(), samples, sample_rate)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_beats_to_samples(
    state: *const std::ffi::c_void,
    beats: f64,
    sample_rate: f64,
) -> u64 {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return 0;
    };
    let events = map.events.load();
    unsafe {
        hirari_tempo_beats_to_samples(events.as_ptr().cast(), events.len(), beats, sample_rate)
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_get_current_bpm(state: *const std::ffi::c_void) -> f32 {
    unsafe { tempo_map_from_ptr(state) }.map_or(120.0, |map| {
        f32::from_bits(
            map.current_bpm_bits
                .load(std::sync::atomic::Ordering::Acquire),
        )
    })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_set_current_bpm(state: *mut std::ffi::c_void, bpm: f32) {
    if let Some(map) = unsafe { state.cast::<NativeTempoMap>().as_ref() } {
        map.current_bpm_bits
            .store(bpm.to_bits(), std::sync::atomic::Ordering::Release);
    }
}

fn valid_time_signature(beat: f64, numerator: u8, denominator: u8, sample_rate: f64) -> bool {
    beat.is_finite()
        && beat >= 0.0
        && (1..=32).contains(&numerator)
        && matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32)
        && sample_rate.is_finite()
        && sample_rate > 0.0
}

#[no_mangle]
pub unsafe extern "C" fn hirari_time_signature_add(
    source: *const std::ffi::c_void,
    signature_count: usize,
    output: *mut std::ffi::c_void,
    output_capacity: usize,
    tempo_events: *const std::ffi::c_void,
    tempo_event_count: usize,
    beat: f64,
    numerator: u8,
    denominator: u8,
    sample_rate: f64,
) -> usize {
    if output.is_null()
        || output_capacity < signature_count.saturating_add(1)
        || (signature_count > 0 && source.is_null())
        || !valid_time_signature(beat, numerator, denominator, sample_rate)
    {
        return 0;
    }
    let mut candidate = if signature_count == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(source.cast::<NativeTimeSignatureEvent>(), signature_count)
            .to_vec()
    };
    let index = candidate.partition_point(|event| event.beat < beat);
    let sample_pos =
        hirari_tempo_beats_to_samples(tempo_events, tempo_event_count, beat, sample_rate);
    if candidate.get(index).is_some_and(|event| event.beat == beat) {
        candidate[index].sample_pos = sample_pos;
        candidate[index].numerator = numerator;
        candidate[index].denominator = denominator;
    } else {
        candidate.insert(
            index,
            NativeTimeSignatureEvent {
                sample_pos,
                numerator,
                denominator,
                beat,
            },
        );
    }
    std::slice::from_raw_parts_mut(output.cast::<NativeTimeSignatureEvent>(), candidate.len())
        .copy_from_slice(&candidate);
    candidate.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_time_signature_remove(
    source: *const std::ffi::c_void,
    signature_count: usize,
    output: *mut std::ffi::c_void,
    output_capacity: usize,
    beat: f64,
    sample_rate: f64,
) -> usize {
    if source.is_null()
        || output.is_null()
        || signature_count <= 1
        || output_capacity < signature_count
        || !beat.is_finite()
        || beat <= 0.0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
    {
        return 0;
    }
    let mut candidate =
        std::slice::from_raw_parts(source.cast::<NativeTimeSignatureEvent>(), signature_count)
            .to_vec();
    let Some(index) = candidate
        .iter()
        .position(|event| (event.beat - beat).abs() < 1.0e-9)
    else {
        return 0;
    };
    candidate.remove(index);
    std::slice::from_raw_parts_mut(output.cast::<NativeTimeSignatureEvent>(), candidate.len())
        .copy_from_slice(&candidate);
    candidate.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_time_signature_find(
    signatures: *const std::ffi::c_void,
    signature_count: usize,
    beat: f64,
    output: *mut std::ffi::c_void,
) -> bool {
    if signatures.is_null()
        || signature_count == 0
        || output.is_null()
        || !beat.is_finite()
        || beat < 0.0
    {
        return false;
    }
    let signatures = std::slice::from_raw_parts(
        signatures.cast::<NativeTimeSignatureEvent>(),
        signature_count,
    );
    let Some(event) = signatures
        .iter()
        .find(|event| (event.beat - beat).abs() < 1.0e-9)
    else {
        return false;
    };
    *output.cast::<NativeTimeSignatureEvent>() = *event;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_add_event_at_sample(
    source: *const std::ffi::c_void,
    event_count: usize,
    output: *mut std::ffi::c_void,
    output_capacity: usize,
    sample_pos: u64,
    bpm: f64,
    ramp: bool,
    sample_rate: f64,
) -> usize {
    if output.is_null()
        || output_capacity < event_count.saturating_add(1)
        || !bpm.is_finite()
        || bpm <= 0.0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || (event_count > 0 && source.is_null())
    {
        return 0;
    }
    let mut candidate = if event_count == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(source.cast::<NativeTempoMapEvent>(), event_count).to_vec()
    };
    let index = candidate.partition_point(|event| event.sample_pos < sample_pos);
    if candidate
        .get(index)
        .is_some_and(|event| event.sample_pos == sample_pos)
    {
        candidate[index].bpm = bpm;
        candidate[index].ramp = ramp;
    } else {
        candidate.insert(
            index,
            NativeTempoMapEvent {
                sample_pos,
                bpm,
                ramp,
                world_beats: 0.0,
            },
        );
    }
    recalculate_tempo_events(&mut candidate, sample_rate);
    std::slice::from_raw_parts_mut(output.cast::<NativeTempoMapEvent>(), candidate.len())
        .copy_from_slice(&candidate);
    candidate.len()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_remove_event_at_sample(
    source: *const std::ffi::c_void,
    event_count: usize,
    output: *mut std::ffi::c_void,
    output_capacity: usize,
    sample_pos: u64,
    sample_rate: f64,
) -> usize {
    if source.is_null()
        || output.is_null()
        || output_capacity < event_count
        || event_count <= 1
        || sample_pos == 0
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
    {
        return 0;
    }
    let mut candidate =
        std::slice::from_raw_parts(source.cast::<NativeTempoMapEvent>(), event_count).to_vec();
    let index = candidate.partition_point(|event| event.sample_pos < sample_pos);
    if candidate
        .get(index)
        .is_none_or(|event| event.sample_pos != sample_pos)
    {
        return 0;
    }
    candidate.remove(index);
    recalculate_tempo_events(&mut candidate, sample_rate);
    std::slice::from_raw_parts_mut(output.cast::<NativeTempoMapEvent>(), candidate.len())
        .copy_from_slice(&candidate);
    candidate.len()
}

#[path = "tempo_recalc.rs"]
mod recalc;
pub(crate) use recalc::*;

fn safe_native_sample_offset(base: u64, offset: f64) -> u64 {
    if offset.is_finite() && offset > 0.0 {
        base.saturating_add(offset as u64)
    } else {
        base
    }
}

#[path = "tempo_orchestrator.rs"]
mod orchestrator;
pub use orchestrator::{TempoOrchestrator, TapTempo};
