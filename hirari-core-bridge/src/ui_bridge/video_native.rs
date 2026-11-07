//! AVFoundation frame decoding behind the Rust video runtime.
//!
//! `video_system` owns the request queue, the cached frame and the worker
//! thread; this module is the only place that speaks to AVFoundation. Decoding
//! happens on that worker thread and never on the audio thread, so the
//! synchronous-looking waits below cannot stall playback.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// A decoded frame: packed RGBA8 plus the geometry it was rendered at.
pub struct DecodedFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Produces the frames the runtime caches.
pub trait FrameSource: Send + Sync {
    /// Opens `path` on behalf of `generation`, returning `(duration, fps)` when
    /// the file exposes a video track with a usable frame rate.
    fn open(&self, path: &str, generation: u64) -> Option<(f64, f64)>;

    /// Decodes the frame nearest `seconds`, or `None` when `generation` is no
    /// longer current or AVFoundation did not answer within its deadline.
    fn decode(&self, seconds: f64, generation: u64) -> Option<DecodedFrame>;
}

/// One-shot reply slot for an AVFoundation completion handler.
#[cfg(target_os = "macos")]
struct Reply<T> {
    value: Mutex<Option<T>>,
    arrived: Condvar,
}

#[cfg(target_os = "macos")]
impl<T> Reply<T> {
    fn new() -> Self {
        Self { value: Mutex::new(None), arrived: Condvar::new() }
    }

    fn fill(&self, value: T) {
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
        self.arrived.notify_one();
    }

    /// Waits for the handler, giving up once `timeout` elapses. A late reply
    /// then lands in a slot nobody reads; the Arc keeps it alive until the
    /// framework releases the handler.
    fn wait(&self, timeout: Duration) -> Option<T> {
        let mut guard = self.value.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(value) = guard.take() { return Some(value); }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return None; }
            let (next, _wait) =
                self.arrived.wait_timeout(guard, remaining).unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }
}

/// The longest either AVFoundation handshake is allowed to take.
#[cfg(target_os = "macos")]
const NATIVE_DEADLINE: Duration = Duration::from_secs(2);

/// Frames larger than this are not blitted: the byte budget is quadratic and
/// a bogus track header must not be able to ask for a huge allocation.
#[cfg(target_os = "macos")]
const MAX_FRAME_EDGE: usize = 8192;

/// AVFoundation image generator held across threads.
///
/// Every call into the generator is serialised by `AvFoundationSource`'s mutex
/// and re-checked against the live generation, exactly as the native layer did
/// before this moved to Rust; AVFoundation itself documents the generator as
/// thread-safe and has no main-thread affinity.
#[cfg(target_os = "macos")]
#[derive(Clone)]
struct Generator(objc2::rc::Retained<objc2_av_foundation::AVAssetImageGenerator>);

#[cfg(target_os = "macos")]
unsafe impl Send for Generator {}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct SourceState {
    generation: u64,
    generator: Option<Generator>,
}

/// Frame source backed by AVURLAsset and AVAssetImageGenerator.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct AvFoundationSource {
    state: Mutex<SourceState>,
}

#[cfg(target_os = "macos")]
impl AvFoundationSource {
    pub fn new() -> Self {
        Self::default()
    }

