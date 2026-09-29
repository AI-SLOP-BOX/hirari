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

    fn sample_rate(&self) -> f64 {
        f64::from_bits(
            self.sample_rate_bits
                .load(std::sync::atomic::Ordering::Relaxed),
        )
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
pub unsafe extern "C" fn hirari_tempo_map_bpm_at_sample(
    state: *const std::ffi::c_void,
    sample_pos: u64,
) -> f64 {
    let Some(map) = (unsafe { tempo_map_from_ptr(state) }) else {
        return 120.0;
    };
    let events = map.events.load();
    unsafe { hirari_tempo_bpm_at_sample(events.as_ptr().cast(), events.len(), sample_pos) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_map_set_bpm(state: *mut std::ffi::c_void, bpm: f64) -> bool {
    let Some(map) = (unsafe { state.cast::<NativeTempoMap>().as_ref() }) else {
        return false;
    };
    unsafe { hirari_tempo_map_add_tempo(state, 0, bpm, map.sample_rate(), false) }
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
pub unsafe extern "C" fn hirari_time_signature_reset(output: *mut std::ffi::c_void) {
    if !output.is_null() {
        *output.cast::<NativeTimeSignatureEvent>() = NativeTimeSignatureEvent {
            sample_pos: 0,
            numerator: 4,
            denominator: 4,
            beat: 0.0,
        };
    }
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

fn recalculate_tempo_events(events: &mut [NativeTempoMapEvent], sample_rate: f64) {
    let (mut current_beats, mut previous_sample, mut previous_bpm, mut previous_ramp) =
        (0.0_f64, 0_u64, 120.0_f64, false);
    for event in events {
        let step = event.sample_pos.saturating_sub(previous_sample);
        if step > 0
            && previous_bpm.is_finite()
            && previous_bpm > 0.0
            && event.bpm.is_finite()
            && event.bpm > 0.0
        {
            let average_bpm = if previous_ramp {
                (previous_bpm + event.bpm) * 0.5
            } else {
                previous_bpm
            };
            current_beats += (step as f64 / sample_rate) * (average_bpm / 60.0);
        }
        event.world_beats = current_beats;
        previous_sample = event.sample_pos;
        previous_bpm = event.bpm;
        previous_ramp = event.ramp;
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_replace_events_at_beats(
    events: *mut std::ffi::c_void,
    event_count: usize,
    sample_rate: f64,
) -> bool {
    if events.is_null() || event_count == 0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return false;
    }
    let output = std::slice::from_raw_parts_mut(events.cast::<NativeTempoMapEvent>(), event_count);
    let mut candidate = output.to_vec();
    candidate.sort_by(|left, right| left.world_beats.total_cmp(&right.world_beats));
    if !candidate[0].world_beats.is_finite() || candidate[0].world_beats.abs() > 1.0e-9 {
        return false;
    }
    candidate[0].world_beats = 0.0;
    candidate[0].sample_pos = 0;
    for index in 0..candidate.len() {
        let event = candidate[index];
        if !event.world_beats.is_finite()
            || event.world_beats < 0.0
            || !event.bpm.is_finite()
            || !(20.0..=300.0).contains(&event.bpm)
            || (index > 0 && event.world_beats <= candidate[index - 1].world_beats)
        {
            return false;
        }
        if index == 0 {
            continue;
        }
        let previous = candidate[index - 1];
        let beat_delta = event.world_beats - previous.world_beats;
        let effective_bpm = if previous.ramp {
            (previous.bpm + event.bpm) * 0.5
        } else {
            previous.bpm
        };
        if !effective_bpm.is_finite() || effective_bpm <= 0.0 {
            return false;
        }
        let sample_delta = beat_delta * 60.0 / effective_bpm * sample_rate;
        let rounded_delta = (sample_delta + 0.5).floor();
        let maximum_delta = (u64::MAX - previous.sample_pos) as f64;
        if !rounded_delta.is_finite() || rounded_delta < 0.0 || rounded_delta > maximum_delta {
            return false;
        }
        let Some(sample_pos) = previous.sample_pos.checked_add(rounded_delta as u64) else {
            return false;
        };
        candidate[index].sample_pos = sample_pos;
    }
    output.copy_from_slice(&candidate);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_bpm_at_sample(
    events: *const std::ffi::c_void,
    event_count: usize,
    sample_pos: u64,
) -> f64 {
    if events.is_null() || event_count == 0 {
        return 120.0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    if sample_pos < events[0].sample_pos {
        return events[0].bpm;
    }
    for (index, event) in events.iter().enumerate() {
        if event.sample_pos > sample_pos {
            break;
        }
        if event.ramp && index + 1 < events.len() && sample_pos < events[index + 1].sample_pos {
            let next = &events[index + 1];
            let span = (next.sample_pos - event.sample_pos) as f64;
            let t = if span > 0.0 {
                ((sample_pos - event.sample_pos) as f64 / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return event.bpm + (next.bpm - event.bpm) * t;
        }
    }
    events
        .iter()
        .rev()
        .find(|event| event.sample_pos <= sample_pos)
        .map(|event| event.bpm)
        .unwrap_or(events[0].bpm)
}

/// Tempo conversion used directly by the native engine's immutable event
/// snapshots. This ABI mirrors `TempoMap::Event`; it allocates nothing and
/// performs no synchronization on the audio thread.
#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_samples_to_beats(
    events: *const std::ffi::c_void,
    event_count: usize,
    samples: u64,
    sample_rate: f64,
) -> f64 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return 0.0;
    }
    if event_count == 0 {
        return (samples as f64 / sample_rate) * 2.0;
    }
    if events.is_null() {
        return 0.0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    if samples < events[0].sample_pos {
        return (samples as f64 / sample_rate) * (events[0].bpm / 60.0);
    }
    let upper = events.partition_point(|event| event.sample_pos <= samples);
    let index = upper.saturating_sub(1).min(events.len() - 1);
    let event = events[index];
    let delta_seconds = (samples - event.sample_pos) as f64 / sample_rate;
    if event.ramp && index + 1 < events.len() {
        let next = events[index + 1];
        let segment_seconds = (next.sample_pos - event.sample_pos) as f64 / sample_rate;
        if segment_seconds > 0.0 {
            let t = (delta_seconds / segment_seconds).clamp(0.0, 1.0);
            let integrated =
                event.bpm * delta_seconds + 0.5 * (next.bpm - event.bpm) * segment_seconds * t * t;
            return event.world_beats + integrated / 60.0;
        }
    }
    event.world_beats + delta_seconds * (event.bpm / 60.0)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_tempo_beats_to_samples(
    events: *const std::ffi::c_void,
    event_count: usize,
    beats: f64,
    sample_rate: f64,
) -> u64 {
    if !beats.is_finite() || !sample_rate.is_finite() || sample_rate <= 0.0 || beats <= 0.0 {
        return 0;
    }
    if event_count == 0 {
        return (beats * 60.0 / 120.0 * sample_rate) as u64;
    }
    if events.is_null() {
        return 0;
    }
    let events = std::slice::from_raw_parts(events.cast::<NativeTempoMapEvent>(), event_count);
    let upper = events.partition_point(|event| event.world_beats <= beats);
    let index = upper.saturating_sub(1).min(events.len() - 1);
    let event = events[index];
    let delta_beats = beats - event.world_beats;
    if event.ramp && index + 1 < events.len() {
        let next = events[index + 1];
        let segment_seconds = (next.sample_pos - event.sample_pos) as f64 / sample_rate;
        if segment_seconds <= 0.0 || next.world_beats <= event.world_beats {
            return event.sample_pos;
        }
        let delta_bpm = next.bpm - event.bpm;
        let mut delta_seconds = (delta_beats * 60.0 / event.bpm).clamp(0.0, segment_seconds);
        for _ in 0..5 {
            let t = (delta_seconds / segment_seconds).clamp(0.0, 1.0);
            let residual = (event.bpm * delta_seconds + 0.5 * delta_bpm * segment_seconds * t * t)
                / 60.0
                - delta_beats;
            let derivative = (event.bpm + delta_bpm * t) / 60.0;
            if derivative.abs() < 1.0e-12 {
                break;
            }
            delta_seconds = (delta_seconds - residual / derivative).clamp(0.0, segment_seconds);
        }
        return safe_native_sample_offset(event.sample_pos, delta_seconds * sample_rate);
    }
    safe_native_sample_offset(
        event.sample_pos,
        delta_beats / (event.bpm / 60.0) * sample_rate,
    )
}

fn safe_native_sample_offset(base: u64, offset: f64) -> u64 {
    if offset.is_finite() && offset > 0.0 {
        base.saturating_add(offset as u64)
    } else {
        base
    }
}

pub struct TempoOrchestrator {
    pub events: Vec<TempoEvent>,
    pub tap_tempo: TapTempo,
}

#[derive(Default, Debug, Clone)]
pub struct TapTempo {
    taps: Vec<u64>,
}
impl TapTempo {
    pub fn tap(&mut self, timestamp_ms: u64) -> Option<f64> {
        if let Some(&last) = self.taps.last() {
            if timestamp_ms <= last {
                return None;
            }
            let interval = timestamp_ms - last;
            if !(200..=4000).contains(&interval) {
                self.taps.clear();
            }
        }
        self.taps.push(timestamp_ms);
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        if self.taps.len() < 2 {
            return None;
        }
        let mut intervals: Vec<u64> = self
            .taps
            .windows(2)
            .map(|window| window[1] - window[0])
            .collect();
        intervals.sort_unstable();
        let median = intervals[intervals.len() / 2] as f64;
        Some((60_000.0 / median).clamp(20.0, 300.0))
    }
    pub fn clear(&mut self) {
        self.taps.clear();
    }
}

impl Default for TempoOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TempoOrchestrator {
    pub fn new() -> Self {
        Self {
            events: vec![TempoEvent {
                sample_pos: 0,
                bpm: 120.0,
                ramp: false,
                world_beats: 0.0,
            }],
            tap_tempo: TapTempo::default(),
        }
    }

    pub fn upsert_event(&mut self, event: TempoEvent) -> bool {
        if event.bpm.is_finite()
            && (20.0..=999.0).contains(&event.bpm)
            && event.world_beats.is_finite()
            && event.world_beats >= 0.0
        {
            if let Some(old) = self
                .events
                .iter_mut()
                .find(|e| e.sample_pos == event.sample_pos)
            {
                *old = event;
            } else {
                self.events.push(event);
            }
            self.events.sort_by_key(|e| e.sample_pos);
            true
        } else {
            false
        }
    }
    pub fn remove_event(&mut self, sample_pos: u64) -> bool {
        if sample_pos == 0 {
            return false;
        }
        let n = self.events.len();
        self.events.retain(|e| e.sample_pos != sample_pos);
        n != self.events.len()
    }

    /// Moves a tempo-map node during time-warp editing while preserving the
    /// unique, strictly ordered event invariant.
    pub fn move_event(&mut self, from_sample: u64, to_sample: u64) -> bool {
        if from_sample == 0
            || to_sample == 0
            || from_sample == to_sample
            || self
                .events
                .iter()
                .any(|event| event.sample_pos == to_sample)
        {
            return false;
        }
        let Some(index) = self
            .events
            .iter()
            .position(|event| event.sample_pos == from_sample)
        else {
            return false;
        };
        let mut candidate = self.events.clone();
        candidate[index].sample_pos = to_sample;
        candidate.sort_by_key(|event| event.sample_pos);
        if candidate.first().map(|event| event.sample_pos) != Some(0)
            || candidate
                .windows(2)
                .any(|pair| pair[0].sample_pos >= pair[1].sample_pos)
        {
            return false;
        }
        self.events = candidate;
        true
    }

    pub fn set_ramp(&mut self, sample_pos: u64, ramp: bool) -> bool {
        let Some(event) = self
            .events
            .iter_mut()
            .find(|event| event.sample_pos == sample_pos)
        else {
            return false;
        };
        event.ramp = ramp;
        true
    }

    /// Feed a wall-clock tap and apply the resulting tempo to the map origin.
    /// The map remains sorted and its integrated beat positions are recalculated
    /// by the caller once the project sample rate is known.
    pub fn tap_and_set_tempo(&mut self, timestamp_ms: u64) -> Option<f64> {
        let bpm = self.tap_tempo.tap(timestamp_ms)?;
        if let Some(origin) = self.events.iter_mut().find(|event| event.sample_pos == 0) {
            origin.bpm = bpm;
        }
        Some(bpm)
    }

    /// INDUSTRIAL: Recalculates integrated beat positions with absolute precision and temporal sovereignty.
    pub fn recalculate_integrated_time(&mut self, sample_rate: f64) {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return;
        }
        self.normalize_events();
        let mut current_beats = 0.0;
        let mut last_samples = 0;
        let mut last_bpm = 120.0;
        let mut last_ramp = false;

        for event in self.events.iter_mut() {
            let step = event.sample_pos.saturating_sub(last_samples);
            if step > 0 {
                let mut avg_bpm = last_bpm;
                if last_ramp {
                    avg_bpm = (last_bpm + event.bpm) * 0.5;
                }
                if last_bpm.is_finite()
                    && last_bpm > 0.0
                    && event.bpm.is_finite()
                    && event.bpm > 0.0
                {
                    current_beats += (step as f64 / sample_rate) * (avg_bpm / 60.0);
                }
            }
            event.world_beats = current_beats;
            last_samples = event.sample_pos;
            last_bpm = event.bpm;
            last_ramp = event.ramp;
        }
    }

    /// INDUSTRIAL: Performs beat-to-sample resolution with absolute precision and temporal sovereignty.
    /// Solves the quadratic trapezoidal equation to determine sample-accurate position inside tempo ramps.
    pub fn beats_to_samples(&self, beats: f64, sample_rate: f64) -> u64 {
        if self.events.is_empty()
            || !beats.is_finite()
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
        {
            return 0;
        }

        let idx = match self
            .events
            .binary_search_by(|e| e.world_beats.total_cmp(&beats))
        {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    0
                } else {
                    idx - 1
                }
            }
        };

        let prev = &self.events[idx];
        if !prev.bpm.is_finite() || prev.bpm <= 0.0 {
            return prev.sample_pos;
        }
        let beat_step = beats - prev.world_beats;
        if beat_step <= 0.0 {
            return prev.sample_pos;
        }

        if prev.ramp && idx + 1 < self.events.len() {
            let next = &self.events[idx + 1];
            let duration_samples = next.sample_pos.saturating_sub(prev.sample_pos);
            if duration_samples > 0 {
                let duration_seconds = duration_samples as f64 / sample_rate;
                let alpha = (next.bpm - prev.bpm) / duration_seconds;

                if alpha.abs() > 1e-6 {
                    // Solve quadratic equation: 0.5 * alpha * t^2 + prev.bpm * t - 60.0 * beat_step = 0
                    let a = 0.5 * alpha;
                    let b = prev.bpm;
                    let c = -60.0 * beat_step;
                    let discriminant = b * b - 4.0 * a * c;
                    if discriminant >= 0.0 {
                        let t = (-b + discriminant.sqrt()) / (2.0 * a);
                        safe_sample_offset(prev.sample_pos, t * sample_rate)
                    } else {
                        safe_sample_offset(
                            prev.sample_pos,
                            (beat_step * 60.0 / prev.bpm) * sample_rate,
                        )
                    }
                } else {
                    safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
                }
            } else {
                safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
            }
        } else {
            safe_sample_offset(prev.sample_pos, (beat_step * 60.0 / prev.bpm) * sample_rate)
        }
    }

    /// INDUSTRIAL: Performs sample-to-beat resolution with absolute precision and temporal sovereignty.
    /// Evaluates cumulative trapezoidal beat integration inside tempo ramps.
    pub fn samples_to_beats(&self, samples: u64, sample_rate: f64) -> f64 {
        if self.events.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }

        let idx = match self.events.binary_search_by_key(&samples, |e| e.sample_pos) {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    0
                } else {
                    idx - 1
                }
            }
        };

        let prev = &self.events[idx];
        if !prev.bpm.is_finite() || prev.bpm <= 0.0 {
            return prev.world_beats;
        }
        let sample_step = samples.saturating_sub(prev.sample_pos);
        let t = sample_step as f64 / sample_rate;

        if prev.ramp && idx + 1 < self.events.len() {
            let next = &self.events[idx + 1];
            let duration_samples = next.sample_pos.saturating_sub(prev.sample_pos);
            if duration_samples > 0 {
                let duration_seconds = duration_samples as f64 / sample_rate;
                let alpha = (next.bpm - prev.bpm) / duration_seconds;
                let beats_diff = (prev.bpm * t + 0.5 * alpha * t * t) / 60.0;
                prev.world_beats + beats_diff
            } else {
                prev.world_beats + (t * prev.bpm / 60.0)
            }
        } else {
            prev.world_beats + (t * prev.bpm / 60.0)
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide temporal synchronization graph.
    pub fn audit_tempo(&self) -> bool {
        !self.events.is_empty()
            && self
                .events
                .first()
                .map(|event| event.sample_pos == 0)
                .unwrap_or(false)
            && self.events.windows(2).all(|pair| {
                pair[0].sample_pos < pair[1].sample_pos
                    && pair[0].world_beats.is_finite()
                    && pair[0].bpm.is_finite()
                    && pair[0].bpm > 0.0
            })
            && self.events.iter().all(|event| {
                event.bpm.is_finite() && event.bpm > 0.0 && event.world_beats.is_finite()
            })
    }

    /// Sorts and sanitizes externally edited tempo events before any binary
    /// search. Duplicate sample positions are collapsed deterministically.
    pub fn normalize_events(&mut self) {
        self.events.retain(|event| {
            event.bpm.is_finite() && event.bpm > 0.0 && event.world_beats.is_finite()
        });
        self.events.sort_by_key(|event| event.sample_pos);
        let mut normalized: Vec<TempoEvent> = Vec::with_capacity(self.events.len().max(1));
        for event in self.events.drain(..) {
            if let Some(previous) = normalized.last_mut() {
                if previous.sample_pos == event.sample_pos {
                    *previous = event;
                    continue;
                }
            }
            normalized.push(event);
        }
        if normalized.first().map(|event| event.sample_pos) != Some(0) {
            normalized.insert(
                0,
                TempoEvent {
                    sample_pos: 0,
                    bpm: 120.0,
                    ramp: false,
                    world_beats: 0.0,
                },
            );
        }
        self.events = normalized;
    }
}

fn safe_sample_offset(base: u64, offset: f64) -> u64 {
    if !offset.is_finite() || offset <= 0.0 {
        return base;
    }
    base.saturating_add(offset.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod native_tempo_map_tests {
    use super::{
        hirari_tempo_map_add_tempo, hirari_tempo_map_add_time_signature,
        hirari_tempo_map_beats_to_samples, hirari_tempo_map_clear, hirari_tempo_map_copy_events,
        hirari_tempo_map_copy_signatures, hirari_tempo_map_create, hirari_tempo_map_destroy,
        hirari_tempo_map_get_current_bpm, hirari_tempo_map_get_event_at,
        hirari_tempo_map_get_event_count, hirari_tempo_map_get_signature_at,
        hirari_tempo_map_get_signature_count, hirari_tempo_map_remove_time_signature,
        hirari_tempo_map_samples_to_beats, hirari_tempo_map_set_current_bpm, NativeTempoMapEvent,
        NativeTimeSignatureEvent,
    };

    #[test]
    fn rust_owned_tempo_map_matches_stateless_ffi_and_keeps_signature_snapshots() {
        let map = hirari_tempo_map_create();
        assert!(!map.is_null());
        unsafe {
            assert_eq!(hirari_tempo_map_get_current_bpm(map), 120.0);
            hirari_tempo_map_set_current_bpm(map, 137.5);
            assert_eq!(hirari_tempo_map_get_current_bpm(map), 137.5);
            assert!(hirari_tempo_map_clear(map, 48_000.0, 120.0));
            assert!(hirari_tempo_map_add_tempo(
                map, 96_000, 90.0, 48_000.0, true
            ));
            assert_eq!(hirari_tempo_map_get_event_count(map), 2);

            let mut events = [NativeTempoMapEvent {
                sample_pos: 0,
                bpm: 0.0,
                ramp: false,
                world_beats: 0.0,
            }; 2];
            assert_eq!(
                hirari_tempo_map_copy_events(map, events.as_mut_ptr().cast(), events.len()),
                2
            );
            assert_eq!(events[1].sample_pos, 96_000);
            assert!(events[1].ramp);
            assert_eq!(
                hirari_tempo_map_samples_to_beats(map, 48_000, 48_000.0),
                2.0
            );
            assert_eq!(
                hirari_tempo_map_beats_to_samples(map, 2.0, 48_000.0),
                48_000
            );

            let mut event = events[0];
            assert!(hirari_tempo_map_get_event_at(
                map,
                0,
                (&mut event as *mut NativeTempoMapEvent).cast::<std::ffi::c_void>(),
            ));
            assert_eq!(event.bpm, 120.0);

            assert!(hirari_tempo_map_add_time_signature(
                map, 4.0, 3, 4, 48_000.0
            ));
            assert_eq!(hirari_tempo_map_get_signature_count(map), 2);
            let mut signatures = [NativeTimeSignatureEvent {
                sample_pos: 0,
                numerator: 0,
                denominator: 0,
                beat: 0.0,
            }; 2];
            assert_eq!(
                hirari_tempo_map_copy_signatures(
                    map,
                    signatures.as_mut_ptr().cast(),
                    signatures.len(),
                ),
                2
            );
            assert_eq!(signatures[1].beat, 4.0);
            let mut signature = signatures[0];
            assert!(hirari_tempo_map_get_signature_at(
                map,
                4.0,
                (&mut signature as *mut NativeTimeSignatureEvent).cast::<std::ffi::c_void>(),
            ));
            assert_eq!((signature.numerator, signature.denominator), (3, 4));
            assert!(hirari_tempo_map_remove_time_signature(map, 4.0, 48_000.0));
            assert_eq!(hirari_tempo_map_get_signature_count(map), 1);
            hirari_tempo_map_destroy(map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TempoEvent, TempoOrchestrator};

    #[test]
    fn normalizes_unsorted_duplicate_and_invalid_events() {
        let mut tempo = TempoOrchestrator {
            events: vec![
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 90.0,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 0,
                    bpm: f64::NAN,
                    ramp: false,
                    world_beats: 0.0,
                },
                TempoEvent {
                    sample_pos: 44_100,
                    bpm: 100.0,
                    ramp: true,
                    world_beats: 0.0,
                },
            ],
            tap_tempo: super::TapTempo::default(),
        };
        tempo.recalculate_integrated_time(44_100.0);
        assert!(tempo.audit_tempo());
        assert_eq!(tempo.events.len(), 2);
        assert_eq!(tempo.events[0].sample_pos, 0);
        assert_eq!(tempo.events[1].sample_pos, 44_100);
        assert_eq!(tempo.events[1].bpm, 100.0);
    }

    #[test]
    fn tap_updates_origin_tempo() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(0).is_none());
        assert_eq!(tempo.tap_and_set_tempo(500), Some(120.0));
        assert_eq!(tempo.events[0].bpm, 120.0);
    }

    #[test]
    fn non_monotonic_taps_are_ignored() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.tap_and_set_tempo(1000).is_none());
        assert!(tempo.tap_and_set_tempo(900).is_none());
        assert_eq!(tempo.tap_and_set_tempo(1500), Some(120.0));
    }

    #[test]
    fn time_warp_moves_nodes_without_collisions() {
        let mut tempo = TempoOrchestrator::new();
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 48_000,
            bpm: 100.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.upsert_event(TempoEvent {
            sample_pos: 96_000,
            bpm: 110.0,
            ramp: false,
            world_beats: 0.0
        }));
        assert!(tempo.move_event(96_000, 72_000));
        assert!(!tempo.move_event(72_000, 48_000));
        assert!(tempo.set_ramp(72_000, true));
        assert!(tempo.audit_tempo());
    }
}
