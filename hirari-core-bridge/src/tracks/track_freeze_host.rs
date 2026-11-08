//! Track freeze rendering: admission, the offline render, the disk artifact
//! and the snapshot the audio thread reads.
//!
//! The engine supplies only the three things that physically live with it:
//! draining the audio callback, pinning the track so its stores cannot be freed
//! mid-render, and its current block size. Walking the track's own Rust runtime
//! block by block, writing and reading the Float32 WAV artifact, publishing the
//! immutable snapshot and dropping it are all decided here.
//!
//! Every entry point runs on the control thread. The render never holds the
//! engine's tracks lock: the handles it works through belong to a track the pin
//! keeps alive, and that lock is taken only for one `TrackSession` at a time.

use std::ffi::{c_void, OsStr};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::audio_buffer_ops::{
    hirari_audio_buffer_storage_channels, hirari_audio_buffer_storage_clear,
    hirari_audio_buffer_storage_create, hirari_audio_buffer_storage_destroy,
    hirari_audio_buffer_storage_read_pointer, hirari_audio_buffer_storage_reserve_buffer,
    hirari_audio_buffer_storage_resize, hirari_audio_buffer_storage_samples,
    hirari_audio_buffer_storage_write_pointer,
};
use crate::region_store::hirari_region_store_project_end;
use crate::track_frozen_store::{
    hirari_track_frozen_store_clear, hirari_track_frozen_store_is_frozen,
    hirari_track_frozen_store_publish, hirari_track_frozen_store_set_path,
};
use crate::track_process::{hirari_track_freeze_render, hirari_track_freeze_write_float32};
use crate::track_runtime::{
    hirari_track_process_block, hirari_track_runtime_reset_for_offline_render,
};
use crate::track_session::TrackSession;
use crate::wav_decoder_compat::hirari_track_freeze_read_float32;

/// Maximum freeze length the control plane will allocate. Two float channels at
/// this limit are about 512 MiB.
pub const MAX_FREEZE_SAMPLES: u64 = 64 * 1024 * 1024;

/// Processor tail captured beyond the last region when freezing to project
/// end, so reverb/delay releases are flushed into the snapshot instead of
/// being cut at the region boundary.
pub const FREEZE_TAIL_SECONDS: u64 = 2;

/// Frames per offline render block, so one freeze needs no more scratch than a
/// single block at a time.
const RENDER_BLOCK_FRAMES: u32 = 1024;

/// Pins a track for the duration of one freeze operation. `wait` drains the
/// audio callback before the track is looked up.
pub type AcquireTrackFn = unsafe extern "C" fn(*mut c_void, u32, bool) -> *mut c_void;
/// Releases the pin returned by `acquire`.
pub type ReleaseTrackFn = unsafe extern "C" fn(*mut c_void);
/// Current engine block size, used as the minimum freeze length.
pub type BlockSizeFn = unsafe extern "C" fn(*mut c_void) -> u32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HirariTrackFreezeHostOps {
    pub acquire: Option<AcquireTrackFn>,
    pub release: Option<ReleaseTrackFn>,
    pub block_size: Option<BlockSizeFn>,
}

// The table only holds function pointers supplied by the native engine.
unsafe impl Send for HirariTrackFreezeHostOps {}
unsafe impl Sync for HirariTrackFreezeHostOps {}

static HOST_OPS: OnceLock<Mutex<Option<HirariTrackFreezeHostOps>>> = OnceLock::new();

fn host_ops() -> Option<HirariTrackFreezeHostOps> {
    HOST_OPS.get()?.lock().ok().and_then(|guard| *guard)
}

/// Installs the native host callbacks. Passing null clears them.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_host_install(ops: *const HirariTrackFreezeHostOps) {
    let slot = HOST_OPS.get_or_init(|| Mutex::new(None));
    let value = if ops.is_null() {
        None
    } else {
        Some(unsafe { std::ptr::read(ops) })
    };
    if let Ok(mut guard) = slot.lock() {
        *guard = value;
    }
}

/// Releases the pin when the freeze operation ends, even on an early return.
struct TrackPin {
    raw: *mut c_void,
    release: ReleaseTrackFn,
}

impl Drop for TrackPin {
    fn drop(&mut self) {
        unsafe { (self.release)(self.raw) };
    }
}

