fn valid_pcm_or_float_wav(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let read_u16 = |v: &[u8]| u16::from_le_bytes([v[0], v[1]]);
    let read_u32 = |v: &[u8]| u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    let read_u64 = |v: &[u8]| v.try_into().ok().map(u64::from_le_bytes);
    let mut cursor = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut rf64_data_size: Option<u64> = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[cursor..cursor + 4];
        let declared_size = read_u32(&bytes[cursor + 4..cursor + 8]);
        let size = if kind == b"data" && declared_size == u32::MAX {
            rf64_data_size.and_then(|value| usize::try_from(value).ok())
        } else {
            usize::try_from(declared_size).ok()
        };
        let Some(size) = size else { return false };
        let start = cursor + 8;
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"ds64" && size >= 16 {
            let Some(value) = read_u64(&bytes[start + 8..start + 16]) else {
                return false;
            };
            rf64_data_size = Some(value);
        } else if kind == b"fmt " && size >= 16 {
            let chunk = &bytes[start..end];
            fmt = Some((
                read_u16(&chunk[0..2]),
                read_u16(&chunk[2..4]),
                read_u32(&chunk[4..8]),
                read_u16(&chunk[12..14]),
                read_u16(&chunk[14..16]),
            ));
        } else if kind == b"data" {
            data = Some(&bytes[start..end]);
        }
        let padded_end = end
            .checked_add(size & 1)
            .filter(|value| *value <= bytes.len());
        let Some(padded_end) = padded_end else {
            return false;
        };
        cursor = padded_end;
    }
    let Some((audio_format, channels, sample_rate, block_align, bits_per_sample)) = fmt else {
        return false;
    };
    let Some(payload) = data else { return false };
    let valid_format = audio_format == 1 || audio_format == 3;
    let valid_depth = matches!(bits_per_sample, 16 | 24 | 32);
    let valid_channels = channels > 0 && channels <= 2;
    valid_format
        && valid_depth
        && valid_channels
        && sample_rate > 0
        && block_align > 0
        && payload.len() % usize::from(block_align) == 0
}

fn float_wav_samples_are_finite(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let read_u16 = |v: &[u8]| u16::from_le_bytes([v[0], v[1]]);
    let read_u32 = |v: &[u8]| u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
    let read_u64 = |v: &[u8]| v.try_into().ok().map(u64::from_le_bytes);
    let mut cursor = 12usize;
    let mut is_float = false;
    let mut payload = None;
    let mut rf64_data_size: Option<u64> = None;
    while cursor.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[cursor..cursor + 4];
        let declared_size = read_u32(&bytes[cursor + 4..cursor + 8]);
        let size = if kind == b"data" && declared_size == u32::MAX {
            rf64_data_size.and_then(|value| usize::try_from(value).ok())
        } else {
            usize::try_from(declared_size).ok()
        };
        let Some(size) = size else {
            return false;
        };
        let start = cursor + 8;
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"ds64" && size >= 16 {
            let Some(value) = read_u64(&bytes[start + 8..start + 16]) else {
                return false;
            };
            rf64_data_size = Some(value);
        } else if kind == b"fmt " && size >= 16 {
            is_float = read_u16(&bytes[start..start + 2]) == 3
                && read_u16(&bytes[start + 14..start + 16]) == 32;
        } else if kind == b"data" {
            payload = Some(&bytes[start..end]);
        }
        let padded_end = end
            .checked_add(size & 1)
            .filter(|value| *value <= bytes.len());
        let Some(padded_end) = padded_end else {
            return false;
        };
        cursor = padded_end;
    }
    let Some(payload) = payload else { return false };
    !is_float
        || payload.chunks(4).all(|sample| {
            sample.len() == 4
                && f32::from_le_bytes(sample.try_into().expect("f32-sized chunk")).is_finite()
        })
}

fn is_wav_output_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}
