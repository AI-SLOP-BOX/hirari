use std::ffi::c_void;

const MAX_DELAY: usize = 8192;
const CAPACITY: usize = MAX_DELAY + 1;

struct TrackPdcDelay {
    delay_l: Box<[f32; CAPACITY]>,
    delay_r: Box<[f32; CAPACITY]>,
    pre_delay_l: Box<[f32; CAPACITY]>,
    pre_delay_r: Box<[f32; CAPACITY]>,
    previous: usize,
    active: usize,
    transition_remaining: u32,
    write: usize,
}

impl TrackPdcDelay {
    fn new() -> Self {
        Self {
            delay_l: Box::new([0.0; CAPACITY]),
            delay_r: Box::new([0.0; CAPACITY]),
            pre_delay_l: Box::new([0.0; CAPACITY]),
            pre_delay_r: Box::new([0.0; CAPACITY]),
            previous: 0,
            active: 0,
            transition_remaining: 0,
            write: 0,
        }
    }

    fn delayed(&self, delay: usize, right: bool, pre_tap: bool) -> f32 {
        if delay == 0 {
            return 0.0;
        }
        let read = (self.write + CAPACITY - delay) % CAPACITY;
        if pre_tap {
            if right {
                self.pre_delay_r[read]
            } else {
                self.pre_delay_l[read]
            }
        } else if right {
            self.delay_r[read]
        } else {
            self.delay_l[read]
        }
    }

    fn reset(&mut self, requested: u32) {
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.pre_delay_l.fill(0.0);
        self.pre_delay_r.fill(0.0);
        self.write = 0;
        self.active = (requested as usize).min(MAX_DELAY);
        self.previous = self.active;
        self.transition_remaining = 0;
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn process(
        &mut self,
        left: *mut f32,
        right: *mut f32,
        pre_left: *mut f32,
        pre_right: *mut f32,
        frames: u32,
        requested: u32,
    ) {
        let requested = (requested as usize).min(MAX_DELAY);
        if requested != self.active {
            self.previous = self.active;
            self.active = requested;
            self.transition_remaining = 64;
        }
        for index in 0..frames as usize {
            let in_l = *left.add(index);
            let in_r = *right.add(index);
            let pre_in_l = if pre_left.is_null() { 0.0 } else { *pre_left.add(index) };
            let pre_in_r = if pre_right.is_null() { 0.0 } else { *pre_right.add(index) };
            let old_l = if self.previous == 0 {
                in_l
            } else {
                self.delayed(self.previous, false, false)
            };
            let old_r = if self.previous == 0 {
                in_r
            } else {
                self.delayed(self.previous, true, false)
            };
            let new_l = if self.active == 0 {
                in_l
            } else {
                self.delayed(self.active, false, false)
            };
            let new_r = if self.active == 0 {
                in_r
            } else {
                self.delayed(self.active, true, false)
            };
            let old_pre_l = if self.previous == 0 {
                pre_in_l
            } else {
                self.delayed(self.previous, false, true)
            };
            let old_pre_r = if self.previous == 0 {
                pre_in_r
            } else {
                self.delayed(self.previous, true, true)
            };
            let new_pre_l = if self.active == 0 {
                pre_in_l
            } else {
                self.delayed(self.active, false, true)
            };
            let new_pre_r = if self.active == 0 {
                pre_in_r
            } else {
                self.delayed(self.active, true, true)
            };
            if self.transition_remaining > 0 {
                let progress = (64 - self.transition_remaining) as f32 / 63.0;
                *left.add(index) = old_l + (new_l - old_l) * progress;
                *right.add(index) = old_r + (new_r - old_r) * progress;
                if !pre_left.is_null() && !pre_right.is_null() {
                    *pre_left.add(index) = old_pre_l + (new_pre_l - old_pre_l) * progress;
                    *pre_right.add(index) = old_pre_r + (new_pre_r - old_pre_r) * progress;
                }
                self.transition_remaining -= 1;
            } else {
                *left.add(index) = new_l;
                *right.add(index) = new_r;
                if !pre_left.is_null() && !pre_right.is_null() {
                    *pre_left.add(index) = new_pre_l;
                    *pre_right.add(index) = new_pre_r;
                }
            }
            self.delay_l[self.write] = if in_l.is_finite() { in_l } else { 0.0 };
            self.delay_r[self.write] = if in_r.is_finite() { in_r } else { 0.0 };
            self.pre_delay_l[self.write] = if pre_in_l.is_finite() { pre_in_l } else { 0.0 };
            self.pre_delay_r[self.write] = if pre_in_r.is_finite() { pre_in_r } else { 0.0 };
            self.write = (self.write + 1) % CAPACITY;
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_track_pdc_delay_create_reference() -> *mut c_void {
    Box::into_raw(Box::new(TrackPdcDelay::new())) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_destroy_reference(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut TrackPdcDelay));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_reset_reference(state: *mut c_void, requested: u32) {
    if !state.is_null() {
        (*(state as *mut TrackPdcDelay)).reset(requested);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_pdc_delay_process_reference(
    state: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    pre_left: *mut f32,
    pre_right: *mut f32,
    frames: u32,
    requested: u32,
) {
    if !state.is_null() && !left.is_null() && !right.is_null() {
        (*(state as *mut TrackPdcDelay)).process(left, right, pre_left, pre_right, frames, requested);
    }
}