fn acquire(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    wait: bool,
) -> Option<TrackPin> {
    let acquire = ops.acquire?;
    let release = ops.release?;
    let raw = unsafe { acquire(host, track_id, wait) };
    if raw.is_null() {
        return None;
    }
    Some(TrackPin { raw, release })
}

/// A stereo render buffer with the engine's growth policy applied. It is freed
/// on any failure; a successful publication takes it through `into_handle`.
struct RenderStorage {
    handle: *mut c_void,
}

impl RenderStorage {
    fn new(channels: u32, samples: u32) -> Option<Self> {
        let handle = hirari_audio_buffer_storage_create();
        if unsafe { hirari_audio_buffer_storage_reserve_buffer(handle, channels, samples) } != 0
            || !unsafe { hirari_audio_buffer_storage_resize(handle, channels, samples) }
        {
            unsafe { hirari_audio_buffer_storage_destroy(handle) };
            return None;
        }
        Some(Self { handle })
    }

    fn frames(&self) -> u32 {
        unsafe { hirari_audio_buffer_storage_samples(self.handle) }
    }

    fn channel_count(&self) -> u32 {
        unsafe { hirari_audio_buffer_storage_channels(self.handle) }
    }

    fn write_plane(&self, channel: u32) -> *mut f32 {
        unsafe { hirari_audio_buffer_storage_write_pointer(self.handle, channel) }
    }

    fn read_plane(&self, channel: u32) -> *const f32 {
        unsafe { hirari_audio_buffer_storage_read_pointer(self.handle, channel, 0) }
    }

    fn into_handle(self) -> *mut c_void {
        let handle = self.handle;
        std::mem::forget(self);
        handle
    }
}

impl Drop for RenderStorage {
    fn drop(&mut self) {
        unsafe { hirari_audio_buffer_storage_destroy(self.handle) };
    }
}

/// The two handles the offline loop drives: the track's runtime and the scratch
/// block it renders through.
struct OfflineRender {
    runtime: *mut c_void,
    block: *mut c_void,
}

unsafe extern "C" fn reset_offline(context: *mut c_void) -> bool {
    let context = unsafe { &*(context as *const OfflineRender) };
    if context.runtime.is_null() {
        return false;
    }
    unsafe { hirari_track_runtime_reset_for_offline_render(context.runtime) };
    true
}

unsafe extern "C" fn render_block(
    context: *mut c_void,
    left: *mut f32,
    right: *mut f32,
    frames: u32,
    playhead: u64,
) -> bool {
    let context = unsafe { &*(context as *const OfflineRender) };
    let block = context.block;
    if context.runtime.is_null()
        || block.is_null()
        || left.is_null()
        || right.is_null()
        || frames == 0
        || unsafe { hirari_audio_buffer_storage_channels(block) } < 2
        || unsafe { hirari_audio_buffer_storage_samples(block) } < frames
    {
        return false;
    }
    unsafe {
        hirari_audio_buffer_storage_clear(block, 0, hirari_audio_buffer_storage_samples(block));
        // A false return means the runtime fully handled the block (mute, an
        // existing freeze or a bad size) and left its own result in `block`; the
        // rendered planes are still what the track produced for this block.
        hirari_track_process_block(context.runtime, block, frames, playhead, 1);
        let source_left = hirari_audio_buffer_storage_read_pointer(block, 0, 0);
        let source_right = hirari_audio_buffer_storage_read_pointer(block, 1, 0);
        if source_left.is_null() || source_right.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(source_left, left, frames as usize);
        std::ptr::copy_nonoverlapping(source_right, right, frames as usize);
    }
    true
}

/// Drives the track's graph offline into a fresh immutable snapshot. The plugin
/// state it walks is mutated here, which is why the caller must have drained the
/// audio callback first.
fn render_offline(runtime: *mut c_void, total_samples: u64) -> Option<RenderStorage> {
    if runtime.is_null() || total_samples == 0 || total_samples > MAX_FREEZE_SAMPLES {
        return None;
    }
    let rendered = RenderStorage::new(2, total_samples as u32)?;
    let block = RenderStorage::new(2, RENDER_BLOCK_FRAMES)?;
    let context = OfflineRender {
        runtime,
        block: block.handle,
    };
    let complete = unsafe {
        hirari_track_freeze_render(
            total_samples,
            RENDER_BLOCK_FRAMES,
            rendered.write_plane(0),
            rendered.write_plane(1),
            rendered.frames() as usize,
            &context as *const OfflineRender as *mut c_void,
            Some(reset_offline),
            Some(render_block),
        )
    };
    complete.then_some(rendered)
}

