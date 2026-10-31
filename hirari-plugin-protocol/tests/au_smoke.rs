#![cfg(feature = "worker-runtime")]

use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// Force the crate under test to be linked; the AuHost entry points are
// `#[no_mangle]` symbols that only exist once the archive is pulled in.
use hirari_plugin_protocol as _;

extern "C" {
    fn hirari_au_instance_load(
        path: *const std::ffi::c_char,
        sample_rate: f64,
        max_frames: u32,
        channels: u32,
        midi_instrument: *mut bool,
    ) -> *mut std::ffi::c_void;
    fn hirari_au_instance_destroy(instance: *mut std::ffi::c_void);
    fn hirari_au_instance_reset(
        instance: *mut std::ffi::c_void,
        sample_rate: f64,
        min_frames: u32,
        max_frames: u32,
    ) -> bool;
    fn hirari_au_process_block(
        instance: *mut std::ffi::c_void,
        input_audio: *const f32,
        output_audio: *mut f32,
        channels: u32,
        frames: u32,
        midi_events: *const std::ffi::c_void,
        midi_count: u32,
        parameter_changes: *const std::ffi::c_void,
        parameter_count: u32,
    ) -> bool;
    fn hirari_au_state_block(
        instance: *mut std::ffi::c_void,
        mode: u8,
        state: *mut u8,
        state_size: *mut std::ffi::c_void,
        state_version: *mut std::ffi::c_void,
        state_checksum: *mut std::ffi::c_void,
    ) -> u16;
}

#[test]
fn load_render_state_cycle() {
    let component = "/System/Library/Components/AudioDSP.component";
    if !Path::new(component).exists() {
        return;
    }
    let path = CString::new(component).unwrap();
    let mut midi = false;
    let instance = unsafe { hirari_au_instance_load(path.as_ptr(), 48_000.0, 512, 2, &mut midi) };
    assert!(!instance.is_null(), "AudioDSP.component failed to load");
    assert!(unsafe { hirari_au_instance_reset(instance, 48_000.0, 1, 512) });

    let mut input = vec![0.0f32; 8192 * 2];
    let mut output = vec![0.0f32; 8192 * 2];
    for frame in 0..512 {
        input[frame] = 0.25;
        input[8192 + frame] = -0.25;
    }
    let ok = unsafe {
        hirari_au_process_block(
            instance,
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            512,
            ptr::null(),
            0,
            ptr::null(),
            0,
        )
    };
    assert!(ok, "process failed");
    assert!(
        output[..512].iter().any(|s| s.is_finite()),
        "output not finite"
    );

    let mut state = vec![0u8; 4 * 1024 * 1024];
    let size = AtomicU32::new(0);
    let version = AtomicU32::new(0);
    let checksum = AtomicU64::new(0);
    let status = unsafe {
        hirari_au_state_block(
            instance,
            1,
            state.as_mut_ptr(),
            (&size as *const AtomicU32).cast_mut().cast(),
            (&version as *const AtomicU32).cast_mut().cast(),
            (&checksum as *const AtomicU64).cast_mut().cast(),
        )
    };
    assert_eq!(status & 0xff, 1, "state save failed: {status:#x}");
    assert!(size.load(Ordering::Acquire) > 0);
    let saved = size.load(Ordering::Acquire);
    assert_eq!(
        unsafe {
            hirari_au_state_block(
                instance,
                2,
                state.as_mut_ptr(),
                (&size as *const AtomicU32).cast_mut().cast(),
                (&version as *const AtomicU32).cast_mut().cast(),
                (&checksum as *const AtomicU64).cast_mut().cast(),
            )
        } & 0xff,
        1,
        "state load failed"
    );
    assert!(saved > 0);
    unsafe { hirari_au_instance_destroy(instance) };
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawMidiEvent {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
    _padding: [u8; 3],
}

#[test]
fn instrument_receives_midi() {
    let component = "/Library/Audio/Plug-Ins/Components/Vital.component";
    if !Path::new(component).exists() {
        return;
    }
    let path = CString::new(component).unwrap();
    let mut midi = false;
    let instance = unsafe { hirari_au_instance_load(path.as_ptr(), 48_000.0, 512, 2, &mut midi) };
    assert!(!instance.is_null(), "Vital.component failed to load");
    assert!(midi, "instrument component not reported as a MIDI device");

    let input = vec![0.0f32; 8192 * 2];
    let mut output = vec![0.0f32; 8192 * 2];
    let mut note = RawMidiEvent {
        sample_offset: 0,
        size: 3,
        data: [0; 256],
        articulation_id: 0,
        _padding: [0; 3],
    };
    note.data[0] = 0x90;
    note.data[1] = 60;
    note.data[2] = 100;
    let ok = unsafe {
        hirari_au_process_block(
            instance,
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            512,
            (&note as *const RawMidiEvent).cast(),
            1,
            ptr::null(),
            0,
        )
    };
    assert!(ok, "instrument process failed");
    for _ in 0..8 {
        assert!(unsafe {
            hirari_au_process_block(
                instance,
                input.as_ptr(),
                output.as_mut_ptr(),
                2,
                512,
                ptr::null(),
                0,
                ptr::null(),
                0,
            )
        });
    }
    assert!(
        output[..512].iter().any(|sample| *sample != 0.0),
        "instrument produced silence for a note-on"
    );
    unsafe { hirari_au_instance_destroy(instance) };
}
