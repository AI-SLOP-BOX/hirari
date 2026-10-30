//! Offline bounce render driver.
//!
//! The engine keeps the audio graph, transport atomics and streaming handles
//! on the C++ side; this module owns the render orchestration: chunking the
//! planned extent, pause/cancel handling, deterministic dither, progress and
//! state publication, and the stream publish boundary. C++ supplies those
//! operations through [`HirariBounceHostOps`].

use crate::bounce_plan::{
    hirari_bounce_output_path_supported, HirariBouncePlan, BOUNCE_STATE_CANCELLED,
    BOUNCE_STATE_COMPLETED, BOUNCE_STATE_FAILED, BOUNCE_STATE_PAUSED, BOUNCE_STATE_RENDERING,
};
use crate::export::{
    hirari_bounce_stream_create, hirari_bounce_stream_free, hirari_bounce_stream_write,
};
use crate::tpdf_dither::{hirari_tpdf_dither_create_with_seed, hirari_tpdf_dither_destroy, hirari_tpdf_dither_next};
use std::ffi::c_void;

/// Left/right noise seeds stay fixed so two bounces of the same project are
/// byte-identical.
const TPDF_DITHER_LEFT_SEED: u32 = 0x1234_5678;
const TPDF_DITHER_RIGHT_SEED: u32 = 0x9E37_79B9;
/// Largest render block; the host lowers this to the prepared block size.
const MAX_RENDER_CHUNK: u32 = 1024;
const CONTROL_CANCEL: u8 = 1;
const CONTROL_PAUSE: u8 = 2;
/// Pause polling interval while the control plane holds the render.
const PAUSE_POLL: std::time::Duration = std::time::Duration::from_millis(10);

/// Engine operations the driver needs. Every entry is mandatory; a missing
/// operation fails the render instead of guessing.
#[repr(C)]
pub struct HirariBounceHostOps {
    /// Acquires offline-render ownership. 0 = acquired, 1 = cancelled,
    /// 2 = busy timeout (both failures already published their state).
    pub begin: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
    pub finish: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Resets the master limiter and every track for a reproducible render.
    pub reset: Option<unsafe extern "C" fn(*mut c_void)>,
    pub plan: Option<unsafe extern "C" fn(*mut c_void, *mut HirariBouncePlan) -> i32>,
    pub render: Option<unsafe extern "C" fn(*mut c_void, *mut f32, *mut f32, u32)>,
    pub render_chunk: Option<unsafe extern "C" fn(*mut c_void) -> u32>,
    pub publish: Option<unsafe extern "C" fn(*mut c_void, f32, u8)>,
    pub control: Option<unsafe extern "C" fn(*mut c_void) -> u8>,
    pub read_transport: Option<unsafe extern "C" fn(*mut c_void, *mut u64, *mut u8)>,
    pub set_transport: Option<unsafe extern "C" fn(*mut c_void, u64, u8)>,
    /// Finishes the stream while holding the publish lock. 0 = cancelled,
    /// 1 = published, 2 = publish failed.
    pub finish_stream: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> u8>,
    pub log: Option<unsafe extern "C" fn(*mut c_void, u32, *const u8, usize)>,
}

struct StreamGuard(*mut c_void);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        unsafe { hirari_bounce_stream_free(self.0) };
    }
}

struct Dither {
    left: *mut c_void,
    right: *mut c_void,
}

impl Dither {
    fn new() -> Self {
        Self {
            left: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_LEFT_SEED),
            right: hirari_tpdf_dither_create_with_seed(TPDF_DITHER_RIGHT_SEED),
        }
    }

    fn next(&mut self) -> (f32, f32) {
        unsafe { (hirari_tpdf_dither_next(self.left), hirari_tpdf_dither_next(self.right)) }
    }
}

impl Drop for Dither {
    fn drop(&mut self) {
        unsafe {
            hirari_tpdf_dither_destroy(self.left);
            hirari_tpdf_dither_destroy(self.right);
        }
    }
}

struct Ops<'a>(&'a HirariBounceHostOps);

impl Ops<'_> {
    fn publish(&self, host: *mut c_void, progress: f32, state: u32) {
        if let Some(publish) = self.0.publish {
            unsafe { publish(host, progress, state as u8) };
        }
    }

    fn log(&self, host: *mut c_void, level: u32, message: &[u8]) {
        if let Some(log) = self.0.log {
            unsafe { log(host, level, message.as_ptr(), message.len()) };
        }
    }

    fn fail(&self, host: *mut c_void, state: u32, level: u32, message: &[u8]) {
        self.publish(host, 0.0, state);
        self.log(host, level, message);
    }
}