/// Publishes a rendered snapshot and bumps the layout revision. The storage is
/// only handed to the store once the publication is accepted, so a track that
/// disappeared mid-render cannot leak its render.
fn publish(
    host: *mut c_void,
    track_id: u32,
    rendered: RenderStorage,
    total_samples: u64,
    sample_rate: u32,
    cache_path: &[u8],
) -> bool {
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let frames = rendered.frames() as u64;
    let channels = rendered.channel_count();
    let audio = rendered.into_handle();
    let accepted = unsafe {
        hirari_track_frozen_store_publish(
            session.frozen(),
            audio,
            channels,
            frames,
            total_samples,
            sample_rate,
            0,
            0,
            cache_path.as_ptr(),
            cache_path.len(),
        )
    };
    if accepted {
        session.mark_layout();
        return true;
    }
    unsafe { hirari_audio_buffer_storage_destroy(audio) };
    false
}

/// Renders the freeze, streams it to `artifact` when one is given, and publishes
/// the snapshot under that path. A rejected publication leaves neither an orphan
/// file nor a half-published freeze behind.
fn freeze_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    total_samples: u64,
    sample_rate: u32,
    artifact: Option<&[u8]>,
) -> bool {
    if total_samples == 0 || sample_rate == 0 || total_samples > MAX_FREEZE_SAMPLES {
        return false;
    }
    let artifact = match artifact {
        Some(path) => match absolute_path(path) {
            Some(absolute) => Some(absolute),
            None => return false,
        },
        None => None,
    };
    // A freeze render mutates processor state while producing its immutable
    // snapshot, so it must never overlap the live callback or another graph
    // hydration.
    let Some(_pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let runtime = session.runtime();
    drop(session);
    let Some(rendered) = render_offline(runtime, total_samples) else {
        return false;
    };
    if let Some(path) = &artifact {
        let streamed = unsafe {
            hirari_track_freeze_write_float32(
                path.as_ptr(),
                path.len(),
                sample_rate,
                rendered.read_plane(0),
                rendered.read_plane(1),
                total_samples,
            )
        };
        if !streamed {
            return false;
        }
    }
    let cache_path: &[u8] = artifact.as_deref().unwrap_or(&[]);
    if publish(host, track_id, rendered, total_samples, sample_rate, cache_path) {
        return true;
    }
    if let Some(path) = &artifact {
        let _ = std::fs::remove_file(Path::new(OsStr::from_bytes(path)));
    }
    false
}

/// Resolves a caller-supplied path the way `std::filesystem::absolute` did:
/// relative paths get the current directory prepended, nothing is canonicalised.
fn absolute_path(path: &[u8]) -> Option<Vec<u8>> {
    let resolved = std::path::absolute(Path::new(OsStr::from_bytes(path))).ok()?;
    if resolved.as_os_str().is_empty() {
        return None;
    }
    Some(resolved.into_os_string().into_vec())
}

fn freeze_to_project_end_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    sample_rate: u32,
) -> bool {
    if sample_rate == 0 {
        return false;
    }
    let Some(block_size) = ops.block_size else {
        return false;
    };
    let Some(_pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let track = session.track();
    drop(session);
    let mut project_end_samples = 0_u64;
    let readable = unsafe { hirari_region_store_project_end(track, &mut project_end_samples) };
    if !readable {
        return false;
    }
    // The end of the last region wins, but a track with no regions still needs
    // a full block round trip so the frozen snapshot is not empty. A non-empty
    // project also renders a processor tail beyond the last region so the
    // freeze does not cut reverb/delay releases at the region boundary.
    let fallback = u64::from(unsafe { block_size(host) }.max(1)) * 2;
    let mut total_samples = project_end_samples.max(fallback);
    if project_end_samples > 0 {
        total_samples = total_samples
            .saturating_add(u64::from(sample_rate).saturating_mul(FREEZE_TAIL_SECONDS));
    }
    if total_samples > MAX_FREEZE_SAMPLES {
        return false;
    }
    // Re-enter the regular freeze path so validation and the callback drain
    // happen exactly once, right before the render.
    freeze_impl(ops, host, track_id, total_samples, sample_rate, None)
}

fn freeze_to_file_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    if path.is_empty() {
        return false;
    }
    freeze_impl(
        ops,
        host,
        track_id,
        total_samples,
        sample_rate,
        Some(path),
    )
}

