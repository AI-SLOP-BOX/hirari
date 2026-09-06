//! Bounded inspection of the native `ARUA` project container.
//!
//! The command-line protocol also accepts JSON project documents, but the
//! desktop application currently persists its native binary container.  Keep
//! this module read-only and deliberately small: hydration still belongs to
//! the native engine, while `project inspect` must not reject a real `.aura`
//! file with a JSON parser error.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::Read;
use std::path::Path;

const MAGIC: u32 = 0x4155_5241;
const MAX_PROJECT_BYTES: usize = 256 * 1024 * 1024;
const MIN_PROJECT_SAMPLE_RATE: u32 = 8_000;
const MAX_PROJECT_SAMPLE_RATE: u32 = 384_000;

#[derive(Debug, Clone, Serialize)]
pub struct NativeProjectInspection {
    pub format: &'static str,
    pub version: u32,
    pub sample_rate: u32,
    pub bpm: f64,
    pub root_note: Option<i32>,
    pub scale_type: Option<i32>,
    pub track_count: Option<u32>,
    pub region_count: Option<u32>,
    pub checksum_valid: bool,
    pub bytes: usize,
}

pub fn is_native_project(path: impl AsRef<Path>) -> Result<bool> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to read project {}", path.display()))?;
    let mut magic = [0u8; 4];
    let read = file
        .read(&mut magic)
        .context("failed to read project header")?;
    Ok(read == magic.len() && u32::from_le_bytes(magic) == MAGIC)
}

