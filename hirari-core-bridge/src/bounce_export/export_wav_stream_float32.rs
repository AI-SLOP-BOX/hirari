//! Float32 WAV stream writer, split out of export_wav_stream.rs.
use super::*;

pub(crate) struct Float32WavStream {
    output: std::path::PathBuf,
    temporary: std::path::PathBuf,
    file: Option<File>,
    _publication_lock: Option<OutputPublicationLock>,
    sample_rate: u32,
    channels: u16,
    frames: u64,
    finished: bool,
    published: bool,
}

pub(crate) static WAV_STREAM_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(crate) const MAX_STREAM_BLOCK_FRAMES: u32 = 65_536;

impl Float32WavStream {
    pub(crate) fn create(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, WavExportError> {
        if path.as_os_str().is_empty() {
            return Err(WavExportError::InvalidPath);
        }
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(WavExportError::InvalidSampleRate);
        }
        if !(1..=32).contains(&channels) {
            return Err(WavExportError::InvalidChannelCount);
        }
        let publication_lock = OutputPublicationLock::acquire(path)?;
        let sequence = WAV_STREAM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = path.as_os_str().to_os_string();
        temporary_name.push(format!(
            ".tmp-hirari-f32-stream-{}-{sequence}",
            std::process::id()
        ));
        let temporary = std::path::PathBuf::from(temporary_name);
        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        if let Err(error) = write_float_stream_header(&mut file, sample_rate, channels, 0, false) {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(Self {
            output: path.to_owned(),
            temporary,
            file: Some(file),
            _publication_lock: Some(publication_lock),
            sample_rate,
            channels,
            frames: 0,
            finished: false,
            published: false,
        })
    }

    pub(crate) fn write_frames(&mut self, channel_ptrs: *const *const f32, count: u32) -> bool {
        if self.finished
            || self.file.is_none()
            || channel_ptrs.is_null()
            || count == 0
            || count > MAX_STREAM_BLOCK_FRAMES
        {
            return false;
        }
        if self.frames.checked_add(count as u64).is_none() {
            return false;
        }
        let ptrs = unsafe { std::slice::from_raw_parts(channel_ptrs, self.channels as usize) };
        if ptrs.iter().any(|ptr| ptr.is_null()) {
            return false;
        }
        let Some(sample_count) = (count as usize).checked_mul(self.channels as usize) else {
            return false;
        };
        let Some(byte_count) = sample_count.checked_mul(std::mem::size_of::<f32>()) else {
            return false;
        };
        let mut interleaved = Vec::with_capacity(byte_count);
        let planes: Vec<&[f32]> = ptrs
            .iter()
            .map(|ptr| unsafe { std::slice::from_raw_parts(*ptr, count as usize) })
            .collect();
        for frame in 0..count as usize {
            for plane in &planes {
                let sample = plane[frame];
                interleaved.extend_from_slice(
                    &(if sample.is_finite() { sample } else { 0.0 }).to_le_bytes(),
                );
            }
        }
        let Some(file) = self.file.as_mut() else {
            return false;
        };
        if file.write_all(&interleaved).is_err() {
            return false;
        }
        self.frames += count as u64;
        true
    }

    pub(crate) fn finish(&mut self) -> bool {
        if self.finished {
            return self.published;
        }
        let Some(data_size) = self
            .frames
            .checked_mul(self.channels as u64 * std::mem::size_of::<f32>() as u64)
        else {
            return self.fail();
        };
        let rf64 = data_size > u32::MAX as u64 - 72;
        let Some(riff_size) = data_size.checked_add(72) else {
            return self.fail();
        };
        let Some(file) = self.file.as_mut() else {
            return self.fail();
        };
        if file
            .seek(SeekFrom::Start(0))
            .map(|_| ())
            .map_err(io_error)
            .and_then(|()| {
                write_float_stream_header(file, self.sample_rate, self.channels, data_size, rf64)
            })
            .and_then(|()| {
                if rf64 && riff_size < data_size {
                    Err(WavExportError::FileTooLarge)
                } else {
                    file.sync_all().map_err(io_error)
                }
            })
            .is_err()
        {
            return self.fail();
        }
        self.file.take();
        if fs::rename(&self.temporary, &self.output)
            .and_then(|()| sync_parent_directory(&self.output))
            .is_err()
        {
            return self.fail();
        }
        self.finished = true;
        self.published = true;
        self._publication_lock.take();
        true
    }

    pub(crate) fn fail(&mut self) -> bool {
        self.file.take();
        let _ = fs::remove_file(&self.temporary);
        self.finished = true;
        self._publication_lock.take();
        false
    }
}

impl Drop for Float32WavStream {
    fn drop(&mut self) {
        if !self.finished {
            self.file.take();
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

pub(crate) fn write_float_stream_header(
    file: &mut File,
    sample_rate: u32,
    channels: u16,
    data_size: u64,
    rf64: bool,
) -> Result<(), WavExportError> {
    let bytes_per_frame = channels as u32 * 4;
    file.write_all(if rf64 { b"RF64" } else { b"RIFF" })
        .map_err(io_error)?;
    file.write_all(
        &(if rf64 {
            u32::MAX
        } else {
            (72 + data_size) as u32
        })
        .to_le_bytes(),
    )
    .map_err(io_error)?;
    file.write_all(b"WAVE").map_err(io_error)?;
    if rf64 {
        file.write_all(b"ds64").map_err(io_error)?;
        file.write_all(&28u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(72 + data_size).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&data_size.to_le_bytes()).map_err(io_error)?;
        file.write_all(&(data_size / bytes_per_frame as u64).to_le_bytes())
            .map_err(io_error)?;
        file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
    } else {
        file.write_all(b"JUNK").map_err(io_error)?;
        file.write_all(&28u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&[0; 28]).map_err(io_error)?;
    }
    file.write_all(b"fmt ").map_err(io_error)?;
    file.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
    file.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&(sample_rate * bytes_per_frame).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&(bytes_per_frame as u16).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(b"data").map_err(io_error)?;
    file.write_all(&(if rf64 { u32::MAX } else { data_size as u32 }).to_le_bytes())
        .map_err(io_error)?;
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_float32_stream_create(
    path: *const std::ffi::c_char,
    path_size: usize,
    sample_rate: u32,
    channels: u16,
) -> *mut std::ffi::c_void {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 {
        return std::ptr::null_mut();
    }
    let path_bytes = std::slice::from_raw_parts(path.cast::<u8>(), path_size);
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(path_bytes))
    };
    #[cfg(not(unix))]
    let path = {
        let Ok(path) = std::str::from_utf8(path_bytes) else {
            return std::ptr::null_mut();
        };
        Path::new(path)
    };
    match Float32WavStream::create(path, sample_rate, channels) {
        Ok(stream) => Box::into_raw(Box::new(stream)).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_float32_stream_write(
    stream: *mut std::ffi::c_void,
    channels: *const *const f32,
    frame_count: u32,
) -> u8 {
    stream
        .cast::<Float32WavStream>()
        .as_mut()
        .is_some_and(|stream| stream.write_frames(channels, frame_count)) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_float32_stream_finish(stream: *mut std::ffi::c_void) -> u8 {
    stream
        .cast::<Float32WavStream>()
        .as_mut()
        .is_some_and(Float32WavStream::finish) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_float32_stream_frames(stream: *const std::ffi::c_void) -> u64 {
    stream
        .cast::<Float32WavStream>()
        .as_ref()
        .map_or(0, |stream| stream.frames)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_float32_stream_free(stream: *mut std::ffi::c_void) {
    if !stream.is_null() {
        drop(Box::from_raw(stream.cast::<Float32WavStream>()));
    }
}
