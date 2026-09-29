use super::{hirari_midi_buffer_add_event, hirari_midi_buffer_copy_event};
use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            sample_offset: 0,
            size: 0,
            data: [0; 256],
            articulation_id: 0,
        }
    }
}

unsafe extern "C" {
    fn midi_buffer_reference_add_event(
        storage: *mut c_void,
        capacity: usize,
        count: *mut usize,
        sample_offset: u64,
        data: *const u8,
        size: u32,
        articulation_id: u8,
    ) -> u8;
    fn midi_buffer_reference_copy_event(
        storage: *mut c_void,
        capacity: usize,
        count: *mut usize,
        source: *const c_void,
    ) -> u8;
}

#[test]
fn rust_fixed_buffer_writes_match_frozen_cpp_midi_buffer() {
    const CAPACITY: usize = 3;
    let mut rust = [Event::default(); CAPACITY];
    let mut cpp = [Event::default(); CAPACITY];
    let (mut rust_count, mut cpp_count) = (0usize, 0usize);
    let packets: [&[u8]; 4] = [
        &[0x90, 60, 100],
        &[0xf0, 0x7e, 0x7f, 0x09, 0x01],
        &[],
        &[0x80, 60, 0],
    ];
    for (index, packet) in packets.iter().enumerate() {
        let data = if packet.is_empty() {
            std::ptr::null()
        } else {
            packet.as_ptr()
        };
        let rust_result = unsafe {
            hirari_midi_buffer_add_event(
                rust.as_mut_ptr().cast::<c_void>(),
                CAPACITY,
                &mut rust_count,
                (index * 13) as u64,
                data,
                packet.len() as u32,
                (index % 2) as u8,
            )
        };
        let cpp_result = unsafe {
            midi_buffer_reference_add_event(
                cpp.as_mut_ptr().cast::<c_void>(),
                CAPACITY,
                &mut cpp_count,
                (index * 13) as u64,
                data,
                packet.len() as u32,
                (index % 2) as u8,
            )
        };
        assert_eq!(rust_result, cpp_result, "append result at packet {index}");
        assert_eq!(rust_count, cpp_count);
        assert_eq!(rust[..rust_count], cpp[..cpp_count]);
    }

    let mut source = Event {
        sample_offset: 91,
        size: 3,
        data: [0xa5; 256],
        articulation_id: 7,
    };
    source.data[..3].copy_from_slice(&[0x91, 64, 80]);
    // Copy into fresh buffers to exercise successful append before the full case.
    let mut rust_copy = [Event::default(); 2];
    let mut cpp_copy = [Event::default(); 2];
    let (mut rust_copy_count, mut cpp_copy_count) = (0usize, 0usize);
    assert_eq!(
        unsafe {
            hirari_midi_buffer_copy_event(
                rust_copy.as_mut_ptr().cast(),
                2,
                &mut rust_copy_count,
                (&source as *const Event).cast(),
            )
        },
        unsafe {
            midi_buffer_reference_copy_event(
                cpp_copy.as_mut_ptr().cast(),
                2,
                &mut cpp_copy_count,
                (&source as *const Event).cast(),
            )
        }
    );
    assert_eq!(rust_copy_count, cpp_copy_count);
    assert_eq!(rust_copy[..rust_copy_count], cpp_copy[..cpp_copy_count]);
    assert!(rust_copy[0].data[3..].iter().all(|byte| *byte == 0));

    for invalid_size in [257u32, u32::MAX] {
        let rust_result = unsafe {
            hirari_midi_buffer_add_event(
                rust.as_mut_ptr().cast(),
                CAPACITY,
                &mut rust_count,
                0,
                [0u8; 257].as_ptr(),
                invalid_size,
                0,
            )
        };
        let cpp_result = unsafe {
            midi_buffer_reference_add_event(
                cpp.as_mut_ptr().cast(),
                CAPACITY,
                &mut cpp_count,
                0,
                [0u8; 257].as_ptr(),
                invalid_size,
                0,
            )
        };
        assert_eq!(rust_result, cpp_result);
    }
    assert_eq!(rust_count, cpp_count);
}

#[test]
fn rust_midi_buffer_write_rejects_null_and_full_inputs_like_cpp() {
    let mut rust = [Event::default(); 1];
    let mut cpp = [Event::default(); 1];
    let (mut rust_count, mut cpp_count) = (0usize, 0usize);
    let rust_null = unsafe {
        hirari_midi_buffer_add_event(
            rust.as_mut_ptr().cast(),
            1,
            &mut rust_count,
            0,
            std::ptr::null(),
            1,
            0,
        )
    };
    let cpp_null = unsafe {
        midi_buffer_reference_add_event(
            cpp.as_mut_ptr().cast(),
            1,
            &mut cpp_count,
            0,
            std::ptr::null(),
            1,
            0,
        )
    };
    assert_eq!(rust_null, cpp_null);
    assert_eq!((rust_count, cpp_count), (0, 0));

    let byte = [0x90];
    assert_eq!(
        unsafe {
            hirari_midi_buffer_add_event(
                rust.as_mut_ptr().cast(),
                1,
                &mut rust_count,
                0,
                byte.as_ptr(),
                1,
                0,
            )
        },
        unsafe {
            midi_buffer_reference_add_event(
                cpp.as_mut_ptr().cast(),
                1,
                &mut cpp_count,
                0,
                byte.as_ptr(),
                1,
                0,
            )
        }
    );
    assert_eq!(
        unsafe {
            hirari_midi_buffer_add_event(
                rust.as_mut_ptr().cast(),
                1,
                &mut rust_count,
                0,
                byte.as_ptr(),
                1,
                0,
            )
        },
        unsafe {
            midi_buffer_reference_add_event(
                cpp.as_mut_ptr().cast(),
                1,
                &mut cpp_count,
                0,
                byte.as_ptr(),
                1,
                0,
            )
        }
    );
    assert_eq!(rust_count, cpp_count);
    assert_eq!(rust[..rust_count], cpp[..cpp_count]);
}
