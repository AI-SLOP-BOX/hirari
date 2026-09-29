//! Read-only, memory-mapped WAVE source used by the realtime streaming path.
//!
//! The native engine only keeps a thin handle adapter. File ownership, RIFF/RF64
//! parsing, sample decoding, and invalidation checks live here.

use memmap2::{Mmap, MmapOptions};
use std::ffi::{c_void, CStr};
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct MappedAudioFile {
    file: File,
    mapping: Mmap,
    data_offset: usize,
    data_size: usize,
    format_tag: u16,
    channels: u16,
    bit_depth: u16,
    bytes_per_sample: usize,
    frame_stride: usize,
    sample_rate: u32,
    invalidated: AtomicBool,
}

impl MappedAudioFile {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let file_size = usize::try_from(file.metadata()?.len())
            .map_err(|_| invalid_data("WAVE file is too large"))?;
        if file_size < 12 {
            return Err(invalid_data("Invalid or empty WAVE file"));
        }
        // The mapping is read-only and the owning File is kept alive for the
        // entire lifetime of the view. Callers must not mutate the file in place.
        let mapping = unsafe { MmapOptions::new().map(&file)? };
        let bytes = &mapping[..];
        let rf64 = read_u32(bytes, 0)? == u32::from_le_bytes(*b"RF64");
        if (!rf64 && read_u32(bytes, 0)? != u32::from_le_bytes(*b"RIFF"))
            || read_u32(bytes, 8)? != u32::from_le_bytes(*b"WAVE")
        {
            return Err(invalid_data("Not a RIFF/WAVE file"));
        }

        let mut rf64_data_size = 0_u64;
        let mut has_ds64 = false;
        let mut offset = 12_usize;
        let mut data_offset = 44_usize;
        let mut data_size = 0_usize;
        let mut format_tag = 1_u16;
        let mut channels = 2_u16;
        let mut sample_rate = 44_100_u32;
        let mut bit_depth = 16_u16;

        while offset <= bytes.len() && bytes.len() - offset >= 8 {
            let chunk_id = read_u32(bytes, offset)?;
            let declared_size = read_u32(bytes, offset + 4)?;
            let mut payload = declared_size as usize;
            if rf64 && declared_size == u32::MAX {
                if !has_ds64
                    || rf64_data_size > (bytes.len() - offset - 8) as u64
                    || rf64_data_size > usize::MAX as u64
                {
                    return Err(invalid_data("Invalid RF64 ds64 data size"));
                }
                payload = rf64_data_size as usize;
            }
            if payload > bytes.len() - offset - 8 {
                return Err(invalid_data("Truncated RIFF chunk"));
            }
            let chunk_data = offset + 8;

            if chunk_id == u32::from_le_bytes(*b"ds64") {
                if !rf64 || payload < 28 {
                    return Err(invalid_data("Invalid RF64 ds64 chunk"));
                }
                let _riff_size = read_u64(bytes, chunk_data)?;
                rf64_data_size = read_u64(bytes, chunk_data + 8)?;
                has_ds64 = true;
            } else if chunk_id == u32::from_le_bytes(*b"fmt ") {
                if payload < 16 {
                    return Err(invalid_data("Invalid fmt chunk"));
                }
                format_tag = read_u16(bytes, chunk_data)?;
                channels = read_u16(bytes, chunk_data + 2)?;
                sample_rate = read_u32(bytes, chunk_data + 4)?;
                let byte_rate = read_u32(bytes, chunk_data + 8)?;
                let block_align = read_u16(bytes, chunk_data + 12)? as u64;
                bit_depth = read_u16(bytes, chunk_data + 14)?;
                let expected_block_align = channels as u64 * (bit_depth / 8) as u64;
                let expected_byte_rate = sample_rate as u64 * expected_block_align;
                if channels == 0
                    || channels > 32
                    || sample_rate == 0
                    || sample_rate > 384_000
                    || !matches!(format_tag, 1 | 3)
                    || !matches!(bit_depth, 16 | 24 | 32)
                    || expected_block_align > u16::MAX as u64
                    || block_align != expected_block_align
                    || expected_byte_rate > u32::MAX as u64
                    || byte_rate as u64 != expected_byte_rate
                {
                    return Err(invalid_data("Unsupported WAVE format"));
                }
            } else if chunk_id == u32::from_le_bytes(*b"data") {
                data_offset = chunk_data;
                data_size = if rf64 && declared_size == u32::MAX {
                    usize::try_from(rf64_data_size)
                        .map_err(|_| invalid_data("RF64 data size is too large"))?
                } else {
                    payload
                };
            }

            let aligned = payload
                .checked_add(payload & 1)
                .ok_or_else(|| invalid_data("RIFF chunk alignment overflows"))?;
            if aligned > bytes.len() - offset - 8 {
                return Err(invalid_data("Invalid RIFF chunk alignment"));
            }
            offset += 8 + aligned;
        }

