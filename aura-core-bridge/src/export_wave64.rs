/// Writes a WAVE64 IEEE-float file. WAVE64 uses 64-bit chunk sizes and is the
/// export path for files that exceed RF64's practical interoperability limits.
/// The GUID/chunk layout is intentionally explicit so it is not confused with
/// RIFF's four-byte identifiers.
pub fn write_wave64_float32(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), WavExportError> {
    if path.as_os_str().is_empty() || samples.is_empty() {
        return Err(if samples.is_empty() {
            WavExportError::EmptyBuffer
        } else {
            WavExportError::InvalidPath
        });
    }
    if !(8_000..=384_000).contains(&sample_rate) {
        return Err(WavExportError::InvalidSampleRate);
    }
    if !(1..=32).contains(&channels) {
        return Err(WavExportError::InvalidChannelCount);
    }
    if !samples.len().is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err(WavExportError::NonFiniteSample);
    }
    let _output_lock = OutputPublicationLock::acquire(path)?;
    const RIFF_GUID: [u8; 16] = [
        0x52, 0x49, 0x46, 0x46, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const WAVE_GUID: [u8; 16] = [
        0x57, 0x41, 0x56, 0x45, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const FMT_GUID: [u8; 16] = [
        0x66, 0x6d, 0x74, 0x20, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const DATA_GUID: [u8; 16] = [
        0x64, 0x61, 0x74, 0x61, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    let payload_bytes = samples
        .len()
        .checked_mul(4)
        .ok_or(WavExportError::FileTooLarge)? as u64;
    let data_chunk = 24u64
        .checked_add(payload_bytes)
        .and_then(|v| v.checked_add((8 - (v % 8)) % 8))
        .ok_or(WavExportError::FileTooLarge)?;
    let file_size = 40u64
        .checked_add(40)
        .and_then(|v| v.checked_add(data_chunk))
        .ok_or(WavExportError::FileTooLarge)?;
    let temp_path = path.with_extension(format!(
        "wave64.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| WavExportError::Io(error.to_string()))?
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| WavExportError::Io(error.to_string()))?;
        file.write_all(&RIFF_GUID).map_err(io_error)?;
        file.write_all(&file_size.to_le_bytes()).map_err(io_error)?;
        file.write_all(&WAVE_GUID).map_err(io_error)?;
        file.write_all(&FMT_GUID).map_err(io_error)?;
        file.write_all(&40u64.to_le_bytes()).map_err(io_error)?;
        file.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
        file.write_all(&sample_rate.to_le_bytes())
            .map_err(io_error)?;
        let byte_rate = sample_rate
            .checked_mul(channels as u32)
            .and_then(|v| v.checked_mul(4))
            .ok_or(WavExportError::FileTooLarge)?;
        file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(channels * 4).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
        file.write_all(&DATA_GUID).map_err(io_error)?;
        file.write_all(&data_chunk.to_le_bytes())
            .map_err(io_error)?;
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).map_err(io_error)?;
        }
        let padding = ((8 - (payload_bytes % 8)) % 8) as usize;
        if padding != 0 {
            file.write_all(&[0u8; 7][..padding]).map_err(io_error)?;
        }
        file.sync_all().map_err(io_error)?;
        fs::rename(&temp_path, path)
            .map_err(io_error)
            .and_then(|()| sync_parent_directory(path).map_err(io_error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// Reads the bounded WAVE64 float32 subset emitted by `write_wave64_float32`.
/// Unknown chunks are skipped using their 64-bit, 8-byte-aligned sizes.
pub fn read_wave64_float32(path: &Path) -> Result<(u32, u16, Vec<f32>), WavExportError> {
    const RIFF_GUID: [u8; 16] = [
        0x52, 0x49, 0x46, 0x46, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const WAVE_GUID: [u8; 16] = [
        0x57, 0x41, 0x56, 0x45, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const FMT_GUID: [u8; 16] = [
        0x66, 0x6d, 0x74, 0x20, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    const DATA_GUID: [u8; 16] = [
        0x64, 0x61, 0x74, 0x61, 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00,
        0x00,
    ];
    // Refuse pathological/sparse inputs before allocating a buffer for them.
    // The bounded reader is intended for files produced by Aura, not arbitrary
    // multi-gigabyte containers.
    const MAX_WAVE64_BYTES: u64 = 512 * 1024 * 1024;
    let file_size = fs::metadata(path)
        .map_err(|error| WavExportError::Io(error.to_string()))?
        .len();
    if file_size > MAX_WAVE64_BYTES {
        return Err(WavExportError::FileTooLarge);
    }
    let bytes = fs::read(path).map_err(|error| WavExportError::Io(error.to_string()))?;
    if bytes.len() < 40 || bytes[0..16] != RIFF_GUID || bytes[24..40] != WAVE_GUID {
        return Err(WavExportError::UnsupportedFormat);
    }
    let declared = u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| WavExportError::UnsupportedFormat)?,
    );
    if declared != bytes.len() as u64 {
        return Err(WavExportError::FileTooLarge);
    }
    let mut pos = 40usize;
    let mut format = None;
    let mut data = None;
    while pos < bytes.len() {
        if bytes.len() - pos < 24 {
            return Err(WavExportError::UnsupportedFormat);
        }
        let guid: [u8; 16] = bytes[pos..pos + 16]
            .try_into()
            .map_err(|_| WavExportError::UnsupportedFormat)?;
        let size = u64::from_le_bytes(
            bytes[pos + 16..pos + 24]
                .try_into()
                .map_err(|_| WavExportError::UnsupportedFormat)?,
        );
        if size < 24 || size > (bytes.len() - pos) as u64 {
            return Err(WavExportError::UnsupportedFormat);
        }
        let payload_len = usize::try_from(size - 24).map_err(|_| WavExportError::FileTooLarge)?;
        let payload_start = pos + 24;
        let payload_end = payload_start + payload_len;
        if guid == FMT_GUID {
            if payload_len < 16
                || u16::from_le_bytes([bytes[payload_start], bytes[payload_start + 1]]) != 3
            {
                return Err(WavExportError::UnsupportedFormat);
            }
            let channels = u16::from_le_bytes([bytes[payload_start + 2], bytes[payload_start + 3]]);
            let rate = u32::from_le_bytes(
                bytes[payload_start + 4..payload_start + 8]
                    .try_into()
                    .map_err(|_| WavExportError::UnsupportedFormat)?,
            );
            let bits = u16::from_le_bytes([bytes[payload_start + 14], bytes[payload_start + 15]]);
            if channels == 0 || channels > 32 || !(8_000..=384_000).contains(&rate) || bits != 32 {
                return Err(WavExportError::UnsupportedFormat);
            }
            format = Some((rate, channels));
        } else if guid == DATA_GUID {
            data = Some(bytes[payload_start..payload_end].to_vec());
        }
        let aligned = (size + 7) & !7;
        pos = pos
            .checked_add(usize::try_from(aligned).map_err(|_| WavExportError::FileTooLarge)?)
            .ok_or(WavExportError::FileTooLarge)?;
    }
    let (sample_rate, channels) = format.ok_or(WavExportError::UnsupportedFormat)?;
    let payload = data.ok_or(WavExportError::UnsupportedFormat)?;
    if payload.len() % 4 != 0 || !(payload.len() / 4).is_multiple_of(channels as usize) {
        return Err(WavExportError::IncompleteFrame);
    }
    let mut samples = Vec::with_capacity(payload.len() / 4);
    for chunk in payload.as_chunks::<4>().0 {
        let value = f32::from_le_bytes(*chunk);
        if !value.is_finite() {
            return Err(WavExportError::NonFiniteSample);
        }
        samples.push(value);
    }
    Ok((sample_rate, channels, samples))
}
