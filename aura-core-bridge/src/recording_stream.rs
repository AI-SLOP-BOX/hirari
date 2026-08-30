//! Disk-backed recording spool for long captures.
//!
//! The audio callback must only enqueue validated blocks. This writer is
//! intended for the non-realtime consumer thread: it owns file I/O, PCM
//! conversion, header finalization, and atomic publication of the take.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum RecordingStreamError {
    InvalidSampleRate,
    InvalidChannelCount,
    InvalidBlock,
    NonFiniteSample,
    FileTooLarge,
    Io(String),
}

impl From<io::Error> for RecordingStreamError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub struct StreamingRecordingWriter {
    file: File,
    temp_path: PathBuf,
    final_path: PathBuf,
    sample_rate: u32,
    channels: u16,
    frames: u64,
    finalized: bool,
    pcm_scratch: Vec<u8>,
}

/// Finds crash-surviving recording spools in a directory. A candidate must
/// have a complete WAV header and at least one PCM frame; malformed partial
/// files are ignored rather than surfaced as recoverable audio.
pub fn recoverable_spools(directory: impl AsRef<Path>) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("aura-capture-") && name.ends_with(".wav.part")
                })
        })
        .filter(|path| {
            let Ok(bytes) = fs::read(path) else {
                return false;
            };
            recording_wav_metadata(&bytes).is_some_and(|(_, data_bytes)| data_bytes > 0)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
}

/// Returns the PCM channel count and data payload size for a RIFF/RF64 WAV.
/// Chunk order is intentionally not assumed: recovery files can be produced
/// by the streaming writer, the export path, or an interrupted external copy.
pub fn recording_wav_metadata(bytes: &[u8]) -> Option<(u16, u64)> {
    if bytes.len() < 12
        || (&bytes[0..4] != b"RIFF" && &bytes[0..4] != b"RF64")
        || &bytes[8..12] != b"WAVE"
    {
        return None;
    }
    let rf64 = &bytes[0..4] == b"RF64";
    let mut offset = 12usize;
    let mut channels = None;
    let mut block_align = None;
    let mut bits_per_sample = None;
    let mut data_bytes = None;
    let mut ds64_data_bytes = None;

    while offset.checked_add(8)? <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let chunk_size =
            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let payload_start = offset + 8;
        let logical_chunk_size = if rf64 && id == b"data" && chunk_size == u32::MAX as usize {
            usize::try_from(ds64_data_bytes?).ok()?
        } else {
            chunk_size
        };
        let payload_end = payload_start.checked_add(logical_chunk_size)?;
        if payload_end > bytes.len() {
            return None;
        }
        match id {
            b"ds64" if chunk_size >= 24 => {
                ds64_data_bytes = Some(u64::from_le_bytes(
                    bytes[payload_start + 8..payload_start + 16]
                        .try_into()
                        .ok()?,
                ));
            }
            b"fmt " if chunk_size >= 16 => {
                let format =
                    u16::from_le_bytes(bytes[payload_start..payload_start + 2].try_into().ok()?);
                let channel_count = u16::from_le_bytes(
                    bytes[payload_start + 2..payload_start + 4]
                        .try_into()
                        .ok()?,
                );
                let align = u16::from_le_bytes(
                    bytes[payload_start + 12..payload_start + 14]
                        .try_into()
                        .ok()?,
                );
                let bits = u16::from_le_bytes(
                    bytes[payload_start + 14..payload_start + 16]
                        .try_into()
                        .ok()?,
                );
                if format != 1 || channel_count == 0 || align == 0 || bits != 16 {
                    return None;
                }
                channels = Some(channel_count);
                block_align = Some(align);
                bits_per_sample = Some(bits);
            }
            b"data" => {
                data_bytes = Some(if rf64 && chunk_size == u32::MAX as usize {
                    ds64_data_bytes?
                } else {
                    chunk_size as u64
                });
            }
            _ => {}
        }
        let next_offset = payload_end.checked_add(logical_chunk_size & 1)?;
        // RIFF chunks with odd payload sizes carry one padding byte. Do not
        // accept a truncated file merely because the payload itself fits.
        if next_offset > bytes.len() {
            return None;
        }
        offset = next_offset;
    }

    let channels = channels?;
    let block_align = block_align?;
    let bits_per_sample = bits_per_sample?;
    let data_bytes = data_bytes?;
    if block_align != channels.checked_mul(bits_per_sample / 8)?
        || data_bytes == 0
        || data_bytes % u64::from(block_align) != 0
    {
        return None;
    }
    Some((channels, data_bytes))
}