fn missing(ops: &HirariBounceHostOps) -> bool {
    ops.begin.is_none()
        || ops.finish.is_none()
        || ops.reset.is_none()
        || ops.plan.is_none()
        || ops.render.is_none()
        || ops.render_chunk.is_none()
        || ops.publish.is_none()
        || ops.control.is_none()
        || ops.read_transport.is_none()
        || ops.set_transport.is_none()
        || ops.finish_stream.is_none()
        || ops.log.is_none()
}

/// Renders one project bounce. Returns true only when a complete file was
/// published; every other path leaves a terminal state behind.
///
/// # Safety
/// `host` must be the engine pointer the operations expect, `ops` must point
/// to a complete table, and `path` must cover `path_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_render_driver(
    host: *mut c_void,
    ops: *const HirariBounceHostOps,
    path: *const u8,
    path_len: usize,
    format: u32,
) -> bool {
    if host.is_null() || ops.is_null() {
        return false;
    }
    let ops = Ops(unsafe { &*ops });
    if missing(ops.0) {
        return false;
    }
    let (begin, finish, reset) = (
        ops.0.begin.unwrap(),
        ops.0.finish.unwrap(),
        ops.0.reset.unwrap(),
    );
    let (plan_op, render, render_chunk) = (
        ops.0.plan.unwrap(),
        ops.0.render.unwrap(),
        ops.0.render_chunk.unwrap(),
    );
    let (control, read_transport, set_transport, finish_stream) = (
        ops.0.control.unwrap(),
        ops.0.read_transport.unwrap(),
        ops.0.set_transport.unwrap(),
        ops.0.finish_stream.unwrap(),
    );

    if !unsafe { hirari_bounce_output_path_supported(path, path_len, format) } {
        ops.fail(
            host,
            BOUNCE_STATE_FAILED,
            1,
            b"BOUNCE_UNSUPPORTED_FORMAT_OR_PATH",
        );
        return false;
    }

    match unsafe { begin(host) } {
        0 => {}
        1 => {
            ops.publish(host, 0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        _ => return false,
    }

    let mut plan = HirariBouncePlan::default();
    if unsafe { plan_op(host, &mut plan) } != 0 {
        ops.publish(host, 0.0, BOUNCE_STATE_FAILED);
        finish(host);
        return false;
    }

    // Public formats map onto the Rust stream ABI: PCM16, WAVE64 float,
    // float32 RIFF/RF64, PCM24.
    const STREAM_FORMATS: [u8; 4] = [1, 3, 0, 2];
    let stream = StreamGuard(unsafe {
        hirari_bounce_stream_create(
            path,
            path_len,
            plan.total_samples,
            plan.output_rate,
            STREAM_FORMATS[format as usize],
        )
    });
    if stream.0.is_null() {
        ops.fail(host, BOUNCE_STATE_FAILED, 0, b"BOUNCE_STREAM_OPEN_FAILED");
        finish(host);
        return false;
    }

    let mut original_playhead = 0_u64;
    let mut original_playing = 0_u8;
    unsafe { read_transport(host, &mut original_playhead, &mut original_playing) };
    unsafe { set_transport(host, plan.playhead_start, 1) };
    unsafe { reset(host) };

    let chunk = unsafe { render_chunk(host) }.clamp(1, MAX_RENDER_CHUNK) as usize;
    let mut left = vec![0.0_f32; chunk];
    let mut right = vec![0.0_f32; chunk];
    let dither_depth = match format {
        0 => 256.0_f32,
        3 => 1.0_f32,
        _ => 0.0_f32,
    };
    let mut dither = (dither_depth != 0.0).then(Dither::new);
    let total = plan.total_samples.min(u64::from(u32::MAX));
    let mut progress = 0.0_f32;
    let mut offset = 0_u64;
    while offset < total {
        let bits = unsafe { control(host) };
        if bits & CONTROL_CANCEL != 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            finish(host);
            ops.publish(host, 0.0, BOUNCE_STATE_CANCELLED);
            return false;
        }
        if bits & CONTROL_PAUSE != 0 {
            ops.publish(host, progress, BOUNCE_STATE_PAUSED);
            std::thread::sleep(PAUSE_POLL);
            continue;
        }
        ops.publish(host, progress, BOUNCE_STATE_RENDERING);
        let frames = (total - offset).min(chunk as u64) as u32;
        left[..frames as usize].fill(0.0);
        right[..frames as usize].fill(0.0);
        unsafe { render(host, left.as_mut_ptr(), right.as_mut_ptr(), frames) };
        if let Some(dither) = dither.as_mut() {
            for sample in 0..frames as usize {
                let (noise_left, noise_right) = dither.next();
                left[sample] += noise_left * dither_depth;
                right[sample] += noise_right * dither_depth;
            }
        }
        let channels: [*const f32; 2] = [left.as_ptr(), right.as_ptr()];
        if unsafe { hirari_bounce_stream_write(stream.0, channels.as_ptr(), frames) } == 0 {
            unsafe { set_transport(host, original_playhead, original_playing) };
            finish(host);
            ops.fail(host, BOUNCE_STATE_FAILED, 0, b"BOUNCE_STREAM_WRITE_FAILED");
            return false;
        }
        offset += u64::from(frames);
        progress = offset as f32 / total as f32;
    }

    unsafe { set_transport(host, original_playhead, original_playing) };
    match unsafe { finish_stream(host, stream.0) } {
        1 => {
            finish(host);
            ops.publish(host, 1.0, BOUNCE_STATE_COMPLETED);
            true
        }
        published => {
            finish(host);
            ops.publish(host, 0.0, BOUNCE_STATE_FAILED);
            ops.log(
                host,
                0,
                if published == 0 {
                    b"BOUNCE_CANCELLED"
                } else {
                    b"BOUNCE_STREAM_PUBLISH_FAILED"
                },
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_operations_fail_closed() {
        let ops = HirariBounceHostOps {
            begin: None,
            finish: None,
            reset: None,
            plan: None,
            render: None,
            render_chunk: None,
            publish: None,
            control: None,
            read_transport: None,
            set_transport: None,
            finish_stream: None,
            log: None,
        };
        assert!(missing(&ops));
        let path = b"/tmp/hirari-bounce.wav";
        let ok = unsafe {
            hirari_bounce_render_driver(std::ptr::null_mut(), &ops, path.as_ptr(), path.len(), 0)
        };
        assert!(!ok);
    }

    #[test]
    fn unsupported_format_reports_failure() {
        struct Recorder {
            state: u32,
            message: u8,
        }
        unsafe extern "C" fn begin(_: *mut c_void) -> u8 {
            0
        }
        unsafe extern "C" fn finish(_: *mut c_void) {}
        unsafe extern "C" fn reset(_: *mut c_void) {}
        unsafe extern "C" fn plan(_: *mut c_void, _: *mut HirariBouncePlan) -> i32 {
            0
        }
        unsafe extern "C" fn render(_: *mut c_void, _: *mut f32, _: *mut f32, _: u32) {}
        unsafe extern "C" fn chunk(_: *mut c_void) -> u32 {
            64
        }
        unsafe extern "C" fn publish(context: *mut c_void, _: f32, state: u8) {
            unsafe { (*(context as *mut Recorder)).state = u32::from(state) };
        }
        unsafe extern "C" fn control(_: *mut c_void) -> u8 {
            0
        }
        unsafe extern "C" fn read(_: *mut c_void, playhead: *mut u64, playing: *mut u8) {
            unsafe {
                *playhead = 0;
                *playing = 0;
            }
        }
        unsafe extern "C" fn set(_: *mut c_void, _: u64, _: u8) {}
        unsafe extern "C" fn finish_stream(_: *mut c_void, _: *mut c_void) -> u8 {
            1
        }
        unsafe extern "C" fn log(context: *mut c_void, _: u32, text: *const u8, len: usize) {
            unsafe { (*(context as *mut Recorder)).message = *text.add(len - 1) };
        }
        let ops = HirariBounceHostOps {
            begin: Some(begin),
            finish: Some(finish),
            reset: Some(reset),
            plan: Some(plan),
            render: Some(render),
            render_chunk: Some(chunk),
            publish: Some(publish),
            control: Some(control),
            read_transport: Some(read),
            set_transport: Some(set),
            finish_stream: Some(finish_stream),
            log: Some(log),
        };
        let mut recorder = Recorder {
            state: 0,
            message: 0,
        };
        let path = b"/tmp/hirari-bounce.txt";
        let context = &mut recorder as *mut Recorder as *mut c_void;
        let ok = unsafe { hirari_bounce_render_driver(context, &ops, path.as_ptr(), path.len(), 0) };
        assert!(!ok);
        assert_eq!(recorder.state, BOUNCE_STATE_FAILED);
        assert_eq!(recorder.message, b'H');
    }
}