fn restore_from_file_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    if path.is_empty()
        || total_samples == 0
        || sample_rate == 0
        || total_samples > MAX_FREEZE_SAMPLES
    {
        return false;
    }
    let Some(_pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    // Restoring reads the exact published artifact rather than re-rendering the
    // live graph: a plugin may be nondeterministic or have changed version since
    // the cache was produced. Rate agreement and length admission are enforced by
    // the Rust freeze reader, which widens a mono artifact to stereo.
    let Some(rendered) = RenderStorage::new(2, total_samples as u32) else {
        return false;
    };
    let decoded = unsafe {
        hirari_track_freeze_read_float32(
            path.as_ptr(),
            path.len(),
            sample_rate,
            total_samples,
            rendered.write_plane(0),
            rendered.write_plane(1),
        )
    };
    decoded && publish(host, track_id, rendered, total_samples, sample_rate, path)
}

fn set_cache_path_impl(
    ops: &HirariTrackFreezeHostOps,
    host: *mut c_void,
    track_id: u32,
    path: &[u8],
) -> bool {
    if path.is_empty() {
        return false;
    }
    let Some(_pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    let recorded =
        unsafe { hirari_track_frozen_store_set_path(session.frozen(), path.as_ptr(), path.len()) };
    if recorded {
        session.mark_layout();
    }
    recorded
}

fn unfreeze_impl(ops: &HirariTrackFreezeHostOps, host: *mut c_void, track_id: u32) -> bool {
    let Some(_pin) = acquire(ops, host, track_id, true) else {
        return false;
    };
    let Some(session) = TrackSession::open(host, track_id) else {
        return false;
    };
    unsafe { hirari_track_frozen_store_clear(session.frozen()) };
    // The offline freeze render left the live processors at their end state.
    // Flush that tail before the live graph resumes so stale delay/reverb
    // state cannot leak into the unfrozen playback.
    unsafe { hirari_track_runtime_reset_for_offline_render(session.runtime()) };
    session.mark_layout();
    true
}

fn is_frozen_impl(ops: &HirariTrackFreezeHostOps, host: *mut c_void, track_id: u32) -> bool {
    let Some(_pin) = acquire(ops, host, track_id, false) else {
        return false;
    };
    TrackSession::open(host, track_id)
        .is_some_and(|session| unsafe { hirari_track_frozen_store_is_frozen(session.frozen()) })
}

fn path_slice<'a>(path: *const u8, length: usize) -> &'a [u8] {
    if path.is_null() || length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(path, length) }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze(
    host: *mut c_void,
    track_id: u32,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_impl(&ops, host, track_id, total_samples, sample_rate, None),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_to_project_end(
    host: *mut c_void,
    track_id: u32,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_to_project_end_impl(&ops, host, track_id, sample_rate),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_freeze_to_file(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => freeze_to_file_impl(
            &ops,
            host,
            track_id,
            path_slice(path, path_length),
            total_samples,
            sample_rate,
        ),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_restore_freeze(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
    total_samples: u64,
    sample_rate: u32,
) -> bool {
    match host_ops() {
        Some(ops) => restore_from_file_impl(
            &ops,
            host,
            track_id,
            path_slice(path, path_length),
            total_samples,
            sample_rate,
        ),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_set_freeze_cache_path(
    host: *mut c_void,
    track_id: u32,
    path: *const u8,
    path_length: usize,
) -> bool {
    match host_ops() {
        Some(ops) => set_cache_path_impl(&ops, host, track_id, path_slice(path, path_length)),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_unfreeze(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => unfreeze_impl(&ops, host, track_id),
        None => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_track_is_frozen(host: *mut c_void, track_id: u32) -> bool {
    match host_ops() {
        Some(ops) => is_frozen_impl(&ops, host, track_id),
        None => false,
    }
}