impl StreamingRecordingWriter {
    pub fn create(
        path: impl AsRef<Path>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, RecordingStreamError> {
        if !(1..=384_000).contains(&sample_rate) {
            return Err(RecordingStreamError::InvalidSampleRate);
        }
        if !(1..=32).contains(&channels) {
            return Err(RecordingStreamError::InvalidChannelCount);
        }
        let final_path = path.as_ref().to_path_buf();
        if final_path.as_os_str().is_empty() || final_path.extension().is_none() {
            return Err(RecordingStreamError::Io("recording path is invalid".into()));
        }
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut temp_path = final_path.clone();
        let extension = temp_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("wav");
        temp_path.set_extension(format!("{extension}.part"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&temp_path)?;
        write_header(&mut file, sample_rate, channels, 0)?;
        Ok(Self {
            file,
            temp_path,
            final_path,
            sample_rate,
            channels,
            frames: 0,
            finalized: false,
            pcm_scratch: Vec::new(),
        })
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn append_interleaved(&mut self, samples: &[f32]) -> Result<(), RecordingStreamError> {
        if samples.is_empty() {
            return Ok(());
        }
        if !samples.len().is_multiple_of(self.channels as usize) {
            return Err(RecordingStreamError::InvalidBlock);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(RecordingStreamError::NonFiniteSample);
        }
        let frame_count = samples.len() / self.channels as usize;
        let next_frames = self
            .frames
            .checked_add(frame_count as u64)
            .ok_or(RecordingStreamError::FileTooLarge)?;
        let data_bytes = next_frames
            .checked_mul(self.channels as u64)
            .and_then(|value| value.checked_mul(2))
            .ok_or(RecordingStreamError::FileTooLarge)?;
        self.pcm_scratch.clear();
        self.pcm_scratch.reserve(samples.len() * 2);
        for sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            self.pcm_scratch.extend_from_slice(&value.to_le_bytes());
        }
        self.file.write_all(&self.pcm_scratch)?;
        self.frames = next_frames;
        // Keep the spool recoverable even if the process dies before
        // finalize. The reserved ds64/JUNK area lets the same header become
        // RF64 when the data crosses the RIFF 4 GiB limit.
        write_header(&mut self.file, self.sample_rate, self.channels, data_bytes)?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<PathBuf, RecordingStreamError> {
        // Never let a late recording worker overwrite a newer take that was
        // published to the same destination after cancellation/retry.
        if self.final_path.exists() {
            return Err(RecordingStreamError::Io(
                "recording destination already exists".into(),
            ));
        }
        let data_bytes = self
            .frames
            .checked_mul(self.channels as u64)
            .and_then(|value| value.checked_mul(2))
            .ok_or(RecordingStreamError::FileTooLarge)?;
        write_header(&mut self.file, self.sample_rate, self.channels, data_bytes)?;
        self.file.sync_all()?;
        publish_recording_without_replace(&self.temp_path, &self.final_path)?;
        sync_parent_directory(&self.final_path)?;
        self.finalized = true;
        Ok(self.final_path.clone())
    }
}

/// Publish a completed take without allowing a late recorder to replace a
/// newer take.  `rename` is atomic but replaces an existing destination on
/// Unix, so it is not sufficient after a check-then-publish race.  A hard
/// link creates the destination with no-replace semantics on the same file
/// system; removing the temporary name then completes the publication.
#[cfg(unix)]
fn publish_recording_without_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)?;
    fs::remove_file(source)
}

#[cfg(not(unix))]
fn publish_recording_without_replace(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "recording destination already exists",
        ));
    }
    fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

