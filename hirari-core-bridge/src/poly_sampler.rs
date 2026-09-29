//! Rust implementation of the legacy multi-zone instrument sampler.
//!
//! The C++ class now only adapts the existing instrument API and transfers
//! zone data at setup time. Voice allocation, envelopes and audio rendering
//! are owned here.

use arc_swap::ArcSwap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

const VOICE_COUNT: usize = 64;

#[repr(C)]
pub struct PolySamplerZoneView {
    min_note: u32,
    max_note: u32,
    min_velocity: f32,
    max_velocity: f32,
    root_note: u32,
    loop_start: u32,
    loop_end: u32,
    loop_enabled: bool,
    source_sample_rate: f64,
    left: *const f32,
    left_length: usize,
    right: *const f32,
    right_length: usize,
}

struct Zone {
    min_note: u32,
    max_note: u32,
    min_velocity: f32,
    max_velocity: f32,
    root_note: u32,
    loop_start: u32,
    loop_end: u32,
    loop_enabled: bool,
    source_sample_rate: f64,
    left: Arc<[f32]>,
    right: Arc<[f32]>,
}

impl Zone {
    fn matches(&self, note: u32, velocity: f32) -> bool {
        note >= self.min_note
            && note <= self.max_note
            && velocity >= self.min_velocity
            && velocity <= self.max_velocity
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

struct Envelope {
    stage: EnvelopeStage,
    level: f32,
    attack_step: f32,
    decay_step: f32,
    sustain: f32,
    release_coefficient: f32,
}

impl Envelope {
    fn new(sample_rate: f64) -> Self {
        let mut envelope = Self {
            stage: EnvelopeStage::Idle,
            level: 0.0,
            attack_step: 0.0,
            decay_step: 0.0,
            sustain: 0.8,
            release_coefficient: 0.0,
        };
        envelope.set_parameters(sample_rate, 0.005, 0.1, 0.8, 0.3);
        envelope
    }

    fn set_parameters(
        &mut self,
        sample_rate: f64,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    ) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44_100.0
        };
        let attack = attack.max(0.001);
        let decay = decay.max(0.001);
        self.sustain = sustain.clamp(0.0, 1.0);
        let release = release.max(0.001);
        self.attack_step = 1.0 / (attack * sr as f32);
        self.decay_step = (1.0 - self.sustain) / (decay * sr as f32);
        self.release_coefficient = (-1.0 / (release as f64 * sr)).exp() as f32;
    }

    fn trigger_on(&mut self) {
        self.stage = EnvelopeStage::Attack;
        self.level = 0.0;
    }

