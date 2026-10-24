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

#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_escape_json_string(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_cap: usize,
    written: *mut usize,
) -> bool {
    if (input_len != 0 && input.is_null()) || output.is_null() || written.is_null() {
        return false;
    }
    let bytes = if input_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };
    let mut encoded = Vec::with_capacity(input_len + 8);
    for &b in bytes {
        match b {
            b'"' => encoded.extend_from_slice(b"\\\""),
            b'\\' => encoded.extend_from_slice(b"\\\\"),
            b'\n' => encoded.extend_from_slice(b"\\n"),
            b'\r' => encoded.extend_from_slice(b"\\r"),
            b'\t' => encoded.extend_from_slice(b"\\t"),
            c if c < 0x20 => encoded.push(b'?'),
            c => encoded.push(c),
        }
    }
    *written = encoded.len();
    if encoded.len() > output_cap {
        return false;
    }
    std::ptr::copy_nonoverlapping(encoded.as_ptr(), output, encoded.len());
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_project_layout_encode_hex(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_cap: usize,
    written: *mut usize,
) -> bool {
    if (input_len != 0 && input.is_null()) || output.is_null() || written.is_null() {
        return false;
    }
    let required = input_len * 2;
    *written = required;
    if required > output_cap {
        return false;
    }
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let bytes = if input_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };
    for (i, &b) in bytes.iter().enumerate() {
        *output.add(i * 2) = DIGITS[(b >> 4) as usize];
        *output.add(i * 2 + 1) = DIGITS[(b & 0x0f) as usize];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escape(input: &[u8], capacity: usize) -> (bool, Vec<u8>, usize) {
        let mut out = vec![0u8; capacity];
        let mut written = 0usize;
        let ok = unsafe {
            hirari_project_layout_escape_json_string(
                input.as_ptr(),
                input.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        (ok, out, written)
    }

    #[test]
    fn escapes_exactly_the_bytes_the_cpp_writer_escaped() {
        let (ok, out, written) = escape(b"a\"b\\c\nd\re\tf\x01g", 64);
        assert!(ok);
        assert_eq!(&out[..written], b"a\\\"b\\\\c\\nd\\re\\tf?g");
    }

    #[test]
    fn reports_the_required_size_when_the_buffer_is_short() {
        let (ok, _, written) = escape(b"\"\"", 1);
        assert!(!ok);
        assert_eq!(written, 4);
    }

    #[test]
    fn encodes_lowercase_hex() {
        let input = [0x00u8, 0xab, 0xff];
        let mut out = [0u8; 6];
        let mut written = 0usize;
        let ok = unsafe {
            hirari_project_layout_encode_hex(
                input.as_ptr(),
                input.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        assert!(ok);
        assert_eq!(&out[..written], b"00abff");
    }

    #[test]
    fn rejects_a_null_input_with_a_nonzero_length() {
        let mut out = [0u8; 8];
        let mut written = 0usize;
        assert!(!unsafe {
            hirari_project_layout_escape_json_string(
                std::ptr::null(),
                3,
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        });
        assert!(!unsafe {
            hirari_project_layout_encode_hex(
                std::ptr::null(),
                3,
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        });
    }
}
