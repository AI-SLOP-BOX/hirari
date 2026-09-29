//! Rust owner for the former native Alchemy sampler modes.
//!
//! Sample memory is owned through `Arc<[f32]>`; realtime voice and grain
//! processing only reads prepared sample data and does not allocate.

use std::sync::Arc;

const VOICE_COUNT: usize = 128;
const GRAIN_COUNT: usize = 256;
const OSCILLATOR_COUNT: usize = 1024;
const ZONE_LIMIT: usize = 256;
const SPECTRAL_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlchemyEngineType {
    Granular,
    Additive,
    Spectral,
    #[default]
    Classic,
}

#[derive(Clone, Debug)]
pub struct SampleZone {
    pub low_key: u8,
    pub high_key: u8,
    pub low_velocity: u8,
    pub high_velocity: u8,
    pub root_key: u8,
    pub sample_rate: f64,
    pub samples: Arc<[f32]>,
}

impl SampleZone {
    pub fn new(
        low_key: u8,
        high_key: u8,
        low_velocity: u8,
        high_velocity: u8,
        root_key: u8,
        sample_rate: f64,
        samples: Arc<[f32]>,
    ) -> Option<Self> {
        if low_key > high_key
            || high_key > 127
            || low_velocity > high_velocity
            || samples.len() < 2
            || (sample_rate != 0.0
                && (!sample_rate.is_finite() || !(8_000.0..=384_000.0).contains(&sample_rate)))
        {
            return None;
        }
        Some(Self {
            low_key,
            high_key,
            low_velocity,
            high_velocity,
            root_key: root_key.min(127),
            sample_rate,
            samples,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AlchemyMidiEvent {
    pub sample_offset: u64,
    pub size: u8,
    pub data: [u8; 3],
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    zone_index: usize,
    position: f64,
    note: u8,
    channel: u8,
    velocity: f32,
}

#[derive(Clone, Copy, Default)]
struct Grain {
    active: bool,
    zone_index: usize,
    position: f64,
    step: f64,
    duration: u32,
    current_sample: u32,
    velocity: f32,
}

#[derive(Clone, Copy, Default)]
struct Oscillator {
    amplitude: f32,
    y1: f32,
    y2: f32,
    coefficient: f32,
}

#[derive(Clone, Copy, Default)]
struct ComplexSample {
    real: f32,
    imaginary: f32,
}

impl ComplexSample {
    fn multiply(self, other: Self) -> Self {
        Self {
            real: self.real * other.real - self.imaginary * other.imaginary,
            imaginary: self.real * other.imaginary + self.imaginary * other.real,
        }
    }
}

/// Multisample playback plus the four render modes previously held by the
/// unreferenced C++ `AlchemySamplerCore` translation unit.
pub struct AlchemySampler {
    pub engine: AlchemyEngineType,
    sample_rate: f64,
    zones: Vec<SampleZone>,
    voices: [Voice; VOICE_COUNT],
    grains: [Grain; GRAIN_COUNT],
    oscillators: [Oscillator; OSCILLATOR_COUNT],
    modulation: [f32; 128],
    grain_sample_accumulator: u64,
    random_state: u64,
    spectral_workspace: [ComplexSample; SPECTRAL_SIZE],
}

impl Default for AlchemySampler {
    fn default() -> Self {
        Self::new()
    }
}

impl AlchemySampler {
    pub fn new() -> Self {
        Self {
            engine: AlchemyEngineType::Classic,
            sample_rate: 44_100.0,
            zones: Vec::with_capacity(ZONE_LIMIT),
            voices: [Voice::default(); VOICE_COUNT],
            grains: [Grain::default(); GRAIN_COUNT],
            oscillators: [Oscillator::default(); OSCILLATOR_COUNT],
            modulation: [0.0; 128],
            grain_sample_accumulator: 0,
            random_state: 1337,
            spectral_workspace: [ComplexSample::default(); SPECTRAL_SIZE],
        }
    }

    pub fn set_engine(&mut self, engine: AlchemyEngineType) {
        self.engine = engine;
    }

    /// Zone changes are control-thread operations. The owned `Arc` keeps any
    /// active voice's immutable sample alive until that voice stops.
    pub fn add_zone(&mut self, zone: SampleZone) -> bool {
        if self.zones.len() >= ZONE_LIMIT {
            return false;
        }
        self.zones.push(zone);
        true
    }

    pub fn clear_zones(&mut self) {
        self.zones.clear();
        for voice in &mut self.voices {
            *voice = Voice::default();
        }
        for grain in &mut self.grains {
            *grain = Grain::default();
        }
    }

    pub fn update_modulation(&mut self, sources: &[(u32, f32)]) {
        for &(kind, value) in sources {
            if kind < self.modulation.len() as u32 && value.is_finite() {
                self.modulation[kind as usize] = value;
            }
        }
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44_100.0
        };
        let base_frequency = 110.0f64;
        for (index, oscillator) in self.oscillators.iter_mut().enumerate() {
            let harmonic = index + 1;
            let frequency = base_frequency * harmonic as f64;
            if frequency > self.sample_rate / 2.0 {
                oscillator.amplitude = 0.0;
                continue;
            }
            oscillator.amplitude = 0.2 / harmonic as f32;
            let omega = std::f64::consts::TAU * frequency / self.sample_rate;
            oscillator.coefficient = 2.0 * omega.cos() as f32;
            oscillator.y1 = omega.sin() as f32;
            oscillator.y2 = 0.0;
        }
    }

    pub fn reset(&mut self) {
        self.grain_sample_accumulator = 0;
        self.voices.fill(Voice::default());
        self.grains.fill(Grain::default());
        for oscillator in &mut self.oscillators {
            oscillator.y1 = 0.0;
            oscillator.y2 = 0.0;
        }
    }

    /// Adds sampler output to a stereo block. Invalid channel layouts are
    /// cleared, matching the old processor's fail-silent behavior.
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        midi_events: &mut [AlchemyMidiEvent],
    ) {
        if left.is_empty() || left.len() != right.len() {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        match self.engine {
            AlchemyEngineType::Classic => self.process_classic(left, right, midi_events),
            AlchemyEngineType::Granular => self.process_granular(left, right),
            AlchemyEngineType::Additive => self.process_additive(left, right),
            AlchemyEngineType::Spectral => self.process_spectral(left, right),
        }
    }

    fn process_classic(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        events: &mut [AlchemyMidiEvent],
    ) {
        for index in 1..events.len() {
            let current = events[index];
            let priority = midi_priority(current);
            let mut slot = index;
            while slot > 0
                && (events[slot - 1].sample_offset > current.sample_offset
                    || (events[slot - 1].sample_offset == current.sample_offset
                        && midi_priority(events[slot - 1]) > priority))
            {
                events[slot] = events[slot - 1];
                slot -= 1;
            }
            events[slot] = current;
        }

        let mut event_index = 0;
        for (sample_index, (out_l, out_r)) in left.iter_mut().zip(right).enumerate() {
            while event_index < events.len()
                && events[event_index].sample_offset <= sample_index as u64
            {
                let event = events[event_index];
                event_index += 1;
                if event.size < 3 {
                    continue;
                }
                let status = event.data[0] & 0xf0;
                let channel = event.data[0] & 0x0f;
                let note = event.data[1];
                let velocity = event.data[2];
                if status == 0x90 && velocity > 0 {
                    if let Some(zone_index) = self.zones.iter().position(|zone| {
                        note >= zone.low_key
                            && note <= zone.high_key
                            && velocity >= zone.low_velocity
                            && velocity <= zone.high_velocity
                    }) {
                        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
                            *voice = Voice {
                                active: true,
                                zone_index,
                                position: 0.0,
                                note,
                                channel,
                                velocity: velocity as f32 / 127.0,
                            };
                        }
                    }
                } else if status == 0x80 || (status == 0x90 && velocity == 0) {
                    for voice in &mut self.voices {
                        if voice.active && voice.note == note && voice.channel == channel {
                            voice.active = false;
                        }
                    }
                }
            }

            for voice in &mut self.voices {
                if !voice.active {
                    continue;
                }
                let Some(zone) = self.zones.get(voice.zone_index) else {
                    voice.active = false;
                    continue;
                };
                let source_rate = if zone.sample_rate.is_finite() && zone.sample_rate > 0.0 {
                    zone.sample_rate
                } else {
                    44_100.0
                };
                let step = 2.0f64.powf((voice.note as f64 - zone.root_key as f64) / 12.0)
                    * source_rate
                    / self.sample_rate;
                let index = voice.position as usize;
                if index + 1 >= zone.samples.len() {
                    voice.active = false;
                    continue;
                }
                let fraction = (voice.position - index as f64) as f32;
                let first = zone.samples[index];
                let second = zone.samples[index + 1];
                let value = (first + fraction * (second - first)) * voice.velocity;
                *out_l += value;
                *out_r += value;
                voice.position += step;
            }
        }
    }

    fn process_granular(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.grain_sample_accumulator = self
            .grain_sample_accumulator
            .saturating_add(left.len() as u64);
        let interval = (self.sample_rate * 0.05).max(1.0) as u64;
        if self.grain_sample_accumulator >= interval && !self.zones.is_empty() {
            self.grain_sample_accumulator %= interval;
            let usable_count = self
                .zones
                .iter()
                .filter(|zone| zone.samples.len() >= 2)
                .count();
            if usable_count > 0 {
                if let Some(grain_index) = self.grains.iter().position(|grain| !grain.active) {
                    let selected = self.next_random() as usize % usable_count;
                    let zone_index = self
                        .zones
                        .iter()
                        .enumerate()
                        .filter(|(_, zone)| zone.samples.len() >= 2)
                        .nth(selected)
                        .map(|(index, _)| index);
                    if let Some(zone_index) = zone_index {
                        let sample_count = self.zones[zone_index].samples.len();
                        let source_sample_rate = self.zones[zone_index].sample_rate;
                        let position = self.next_random() as usize % sample_count;
                        let source_rate =
                            if source_sample_rate.is_finite() && source_sample_rate > 0.0 {
                                source_sample_rate
                            } else {
                                44_100.0
                            };
                        self.grains[grain_index] = Grain {
                            active: true,
                            zone_index,
                            position: position as f64,
                            step: source_rate / self.sample_rate,
                            duration: (self.sample_rate * 0.1).max(1.0) as u32,
                            current_sample: 0,
                            velocity: 0.5,
                        };
                    }
                }
            }
        }

        for grain in &mut self.grains {
            if !grain.active {
                continue;
            }
            let Some(zone) = self.zones.get(grain.zone_index) else {
                grain.active = false;
                continue;
            };
            let remaining = grain.duration.saturating_sub(grain.current_sample);
            let count = (left.len() as u32).min(remaining) as usize;
            for index in 0..count {
                let source_index = grain.position as usize;
                if source_index >= zone.samples.len() {
                    grain.active = false;
                    break;
                }
                let phase = grain.current_sample as f32 / grain.duration as f32;
                let window = 0.5 * (1.0 - (std::f32::consts::TAU * phase).cos());
                let value = zone.samples[source_index] * window * grain.velocity;
                left[index] += value;
                right[index] += value;
                grain.position += grain.step;
                grain.current_sample += 1;
            }
            if grain.current_sample >= grain.duration {
                grain.active = false;
            }
        }
    }

    fn process_additive(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (out_l, out_r) in left.iter_mut().zip(right) {
            let mut sum = 0.0f32;
            for oscillator in &mut self.oscillators {
                if oscillator.amplitude < 0.0001 {
                    continue;
                }
                let sample = oscillator.coefficient * oscillator.y1 - oscillator.y2;
                oscillator.y2 = oscillator.y1;
                oscillator.y1 = sample;
                sum += sample * oscillator.amplitude;
            }
            *out_l += sum;
            *out_r += sum;
        }
    }

    fn process_spectral(&mut self, left: &mut [f32], right: &mut [f32]) {
        for offset in (0..left.len()).step_by(SPECTRAL_SIZE) {
            let count = (left.len() - offset).min(SPECTRAL_SIZE);
            if count < 128 {
                break;
            }
            for index in 0..count {
                self.spectral_workspace[index] = ComplexSample {
                    real: (left[offset + index] + right[offset + index]) * 0.5,
                    imaginary: 0.0,
                };
            }
            self.spectral_workspace[count..].fill(ComplexSample::default());
            fft(&mut self.spectral_workspace, false);
            for bin in 100..SPECTRAL_SIZE - 100 {
                self.spectral_workspace[bin].real *= 0.05;
                self.spectral_workspace[bin].imaginary *= 0.05;
            }
            fft(&mut self.spectral_workspace, true);
            for index in 0..count {
                let output = self.spectral_workspace[index].real;
                left[offset + index] = output;
                right[offset + index] = output;
            }
        }
    }

    fn next_random(&mut self) -> u64 {
        let mut value = self.random_state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.random_state = value.max(1);
        self.random_state
    }

    pub fn audit(&self) -> bool {
        self.sample_rate.is_finite()
            && self.sample_rate > 0.0
            && self.zones.len() <= ZONE_LIMIT
            && self.zones.iter().all(|zone| {
                zone.low_key <= zone.high_key
                    && zone.low_velocity <= zone.high_velocity
                    && zone.samples.len() >= 2
                    && zone.samples.iter().all(|sample| sample.is_finite())
            })
            && self.modulation.iter().all(|value| value.is_finite())
    }
}

fn midi_priority(event: AlchemyMidiEvent) -> u8 {
    if event.size < 3 {
        return 1;
    }
    match (event.data[0] & 0xf0, event.data[1], event.data[2]) {
        (0xb0, 123, _) => 0,
        (0x80, _, _) | (0x90, _, 0) => 1,
        (0x90, _, _) => 3,
        _ => 2,
    }
}

fn fft(samples: &mut [ComplexSample; SPECTRAL_SIZE], inverse: bool) {
    let mut reversed = 0usize;
    for index in 1..SPECTRAL_SIZE {
        let mut bit = SPECTRAL_SIZE >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            samples.swap(index, reversed);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut length = 2;
    while length <= SPECTRAL_SIZE {
        let angle = sign * std::f32::consts::TAU / length as f32;
        let root = ComplexSample {
            real: angle.cos(),
            imaginary: angle.sin(),
        };
        for start in (0..SPECTRAL_SIZE).step_by(length) {
            let mut twiddle = ComplexSample {
                real: 1.0,
                imaginary: 0.0,
            };
            for offset in 0..length / 2 {
                let even_index = start + offset;
                let odd_index = even_index + length / 2;
                let even = samples[even_index];
                let odd = samples[odd_index].multiply(twiddle);
                samples[even_index] = ComplexSample {
                    real: even.real + odd.real,
                    imaginary: even.imaginary + odd.imaginary,
                };
                samples[odd_index] = ComplexSample {
                    real: even.real - odd.real,
                    imaginary: even.imaginary - odd.imaginary,
                };
                twiddle = twiddle.multiply(root);
            }
        }
        length <<= 1;
    }
    if inverse {
        let scale = 1.0 / SPECTRAL_SIZE as f32;
        for sample in samples {
            sample.real *= scale;
            sample.imaginary *= scale;
        }
    }
}