    fn trigger_off(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
        }
    }

    fn reset(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.level = 0.0;
    }

    fn next(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => return 0.0,
            EnvelopeStage::Attack => {
                self.level = (self.level + self.attack_step).min(1.0);
                if self.level >= 1.0 {
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                self.level = (self.level - self.decay_step).max(self.sustain);
                if self.level <= self.sustain {
                    self.stage = EnvelopeStage::Sustain;
                }
            }
            EnvelopeStage::Sustain => self.level = self.sustain,
            EnvelopeStage::Release => {
                self.level *= self.release_coefficient;
                if self.level <= 1.0e-5 {
                    self.level = 0.0;
                    self.stage = EnvelopeStage::Idle;
                }
            }
        }
        if self.level.is_finite() {
            self.level.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

struct Voice {
    active: bool,
    note: u32,
    position: f64,
    speed: f32,
    target_speed: f32,
    sample_rate_ratio: f64,
    slide_rate: f32,
    velocity: f32,
    envelope: Envelope,
    filter_left: f32,
    filter_right: f32,
    filter_cutoff: f32,
    start_time: u64,
    zone: Option<Arc<Zone>>,
}

impl Voice {
    fn new(sample_rate: f64) -> Self {
        Self {
            active: false,
            note: 0,
            position: 0.0,
            speed: 1.0,
            target_speed: 1.0,
            sample_rate_ratio: 1.0,
            slide_rate: 0.005,
            velocity: 1.0,
            envelope: Envelope::new(sample_rate),
            filter_left: 0.0,
            filter_right: 0.0,
            filter_cutoff: 1_000.0,
            start_time: 0,
            zone: None,
        }
    }

    fn release(&mut self) {
        self.active = false;
        self.envelope.reset();
        self.zone = None;
        self.filter_left = 0.0;
        self.filter_right = 0.0;
    }
}

struct PolySampler {
    sample_rate: f64,
    zones: ArcSwap<Vec<Arc<Zone>>>,
    voices: [Voice; VOICE_COUNT],
    zone_round_robin: [AtomicU32; 128],
    global_time: u64,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    looping: AtomicBool,
}

impl PolySampler {
    fn new(sample_rate: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate
        } else {
            44_100.0
        };
        Self {
            sample_rate,
            zones: ArcSwap::from_pointee(Vec::new()),
            voices: std::array::from_fn(|_| Voice::new(sample_rate)),
            zone_round_robin: std::array::from_fn(|_| AtomicU32::new(0)),
            global_time: 0,
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            looping: AtomicBool::new(false),
        }
    }

    unsafe fn set_zones(&self, input: *const PolySamplerZoneView, count: usize) {
        if count != 0 && input.is_null() {
            return;
        }
        let mut zones = Vec::with_capacity(count);
        let views = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(input, count) }
        };
        for view in views {
            if view.min_note > view.max_note
                || view.min_note > 127
                || view.max_note > 127
                || !view.min_velocity.is_finite()
                || !view.max_velocity.is_finite()
                || view.min_velocity < 0.0
                || view.max_velocity > 1.0
                || view.min_velocity > view.max_velocity
                || (view.right_length != 0 && view.right_length != view.left_length)
                || (view.source_sample_rate != 0.0
                    && (!view.source_sample_rate.is_finite()
                        || !(8_000.0..=384_000.0).contains(&view.source_sample_rate)))
                || (view.loop_enabled
                    && (view.loop_start >= view.loop_end
                        || view.loop_end as usize > view.left_length))
                || view.left_length == 0
                || view.left.is_null()
                || (view.right_length != 0 && view.right.is_null())
            {
                continue;
            }
            let left = unsafe { std::slice::from_raw_parts(view.left, view.left_length) }
                .to_vec()
                .into();
            let right: Arc<[f32]> = if view.right_length == 0 {
                Arc::from([])
            } else {
                unsafe { std::slice::from_raw_parts(view.right, view.right_length) }
                    .to_vec()
                    .into()
            };
            zones.push(Arc::new(Zone {
                min_note: view.min_note,
                max_note: view.max_note,
                min_velocity: view.min_velocity,
                max_velocity: view.max_velocity,
                root_note: view.root_note,
                loop_start: view.loop_start,
                loop_end: view.loop_end,
                loop_enabled: view.loop_enabled,
                source_sample_rate: view.source_sample_rate,
                left,
                right,
            }));
        }
        self.zones.store(Arc::new(zones));
        for counter in &self.zone_round_robin {
            counter.store(0, Ordering::Relaxed);
        }
    }

    fn note_on(&mut self, note: u32, velocity: f32) {
        let mut free = None;
        let mut oldest = 0usize;
        for (index, voice) in self.voices.iter().enumerate() {
            if !voice.active {
                free = Some(index);
                break;
            }
            if voice.start_time < self.voices[oldest].start_time {
                oldest = index;
            }
        }
        let target = free.unwrap_or(oldest);
        let zones = self.zones.load_full();
        let mut selected = None;
        let mut best_velocity_span = u32::MAX;
        let mut best_key_span = u32::MAX;
        let mut candidates = 0u32;
        for (index, zone) in zones.iter().enumerate() {
            if zone.matches(note, velocity) {
                let velocity_span = ((zone.max_velocity - zone.min_velocity) * 1_000_000.0) as u32;
                let key_span = zone.max_note - zone.min_note;
                if velocity_span < best_velocity_span
                    || (velocity_span == best_velocity_span && key_span < best_key_span)
                {
                    selected = Some(index);
                    best_velocity_span = velocity_span;
                    best_key_span = key_span;
                    candidates = 1;
                } else if velocity_span == best_velocity_span && key_span == best_key_span {
                    candidates += 1;
                }
            }
        }
        let Some(mut selected) = selected else {
            return;
        };
        if candidates > 1 {
            let note_slot = (note as usize).min(127);
            let target_ordinal =
                self.zone_round_robin[note_slot].fetch_add(1, Ordering::Relaxed) % candidates;
            let mut ordinal = 0u32;
            for (index, zone) in zones.iter().enumerate() {
                if !zone.matches(note, velocity) {
                    continue;
                }
                let velocity_span = ((zone.max_velocity - zone.min_velocity) * 1_000_000.0) as u32;
                if velocity_span != best_velocity_span
                    || zone.max_note - zone.min_note != best_key_span
                {
                    continue;
                }
                if ordinal == target_ordinal {
                    selected = index;
                    break;
                }
                ordinal += 1;
            }
        }
        let zone = Arc::clone(&zones[selected]);
        let voice = &mut self.voices[target];
        voice.active = true;
        voice.note = note;
        voice.position = 0.0;
        voice.velocity = velocity;
        voice.zone = Some(Arc::clone(&zone));
        let speed = 2.0f32.powf((note as f32 - zone.root_note as f32) / 12.0);
        voice.speed = speed;
        voice.target_speed = speed;
        voice.sample_rate_ratio = if zone.source_sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&zone.source_sample_rate)
            && self.sample_rate > 0.0
        {
            zone.source_sample_rate / self.sample_rate
        } else {
            1.0
        };
        voice.envelope.trigger_on();
        self.global_time = self.global_time.wrapping_add(1);
        voice.start_time = self.global_time;
    }

    fn note_off(&mut self, note: u32) {
        for voice in &mut self.voices {
            if voice.active && voice.note == note && voice.envelope.stage != EnvelopeStage::Release
            {
                voice.envelope.trigger_off();
            }
        }
    }

    fn process_additive(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        let fallback_start = self.loop_start.load(Ordering::Relaxed);
        let fallback_end = self.loop_end.load(Ordering::Relaxed);
        let fallback_looping = self.looping.load(Ordering::Relaxed);
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let Some(zone) = voice.zone.as_ref().cloned() else {
                voice.release();
                continue;
            };
            if zone.left.is_empty() {
                voice.release();
                continue;
            }
            let source_right = if zone.right.is_empty() {
                &zone.left
            } else {
                &zone.right
            };
            let sample_size = zone.left.len();
            let loop_start = if zone.loop_enabled {
                zone.loop_start as u64
            } else {
                fallback_start
            };
            let loop_end = if zone.loop_enabled {
                zone.loop_end as u64
            } else {
                fallback_end
            };
            let looping = (zone.loop_enabled || fallback_looping)
                && loop_end > loop_start.saturating_add(1)
                && loop_end <= sample_size as u64;
            let cutoff = voice.filter_cutoff.clamp(20.0, 20_000.0);
            let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
            let dt = 1.0 / self.sample_rate as f32;
            let alpha = (dt / (rc + dt)).clamp(0.001, 1.0);
            for frame in 0..frames {
                if voice.envelope.stage == EnvelopeStage::Idle {
                    voice.release();
                    break;
                }
                let mut position = voice.position;
                if looping && position >= loop_end as f64 {
                    let loop_length = (loop_end - loop_start) as f64;
                    voice.position =
                        loop_start as f64 + (position - loop_start as f64).max(0.0) % loop_length;
                    position = voice.position;
                }
                if position.floor() as usize >= sample_size {
                    voice.release();
                    break;
                }
                let sample_left =
                    interpolate_sample(&zone.left, position, looping, loop_start, loop_end);
                let sample_right =
                    interpolate_sample(source_right, position, looping, loop_start, loop_end);
                let amp = voice.envelope.next() * voice.velocity;
                let sample_left = sample_left * amp;
                let sample_right = sample_right * amp;
                voice.filter_left += alpha * (sample_left - voice.filter_left);
                voice.filter_right += alpha * (sample_right - voice.filter_right);
                let output_left = if voice.filter_left.is_finite() {
                    voice.filter_left
                } else {
                    sample_left
                };
                let output_right = if voice.filter_right.is_finite() {
                    voice.filter_right
                } else {
                    sample_right
                };
                left[frame] += if output_left.is_finite() {
                    output_left
                } else {
                    0.0
                };
                right[frame] += if output_right.is_finite() {
                    output_right
                } else {
                    0.0
                };
                if (voice.speed - voice.target_speed).abs() < 1.0e-4 {
                    voice.speed = voice.target_speed;
                } else {
                    voice.speed += (voice.target_speed - voice.speed) * voice.slide_rate;
                }
                voice.position += voice.speed as f64 * voice.sample_rate_ratio;
            }
        }
    }
}

