#[repr(C)]
#[derive(Clone, Copy)]
struct MidiEventReference {
    sample_offset: u64,
    size: u32,
    data: [u8; 256],
    articulation_id: u8,
}

const _: () = assert!(std::mem::size_of::<MidiEventReference>() == 272);

#[no_mangle]
pub unsafe extern "C" fn midi_buffer_reference_add_event(
    storage: *mut std::ffi::c_void,
    capacity: usize,
    count: *mut usize,
    sample_offset: u64,
    data: *const u8,
    size: u32,
    articulation_id: u8,
) -> u8 {
    if size > 256 {
        return 1;
    }
    if size != 0 && data.is_null() {
        return 2;
    }
    if storage.is_null() || count.is_null() {
        return 4;
    }
    if *count >= capacity {
        return 3;
    }
    let events = storage as *mut MidiEventReference;
    let event = &mut *events.add(*count);
    *count += 1;
    event.sample_offset = sample_offset;
    event.size = size;
    event.articulation_id = articulation_id;
    if size > 0 {
        std::ptr::copy_nonoverlapping(data, event.data.as_mut_ptr(), size as usize);
    }
    event.data[size as usize..].fill(0);
    0
}

#[no_mangle]
pub unsafe extern "C" fn midi_buffer_reference_copy_event(
    storage: *mut std::ffi::c_void,
    capacity: usize,
    count: *mut usize,
    source: *const std::ffi::c_void,
) -> u8 {
    if source.is_null() {
        return 4;
    }
    let event = *(source as *const MidiEventReference);
    if event.size > 256 {
        return 1;
    }
    if storage.is_null() || count.is_null() {
        return 4;
    }
    if *count >= capacity {
        return 3;
    }
    let events = storage as *mut MidiEventReference;
    let destination = &mut *events.add(*count);
    *count += 1;
    *destination = event;
    destination.data[destination.size as usize..].fill(0);
    0
}
