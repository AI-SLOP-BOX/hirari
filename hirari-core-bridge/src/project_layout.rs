/// Advances the project-layout FNV fingerprint for one stream chunk.
#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_fingerprint_update(
    state: *mut u64,
    data: *const u8,
    length: usize,
) {
    if state.is_null() || (length != 0 && data.is_null()) {
        return;
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    let hash = unsafe { &mut *state };
    for byte in bytes {
        *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
}

/// Builds a cheap polling token that includes layout, MIDI note edits, and
/// control changes while keeping the combination arithmetic Rust-owned.
#[no_mangle]
pub extern "C" fn hirari_project_state_revision_mix(
    layout: u64,
    midi: u64,
    control: u64,
) -> u64 {
    let mut value = layout ^ (midi.wrapping_add(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(layout << 6).wrapping_add(layout >> 2));
    value ^= control.wrapping_add(0x517c_c1b7_2722_0a95)
        .wrapping_add(value << 6).wrapping_add(value >> 2);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