fn interpolate_sample(
    data: &[f32],
    position: f64,
    looping: bool,
    loop_start: u64,
    loop_end: u64,
) -> f32 {
    if data.is_empty() || !position.is_finite() {
        return 0.0;
    }
    let at = |raw: i64| -> f32 {
        let index = if looping && loop_end > loop_start + 1 {
            let start = loop_start as i64;
            let end = loop_end as i64;
            let span = end - start;
            if raw < start {
                let i = end - ((start - raw) % span);
                if i == end {
                    start
                } else {
                    i
                }
            } else if raw >= end {
                start + ((raw - start) % span)
            } else {
                raw
            }
        } else {
            raw.clamp(0, data.len() as i64 - 1)
        };
        let value = data[index as usize];
        if value.is_finite() {
            value
        } else {
            0.0
        }
    };
    let integer = position.floor();
    if integer < i64::MIN as f64 || integer > i64::MAX as f64 {
        return 0.0;
    }
    let i = integer as i64;
    let t = (position - integer) as f32;
    let y0 = at(i - 1);
    let y1 = at(i);
    let y2 = at(i + 1);
    let y3 = at(i + 2);
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    let result = ((c3 * t + c2) * t + c1) * t + c0;
    if result.is_finite() {
        result
    } else {
        y1
    }
}