    fn current_generator(&self, generation: u64) -> Option<Generator> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation { return None; }
        state.generator.clone()
    }

    /// Renders a `CGImage` into a packed RGBA8 buffer, top row first.
    fn rasterize(image: &objc2_core_graphics::CGImage) -> Option<DecodedFrame> {
        use objc2_core_foundation::{CGPoint, CGRect, CGSize};
        use objc2_core_graphics::{
            CGBitmapContextCreate, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGContext,
            CGColorSpace,
        };

        let width = unsafe { CGImage::width(Some(image)) };
        let height = unsafe { CGImage::height(Some(image)) };
        if width == 0 || height == 0 || width > MAX_FRAME_EDGE || height > MAX_FRAME_EDGE {
            return None;
        }
        let mut pixels = vec![0u8; width * height * 4];
        let color_space = unsafe { CGColorSpace::new_device_rgb() }?;
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }?;
        CGContext::draw_image(
            Some(&context),
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64)),
            Some(image),
        );
        Some(DecodedFrame { pixels, width: width as u32, height: height as u32 })
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
impl FrameSource for AvFoundationSource {
    fn open(&self, path: &str, generation: u64) -> Option<(f64, f64)> {
        use block2::RcBlock;
        use objc2::rc::Retained;
        use objc2_av_foundation::{
            AVAssetImageGenerator, AVAssetTrack, AVMediaTypeVideo, AVURLAsset,
        };
        use objc2_core_media::{kCMTimeZero, CMTimeGetSeconds};
        use objc2_foundation::{NSArray, NSError, NSString, NSURL};

        let url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(path)) };
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };

        let reply = Arc::new(Reply::new());
        let handler = {
            let reply = Arc::clone(&reply);
            RcBlock::new(move |tracks: *mut NSArray<AVAssetTrack>, _: *mut NSError| {
                reply.fill(unsafe { Retained::retain(tracks) });
            })
        };
        unsafe { asset.loadTracksWithMediaType_completionHandler(AVMediaTypeVideo.unwrap(), &handler) };
        let tracks = reply.wait(NATIVE_DEADLINE).flatten()?;
        let track = unsafe { tracks.firstObject() }?;
        let fps = unsafe { track.nominalFrameRate() } as f64;
        if !(fps > 0.0) { return None; }
        let duration = unsafe { CMTimeGetSeconds(asset.duration()) };

        let generator = unsafe {
            AVAssetImageGenerator::initWithAsset(
                <AVAssetImageGenerator as objc2::AnyThread>::alloc(),
                &asset,
            )
        };
        unsafe {
            generator.setAppliesPreferredTrackTransform(true);
            generator.setRequestedTimeToleranceBefore(kCMTimeZero);
            generator.setRequestedTimeToleranceAfter(kCMTimeZero);
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // A load that already adopted a newer generation owns the source; this
        // late reply for the superseded asset must not hand back a generator.
        if generation < state.generation { return None; }
        state.generation = generation;
        state.generator = Some(Generator(generator));
        Some((duration, fps))
    }

    fn decode(&self, seconds: f64, generation: u64) -> Option<DecodedFrame> {
        use block2::RcBlock;
        use objc2::rc::Retained;
        use objc2_core_graphics::CGImage;
        use objc2_core_media::{CMTime, CMTimeMakeWithSeconds};
        use objc2_foundation::NSError;

        let Generator(generator) = self.current_generator(generation)?;

        let reply = Arc::new(Reply::new());
        let handler = {
            let reply = Arc::clone(&reply);
            RcBlock::new(move |image: *mut CGImage, _: CMTime, _: *mut NSError| {
                reply.fill(unsafe { Retained::retain(image) });
            })
        };
        unsafe {
            generator.generateCGImageAsynchronouslyForTime_completionHandler(
                CMTimeMakeWithSeconds(seconds, 600),
                &handler,
            )
        };
        let image = match reply.wait(NATIVE_DEADLINE) {
            Some(Some(image)) => image,
            _ => {
                unsafe { generator.cancelAllCGImageGeneration() };
                return None;
            }
        };

        // A load that landed while this frame was rendering owns the
        // generation now; its frames must not overwrite the new asset.
        if self.current_generator(generation).is_none() { return None; }
        let frame = Self::rasterize(&image)?;

        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation != generation { return None; }
        Some(frame)
    }
}

/// Frame source for platforms without AVFoundation: video is never available,
/// so the runtime keeps the same "no producer" behaviour the native bridge
/// had behind its `__APPLE__` guards.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct AvFoundationSource;

#[cfg(not(target_os = "macos"))]
impl AvFoundationSource {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(target_os = "macos"))]
impl FrameSource for AvFoundationSource {
    fn open(&self, _path: &str, _generation: u64) -> Option<(f64, f64)> {
        None
    }

    fn decode(&self, _seconds: f64, _generation: u64) -> Option<DecodedFrame> {
        None
    }
}
