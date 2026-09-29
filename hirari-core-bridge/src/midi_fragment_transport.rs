use std::ffi::c_void;

pub use hirari_plugin_protocol::{
    hirari_midi_extended_ring_init, hirari_midi_extended_ring_pop,
    hirari_midi_extended_ring_push, hirari_midi_extended_ring_size,
};

pub const MIDI_FRAGMENT_PAYLOAD_BYTES: usize = 240;
pub const MIDI_FRAGMENT_MAXIMUM_MESSAGE_BYTES: usize = 1024 * 1024;
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiFragmentResult {
    Accepted = 0,
    Complete = 1,
    Invalid = 2,
    OutOfOrder = 3,
    Oversize = 4,
}

struct MidiFragmentReassembler {
    message_id: u32,
    next_index: u16,
    total: u16,
    size: usize,
    sample_offset: u64,
    articulation_id: u8,
    complete: bool,
    bytes: Vec<u8>,
}

impl MidiFragmentReassembler {
    fn new() -> Self {
        Self {
            message_id: 0,
            next_index: 0,
            total: 0,
            size: 0,
            sample_offset: 0,
            articulation_id: 0,
            complete: false,
            bytes: vec![0; MIDI_FRAGMENT_MAXIMUM_MESSAGE_BYTES],
        }
    }

    fn reset(&mut self) {
        self.message_id = 0;
        self.next_index = 0;
        self.total = 0;
        self.size = 0;
        self.sample_offset = 0;
        self.articulation_id = 0;
        self.complete = false;
    }

