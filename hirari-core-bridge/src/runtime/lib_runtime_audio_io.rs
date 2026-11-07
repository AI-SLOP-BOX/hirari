#[cfg(test)]
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

/// Validates render output without loading the whole audio file into memory.
/// WAVE64, RIFF, and RF64 data are inspected in bounded chunks, which keeps
/// post-render validation suitable for multi-hour sessions.
fn validate_render_wave_file(path: &std::path::Path, require_audio: bool) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let file_size = file.metadata()?.len();
    if file_size < 12 {
        return Ok(false);
    }
    let mut head = [0u8; 104];
    let head_len = usize::try_from(file_size.min(head.len() as u64)).unwrap_or(head.len());
    file.read_exact(&mut head[..head_len])?;

    let read_u16 = |bytes: &[u8]| u16::from_le_bytes([bytes[0], bytes[1]]);
    let read_u32 = |bytes: &[u8]| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let read_u64 =
        |bytes: &[u8]| u64::from_le_bytes(bytes[0..8].try_into().expect("eight-byte integer"));

    let (format, channels, sample_rate, block_align, bit_depth, data_offset, data_bytes) =
        if head_len == head.len()
            && &head[0..4] == b"RIFF"
            && &head[24..28] == b"WAVE"
            && &head[40..44] == b"fmt "
            && &head[80..84] == b"data"
        {
            let riff_size = read_u64(&head[16..24]);
            let format_chunk_size = read_u64(&head[56..64]);
            let data_chunk_size = read_u64(&head[96..104]);
            if riff_size != file_size || format_chunk_size < 40 || data_chunk_size < 24 {
                return Ok(false);
            }
            let payload = data_chunk_size - 24;
            if 80u64.checked_add(data_chunk_size) != Some(file_size) || payload % 8 != 0 {
                return Ok(false);
            }
            (
                read_u16(&head[64..66]),
                read_u16(&head[66..68]),
                read_u32(&head[68..72]),
                read_u16(&head[76..78]),
                read_u16(&head[78..80]),
                104u64,
                payload,
            )
        } else {
            let is_rf64 = &head[0..4] == b"RF64";
            if (!is_rf64 && &head[0..4] != b"RIFF") || &head[8..12] != b"WAVE" {
                return Ok(false);
            }
            if !is_rf64 && u64::from(read_u32(&head[4..8])).saturating_add(8) != file_size {
                return Ok(false);
            }
            let mut offset = 12u64;
            let mut rf64_data_size = None;
            let mut fmt = None;
            let mut data = None;
            while offset.checked_add(8).is_some_and(|end| end <= file_size) {
                file.seek(SeekFrom::Start(offset))?;
                let mut chunk_head = [0u8; 8];
                file.read_exact(&mut chunk_head)?;
                let declared_size = read_u32(&chunk_head[4..8]);
                let chunk_size = if &chunk_head[0..4] == b"data" && declared_size == u32::MAX {
                    let Some(size) = rf64_data_size else {
                        return Ok(false);
                    };
                    size
                } else {
                    u64::from(declared_size)
                };
                let payload_offset = offset + 8;
                let Some(chunk_end) = payload_offset.checked_add(chunk_size) else {
                    return Ok(false);
                };
                if chunk_end > file_size {
                    return Ok(false);
                }
                if &chunk_head[0..4] == b"ds64" && chunk_size >= 28 {
                    let mut ds64 = [0u8; 28];
                    file.read_exact(&mut ds64)?;
                    let declared_riff = read_u64(&ds64[0..8]);
                    rf64_data_size = Some(read_u64(&ds64[8..16]));
                    if declared_riff.checked_add(8) != Some(file_size) {
                        return Ok(false);
                    }
                } else if &chunk_head[0..4] == b"fmt " && chunk_size >= 16 {
                    let mut fmt_head = [0u8; 16];
                    file.read_exact(&mut fmt_head)?;
                    fmt = Some((
                        read_u16(&fmt_head[0..2]),
                        read_u16(&fmt_head[2..4]),
                        read_u32(&fmt_head[4..8]),
                        read_u16(&fmt_head[12..14]),
                        read_u16(&fmt_head[14..16]),
                    ));
                } else if &chunk_head[0..4] == b"data" {
                    data = Some((payload_offset, chunk_size));
                }
                let Some(next) = chunk_end
                    .checked_add(chunk_size & 1)
                    .filter(|next| *next <= file_size)
                else {
                    return Ok(false);
                };
                offset = next;
            }
            let Some(fmt) = fmt else { return Ok(false) };
            let Some((data_offset, data_bytes)) = data else {
                return Ok(false);
            };
            (fmt.0, fmt.1, fmt.2, fmt.3, fmt.4, data_offset, data_bytes)
        };

    let bytes_per_sample = u64::from(bit_depth / 8);
    if !matches!(format, 1 | 3)
        || channels == 0
        || channels > 2
        || sample_rate == 0
        || !matches!(bit_depth, 16 | 24 | 32)
        || (format == 3 && bit_depth != 32)
        || u64::from(block_align) != u64::from(channels) * bytes_per_sample
        || data_bytes == 0
        || data_bytes % u64::from(block_align) != 0
        || data_offset
            .checked_add(data_bytes)
            .is_none_or(|end| end > file_size)
    {
        return Ok(false);
    }
    if !require_audio && format != 3 {
        return Ok(true);
    }

    file.seek(SeekFrom::Start(data_offset))?;
    let mut remaining = data_bytes;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut has_audio = false;
    let mut trailing = Vec::with_capacity(3);
    while remaining > 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        file.read_exact(&mut buffer[..count])?;
        remaining -= count as u64;
        if format == 3 {
            let mut samples = trailing.drain(..).collect::<Vec<_>>();
            samples.extend_from_slice(&buffer[..count]);
            let complete = samples.len() / 4 * 4;
            if samples[..complete].chunks_exact(4).any(|sample| {
                let value = f32::from_le_bytes(sample.try_into().expect("float-sized chunk"));
                !value.is_finite() || (require_audio && value != 0.0)
            }) {
                if samples[..complete].chunks_exact(4).any(|sample| {
                    !f32::from_le_bytes(sample.try_into().expect("float-sized chunk")).is_finite()
                }) {
                    return Ok(false);
                }
                has_audio = true;
            }
            trailing.extend_from_slice(&samples[complete..]);
        } else if require_audio && buffer[..count].iter().any(|byte| *byte != 0) {
            has_audio = true;
        }
    }
    if !trailing.is_empty() {
        return Ok(false);
    }
    Ok(!require_audio || has_audio)
}

#[cfg(test)]
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
