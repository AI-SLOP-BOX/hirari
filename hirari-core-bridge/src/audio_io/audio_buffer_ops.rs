//! Allocation-free sample operations for the engine's audio blocks.

use std::alloc::{alloc, dealloc, Layout};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

thread_local! {
    static AUDIO_BUFFER_REALTIME_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static AUDIO_BUFFER_RESIZE_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static AUDIO_BUFFER_LAST_REQUESTED_CAPACITY: AtomicUsize = AtomicUsize::new(0);
static AUDIO_BUFFER_LAST_CAPACITY: AtomicUsize = AtomicUsize::new(0);

const AUDIO_BUFFER_INLINE_CHANNELS: usize = 16;

struct AlignedAudioStorage {
    samples: *mut f32,
    capacity: usize,
    inline_channel_pointers: [*mut f32; AUDIO_BUFFER_INLINE_CHANNELS],
    channel_pointers: Vec<*mut f32>,
    pointer_count: usize,
    channels: u32,
    frame_count: u32,
    external: bool,
    external_data: *mut *mut f32,
}

fn new_aligned_audio_storage() -> AlignedAudioStorage {
    AlignedAudioStorage {
        samples: std::ptr::null_mut(),
        capacity: 0,
        inline_channel_pointers: [std::ptr::null_mut(); AUDIO_BUFFER_INLINE_CHANNELS],
        channel_pointers: Vec::new(),
        pointer_count: 0,
        channels: 0,
        frame_count: 0,
        external: false,
        external_data: std::ptr::null_mut(),
    }
}

impl Drop for AlignedAudioStorage {
    fn drop(&mut self) {
        self.release();
    }
}

impl AlignedAudioStorage {
    fn channel_pointers(&self) -> *mut *mut f32 {
        if self.pointer_count <= AUDIO_BUFFER_INLINE_CHANNELS {
            self.inline_channel_pointers.as_ptr() as *mut *mut f32
        } else {
            self.channel_pointers.as_ptr() as *mut *mut f32
        }
    }

    fn channel_pointers_mut(&mut self) -> &mut [*mut f32] {
        if self.pointer_count <= AUDIO_BUFFER_INLINE_CHANNELS {
            &mut self.inline_channel_pointers[..self.pointer_count]
        } else {
            &mut self.channel_pointers[..self.pointer_count]
        }
    }

    fn release(&mut self) {
        if !self.samples.is_null() {
            if let Some(layout) = audio_storage_layout(self.capacity) {
                unsafe { dealloc(self.samples.cast(), layout) };
            }
        }
        self.samples = std::ptr::null_mut();
        self.capacity = 0;
    }
}

fn audio_storage_layout(capacity: usize) -> Option<Layout> {
    capacity
        .checked_mul(std::mem::size_of::<f32>())
        .and_then(|bytes| Layout::from_size_align(bytes, 4096).ok())
}

/// Copies decoded interleaved PCM into the planar channel buffers owned by the
/// native AudioBuffer adapter. The caller must provide distinct writable
/// channel planes, as returned by AudioBuffer::getArrayOfWritePointers().
#[path = "audio_buffer_ops_ffi.rs"]
mod ffi;
pub use ffi::*;


#[cfg(test)]
mod builtin_gain_tests {
    use super::hirari_builtin_gain_process;

    #[cfg(feature = "dsp-differential-reference")]
    unsafe extern "C" {
        fn hirari_builtin_gain_process_reference(
            channels: *const *mut f32,
            channel_count: u32,
            sample_count: u32,
            gain: f32,
        ) -> u32;
    }

    #[test]
    fn registered_gain_matches_reference_and_contains_non_finite_output() {
        let mut left = [0.25_f32, -0.5, f32::INFINITY, f32::NAN];
        let mut right = [2.0_f32, -3.0, f32::NEG_INFINITY, 0.125];
        let mut expected_left = left;
        let mut expected_right = right;
        let gain = 1.75_f32;
        let mut reference_sanitized = 0;
        for channel in [&mut expected_left, &mut expected_right] {
            for sample in channel {
                let output = *sample * gain;
                if output.is_finite() {
                    *sample = output;
                } else {
                    *sample = 0.0;
                    reference_sanitized += 1;
                }
            }
        }

        let channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let sanitized =
            unsafe { hirari_builtin_gain_process(channels.as_ptr(), 2, left.len() as u32, gain) };
        assert_eq!(sanitized, reference_sanitized);
        assert_eq!(left, expected_left);
        assert_eq!(right, expected_right);
    }

    #[cfg(feature = "dsp-differential-reference")]
    #[test]
    fn registered_gain_matches_legacy_cpp_reference() {
        let original_left = [0.25_f32, -0.5, f32::INFINITY, f32::NAN, 0.0];
        let original_right = [2.0_f32, -3.0, f32::NEG_INFINITY, 0.125, -0.0];
        let mut rust_left = original_left;
        let mut rust_right = original_right;
        let mut cpp_left = original_left;
        let mut cpp_right = original_right;
        let gain = 1.75_f32;
        let rust_channels = [rust_left.as_mut_ptr(), rust_right.as_mut_ptr()];
        let cpp_channels = [cpp_left.as_mut_ptr(), cpp_right.as_mut_ptr()];

        let rust_sanitized = unsafe {
            hirari_builtin_gain_process(rust_channels.as_ptr(), 2, original_left.len() as u32, gain)
        };
        let cpp_sanitized = unsafe {
            hirari_builtin_gain_process_reference(
                cpp_channels.as_ptr(),
                2,
                original_left.len() as u32,
                gain,
            )
        };

        assert_eq!(rust_sanitized, cpp_sanitized);
        assert_eq!(rust_left, cpp_left);
        assert_eq!(rust_right, cpp_right);
    }
}

/// The whole owned/wrapped buffer contract: dimensions, channel views, bounds,
/// non-finite sanitising and the mixing entry points the native audio thread
/// uses. These drive the same C ABI the engine's typed handle forwards to.
#[cfg(test)]
mod storage_contract_tests {
    use std::ffi::c_void;
    use std::ptr;

    use super::{
        hirari_audio_buffer_storage_add_channels_from, hirari_audio_buffer_storage_add_from,
        hirari_audio_buffer_storage_apply_gain, hirari_audio_buffer_storage_array_read,
        hirari_audio_buffer_storage_array_write, hirari_audio_buffer_storage_channels,
        hirari_audio_buffer_storage_clear, hirari_audio_buffer_storage_create,
        hirari_audio_buffer_storage_destroy, hirari_audio_buffer_storage_read_pointer,
        hirari_audio_buffer_storage_reserve_buffer, hirari_audio_buffer_storage_resize,
        hirari_audio_buffer_storage_samples, hirari_audio_buffer_storage_sanitize,
        hirari_audio_buffer_storage_wrap_channels, hirari_audio_buffer_storage_write_pointer,
    };

    struct Storage(*mut c_void);

    impl Storage {
        fn empty() -> Self {
            Self(hirari_audio_buffer_storage_create())
        }

        fn owned(channels: u32, samples: u32) -> Self {
            let storage = Self::empty();
            assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(storage.0, channels, samples) }, 0);
            assert!(unsafe { hirari_audio_buffer_storage_resize(storage.0, channels, samples) });
            storage
        }

        fn write(&self, channel: u32) -> *mut f32 {
            unsafe { hirari_audio_buffer_storage_write_pointer(self.0, channel) }
        }

        fn read(&self, channel: u32) -> *const f32 {
            unsafe { hirari_audio_buffer_storage_read_pointer(self.0, channel, 0) }
        }

        fn fill(&self, channel: u32, values: &[f32]) {
            let pointer = self.write(channel);
            assert!(!pointer.is_null());
            for (index, value) in values.iter().enumerate() {
                unsafe { *pointer.add(index) = *value };
            }
        }

        fn sample(&self, channel: u32, index: u32) -> f32 {
            unsafe { *self.read(channel).add(index as usize) }
        }

        fn dimensions(&self) -> (u32, u32) {
            unsafe {
                (
                    hirari_audio_buffer_storage_channels(self.0),
                    hirari_audio_buffer_storage_samples(self.0),
                )
            }
        }
    }

    impl Drop for Storage {
        fn drop(&mut self) {
            unsafe { hirari_audio_buffer_storage_destroy(self.0) };
        }
    }

    #[test]
    fn mixed_planes_ignore_incomplete_sources_and_keep_finite_audio() {
        let stereo = Storage::owned(2, 4);
        stereo.fill(0, &[0.25, 0.0, 0.0, 0.0]);
        stereo.fill(1, &[-0.25, 0.0, 0.0, 0.0]);

        // Invalid raw bridge input must be rejected without touching audio.
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(stereo.0, ptr::null(), ptr::null(), 4);
        }
        assert_eq!(stereo.sample(0, 0), 0.25);
        assert_eq!(stereo.sample(1, 0), -0.25);

        // A stereo destination needs both planes; a missing right is refused.
        let left = [0.5_f32, 0.0, 0.0, 0.0];
        let right = [-0.5_f32, 0.0, 0.0, 0.0];
        unsafe {
            hirari_audio_buffer_storage_add_channels_from(stereo.0, left.as_ptr(), ptr::null(), 4);
        }
        assert_eq!(stereo.sample(0, 0), 0.25);
        assert_eq!(stereo.sample(1, 0), -0.25);

        unsafe {
            hirari_audio_buffer_storage_add_channels_from(
                stereo.0,
                left.as_ptr(),
                right.as_ptr(),
                4,
            );
        }
        assert_eq!(stereo.sample(0, 0), 0.75);
        assert_eq!(stereo.sample(1, 0), -0.75);

        // A non-finite gain is a no-op rather than a way to poison the block.
        unsafe {
            hirari_audio_buffer_storage_apply_gain(stereo.0, f32::NAN);
            hirari_audio_buffer_storage_apply_gain(stereo.0, f32::INFINITY);
        }
        assert!(stereo.sample(0, 0).is_finite());
        assert!(stereo.sample(1, 0).is_finite());
    }

    #[test]
    fn dimensions_and_channel_views_are_bounded() {
        let stereo = Storage::owned(2, 4);
        assert_eq!(stereo.dimensions(), (2, 4));
        assert!(stereo.write(2).is_null());
        assert!(stereo.read(2).is_null());

        // Read offsets clamp to the frame count instead of running past a plane.
        let offset = unsafe { hirari_audio_buffer_storage_read_pointer(stereo.0, 0, 99) };
        assert_eq!(offset, unsafe { stereo.read(0).add(4) });

        let array_write = unsafe { hirari_audio_buffer_storage_array_write(stereo.0) };
        assert_eq!(unsafe { *array_write.add(1) } as usize, stereo.write(1) as usize);
        let array_read = unsafe { hirari_audio_buffer_storage_array_read(stereo.0) };
        assert_eq!(unsafe { *array_read } as usize, stereo.read(0) as usize);

        stereo.fill(0, &[0.0, f32::INFINITY, 0.0, 0.0]);
        assert_eq!(unsafe { hirari_audio_buffer_storage_sanitize(stereo.0) }, 1);
        assert_eq!(stereo.sample(0, 1), 0.0);

        unsafe { hirari_audio_buffer_storage_clear(stereo.0, 0, 1) };
        assert_eq!(stereo.sample(0, 0), 0.0);
    }

    #[test]
    fn another_storage_is_summed_and_an_external_wrap_replaces_the_block() {
        let destination = Storage::owned(2, 4);
        let source = Storage::owned(2, 4);
        source.fill(0, &[2.0, 0.0, 0.0, 0.0]);
        source.fill(1, &[-2.0, 0.0, 0.0, 0.0]);
        unsafe { hirari_audio_buffer_storage_add_from(destination.0, source.0, 4) };
        assert_eq!(destination.sample(0, 0), 2.0);
        assert_eq!(destination.sample(1, 0), -2.0);

        let mut external_left = [1.0_f32, 0.0, 0.0, 0.0];
        let mut external_right = [-1.0_f32, 0.0, 0.0, 0.0];
        let mut planes = [external_left.as_mut_ptr(), external_right.as_mut_ptr()];
        let wrapped = Storage::empty();
        assert!(unsafe { hirari_audio_buffer_storage_wrap_channels(wrapped.0, planes.as_mut_ptr(), 2, 4) });
        assert_eq!(wrapped.dimensions().0, 2);
        assert_eq!(wrapped.write(0), external_left.as_mut_ptr());

        // An invalid wrap releases the owned block and leaves an empty view.
        assert!(!unsafe { hirari_audio_buffer_storage_wrap_channels(wrapped.0, ptr::null_mut(), 0, 0) });
        assert_eq!(wrapped.dimensions(), (0, 0));
    }

    #[test]
    fn the_realtime_guard_declines_growth_and_reports_why() {
        use super::{
            hirari_audio_buffer_last_realtime_requested_capacity,
            hirari_audio_buffer_realtime_resize_attempts, hirari_audio_buffer_set_realtime_guard,
            AUDIO_BUFFER_INLINE_CHANNELS,
        };
        use crate::status_queue::{
            default_queue_test_lock, hirari_status_queue_pop_default, StatusMessage,
            SEVERITY_CRITICAL,
        };

        // The notification sink is process-wide, so this test owns it while it
        // reads the two rejections below.
        let _serial = default_queue_test_lock();
        let mut stale = StatusMessage::default();
        loop {
            if !unsafe { hirari_status_queue_pop_default(&mut stale) } {
                break;
            }
        }

        let stereo = Storage::owned(2, 16);
        // A wide channel table inside this sample block is declined by the
        // pointer rule rather than the sample rule.
        let wide = Storage::owned(1, 512);
        let attempts = { hirari_audio_buffer_realtime_resize_attempts() };
        hirari_audio_buffer_set_realtime_guard(true);
        let sample_status =
            unsafe { hirari_audio_buffer_storage_reserve_buffer(stereo.0, 2, 128) };
        let sample_capacity = { hirari_audio_buffer_last_realtime_requested_capacity() };
        let pointer_status =
            unsafe { hirari_audio_buffer_storage_reserve_buffer(wide.0, 18, 16) };
        hirari_audio_buffer_set_realtime_guard(false);

        // Both growth plans are declined without touching the block, so the
        // prepared realtime view stays exactly as the callback left it.
        assert_eq!(sample_status, 1);
        assert_eq!(pointer_status, 5);
        assert_eq!(stereo.dimensions(), (2, 16));
        assert_eq!(wide.dimensions(), (1, 512));
        assert_eq!(
            { hirari_audio_buffer_realtime_resize_attempts() },
            attempts + 2
        );
        assert!(sample_capacity >= 2 * 128);

        // The rejection is published as a Critical notification by the same
        // code that declines it; nothing has to report it on the caller side.
        let mut reasons = [String::new(), String::new()];
        for reason in &mut reasons {
            let mut message = StatusMessage::default();
            assert!(unsafe { hirari_status_queue_pop_default(&mut message) });
            assert_eq!(message.severity, SEVERITY_CRITICAL);
            let end = message
                .text
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(message.text.len());
            *reason = String::from_utf8_lossy(&message.text[..end]).into_owned();
        }
        assert!(reasons.iter().any(|text| text == "AUDIO_BUFFER_RESIZE_ATTEMPTED"));
        assert!(reasons
            .iter()
            .any(|text| text == "AUDIO_BUFFER_POINTER_RESIZE_ATTEMPTED"));

        // Off the realtime thread the same growth is admitted, and a table
        // beyond the inline channels is allocated then.
        assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(stereo.0, 2, 128) }, 0);
        assert!(unsafe { hirari_audio_buffer_storage_resize(stereo.0, 2, 128) });
        assert_eq!(stereo.dimensions(), (2, 128));
        assert!({ AUDIO_BUFFER_INLINE_CHANNELS < 18 });
        assert_eq!(unsafe { hirari_audio_buffer_storage_reserve_buffer(wide.0, 18, 16) }, 0);
        assert!(unsafe { hirari_audio_buffer_storage_resize(wide.0, 18, 16) });
        assert_eq!(wide.dimensions(), (18, 16));
    }
}

#[cfg(test)]
#[path = "audio_buffer_ops_frozen_render_tests.rs"]
mod frozen_render_tests;
