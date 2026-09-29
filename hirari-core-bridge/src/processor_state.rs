use std::slice;

const STATE_SIZE: usize = 16;
const STATE_MAGIC: u32 = 0x4155_5241;
const STATE_VERSION: u16 = 1;

/// Encodes the host-owned processor state in the existing native-endian format.
#[no_mangle]
pub unsafe extern "C" fn hirari_processor_state_encode(
    bypassed: bool,
    mix: f32,
    sidechain_bus: u32,
    output: *mut u8,
    output_len: usize,
) -> bool {
    if output.is_null() || output_len != STATE_SIZE {
        return false;
    }
    // SAFETY: the caller provides a writable buffer of exactly STATE_SIZE bytes.
    let output = unsafe { slice::from_raw_parts_mut(output, STATE_SIZE) };
    output[..4].copy_from_slice(&STATE_MAGIC.to_ne_bytes());
    output[4..6].copy_from_slice(&STATE_VERSION.to_ne_bytes());
    output[6..8].copy_from_slice(&u16::from(bypassed).to_ne_bytes());
    output[8..12].copy_from_slice(&mix.to_ne_bytes());
    output[12..16].copy_from_slice(&sidechain_bus.to_ne_bytes());
    true
}

/// Validates and decodes the host-owned processor state.
#[no_mangle]
pub unsafe extern "C" fn hirari_processor_state_decode(
    input: *const u8,
    input_len: usize,
    bypassed_out: *mut u8,
    mix_out: *mut f32,
    sidechain_bus_out: *mut u32,
) -> bool {
    if input.is_null()
        || input_len != STATE_SIZE
        || bypassed_out.is_null()
        || mix_out.is_null()
        || sidechain_bus_out.is_null()
    {
        return false;
    }
    // SAFETY: the caller provides a readable buffer of exactly STATE_SIZE bytes.
    let input = unsafe { slice::from_raw_parts(input, STATE_SIZE) };
    let magic = u32::from_ne_bytes(input[..4].try_into().unwrap());
    let version = u16::from_ne_bytes(input[4..6].try_into().unwrap());
    let flags = u16::from_ne_bytes(input[6..8].try_into().unwrap());
    let mix = f32::from_ne_bytes(input[8..12].try_into().unwrap());
    let sidechain_bus = u32::from_ne_bytes(input[12..16].try_into().unwrap());
    if magic != STATE_MAGIC
        || version != STATE_VERSION
        || flags & !1 != 0
        || !mix.is_finite()
        || !(0.0..=1.0).contains(&mix)
    {
        return false;
    }
    // SAFETY: all output pointers were checked and point to caller-owned scalars.
    unsafe {
        *bypassed_out = (flags & 1) as u8;
        *mix_out = mix;
        *sidechain_bus_out = sidechain_bus;
    }
    true
}
