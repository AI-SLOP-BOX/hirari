use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const BUFFER_SIZE: usize = 1024;

pub struct VirtuosoTapeEngine {
    sample_rate: f64,
    delay_l: [f32; BUFFER_SIZE],
    delay_r: [f32; BUFFER_SIZE],
    write_idx: usize,
    z1: [f32; 2],
    wow_phase: f64,
    flutter_phase: f64,
    rng_state: u32,
    drive_db: AtomicU32,
    hiss_level: AtomicU32,
    wow_depth: AtomicU32,
    mix: AtomicU32,
}

impl VirtuosoTapeEngine {
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate > 0.0 {
            sample_rate
        } else {
            44_100.0
        };
        let time_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0xACE1, |duration| {
                duration.subsec_nanos() ^ duration.as_secs() as u32
            });
        let mut engine = Self {
            sample_rate: rate,
            delay_l: [0.0; BUFFER_SIZE],
            delay_r: [0.0; BUFFER_SIZE],
            write_idx: 0,
            z1: [0.0; 2],
            wow_phase: 0.0,
            flutter_phase: 0.0,
            rng_state: if time_seed == 0 { 0xACE1 } else { time_seed },
            drive_db: AtomicU32::new(12.0_f32.to_bits()),
            hiss_level: AtomicU32::new(0.00001_f32.to_bits()),
            wow_depth: AtomicU32::new(0.15_f32.to_bits()),
            mix: AtomicU32::new(1.0_f32.to_bits()),
        };
        engine.reset();
        engine
    }

    fn load(parameter: &AtomicU32) -> f32 {
        f32::from_bits(parameter.load(Ordering::Relaxed))
    }

    fn store(parameter: &AtomicU32, value: f32) {
        parameter.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn prepare_to_play(&mut self, sample_rate: f64) {
        self.sample_rate =
            if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate) {
                sample_rate
            } else {
                44_100.0
            };
        self.reset();
    }

    pub fn reset(&mut self) {
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.z1 = [0.0; 2];
        self.write_idx = 0;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
    }

    fn set_control(&self, control: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        match control {
            0 => Self::store(&self.drive_db, value.clamp(-12.0, 36.0)),
            1 => Self::store(&self.hiss_level, value.clamp(0.0, 0.002)),
            2 => Self::store(&self.wow_depth, value.clamp(0.0, 1.0)),
            3 => Self::store(&self.mix, value.clamp(0.0, 1.0)),
            _ => {}
        }
    }

    pub fn set_parameter(&self, id: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match id {
            0 => self.set_control(0, -12.0 + value * 48.0),
            1 => self.set_control(1, value * 0.002),
            2 => self.set_control(2, value),
            3 => self.set_control(3, value),
            _ => {}
        }
    }

    pub fn get_parameter(&self, id: u32) -> f32 {
        match id {
            0 => ((Self::load(&self.drive_db) + 12.0) / 48.0).clamp(0.0, 1.0),
            1 => (Self::load(&self.hiss_level) / 0.002).clamp(0.0, 1.0),
            2 => Self::load(&self.wow_depth).clamp(0.0, 1.0),
            3 => Self::load(&self.mix).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    fn next_noise(&mut self) -> f32 {
        let mut value = self.rng_state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.rng_state = value;
        (value as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn update_lfos(&mut self) {
        let tau = std::f64::consts::TAU;
        self.wow_phase += (tau * 0.5) / self.sample_rate;
        self.flutter_phase += (tau * 15.6) / self.sample_rate;
        if self.wow_phase > tau {
            self.wow_phase -= tau;
        }
        if self.flutter_phase > tau {
            self.flutter_phase -= tau;
        }
    }

    pub fn process(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>) {
        let frames = right
            .as_ref()
            .map_or(left.len(), |channel| left.len().min(channel.len()));
        if frames == 0 {
            return;
        }
        let drive = 10.0_f32.powf(Self::load(&self.drive_db).clamp(-12.0, 36.0) / 20.0);
        let hiss_level = Self::load(&self.hiss_level);
        let wow_depth = Self::load(&self.wow_depth);
        let mix = Self::load(&self.mix);

        for frame in 0..frames {
            let in_l = if left[frame].is_finite() {
                left[frame]
            } else {
                0.0
            };
            let in_r = right.as_ref().map_or(in_l, |channel| {
                if channel[frame].is_finite() {
                    channel[frame]
                } else {
                    in_l
                }
            });
            self.update_lfos();

            let wow = self.wow_phase.sin() as f32 * wow_depth * 50.0;
            let flutter = self.flutter_phase.sin() as f32 * 0.05 * 6.0;
            let delay = (50.0 + wow + flutter).clamp(1.0, (BUFFER_SIZE - 2) as f32);

            self.delay_l[self.write_idx] = in_l;
            self.delay_r[self.write_idx] = in_r;
            let read_l = Self::read_interpolated(&self.delay_l, self.write_idx, delay);
            let read_r = Self::read_interpolated(&self.delay_r, self.write_idx, delay);
            let output_l = self.process_channel(0, in_l, read_l, drive, hiss_level, mix);
            left[frame] = output_l;
            if let Some(channel) = right.as_deref_mut() {
                channel[frame] = self.process_channel(1, in_r, read_r, drive, hiss_level, mix);
            }
            self.write_idx = (self.write_idx + 1) % BUFFER_SIZE;
        }
    }

    fn read_interpolated(buffer: &[f32; BUFFER_SIZE], write_idx: usize, delay: f32) -> f32 {
        let mut read_pos = write_idx as f32 + BUFFER_SIZE as f32 - delay;
        while read_pos >= BUFFER_SIZE as f32 {
            read_pos -= BUFFER_SIZE as f32;
        }
        let index0 = read_pos as usize;
        let index1 = (index0 + 1) % BUFFER_SIZE;
        let fraction = read_pos - index0 as f32;
        buffer[index0] * (1.0 - fraction) + buffer[index1] * fraction
    }

    fn process_channel(
        &mut self,
        channel: usize,
        input: f32,
        delayed: f32,
        drive: f32,
        hiss_level: f32,
        mix: f32,
    ) -> f32 {
        let bias = 0.05;
        let biased = delayed * drive + bias;
        let saturated = biased.tanh() - bias.tanh() + self.next_noise() * hiss_level;
        self.z1[channel] += (saturated - self.z1[channel]) * 0.78;
        if !self.z1[channel].is_finite() {
            self.z1[channel] = 0.0;
        }
        input * (1.0 - mix) + self.z1[channel] * mix
    }
}

#[no_mangle]
pub extern "C" fn hirari_virtuoso_tape_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(VirtuosoTapeEngine::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_destroy(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state.cast::<VirtuosoTapeEngine>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_prepare(state: *mut c_void, sample_rate: f64) {
    if let Some(state) = state.cast::<VirtuosoTapeEngine>().as_mut() {
        state.prepare_to_play(sample_rate);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<VirtuosoTapeEngine>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_set_parameter(
    state: *const c_void,
    id: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<VirtuosoTapeEngine>().as_ref() {
        state.set_parameter(id, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_get_parameter(state: *const c_void, id: u32) -> f32 {
    state
        .cast::<VirtuosoTapeEngine>()
        .as_ref()
        .map_or(0.0, |state| state.get_parameter(id))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_set_control(
    state: *const c_void,
    control: u32,
    value: f32,
) {
    if let Some(state) = state.cast::<VirtuosoTapeEngine>().as_ref() {
        state.set_control(control, value);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_virtuoso_tape_process(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) {
    if left.is_null() {
        return;
    }
    let Some(state) = state.cast::<VirtuosoTapeEngine>().as_mut() else {
        return;
    };
    let left = std::slice::from_raw_parts_mut(left, frames);
    if right.is_null() {
        state.process(left, None);
    } else {
        state.process(left, Some(std::slice::from_raw_parts_mut(right, frames)));
    }
}