pub fn inspect(path: impl AsRef<Path>) -> Result<NativeProjectInspection> {
    let path = path.as_ref();
    let size = std::fs::metadata(path)
        .with_context(|| format!("failed to stat project {}", path.display()))?
        .len();
    if size > MAX_PROJECT_BYTES as u64 {
        bail!(
            "native project exceeds {} byte inspection limit",
            MAX_PROJECT_BYTES
        );
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read project {}", path.display()))?;
    if bytes.len() < 12 || read_u32(&bytes, 0) != Some(MAGIC) {
        bail!("not a native ARUA project container");
    }

    let version = read_u32(&bytes, 4).context("native project header is truncated")?;
    if !(1..=28).contains(&version) {
        bail!("unsupported native project version {version}");
    }
    let sample_rate = read_u32(&bytes, 8).context("native project sample rate is truncated")?;
    let bpm = read_f64(&bytes, 12).context("native project tempo is truncated")?;
    if !(MIN_PROJECT_SAMPLE_RATE..=MAX_PROJECT_SAMPLE_RATE).contains(&sample_rate)
        || !bpm.is_finite()
        || !(20.0..=300.0).contains(&bpm)
    {
        bail!("native project header contains invalid audio or tempo metadata");
    }

    let (root_note, scale_type, count_offset) = if version >= 14 {
        (
            Some(read_i32(&bytes, 20).context("native project root note is truncated")?),
            Some(read_i32(&bytes, 24).context("native project scale is truncated")?),
            28,
        )
    } else {
        (None, None, 20)
    };
    if root_note.is_some_and(|value| !(0..=11).contains(&value)) {
        bail!("native project root note is outside the supported range");
    }

    let (payload, checksum_valid) = if version >= 27 {
        if bytes.len() < 4 + 4 + 4 {
            bail!("native project checksum trailer is truncated");
        }
        let payload_len = bytes.len() - 4;
        let stored =
            read_u32(&bytes, payload_len).context("native project checksum is truncated")?;
        (
            &bytes[..payload_len],
            crc32(&bytes[..payload_len]) == stored,
        )
    } else {
        (&bytes[..], true)
    };
    if !checksum_valid {
        bail!("native project checksum mismatch");
    }

    // Counts are safe to expose from the fixed header without pretending that
    // this lightweight inspector has hydrated the full native graph.
    let (track_count, region_count) = parse_counts(payload, version, count_offset)
        .map_or((read_u32(payload, count_offset), None), |counts| {
            (Some(counts.0), Some(counts.1))
        });

    Ok(NativeProjectInspection {
        format: "aura-native-binary",
        version,
        sample_rate,
        bpm,
        root_note,
        scale_type,
        track_count,
        region_count,
        checksum_valid,
        bytes: bytes.len(),
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let raw: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    read_u32(bytes, offset).map(|value| i32::from_le_bytes(value.to_le_bytes()))
}

fn read_f64(bytes: &[u8], offset: usize) -> Option<f64> {
    let end = offset.checked_add(8)?;
    let raw: [u8; 8] = bytes.get(offset..end)?.try_into().ok()?;
    Some(f64::from_le_bytes(raw))
}

fn parse_counts(bytes: &[u8], version: u32, track_offset: usize) -> Option<(u32, u32)> {
    let mut cursor = Cursor {
        bytes,
        offset: track_offset,
    };
    let track_count = cursor.u32()?;
    if track_count > 512 {
        return None;
    }
    for _ in 0..track_count {
        cursor.skip(4)?; // track id
        if version >= 12 {
            cursor.skip(4)?; // track type
        }
        cursor.skip(5 * 4)?; // volume, pan, and 3D position
        if version >= 12 {
            cursor.skip(2)?; // mute, solo
            if version >= 15 {
                cursor.skip(1)?; // phase invert
            }
            if version >= 23 {
                cursor.skip(1)?; // record armed
            }
        }
        cursor.skip_blob()?; // plugin name
        cursor.skip_blob()?; // plugin data
        if version >= 24 {
            let state_count = cursor.u32()?;
            if state_count > 1_000_000 {
                return None;
            }
            for _ in 0..state_count {
                cursor.skip_blob()?;
            }
            if version >= 25 {
                let bypass_count = cursor.u32()?;
                cursor.skip(bypass_count as usize)?;
            }
        }
        if version >= 16 {
            let sandbox_count = cursor.u32()?;
            if sandbox_count > 65_535 {
                return None;
            }
            for _ in 0..sandbox_count {
                cursor.skip_blob()?;
            }
            if version >= 17 {
                let state_count = cursor.u32()?;
                if state_count > 1_000_000 {
                    return None;
                }
                for _ in 0..state_count {
                    cursor.skip_blob()?;
                }
            }
        }
        if version >= 13 {
            skip_automation(&mut cursor)?;
            skip_automation(&mut cursor)?;
        }
    }
    let region_count = cursor.u32()?;
    (region_count <= 4_000_000).then_some((track_count, region_count))
}

fn skip_automation(cursor: &mut Cursor<'_>) -> Option<()> {
    let count = cursor.u32()?;
    if count > 4_000_000 {
        return None;
    }
    cursor.skip((count as usize).checked_mul(16)?)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Cursor<'_> {
    fn skip(&mut self, amount: usize) -> Option<()> {
        self.offset = self.offset.checked_add(amount)?;
        (self.offset <= self.bytes.len()).then_some(())
    }

    fn u32(&mut self) -> Option<u32> {
        let value = read_u32(self.bytes, self.offset)?;
        self.offset = self.offset.checked_add(4)?;
        Some(value)
    }

    fn skip_blob(&mut self) -> Option<()> {
        let size = self.u32()? as usize;
        self.skip(size)
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn inspects_version_28_header_and_crc() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        let checksum = crc32(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        let path =
            std::env::temp_dir().join(format!("aura-native-inspect-{}.aura", std::process::id()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let result = inspect(&path).unwrap();
        assert_eq!(result.format, "aura-native-binary");
        assert_eq!(result.version, 28);
        assert_eq!(result.sample_rate, 48_000);
        assert_eq!(result.track_count, Some(3));
        assert!(result.checksum_valid);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_torn_native_container() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "aura-native-inspect-corrupt-{}.aura",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let error = inspect(&path).unwrap_err().to_string();
        assert!(error.contains("checksum") || error.contains("trailer"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_native_project_sample_rate_below_audio_floor() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC.to_le_bytes());
        bytes.extend_from_slice(&28u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&120.0f64.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0i32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let checksum = crc32(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "aura-native-inspect-invalid-rate-{}.aura",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(inspect(&path).is_err());
        let _ = std::fs::remove_file(path);
    }
}
