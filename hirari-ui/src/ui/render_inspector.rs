use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RENDER_PATH: AtomicU64 = AtomicU64::new(1);

pub(crate) fn default_render_output_path() -> PathBuf {
    let sequence = NEXT_RENDER_PATH.fetch_add(1, Ordering::Relaxed);
    let timestamp = unix_time_millis();
    std::env::temp_dir().join(format!(
        "hirari_master-{}-{timestamp}-{sequence}.wav",
        std::process::id()
    ))
}

#[cfg(test)]
pub(crate) fn inspect_rendered_wav(path: &Path) -> String {
    inspect_rendered_wav_result(path).unwrap_or_else(|error| error)
}

pub(crate) fn inspect_rendered_wav_result(path: &Path) -> Result<String, String> {
    let Ok(mut file) = File::open(path) else {
        return Err(
            "出力ファイルを読み込めません。保存先の権限と空き容量を確認してください".to_string(),
        );
    };
    let Ok(file_len) = file.metadata().map(|metadata| metadata.len()) else {
        return Err("出力ファイルの情報を読み込めません".to_string());
    };
    if file_len < 12 {
        return Err(
            "出力ファイルは有効なWAVではありません。別の形式または保存先を確認してください"
                .to_string(),
        );
    }
    let mut header = [0u8; 12];
    if file.read_exact(&mut header).is_err()
        || !matches!(&header[0..4], b"RIFF" | b"RF64")
        || &header[8..12] != b"WAVE"
    {
        return Err(
            "出力ファイルは有効なWAVではありません。別の形式または保存先を確認してください"
                .to_string(),
        );
    }
    let mut cursor = 12u64;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut format = 0u16;
    let mut data_start = None;
    let mut data_len = 0u64;
    let mut rf64_data_len = None::<u64>;
    while cursor.checked_add(8).is_some_and(|end| end <= file_len) {
        if file.seek(SeekFrom::Start(cursor)).is_err() {
            return Err("WAV chunk cannot be read".to_string());
        }
        let mut chunk_header = [0u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            return Err("WAV chunk header is truncated".to_string());
        }
        let id = &chunk_header[0..4];
        let declared_len = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap());
        let len = if id == b"data" && declared_len == u32::MAX {
            let Some(length) = rf64_data_len else {
                return Err("RF64 data size is missing".to_string());
            };
            length
        } else {
            u64::from(declared_len)
        };
        let start = cursor + 8;
        let Some(end) = start.checked_add(len) else {
            return Err("WAV chunk is too large".to_string());
        };
        if end > file_len {
            return Err("WAV chunk is truncated".to_string());
        }
        if id == b"ds64" && len >= 16 {
            if file.seek(SeekFrom::Start(start + 8)).is_err() {
                return Err("RF64 size metadata is truncated".to_string());
            }
            let mut data_size = [0u8; 8];
            if file.read_exact(&mut data_size).is_err() {
                return Err("RF64 size metadata is truncated".to_string());
            }
            rf64_data_len = Some(u64::from_le_bytes(data_size));
        }
        if id == b"fmt " && end >= start + 16 {
            let mut fmt = [0u8; 16];
            if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut fmt).is_err() {
                return Err("WAV format metadata is truncated".to_string());
            }
            format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
        } else if id == b"data" {
            data_start = Some(start);
            data_len = len;
        }
        let Some(next) = end.checked_add(len & 1) else {
            return Err("WAV chunk alignment overflow".to_string());
        };
        if next > file_len {
            return Err("WAV chunk padding is truncated".to_string());
        }
        cursor = next;
    }
    let Some(start) = data_start else {
        return Err("WAV has no audio data".to_string());
    };
    if !matches!(format, 1 | 3)
        || channels == 0
        || sample_rate == 0
        || !matches!(bits, 16 | 24 | 32)
    {
        return Ok(format!(
            "{} Hz · {} ch · {} bit",
            sample_rate, channels, bits
        ));
    }
    let bytes_per_sample = (bits / 8) as usize;
    let Some(frame_bytes) = bytes_per_sample.checked_mul(channels as usize) else {
        return Err("WAV frame size is invalid".to_string());
    };
    if frame_bytes == 0 || data_len % frame_bytes as u64 != 0 {
        return Err("WAV audio data is not aligned to complete frames".to_string());
    }
    let frames = data_len / frame_bytes as u64;
    let mut peak = 0.0f32;
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = data_len;
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Err("WAV audio data cannot be read".to_string());
    }
    while remaining > 0 {
        let amount = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let amount = amount - amount % bytes_per_sample;
        if amount == 0 || file.read_exact(&mut buffer[..amount]).is_err() {
            return Err("WAV audio data is truncated".to_string());
        }
        for sample in buffer[..amount].chunks_exact(bytes_per_sample) {
            let value = match bits {
                16 => i16::from_le_bytes(sample.try_into().unwrap()) as f32 / 32768.0,
                24 => {
                    let raw =
                        (sample[0] as i32) | ((sample[1] as i32) << 8) | ((sample[2] as i32) << 16);
                    let signed = if raw & 0x0080_0000 != 0 {
                        raw | !0x00ff_ffff
                    } else {
                        raw
                    };
                    signed as f32 / 8_388_608.0
                }
                32 if format == 3 => f32::from_le_bytes(sample.try_into().unwrap()),
                32 => i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0,
                _ => 0.0,
            };
            peak = peak.max(value.abs());
        }
        remaining -= amount as u64;
    }
    let peak_db = if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -100.0
    };
    Ok(format!(
        "{} Hz · {} ch · {}{} · {} frames · {:.1} dBFS · {} bytes",
        sample_rate,
        channels,
        bits,
        if format == 3 { " bit float" } else { " bit" },
        frames,
        peak_db,
        file_len
    ))
}

pub(crate) fn unix_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}
