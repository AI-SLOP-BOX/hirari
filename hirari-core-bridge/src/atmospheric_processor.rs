use std::ffi::c_void;

const LINE_SIZE: usize = 32_768;
const DELAY_TIMES: [f32; 4] = [1531.0, 2307.0, 3109.0, 4409.0];

#[derive(Clone)]
struct AtmosphericProcessor {
    delay_lines: Vec<Vec<f32>>,
    write_positions: Vec<usize>,
}

impl AtmosphericProcessor {
    fn new(delay_count: usize) -> Self {
        let count = delay_count.min(DELAY_TIMES.len());
        Self { delay_lines: vec![vec![0.0; LINE_SIZE]; count], write_positions: vec![0; count] }
    }

    fn read_cubic(line: &[f32], position: f32) -> f32 {
        let size = line.len() as i64;
        let i0 = position.floor() as i64;
        let fraction = position - i0 as f32;
        let wrap = |index: i64| index.rem_euclid(size) as usize;
        let ym1 = line[wrap(i0 - 1)];
        let y0 = line[wrap(i0)];
        let y1 = line[wrap(i0 + 1)];
        let y2 = line[wrap(i0 + 2)];
        let a0 = -0.5 * ym1 + 1.5 * y0 - 1.5 * y1 + 0.5 * y2;
        let a1 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let a2 = -0.5 * ym1 + 0.5 * y1;
        let a3 = y0;
        a0 * fraction * fraction * fraction + a1 * fraction * fraction + a2 * fraction + a3
    }

    unsafe fn process(&mut self, buffer: *mut f32, samples: u32, decay: f32,
                      tension: f32, transient_clarity: f32) -> bool {
        if buffer.is_null() || samples == 0 { return false; }
        let width = 0.5 + transient_clarity * 0.5;
        let adaptive_decay = decay * (1.0 + tension * 0.2);
        for i in 0..samples as usize {
            let input = *buffer.add(i);
            let mut feedback = [0.0f32; 4];
            let modulation = (i as f32 * 0.0001).sin() * (tension * 10.0);
            for d in 0..self.delay_lines.len() {
                let line = &self.delay_lines[d];
                let position = self.write_positions[d] as f32 - (DELAY_TIMES[d] + modulation);
                let position = if position < 0.0 { position + line.len() as f32 } else { position };
                feedback[d] = Self::read_cubic(line, position);
            }
            let sum = feedback.iter().sum::<f32>();
            for d in 0..self.delay_lines.len() {
                let output = feedback[d] - 0.5 * sum;
                let position = self.write_positions[d];
                self.delay_lines[d][position] = input + output * adaptive_decay;
                self.write_positions[d] = (position + 1) % LINE_SIZE;
            }
            *buffer.add(i) = input * (1.0 - width)
                + (sum / self.delay_lines.len() as f32) * width;
        }
        true
    }
}

#[no_mangle]
pub extern "C" fn hirari_atmospheric_processor_create(delay_count: usize) -> *mut c_void {
    Box::into_raw(Box::new(AtmosphericProcessor::new(delay_count))).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmospheric_processor_clone(state: *const c_void) -> *mut c_void {
    let Some(state) = state.cast::<AtmosphericProcessor>().as_ref() else { return std::ptr::null_mut() };
    Box::into_raw(Box::new(state.clone())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmospheric_processor_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<AtmosphericProcessor>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_atmospheric_processor_process(
    state: *mut c_void, buffer: *mut f32, samples: u32, decay: f32,
    tension: f32, transient_clarity: f32,
) -> bool {
    let Some(state) = state.cast::<AtmosphericProcessor>().as_mut() else { return false };
    state.process(buffer, samples, decay, tension, transient_clarity)
}
