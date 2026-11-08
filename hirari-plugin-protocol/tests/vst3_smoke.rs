#![cfg(all(feature = "worker-runtime", vst3_sdk))]

// Smoke test against a real-world VST3 (Surge XT by default): load,
// render, state save/load. Skips when the fixture is absent.
use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::Ordering;

use hirari_plugin_protocol::mailbox::SharedAudioBlock;

extern "C" {
    fn hirari_worker_vst3_supported() -> bool;
    fn hirari_worker_vst3_load(
        path: *const std::ffi::c_char,
        sample_rate: f64,
        max_frames: u32,
        midi_instrument: *mut bool,
    ) -> *mut std::ffi::c_void;
    fn hirari_worker_vst3_destroy(instance: *mut std::ffi::c_void);
    fn hirari_worker_vst3_reset(
        instance: *mut std::ffi::c_void,
        sample_rate: f64,
        min_frames: u32,
        max_frames: u32,
    ) -> bool;
    fn hirari_worker_vst3_process(
        instance: *mut std::ffi::c_void,
        opaque: *mut std::ffi::c_void,
        channels: u32,
        frames: u32,
    ) -> bool;
    fn hirari_worker_vst3_state(
        instance: *mut std::ffi::c_void,
        opaque: *mut std::ffi::c_void,
        mode: u8,
    ) -> u16;
}

#[test]
fn load_render_state_cycle() {
    let fixture = std::env::var("HIRARI_VST3_FIXTURE").ok().or_else(|| {
        std::env::var("HOME")
            .ok()
            .map(|home| format!("{home}/Library/Audio/Plug-Ins/VST3/Surge XT.vst3"))
    });
    let Some(fixture) = fixture else { return };
    if !Path::new(&fixture).exists() {
        return;
    }

    assert!(unsafe { hirari_worker_vst3_supported() });
    let path = CString::new(fixture).unwrap();
    let mut midi = false;
    let instance = unsafe { hirari_worker_vst3_load(path.as_ptr(), 48_000.0, 512, &mut midi) };
    assert!(!instance.is_null(), "VST3 fixture failed to load");
    assert!(unsafe { hirari_worker_vst3_reset(instance, 48_000.0, 1, 512) });

    let mut block: Box<SharedAudioBlock> = unsafe { Box::new_zeroed().assume_init() };
    block.frames.store(512, Ordering::Release);
    block.channels.store(2, Ordering::Release);
    block.midi_events.store(0, Ordering::Release);
    block.parameter_changes.store(0, Ordering::Release);
    block.output_midi_events.store(0, Ordering::Release);
    for channel in 0..2 {
        for frame in 0..512 {
            block.input.samples[channel][frame] = 0.25;
        }
    }

    let opaque = Box::<SharedAudioBlock>::as_mut_ptr(&mut block).cast::<std::ffi::c_void>();
    let ok = unsafe { hirari_worker_vst3_process(instance, opaque, 2, 512) };
    assert!(ok, "VST3 process failed");
    let output_finite = (0..2)
        .any(|channel| (0..512).any(|frame| block.output.samples[channel][frame].is_finite()));
    assert!(output_finite, "output not finite");

    let status = unsafe { hirari_worker_vst3_state(instance, opaque, 1) };
    assert_eq!(status & 0xff, 1, "state save failed: {status:#x}");
    let saved = block.state_size.load(Ordering::Acquire);
    assert!(saved > 0);
    assert_ne!(block.state_version.load(Ordering::Acquire), 0);
    assert_ne!(block.state_checksum.load(Ordering::Acquire), 0);

    let loaded = unsafe { hirari_worker_vst3_state(instance, opaque, 2) };
    assert_eq!(loaded & 0xff, 1, "state load failed: {loaded:#x}");
    unsafe { hirari_worker_vst3_destroy(instance) };
}