    fn push(
        &mut self,
        message_id: u32,
        index: u16,
        total: u16,
        sample_offset: u64,
        articulation_id: u8,
        bytes: *const u8,
        size: u16,
    ) -> MidiFragmentResult {
        if total == 0
            || index >= total
            || size == 0
            || size as usize > MIDI_FRAGMENT_PAYLOAD_BYTES
            || bytes.is_null()
        {
            self.reset();
            return MidiFragmentResult::Invalid;
        }
        if index == 0 {
            self.reset();
            self.message_id = message_id;
            self.total = total;
            self.sample_offset = sample_offset;
            self.articulation_id = articulation_id;
        }
        if message_id != self.message_id || total != self.total || index != self.next_index {
            self.reset();
            return MidiFragmentResult::OutOfOrder;
        }
        let fragment_size = size as usize;
        if self.size > MIDI_FRAGMENT_MAXIMUM_MESSAGE_BYTES - fragment_size {
            self.reset();
            return MidiFragmentResult::Oversize;
        }
        // SAFETY: The C++ fragment owns a fixed 240-byte payload, and size was
        // checked above to fit that payload before copying into our fixed buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes,
                self.bytes.as_mut_ptr().add(self.size),
                fragment_size,
            );
        }
        self.size += fragment_size;
        self.next_index += 1;
        if self.next_index == self.total {
            self.complete = true;
            MidiFragmentResult::Complete
        } else {
            MidiFragmentResult::Accepted
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_midi_fragment_reassembler_create() -> *mut c_void {
    Box::into_raw(Box::new(MidiFragmentReassembler::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: This pointer is created by the matching constructor and destroyed once.
        drop(Box::from_raw(state.cast::<MidiFragmentReassembler>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_reset(state: *mut c_void) {
    if let Some(state) = state.cast::<MidiFragmentReassembler>().as_mut() {
        state.reset();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_push(
    state: *mut c_void,
    message_id: u32,
    index: u16,
    total: u16,
    sample_offset: u64,
    articulation_id: u8,
    bytes: *const u8,
    size: u16,
) -> u8 {
    state.cast::<MidiFragmentReassembler>().as_mut().map_or(
        MidiFragmentResult::Invalid as u8,
        |state| {
            state.push(
                message_id,
                index,
                total,
                sample_offset,
                articulation_id,
                bytes,
                size,
            ) as u8
        },
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_complete(state: *const c_void) -> bool {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .is_some_and(|state| state.complete)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_size(state: *const c_void) -> usize {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .map_or(0, |state| state.size)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_message_id(state: *const c_void) -> u32 {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .map_or(0, |state| state.message_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_sample_offset(
    state: *const c_void,
) -> u64 {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .map_or(0, |state| state.sample_offset)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_articulation_id(
    state: *const c_void,
) -> u8 {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .map_or(0, |state| state.articulation_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_midi_fragment_reassembler_data(state: *const c_void) -> *const u8 {
    state
        .cast::<MidiFragmentReassembler>()
        .as_ref()
        .map_or(std::ptr::null(), |state| state.bytes.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::{
        hirari_midi_extended_ring_init, hirari_midi_extended_ring_pop,
        hirari_midi_extended_ring_push, hirari_midi_extended_ring_size, MidiFragmentReassembler,
        MidiFragmentResult, MIDI_EXTENDED_RING_STORAGE_BYTES,
    };
    use std::alloc::{alloc_zeroed, dealloc, Layout};
    use std::ffi::c_void;

    #[test]
    fn reassembles_in_order_and_preserves_first_fragment_metadata() {
        let mut reassembler = MidiFragmentReassembler::new();
        let first = [1u8, 2, 3];
        let second = [4u8, 5];
        assert_eq!(
            reassembler.push(42, 0, 2, 128, 7, first.as_ptr(), 3),
            MidiFragmentResult::Accepted
        );
        assert_eq!(
            reassembler.push(42, 1, 2, 999, 2, second.as_ptr(), 2),
            MidiFragmentResult::Complete
        );
        assert!(reassembler.complete);
        assert_eq!(reassembler.message_id, 42);
        assert_eq!(reassembler.sample_offset, 128);
        assert_eq!(reassembler.articulation_id, 7);
        assert_eq!(&reassembler.bytes[..reassembler.size], &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn invalid_and_out_of_order_fragments_clear_partial_state() {
        let mut reassembler = MidiFragmentReassembler::new();
        let payload = [1u8];
        assert_eq!(
            reassembler.push(1, 0, 2, 0, 0, payload.as_ptr(), 1),
            MidiFragmentResult::Accepted
        );
        assert_eq!(
            reassembler.push(1, 2, 2, 0, 0, payload.as_ptr(), 1),
            MidiFragmentResult::Invalid
        );
        assert_eq!(reassembler.size, 0);
        assert_eq!(
            reassembler.push(1, 0, 2, 0, 0, payload.as_ptr(), 1),
            MidiFragmentResult::Accepted
        );
        assert_eq!(
            reassembler.push(2, 1, 2, 0, 0, payload.as_ptr(), 1),
            MidiFragmentResult::OutOfOrder
        );
        assert_eq!(reassembler.size, 0);
    }

    #[test]
    fn extended_ring_preserves_capacity_fifo_wrap_and_message_payload() {
        // Allocate with the same cache-line alignment required by the C++ wrapper.
        let layout = Layout::from_size_align(MIDI_EXTENDED_RING_STORAGE_BYTES, 64).unwrap();
        // SAFETY: allocation has sufficient size/alignment and is released below.
        let storage = unsafe { alloc_zeroed(layout) };
        assert!(!storage.is_null());
        // SAFETY: this test owns the fresh storage exclusively.
        assert!(unsafe { hirari_midi_extended_ring_init(storage.cast::<c_void>(), layout.size()) });
        let first = [10u8, 11, 12];
        for index in 0..4u64 {
            // SAFETY: storage is initialized and payload remains readable for its length.
            assert!(unsafe {
                hirari_midi_extended_ring_push(
                    storage.cast(),
                    100 + index,
                    index as u8,
                    first.as_ptr(),
                    first.len(),
                )
            });
        }
        // SAFETY: the state remains initialized.
        assert_eq!(unsafe { hirari_midi_extended_ring_size(storage.cast()) }, 4);
        // SAFETY: validated rejection cases do not dereference payload pointers.
        assert!(!unsafe {
            hirari_midi_extended_ring_push(storage.cast(), 0, 0, first.as_ptr(), first.len())
        });

        let message_layout =
            Layout::from_size_align(16 + super::MIDI_FRAGMENT_MAXIMUM_MESSAGE_BYTES, 8).unwrap();
        // SAFETY: output buffer is large enough and aligned for the fixed C++ Message layout.
        let message = unsafe { alloc_zeroed(message_layout) };
        assert!(!message.is_null());
        for index in 0..4u64 {
            // SAFETY: ring contains the queued message and output points to writable message storage.
            assert!(unsafe { hirari_midi_extended_ring_pop(storage.cast(), message.cast()) });
            // SAFETY: pop copied a complete fixed-layout Message into this buffer.
            let copied = unsafe { std::slice::from_raw_parts(message, message_layout.size()) };
            assert_eq!(
                u64::from_ne_bytes(copied[0..8].try_into().unwrap()),
                100 + index
            );
            assert_eq!(copied[8], index as u8);
            assert_eq!(u32::from_ne_bytes(copied[12..16].try_into().unwrap()), 3);
            assert_eq!(&copied[16..19], &first);
        }
        // The producer can reuse slots after the consumer publishes tail.
        // SAFETY: ring is empty and the same payload is still valid.
        assert!(unsafe {
            hirari_midi_extended_ring_push(storage.cast(), 999, 7, first.as_ptr(), first.len())
        });
        // SAFETY: ring contains one message and output has full capacity.
        assert!(unsafe { hirari_midi_extended_ring_pop(storage.cast(), message.cast()) });
        // SAFETY: pop copied the complete message.
        let copied = unsafe { std::slice::from_raw_parts(message, message_layout.size()) };
        assert_eq!(u64::from_ne_bytes(copied[0..8].try_into().unwrap()), 999);
        // SAFETY: both allocations are live and use the matching layouts.
        unsafe {
            dealloc(message, message_layout);
            dealloc(storage, layout);
        }
    }
}