unsafe fn with_state<R>(state: *mut c_void, f: impl FnOnce(&mut PolySampler) -> R) -> Option<R> {
    if state.is_null() {
        None
    } else {
        Some(f(unsafe { &mut *state.cast::<PolySampler>() }))
    }
}

#[no_mangle]
pub extern "C" fn hirari_poly_sampler_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(PolySampler::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_destroy(state: *mut c_void) {
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state.cast::<PolySampler>())) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_set_zones(
    state: *mut c_void,
    zones: *const PolySamplerZoneView,
    count: usize,
) {
    if !state.is_null() {
        unsafe { (&*state.cast::<PolySampler>()).set_zones(zones, count) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_note_on(state: *mut c_void, note: u32, velocity: f32) {
    let _ = unsafe { with_state(state, |sampler| sampler.note_on(note, velocity)) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_note_off(state: *mut c_void, note: u32) {
    let _ = unsafe { with_state(state, |sampler| sampler.note_off(note)) };
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_set_loop(
    state: *mut c_void,
    start: u64,
    end: u64,
    enabled: bool,
) -> bool {
    if state.is_null() {
        return false;
    }
    let sampler = unsafe { &*state.cast::<PolySampler>() };
    if !enabled {
        sampler.loop_start.store(0, Ordering::Relaxed);
        sampler.loop_end.store(0, Ordering::Relaxed);
        sampler.looping.store(false, Ordering::Relaxed);
        return true;
    }
    if start >= end || end - start < 2 {
        return false;
    }
    sampler.loop_start.store(start, Ordering::Relaxed);
    sampler.loop_end.store(end, Ordering::Relaxed);
    sampler.looping.store(true, Ordering::Relaxed);
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_is_looping(state: *const c_void) -> bool {
    !state.is_null()
        && unsafe {
            (&*state.cast::<PolySampler>())
                .looping
                .load(Ordering::Relaxed)
        }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_loop_start(state: *const c_void) -> u64 {
    if state.is_null() {
        0
    } else {
        unsafe {
            (&*state.cast::<PolySampler>())
                .loop_start
                .load(Ordering::Relaxed)
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_loop_end(state: *const c_void) -> u64 {
    if state.is_null() {
        0
    } else {
        unsafe {
            (&*state.cast::<PolySampler>())
                .loop_end
                .load(Ordering::Relaxed)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_poly_sampler_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
    clear: bool,
) {
    if state.is_null() || left.is_null() || right.is_null() || frames == 0 {
        return;
    }
    let sampler = unsafe { &mut *state.cast::<PolySampler>() };
    let left = unsafe { std::slice::from_raw_parts_mut(left, frames) };
    let right = unsafe { std::slice::from_raw_parts_mut(right, frames) };
    if clear {
        left.fill(0.0);
        right.fill(0.0);
    }
    sampler.process_additive(left, right);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_poly_sampler_reference_process(
            sample: *const f32,
            sample_size: u32,
            output_left: *mut f32,
            output_right: *mut f32,
            frames: u32,
            sample_rate: f64,
            source_rate: f64,
            loop_start: u32,
            loop_end: u32,
            release_at: u32,
            velocity: f32,
        );
    }

    #[test]
    fn zone_sampler_renders_pitched_looped_voice_and_releases_it() {
        let mut sampler = PolySampler::new(48_000.0);
        let samples = [0.25f32, -0.25, 0.5, -0.5];
        let zone = PolySamplerZoneView {
            min_note: 60,
            max_note: 60,
            min_velocity: 0.0,
            max_velocity: 1.0,
            root_note: 60,
            loop_start: 1,
            loop_end: 4,
            loop_enabled: true,
            source_sample_rate: 48_000.0,
            left: samples.as_ptr(),
            left_length: samples.len(),
            right: std::ptr::null(),
            right_length: 0,
        };
        unsafe {
            sampler.set_zones(&zone, 1);
        }
        sampler.note_on(60, 0.75);
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        sampler.process_additive(&mut left, &mut right);
        assert!(left.iter().any(|sample| sample.abs() > 0.01));
        assert_eq!(left, right);
        sampler.note_off(60);
        let mut release = [0.0; 200_000];
        let mut release_right = [0.0; 200_000];
        sampler.process_additive(&mut release, &mut release_right);
        assert!(sampler.voices.iter().all(|voice| !voice.active));
    }

    #[test]
    fn fallback_loop_rejects_invalid_range_and_can_be_disabled() {
        let mut sampler = PolySampler::new(44_100.0);
        assert!(!unsafe {
            hirari_poly_sampler_set_loop((&mut sampler as *mut PolySampler).cast(), 8, 9, true)
        });
        assert!(unsafe {
            hirari_poly_sampler_set_loop((&mut sampler as *mut PolySampler).cast(), 4, 12, true)
        });
        assert!(sampler.looping.load(Ordering::Relaxed));
        assert!(unsafe {
            hirari_poly_sampler_set_loop((&mut sampler as *mut PolySampler).cast(), 0, 0, false)
        });
        assert!(!sampler.looping.load(Ordering::Relaxed));
    }

    #[test]
    fn zone_view_abi_matches_cpp_layout() {
        assert_eq!(std::mem::size_of::<PolySamplerZoneView>(), 72);
        assert_eq!(
            std::mem::offset_of!(PolySamplerZoneView, source_sample_rate),
            32
        );
        assert_eq!(std::mem::offset_of!(PolySamplerZoneView, right_length), 64);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn rust_voice_render_matches_frozen_cpp_reference() {
        let mut sampler = PolySampler::new(48_000.0);
        let samples = [0.25f32, -0.25, 0.5, -0.5];
        let zone = PolySamplerZoneView {
            min_note: 60,
            max_note: 60,
            min_velocity: 0.0,
            max_velocity: 1.0,
            root_note: 60,
            loop_start: 1,
            loop_end: 4,
            loop_enabled: true,
            source_sample_rate: 48_000.0,
            left: samples.as_ptr(),
            left_length: samples.len(),
            right: std::ptr::null(),
            right_length: 0,
        };
        unsafe { sampler.set_zones(&zone, 1) };
        sampler.note_on(60, 0.75);

        let frame_count = 200_000;
        let release_at = 64;
        let mut actual_left = vec![0.0f32; frame_count];
        let mut actual_right = vec![0.0f32; frame_count];
        sampler.process_additive(
            &mut actual_left[..release_at],
            &mut actual_right[..release_at],
        );
        sampler.note_off(60);
        sampler.process_additive(
            &mut actual_left[release_at..],
            &mut actual_right[release_at..],
        );

        let mut expected_left = vec![0.0f32; frame_count];
        let mut expected_right = vec![0.0f32; frame_count];
        unsafe {
            hirari_poly_sampler_reference_process(
                samples.as_ptr(),
                samples.len() as u32,
                expected_left.as_mut_ptr(),
                expected_right.as_mut_ptr(),
                frame_count as u32,
                48_000.0,
                48_000.0,
                1,
                4,
                release_at as u32,
                0.75,
            );
        }
        for (actual, expected) in actual_left.iter().zip(&expected_left) {
            assert!(
                (actual - expected).abs() <= 1.0e-6,
                "{actual} != {expected}"
            );
        }
        assert_eq!(actual_left, actual_right);
        assert_eq!(expected_left, expected_right);
    }
}
