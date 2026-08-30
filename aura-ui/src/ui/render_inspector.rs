use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn default_render_output_path() -> PathBuf {
    std::env::temp_dir().join("aura_master.wav")
}

pub(crate) fn inspect_rendered_wav(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return "出力ファイルを読み込めません。保存先の権限と空き容量を確認してください"
            .to_string();
    };
    if bytes.len() < 12 || !matches!(&bytes[0..4], b"RIFF" | b"RF64") || &bytes[8..12] != b"WAVE" {
        return "出力ファイルは有効なWAVではありません。別の形式または保存先を確認してください"
            .to_string();
    }
    let mut cursor = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut format = 0u16;
    let mut data_start = None;
    let mut data_len = 0usize;
    let mut rf64_data_len = None;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let declared_len = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
        let len = if id == b"data" && declared_len == u32::MAX {
            let Some(length) = rf64_data_len else {
                return "RF64 data size is missing".to_string();
            };
            length
        } else {
            declared_len as usize
        };
        let start = cursor + 8;
        let Some(end) = start.checked_add(len) else {
            return "WAV chunk is too large".to_string();
        };
        if end > bytes.len() {
            return "WAV chunk is truncated".to_string();
        }
        if id == b"ds64" && len >= 16 {
            rf64_data_len = usize::try_from(u64::from_le_bytes(
                bytes[start + 8..start + 16].try_into().unwrap(),
            ))
            .ok();
        }
        if id == b"fmt " && end >= start + 16 {
            format = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
            channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
            bits = u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap());
        } else if id == b"data" {
            data_start = Some(start);
            data_len = end.saturating_sub(start);
        }
        let Some(next) = end.checked_add(len & 1) else {
            return "WAV chunk alignment overflow".to_string();
        };
        if next > bytes.len() {
            return "WAV chunk padding is truncated".to_string();
        }
        cursor = next;
    }
    let Some(start) = data_start else {
        return "WAV has no audio data".to_string();
    };
    if !matches!(format, 1 | 3)
        || channels == 0
        || sample_rate == 0
        || !matches!(bits, 16 | 24 | 32)
    {
        return format!("{} Hz · {} ch · {} bit", sample_rate, channels, bits);
    }
    let bytes_per_sample = (bits / 8) as usize;
    let frame_bytes = bytes_per_sample * channels as usize;
    let frames = data_len / frame_bytes.max(1);
    let mut peak = 0.0f32;
    let payload_end = (start + data_len).min(bytes.len());
    let mut offset = start;
    while offset + bytes_per_sample <= payload_end {
        let value = match bits {
            16 => {
                i16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as f32 / 32768.0
            }
            24 => {
                let raw = (bytes[offset] as i32)
                    | ((bytes[offset + 1] as i32) << 8)
                    | ((bytes[offset + 2] as i32) << 16);
                let signed = if raw & 0x0080_0000 != 0 {
                    raw | !0x00ff_ffff
                } else {
                    raw
                };
                signed as f32 / 8_388_608.0
            }
            32 if format == 3 => f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            32 => {
                i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f32
                    / 2_147_483_648.0
            }
            _ => 0.0,
        };
        peak = peak.max(value.abs());
        offset += bytes_per_sample;
    }
    let peak_db = if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -100.0
    };
    format!(
        "{} Hz · {} ch · {}{} · {} frames · {:.1} dBFS · {} bytes",
        sample_rate,
        channels,
        bits,
        if format == 3 { " bit float" } else { " bit" },
        frames,
        peak_db,
        bytes.len()
    )
}

pub(crate) fn unix_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}