        if data_size == 0 || channels == 0 || bit_depth == 0 {
            return Err(invalid_data("WAVE data chunk is missing or empty"));
        }
        let bytes_per_sample = (bit_depth / 8) as usize;
        let frame_stride = bytes_per_sample * channels as usize;
        if frame_stride == 0 || data_size % frame_stride != 0 {
            return Err(invalid_data("WAVE data is not frame aligned"));
        }
        if data_offset > bytes.len() || data_size > bytes.len() - data_offset {
            return Err(invalid_data("WAVE data is outside the mapped file"));
        }

        Ok(Self {
            file,
            mapping,
            data_offset,
            data_size,
            format_tag,
            channels,
            bit_depth,
            bytes_per_sample,
            frame_stride,
            sample_rate,
            invalidated: AtomicBool::new(false),
        })
    }

    pub(crate) fn sample(&self, channel: u32, frame: u64) -> f32 {
        if self.invalidated.load(Ordering::Acquire)
            || channel >= self.channels as u32
            || self.frame_stride == 0
            || frame >= self.frame_count()
        {
            return 0.0;
        }
        let Some(offset) = usize::try_from(frame)
            .ok()
            .and_then(|frame| frame.checked_mul(self.frame_stride))
            .and_then(|base| base.checked_add(self.data_offset))
            .and_then(|base| base.checked_add(channel as usize * self.bytes_per_sample))
        else {
            return 0.0;
        };
        let Some(sample_bytes) = self.mapping.get(offset..offset + self.bytes_per_sample) else {
            return 0.0;
        };
        let value = match (self.format_tag, self.bit_depth) {
            (3, 32) => f32::from_le_bytes(sample_bytes.try_into().unwrap_or([0; 4])),
            (1, 16) => {
                i16::from_le_bytes(sample_bytes.try_into().unwrap_or([0; 2])) as f32
                    * (1.0 / 32768.0)
            }
            (1, 24) => {
                let raw = sample_bytes[0] as i32
                    | ((sample_bytes[1] as i32) << 8)
                    | ((sample_bytes[2] as i32) << 16);
                let signed = if raw & 0x80_0000 != 0 {
                    raw | !0xFF_FFFF
                } else {
                    raw
                };
                signed as f32 * (1.0 / 8_388_608.0)
            }
            (1, 32) => {
                i32::from_le_bytes(sample_bytes.try_into().unwrap_or([0; 4])) as f32
                    * (1.0 / 2_147_483_648.0)
            }
            _ => 0.0,
        };
        if value.is_finite() {
            value
        } else {
            0.0
        }
    }

    pub(crate) fn frame_count(&self) -> u64 {
        (self.data_size / self.frame_stride) as u64
    }

    fn refresh(&self) -> bool {
        if self.invalidated.load(Ordering::Acquire) {
            return false;
        }
        let valid = self.file.metadata().is_ok_and(|metadata| {
            metadata.len() == self.mapping.len() as u64
                && self.data_offset <= self.mapping.len()
                && self.data_size <= self.mapping.len() - self.data_offset
        });
        if !valid {
            self.invalidated.store(true, Ordering::Release);
        }
        valid
    }
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_u16(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let slice = bytes
        .get(
            offset
                ..offset
                    .checked_add(2)
                    .ok_or_else(|| invalid_data("offset overflow"))?,
        )
        .ok_or_else(|| invalid_data("truncated WAVE field"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let slice = bytes
        .get(
            offset
                ..offset
                    .checked_add(4)
                    .ok_or_else(|| invalid_data("offset overflow"))?,
        )
        .ok_or_else(|| invalid_data("truncated WAVE field"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four byte slice"),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> io::Result<u64> {
    let slice = bytes
        .get(
            offset
                ..offset
                    .checked_add(8)
                    .ok_or_else(|| invalid_data("offset overflow"))?,
        )
        .ok_or_else(|| invalid_data("truncated WAVE field"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight byte slice"),
    ))
}

pub(crate) unsafe fn audio_file<'a>(handle: *const c_void) -> Option<&'a MappedAudioFile> {
    handle.cast::<MappedAudioFile>().as_ref()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_open(
    path: *const std::ffi::c_char,
) -> *mut c_void {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let path = CStr::from_ptr(path);
    let Ok(path) = path.to_str() else {
        return std::ptr::null_mut();
    };
    MappedAudioFile::open(Path::new(path))
        .map(Box::new)
        .map(Box::into_raw)
        .map_or(std::ptr::null_mut(), |handle| handle.cast())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        drop(Box::from_raw(handle.cast::<MappedAudioFile>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_is_valid(handle: *const c_void) -> bool {
    audio_file(handle).is_some_and(MappedAudioFile::refresh)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_sample(
    handle: *const c_void,
    channel: u32,
    frame: u64,
) -> f32 {
    audio_file(handle).map_or(0.0, |file| file.sample(channel, frame))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_frames(handle: *const c_void) -> u64 {
    audio_file(handle).map_or(0, MappedAudioFile::frame_count)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_channels(handle: *const c_void) -> u32 {
    audio_file(handle).map_or(0, |file| file.channels as u32)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_sample_rate(handle: *const c_void) -> u32 {
    audio_file(handle).map_or(0, |file| file.sample_rate)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_size(handle: *const c_void) -> usize {
    audio_file(handle).map_or(0, |file| file.mapping.len())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_data(handle: *const c_void) -> *const c_void {
    let Some(file) = audio_file(handle) else {
        return std::ptr::null();
    };
    if file.invalidated.load(Ordering::Acquire) {
        return std::ptr::null();
    }
    file.mapping
        .get(file.data_offset..)
        .map_or(std::ptr::null(), |data| data.as_ptr().cast())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mapped_audio_file_refresh(handle: *const c_void) -> bool {
    audio_file(handle).is_some_and(MappedAudioFile::refresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hirari-mmap-{label}-{}-{nonce}.wav",
            std::process::id()
        ))
    }

    fn write_pcm16(path: &Path, rate: u32, samples: &[i16]) {
        let data_size = (samples.len() * 2) as u32;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&rate.to_le_bytes()).unwrap();
        file.write_all(&(rate * 2).to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    fn write_wave(path: &Path, format: u16, bits: u16, samples: &[u8]) {
        let bytes_per_sample = (bits / 8) as u16;
        let data_size = samples.len() as u32;
        let mut file = File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&format.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&48_000_u32.to_le_bytes()).unwrap();
        file.write_all(&(48_000_u32 * bytes_per_sample as u32).to_le_bytes())
            .unwrap();
        file.write_all(&bytes_per_sample.to_le_bytes()).unwrap();
        file.write_all(&bits.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        file.write_all(samples).unwrap();
    }

    #[test]
    fn maps_and_decodes_pcm_wave_and_rejects_out_of_range_samples() {
        let path = fixture_path("pcm16");
        write_pcm16(&path, 48_000, &[16_384, -16_384]);
        let file = MappedAudioFile::open(&path).unwrap();
        assert!(file.refresh());
        assert_eq!(file.channels, 1);
        assert_eq!(file.sample_rate, 48_000);
        assert_eq!(file.frame_count(), 2);
        assert!((file.sample(0, 0) - 0.5).abs() < f32::EPSILON);
        assert!((file.sample(0, 1) + 0.5).abs() < f32::EPSILON);
        assert_eq!(file.sample(1, 0), 0.0);
        assert_eq!(file.sample(0, 2), 0.0);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalidates_a_mapping_after_external_truncation_before_reading_again() {
        let path = fixture_path("truncate");
        write_pcm16(&path, 48_000, &[16_384, -16_384]);
        let file = MappedAudioFile::open(&path).unwrap();
        assert!(file.refresh());
        std::fs::write(&path, b"short").unwrap();
        assert!(!file.refresh());
        assert_eq!(file.sample(0, 0), 0.0);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_invalid_rate_and_non_wave_files() {
        let invalid_rate = fixture_path("rate");
        write_pcm16(&invalid_rate, 500_000, &[0]);
        assert!(MappedAudioFile::open(&invalid_rate).is_err());
        std::fs::remove_file(invalid_rate).unwrap();

        let invalid = fixture_path("invalid");
        std::fs::write(&invalid, b"not a WAVE file").unwrap();
        assert!(MappedAudioFile::open(&invalid).is_err());
        std::fs::remove_file(invalid).unwrap();
    }

    #[test]
    fn decodes_pcm24_pcm32_and_float32_with_non_finite_sanitizing() {
        let pcm24 = fixture_path("pcm24");
        write_wave(&pcm24, 1, 24, &[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0]);
        let file = MappedAudioFile::open(&pcm24).unwrap();
        assert!((file.sample(0, 0) - 0.5).abs() < f32::EPSILON);
        assert!((file.sample(0, 1) + 0.5).abs() < f32::EPSILON);
        drop(file);
        std::fs::remove_file(pcm24).unwrap();

        let pcm32 = fixture_path("pcm32");
        write_wave(
            &pcm32,
            1,
            32,
            &[0x00, 0x00, 0x00, 0x80, 0xFF, 0xFF, 0xFF, 0x7F],
        );
        let file = MappedAudioFile::open(&pcm32).unwrap();
        assert_eq!(file.sample(0, 0), -1.0);
        assert!(file.sample(0, 1) > 0.999_999);
        drop(file);
        std::fs::remove_file(pcm32).unwrap();

        let float = fixture_path("float32");
        let mut samples = Vec::new();
        samples.extend_from_slice(&0.25_f32.to_le_bytes());
        samples.extend_from_slice(&f32::NAN.to_le_bytes());
        write_wave(&float, 3, 32, &samples);
        let file = MappedAudioFile::open(&float).unwrap();
        assert_eq!(file.sample(0, 0), 0.25);
        assert_eq!(file.sample(0, 1), 0.0);
        drop(file);
        std::fs::remove_file(float).unwrap();
    }

    #[test]
    fn reads_rf64_using_ds64_data_size() {
        let path = fixture_path("rf64");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RF64");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"ds64");
        bytes.extend_from_slice(&28_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&2_u64.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&44_100_u32.to_le_bytes());
        bytes.extend_from_slice(&88_200_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let file = MappedAudioFile::open(&path).unwrap();
        assert_eq!(file.sample_rate, 44_100);
        assert_eq!(file.frame_count(), 1);
        assert_eq!(file.sample(0, 0), 0.0);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