impl Drop for StreamingRecordingWriter {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

fn write_header(
    file: &mut File,
    sample_rate: u32,
    channels: u16,
    data_bytes: u64,
) -> Result<(), RecordingStreamError> {
    // Reserve a 28-byte JUNK/ds64 payload in both forms. This keeps the fmt
    // and data offsets stable while allowing a RIFF spool to be promoted to
    // RF64 at finalization.
    let riff_size = 72u64
        .checked_add(data_bytes)
        .ok_or(RecordingStreamError::FileTooLarge)?;
    let byte_rate = sample_rate
        .checked_mul(channels as u32)
        .and_then(|value| value.checked_mul(2))
        .ok_or(RecordingStreamError::FileTooLarge)?;
    let block_align = channels * 2;
    file.seek(SeekFrom::Start(0))?;
    let use_rf64 = riff_size > u32::MAX as u64;
    file.write_all(if use_rf64 { b"RF64" } else { b"RIFF" })?;
    file.write_all(&(if use_rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    // The combined write above is intentionally followed by a seek: the
    // extended metadata lives between WAVE and fmt.
    file.seek(SeekFrom::Start(12))?;
    file.write_all(if use_rf64 { b"ds64" } else { b"JUNK" })?;
    file.write_all(&28u32.to_le_bytes())?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(&data_bytes.to_le_bytes())?;
    let sample_count = data_bytes / u64::from(block_align);
    file.write_all(&sample_count.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.seek(SeekFrom::Start(48))?;
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(
        &(if use_rf64 {
            u32::MAX
        } else {
            data_bytes as u32
        })
        .to_le_bytes(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        recording_wav_metadata, recoverable_spools, write_header, RecordingStreamError,
        StreamingRecordingWriter,
    };
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aura-stream-{}-{}.wav",
            std::process::id(),
            TEST_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ))
    }

    fn unique_test_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aura-stream-{}-{}-{suffix}.wav",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn streams_blocks_and_publishes_a_valid_wav() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.0, 0.5, -0.5, 1.0]).unwrap();
        assert_eq!(writer.frames(), 2);
        let published = writer.finalize().unwrap();
        let bytes = std::fs::read(&published).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[72..76], b"data");
        assert_eq!(u32::from_le_bytes(bytes[76..80].try_into().unwrap()), 8);
        assert!(!path.with_extension("wav.part").exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn finalize_never_replaces_an_existing_take() {
        let path = unique_test_path("no-replace");
        std::fs::write(&path, b"newer take").unwrap();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 1).unwrap();
        writer.append_interleaved(&[0.25]).unwrap();
        let error = writer.finalize().unwrap_err();
        assert!(
            matches!(error, RecordingStreamError::Io(message) if message.contains("already exists"))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"newer take");
        assert!(!path.with_extension("wav.part").exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn metadata_parser_accepts_standard_riff_and_reserved_rf64_headers() {
        let riff_path = temp_path();
        let mut riff = StreamingRecordingWriter::create(&riff_path, 48_000, 2).unwrap();
        riff.append_interleaved(&[0.1, -0.1]).unwrap();
        let riff_path = riff.finalize().unwrap();
        let riff_bytes = std::fs::read(&riff_path).unwrap();
        assert_eq!(recording_wav_metadata(&riff_bytes), Some((2, 4)));
        let _ = std::fs::remove_file(riff_path);

        let rf64_path = unique_test_path("rf64");
        let mut rf64_file = std::fs::File::create(&rf64_path).unwrap();
        write_header(&mut rf64_file, 48_000, 2, 4).unwrap();
        rf64_file.write_all(&[0, 0, 0, 0]).unwrap();
        rf64_file.sync_all().unwrap();
        let mut rf64_bytes = std::fs::read(&rf64_path).unwrap();
        rf64_bytes[0..4].copy_from_slice(b"RF64");
        rf64_bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        rf64_bytes[12..16].copy_from_slice(b"ds64");
        rf64_bytes[20..28].copy_from_slice(&u64::from(80u32).to_le_bytes());
        rf64_bytes[28..36].copy_from_slice(&4u64.to_le_bytes());
        rf64_bytes[76..80].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&rf64_path, &rf64_bytes).unwrap();
        assert_eq!(recording_wav_metadata(&rf64_bytes), Some((2, 4)));
        let _ = std::fs::remove_file(rf64_path);
    }

    #[test]
    fn metadata_parser_rejects_truncation_and_header_mutations_without_panicking() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.1, -0.1, 0.2, -0.2]).unwrap();
        let published = writer.finalize().unwrap();
        let valid = std::fs::read(&published).unwrap();

