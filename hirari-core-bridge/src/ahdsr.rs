use std::ffi::c_void;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

struct AhdsrEnvelope {
    sample_rate: f64,
    state: State,
    current_value: f32,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    counter: u32,
}

impl AhdsrEnvelope {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: if sample_rate.is_finite() && sample_rate >= 1000.0 {
                sample_rate
            } else {
                44100.0
            },
            state: State::Idle,
            current_value: 0.0,
            attack: 0.01,
            hold: 0.0,
            decay: 0.1,
            sustain: 0.7,
            release: 0.2,
            counter: 0,
        }
    }

    fn set_parameters(&mut self, attack: f32, hold: f32, decay: f32, sustain: f32, release: f32) {
        self.attack = sanitize_time(attack, 0.01);
        self.hold = sanitize_time(hold, 0.0);
        self.decay = sanitize_time(decay, 0.1);
        self.sustain = if sustain.is_finite() {
            sustain.clamp(0.0, 1.0)
        } else {
            0.7
        };
        self.release = sanitize_time(release, 0.2);
    }

    fn reset(&mut self) {
        self.state = State::Idle;
        self.current_value = 0.0;
        self.counter = 0;
    }

    fn trigger(&mut self) {
        self.state = State::Attack;
        self.current_value = 0.0;
    }

    fn release(&mut self) {
        self.state = State::Release;
    }

    fn next_value(&mut self) -> f32 {
        match self.state {
            State::Attack => {
                self.current_value += (1.0 / (self.attack as f64 * self.sample_rate + 1.0)) as f32;
                if self.current_value >= 1.0 {
                    self.current_value = 1.0;
                    self.state = State::Hold;
                    self.counter = (self.hold as f64 * self.sample_rate) as u32;
                }
            }
            State::Hold => {
                if self.counter > 0 {
                    self.counter -= 1;
                } else {
                    self.state = State::Decay;
                }
            }
            State::Decay => {
                self.current_value -= ((1.0_f32 - self.sustain) as f64
                    / (self.decay as f64 * self.sample_rate + 1.0))
                    as f32;
                if self.current_value <= self.sustain {
                    self.current_value = self.sustain;
                    self.state = State::Sustain;
                }
            }
            State::Sustain => {}
            State::Release => {
                self.current_value -=
                    (self.sustain as f64 / (self.release as f64 * self.sample_rate + 1.0)) as f32;
                if self.current_value <= 0.0 {
                    self.current_value = 0.0;
                    self.state = State::Idle;
                }
            }
            State::Idle => self.current_value = 0.0,
        }
        self.current_value
    }

    fn is_active(&self) -> bool {
        self.state != State::Idle
    }
}

fn sanitize_time(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(1.0e-5, 60.0)
    } else {
        fallback
    }
}

#[no_mangle]
pub extern "C" fn hirari_ahdsr_create(sample_rate: f64) -> *mut c_void {
    Box::into_raw(Box::new(AhdsrEnvelope::new(sample_rate))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_destroy(envelope: *mut c_void) {
    if !envelope.is_null() {
        drop(Box::from_raw(envelope.cast::<AhdsrEnvelope>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_set_parameters(
    envelope: *mut c_void,
    attack: f32,
    hold: f32,
    decay: f32,
    sustain: f32,
    release: f32,
) {
    if let Some(envelope) = envelope.cast::<AhdsrEnvelope>().as_mut() {
        envelope.set_parameters(attack, hold, decay, sustain, release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_reset(envelope: *mut c_void) {
    if let Some(envelope) = envelope.cast::<AhdsrEnvelope>().as_mut() {
        envelope.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_trigger(envelope: *mut c_void) {
    if let Some(envelope) = envelope.cast::<AhdsrEnvelope>().as_mut() {
        envelope.trigger();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_release(envelope: *mut c_void) {
    if let Some(envelope) = envelope.cast::<AhdsrEnvelope>().as_mut() {
        envelope.release();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_next_value(envelope: *mut c_void) -> f32 {
    envelope
        .cast::<AhdsrEnvelope>()
        .as_mut()
        .map_or(0.0, AhdsrEnvelope::next_value)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_ahdsr_is_active(envelope: *const c_void) -> bool {
    envelope
        .cast::<AhdsrEnvelope>()
        .as_ref()
        .is_some_and(AhdsrEnvelope::is_active)
}