        for end in 0..valid.len() {
            assert!(recording_wav_metadata(&valid[..end]).is_none());
        }
        for offset in [0usize, 4, 12, 20, 28, 36, 40, 76] {
            if offset < valid.len() {
                let mut mutated = valid.clone();
                mutated[offset] ^= 0xff;
                let _ = recording_wav_metadata(&mutated);
            }
        }
        let _ = std::fs::remove_file(published);
    }

    #[test]
    fn metadata_parser_handles_unknown_odd_chunks_and_rejects_missing_padding() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        writer.append_interleaved(&[0.1, -0.1]).unwrap();
        let published = writer.finalize().unwrap();
        let valid = std::fs::read(&published).unwrap();

        let mut with_unknown = valid[..12].to_vec();
        with_unknown.extend_from_slice(b"JUNK");
        with_unknown.extend_from_slice(&1u32.to_le_bytes());
        with_unknown.push(0x7f);
        with_unknown.push(0); // RIFF padding for the odd-sized payload.
        with_unknown.extend_from_slice(&valid[12..]);
        assert_eq!(recording_wav_metadata(&with_unknown), Some((2, 4)));

        let mut missing_padding = valid[..12].to_vec();
        missing_padding.extend_from_slice(b"JUNK");
        missing_padding.extend_from_slice(&1u32.to_le_bytes());
        missing_padding.push(0x7f);
        missing_padding.extend_from_slice(&valid[12..]);
        assert!(recording_wav_metadata(&missing_padding).is_none());

        let _ = std::fs::remove_file(published);
    }

    #[test]
    fn metadata_parser_fuzz_corpus_never_panics() {
        // Deterministic mutation corpus: this exercises truncation, random
        // chunk lengths, odd alignment, and bogus RF64 markers without
        // requiring an external fuzzing tool in every developer checkout.
        let mut state = 0x9e37_79b9_u32;
        for length in 0..4096usize {
            let mut bytes = vec![0u8; length];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let _ = recording_wav_metadata(&bytes);
        }
    }

    #[test]
    fn finalize_does_not_overwrite_an_existing_destination() {
        let path = temp_path();
        std::fs::write(&path, b"newer-take").unwrap();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 1).unwrap();
        writer.append_interleaved(&[0.25]).unwrap();
        assert!(matches!(
            writer.finalize(),
            Err(RecordingStreamError::Io(message)) if message.contains("already exists")
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"newer-take");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_bad_blocks_without_writing_partial_audio() {
        let path = temp_path();
        let mut writer = StreamingRecordingWriter::create(&path, 48_000, 2).unwrap();
        assert_eq!(
            writer.append_interleaved(&[0.0]),
            Err(RecordingStreamError::InvalidBlock)
        );
        assert_eq!(
            writer.append_interleaved(&[f32::NAN, 0.0]),
            Err(RecordingStreamError::NonFiniteSample)
        );
        assert_eq!(writer.frames(), 0);
        drop(writer);
        assert!(!path.with_extension("wav.part").exists());
    }

    #[test]
    fn crash_surviving_spools_are_detected_and_invalid_files_are_ignored() {
        let path = std::env::temp_dir().join(format!(
            "aura-capture-test-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let part = path.with_extension("wav.part");
        let _ = std::fs::remove_file(&part);
        let mut file = std::fs::File::create(&part).unwrap();
        write_header(&mut file, 48_000, 1, 2).unwrap();
        file.write_all(&[0x7f, 0x00]).unwrap();
        file.sync_all().unwrap();
        let invalid = part.with_file_name("aura-capture-invalid.wav.part");
        std::fs::write(&invalid, b"partial").unwrap();

        let candidates = recoverable_spools(std::env::temp_dir());
        assert!(candidates.iter().any(|candidate| candidate == &part));
        assert!(!candidates.iter().any(|candidate| candidate == &invalid));
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(invalid);
    }
}
