use std::io::{Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};

struct Float32WavStream {
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

static WAV_STREAM_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_STREAM_BLOCK_FRAMES: u32 = 65_536;

impl Float32WavStream {
    fn create(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, WavExportError> {
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

    fn write_frames(&mut self, channel_ptrs: *const *const f32, count: u32) -> bool {
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

    fn finish(&mut self) -> bool {
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

    fn fail(&mut self) -> bool {
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

fn write_float_stream_header(
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

struct PcmWavStream {
    output: std::path::PathBuf,
    temporary: std::path::PathBuf,
    file: Option<File>,
    _publication_lock: Option<OutputPublicationLock>,
    frames: u64,
    frames_written: u64,
    channels: u16,
    bit_depth: u16,
    padding_bytes: u8,
    finished: bool,
    published: bool,
}

impl PcmWavStream {
    fn create(
        path: &Path,
        frames: u64,
        sample_rate: u32,
        channels: u16,
        bit_depth: u16,
        broadcast_wave: bool,
    ) -> Result<Self, WavExportError> {
        if path.as_os_str().is_empty() {
            return Err(WavExportError::InvalidPath);
        }
        if frames == 0 {
            return Err(WavExportError::EmptyBuffer);
        }
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(WavExportError::InvalidSampleRate);
        }
        if !(1..=32).contains(&channels) || !matches!(bit_depth, 16 | 24) {
            return Err(WavExportError::UnsupportedFormat);
        }
        let bytes_per_frame = (bit_depth / 8) as u64 * channels as u64;
        let data_size = frames
            .checked_mul(bytes_per_frame)
            .ok_or(WavExportError::FileTooLarge)?;
        let padding_bytes = u8::from(bit_depth == 24 && data_size % 2 != 0);
        let riff_payload_size = data_size
            .checked_add(padding_bytes as u64)
            .ok_or(WavExportError::FileTooLarge)?;
        let riff_overhead = if broadcast_wave { 646 } else { 36 };
        let rf64 = riff_payload_size > u32::MAX as u64 - riff_overhead;
        let riff_size = riff_payload_size
            .checked_add(if rf64 {
                if broadcast_wave {
                    682
                } else {
                    72
                }
            } else {
                riff_overhead
            })
            .ok_or(WavExportError::FileTooLarge)?;
        let publication_lock = OutputPublicationLock::acquire(path)?;
        let sequence = WAV_STREAM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = path.as_os_str().to_os_string();
        temporary_name.push(format!(
            ".tmp-hirari-pcm{}-stream-{}-{sequence}",
            bit_depth,
            std::process::id()
        ));
        let temporary = std::path::PathBuf::from(temporary_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        let header = write_pcm_stream_header(
            &mut file,
            frames,
            sample_rate,
            channels,
            bit_depth,
            broadcast_wave,
            data_size,
            riff_size,
            rf64,
        );
        if let Err(error) = header {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(Self {
            output: path.to_owned(),
            temporary,
            file: Some(file),
            _publication_lock: Some(publication_lock),
            frames,
            frames_written: 0,
            channels,
            bit_depth,
            padding_bytes,
            finished: false,
            published: false,
        })
    }

    fn write_frames(&mut self, channel_ptrs: *const *const f32, count: u32) -> bool {
        if self.finished
            || self.file.is_none()
            || channel_ptrs.is_null()
            || count == 0
            || count > MAX_STREAM_BLOCK_FRAMES
            || self.frames_written > self.frames
            || count as u64 > self.frames - self.frames_written
        {
            return false;
        }
        let ptrs = unsafe { std::slice::from_raw_parts(channel_ptrs, self.channels as usize) };
        if ptrs.iter().any(|ptr| ptr.is_null()) {
            return false;
        }
        let Some(sample_count) = (count as usize).checked_mul(self.channels as usize) else {
            return false;
        };
        let bytes_per_sample = (self.bit_depth / 8) as usize;
        let Some(byte_count) = sample_count.checked_mul(bytes_per_sample) else {
            return false;
        };
        let mut payload = Vec::with_capacity(byte_count);
        let planes: Vec<&[f32]> = ptrs
            .iter()
            .map(|ptr| unsafe { std::slice::from_raw_parts(*ptr, count as usize) })
            .collect();
        for index in 0..count as usize {
            for plane in &planes {
                let sample = plane[index];
                let sample = if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                match self.bit_depth {
                    16 => {
                        let value = (sample * 32_767.0).round_ties_even() as i16;
                        payload.extend_from_slice(&value.to_le_bytes());
                    }
                    24 => {
                        let value = (sample * 8_388_607.0).round_ties_even() as i32;
                        payload.extend_from_slice(&value.to_le_bytes()[..3]);
                    }
                    _ => return false,
                }
            }
        }
        if self
            .file
            .as_mut()
            .is_some_and(|file| file.write_all(&payload).is_ok())
        {
            self.frames_written += count as u64;
            true
        } else {
            false
        }
    }

    fn finish(&mut self) -> bool {
        if self.finished {
            return self.published;
        }
        if self.file.is_none() || self.frames_written != self.frames {
            return self.fail();
        }
        if self.padding_bytes != 0
            && self
                .file
                .as_mut()
                .map_or(true, |file| file.write_all(&[0]).is_err())
        {
            return self.fail();
        }
        let Some(file) = self.file.take() else {
            return self.fail();
        };
        if file.sync_all().is_err() {
            drop(file);
            return self.fail();
        }
        drop(file);
        if fs::rename(&self.temporary, &self.output).is_err()
            || sync_parent_directory(&self.output).is_err()
        {
            return self.fail();
        }
        self.finished = true;
        self.published = true;
        self._publication_lock.take();
        true
    }

    fn fail(&mut self) -> bool {
        self.file.take();
        let _ = fs::remove_file(&self.temporary);
        self.finished = true;
        self._publication_lock.take();
        false
    }
}

impl Drop for PcmWavStream {
    fn drop(&mut self) {
        if !self.finished {
            self.file.take();
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_pcm_stream_header(
    file: &mut File,
    frames: u64,
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
    broadcast_wave: bool,
    data_size: u64,
    riff_size: u64,
    rf64: bool,
) -> Result<(), WavExportError> {
    let bytes_per_frame = (bit_depth / 8) as u32 * channels as u32;
    file.write_all(if rf64 { b"RF64" } else { b"RIFF" })
        .map_err(io_error)?;
    file.write_all(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(b"WAVE").map_err(io_error)?;
    if broadcast_wave {
        file.write_all(b"bext").map_err(io_error)?;
        file.write_all(&602u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&[0; 602]).map_err(io_error)?;
    }
    if rf64 {
        file.write_all(b"ds64").map_err(io_error)?;
        file.write_all(&28u32.to_le_bytes()).map_err(io_error)?;
        file.write_all(&riff_size.to_le_bytes()).map_err(io_error)?;
        file.write_all(&data_size.to_le_bytes()).map_err(io_error)?;
        file.write_all(&frames.to_le_bytes()).map_err(io_error)?;
        file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
    }
    file.write_all(b"fmt ").map_err(io_error)?;
    file.write_all(&16u32.to_le_bytes()).map_err(io_error)?;
    file.write_all(&1u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&(sample_rate * bytes_per_frame).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&(bytes_per_frame as u16).to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&bit_depth.to_le_bytes()).map_err(io_error)?;
    file.write_all(b"data").map_err(io_error)?;
    file.write_all(&(if rf64 { u32::MAX } else { data_size as u32 }).to_le_bytes())
        .map_err(io_error)?;
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_pcm_stream_create(
    path: *const std::ffi::c_char,
    path_size: usize,
    frames: u64,
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
    broadcast_wave: u8,
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
    match PcmWavStream::create(
        path,
        frames,
        sample_rate,
        channels,
        bit_depth,
        broadcast_wave != 0,
    ) {
        Ok(stream) => Box::into_raw(Box::new(stream)).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_pcm_stream_write(
    stream: *mut std::ffi::c_void,
    channels: *const *const f32,
    frame_count: u32,
) -> u8 {
    stream
        .cast::<PcmWavStream>()
        .as_mut()
        .is_some_and(|stream| stream.write_frames(channels, frame_count)) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_pcm_stream_finish(stream: *mut std::ffi::c_void) -> u8 {
    stream
        .cast::<PcmWavStream>()
        .as_mut()
        .is_some_and(PcmWavStream::finish) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_pcm_stream_free(stream: *mut std::ffi::c_void) {
    if !stream.is_null() {
        drop(Box::from_raw(stream.cast::<PcmWavStream>()));
    }
}

struct Wave64FloatStream {
    output: std::path::PathBuf,
    temporary: std::path::PathBuf,
    file: Option<File>,
    _publication_lock: Option<OutputPublicationLock>,
    frames: u64,
    frames_written: u64,
    payload_bytes: u64,
    padded_payload_bytes: u64,
    channels: u16,
    finished: bool,
    published: bool,
}

impl Wave64FloatStream {
    fn create(
        path: &Path,
        frames: u64,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, WavExportError> {
        if path.as_os_str().is_empty() {
            return Err(WavExportError::InvalidPath);
        }
        if frames == 0 {
            return Err(WavExportError::EmptyBuffer);
        }
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(WavExportError::InvalidSampleRate);
        }
        if !(1..=32).contains(&channels) {
            return Err(WavExportError::InvalidChannelCount);
        }
        let bytes_per_frame = channels as u64 * 4;
        let payload_bytes = frames
            .checked_mul(bytes_per_frame)
            .ok_or(WavExportError::FileTooLarge)?;
        let padded_payload_bytes = payload_bytes
            .checked_add((8 - payload_bytes % 8) % 8)
            .ok_or(WavExportError::FileTooLarge)?;
        let file_size = 104u64
            .checked_add(padded_payload_bytes)
            .ok_or(WavExportError::FileTooLarge)?;
        let byte_rate = sample_rate
            .checked_mul(bytes_per_frame as u32)
            .ok_or(WavExportError::FileTooLarge)?;
        let publication_lock = OutputPublicationLock::acquire(path)?;
        let sequence = WAV_STREAM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = path.as_os_str().to_os_string();
        temporary_name.push(format!(
            ".tmp-hirari-wave64-stream-{}-{sequence}",
            std::process::id()
        ));
        let temporary = std::path::PathBuf::from(temporary_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        let header = write_wave64_stream_header(
            &mut file,
            file_size,
            sample_rate,
            byte_rate,
            channels,
            bytes_per_frame as u16,
            padded_payload_bytes,
        );
        if let Err(error) = header {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(Self {
            output: path.to_owned(),
            temporary,
            file: Some(file),
            _publication_lock: Some(publication_lock),
            frames,
            frames_written: 0,
            payload_bytes,
            padded_payload_bytes,
            channels,
            finished: false,
            published: false,
        })
    }

    fn write_frames(&mut self, channel_ptrs: *const *const f32, count: u32) -> bool {
        if self.finished
            || self.file.is_none()
            || channel_ptrs.is_null()
            || count == 0
            || count > MAX_STREAM_BLOCK_FRAMES
            || self.frames_written > self.frames
            || count as u64 > self.frames - self.frames_written
        {
            return false;
        }
        let ptrs = unsafe { std::slice::from_raw_parts(channel_ptrs, self.channels as usize) };
        if ptrs.iter().any(|ptr| ptr.is_null()) {
            return false;
        }
        let sample_count = count as usize * self.channels as usize;
        let mut payload = Vec::with_capacity(sample_count * 4);
        let planes: Vec<&[f32]> = ptrs
            .iter()
            .map(|ptr| unsafe { std::slice::from_raw_parts(*ptr, count as usize) })
            .collect();
        for frame in 0..count as usize {
            for plane in &planes {
                let sample = plane[frame];
                payload.extend_from_slice(
                    &(if sample.is_finite() { sample } else { 0.0 }).to_le_bytes(),
                );
            }
        }
        if self
            .file
            .as_mut()
            .is_some_and(|file| file.write_all(&payload).is_ok())
        {
            self.frames_written += count as u64;
            true
        } else {
            false
        }
    }

    fn finish(&mut self) -> bool {
        if self.finished {
            return self.published;
        }
        if self.file.is_none() || self.frames_written != self.frames {
            return self.fail();
        }
        let padding = (self.padded_payload_bytes - self.payload_bytes) as usize;
        if padding > 0 {
            if self
                .file
                .as_mut()
                .map_or(true, |file| file.write_all(&[0; 7][..padding]).is_err())
            {
                return self.fail();
            }
        }
        let Some(file) = self.file.take() else {
            return self.fail();
        };
        if file.sync_all().is_err() {
            drop(file);
            return self.fail();
        }
        drop(file);
        if fs::rename(&self.temporary, &self.output).is_err()
            || sync_parent_directory(&self.output).is_err()
        {
            return self.fail();
        }
        self.finished = true;
        self.published = true;
        self._publication_lock.take();
        true
    }

    fn fail(&mut self) -> bool {
        self.file.take();
        let _ = fs::remove_file(&self.temporary);
        self.finished = true;
        self._publication_lock.take();
        false
    }
}

impl Drop for Wave64FloatStream {
    fn drop(&mut self) {
        if !self.finished {
            self.file.take();
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

fn write_wave64_stream_header(
    file: &mut File,
    file_size: u64,
    sample_rate: u32,
    byte_rate: u32,
    channels: u16,
    block_align: u16,
    padded_payload_bytes: u64,
) -> Result<(), WavExportError> {
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
    file.write_all(&RIFF_GUID).map_err(io_error)?;
    file.write_all(&file_size.to_le_bytes()).map_err(io_error)?;
    file.write_all(&WAVE_GUID).map_err(io_error)?;
    file.write_all(&FMT_GUID).map_err(io_error)?;
    file.write_all(&40u64.to_le_bytes()).map_err(io_error)?;
    file.write_all(&3u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(&channels.to_le_bytes()).map_err(io_error)?;
    file.write_all(&sample_rate.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&byte_rate.to_le_bytes()).map_err(io_error)?;
    file.write_all(&block_align.to_le_bytes())
        .map_err(io_error)?;
    file.write_all(&32u16.to_le_bytes()).map_err(io_error)?;
    file.write_all(&DATA_GUID).map_err(io_error)?;
    file.write_all(&(24u64 + padded_payload_bytes).to_le_bytes())
        .map_err(io_error)?;
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_wave64_stream_create(
    path: *const std::ffi::c_char,
    path_size: usize,
    frames: u64,
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
    match Wave64FloatStream::create(path, frames, sample_rate, channels) {
        Ok(stream) => Box::into_raw(Box::new(stream)).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_wave64_stream_write(
    stream: *mut std::ffi::c_void,
    channels: *const *const f32,
    frame_count: u32,
) -> u8 {
    stream
        .cast::<Wave64FloatStream>()
        .as_mut()
        .is_some_and(|stream| stream.write_frames(channels, frame_count)) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_wave64_stream_finish(stream: *mut std::ffi::c_void) -> u8 {
    stream
        .cast::<Wave64FloatStream>()
        .as_mut()
        .is_some_and(Wave64FloatStream::finish) as u8
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_wave64_stream_frames(stream: *const std::ffi::c_void) -> u64 {
    stream
        .cast::<Wave64FloatStream>()
        .as_ref()
        .map_or(0, |stream| stream.frames_written)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_wav_wave64_stream_free(stream: *mut std::ffi::c_void) {
    if !stream.is_null() {
        drop(Box::from_raw(stream.cast::<Wave64FloatStream>()));
    }
}

/// Writes a complete planar buffer through the canonical bounded-memory
/// stream writers. Format values match the native compatibility facade:
/// 0=float32 RIFF/RF64, 1=PCM16, 2=PCM24, 3=WAVE64 float32.
#[no_mangle]
pub unsafe extern "C" fn hirari_wav_write_planar(
    path: *const u8,
    path_size: usize,
    channels: *const *const f32,
    channel_count: usize,
    frames: u64,
    sample_rate: u32,
    format: u8,
) -> u8 {
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024
        || channels.is_null() || channel_count == 0 || channel_count > 32
        || frames == 0 || frames > usize::MAX as u64
        || !(8_000..=384_000).contains(&sample_rate) || format > 3
    {
        return 1;
    }
    let channel_data = unsafe { std::slice::from_raw_parts(channels, channel_count) };
    if channel_data.iter().any(|channel| channel.is_null()) {
        return 2;
    }
    let path_bytes = unsafe { std::slice::from_raw_parts(path, path_size) };
    #[cfg(unix)]
    let output = {
        use std::os::unix::ffi::OsStrExt;
        Path::new(std::ffi::OsStr::from_bytes(path_bytes))
    };
    #[cfg(not(unix))]
    let output = {
        let Ok(path) = std::str::from_utf8(path_bytes) else { return 1 };
        Path::new(path)
    };
    let parent = output.parent().filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if std::fs::create_dir_all(parent).is_err() {
        return 3;
    }
    let path = path.cast::<std::ffi::c_char>();
    let mut stream = match format {
        0 => {
            let handle = unsafe { hirari_wav_float32_stream_create(
                path, path_size, sample_rate, channel_count as u16,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Float32(handle)
        }
        1 | 2 => {
            let handle = unsafe { hirari_wav_pcm_stream_create(
                path, path_size, frames, sample_rate, channel_count as u16,
                if format == 1 { 16 } else { 24 }, 0,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Pcm(handle)
        }
        _ => {
            let handle = unsafe { hirari_wav_wave64_stream_create(
                path, path_size, frames, sample_rate, channel_count as u16,
            ) };
            if handle.is_null() { return 4; }
            StreamHandle::Wave64(handle)
        }
    };
    let mut block = Vec::with_capacity(channel_count);
    let mut offset = 0u64;
    while offset < frames {
        let count = (frames - offset).min(u64::from(MAX_STREAM_BLOCK_FRAMES)) as u32;
        block.clear();
        for channel in channel_data {
            block.push(unsafe { channel.add(offset as usize) });
        }
        let written = unsafe { stream.write(block.as_ptr(), count) };
        if written == 0 { return 5; }
        offset += u64::from(count);
    }
    if unsafe { stream.finish() } == 0 { return 6; }
    0
}

/// Channel-layout admission for planar exports. Every plane must hold exactly
/// `frames` samples and the channel count must fit the WAVE field width, so a
/// caller never has to re-derive the rule before building its pointer array.
#[no_mangle]
pub unsafe extern "C" fn hirari_wav_planar_shapes_valid(
    sizes: *const usize,
    channel_count: usize,
    frames: u64,
) -> bool {
    if sizes.is_null() || channel_count == 0 || channel_count > u16::MAX as usize || frames == 0 {
        return false;
    }
    let sizes = unsafe { std::slice::from_raw_parts(sizes, channel_count) };
    sizes.iter().all(|size| *size as u64 == frames)
}

enum StreamHandle {
    Float32(*mut std::ffi::c_void),
    Pcm(*mut std::ffi::c_void),
    Wave64(*mut std::ffi::c_void),
}

struct BounceStream {
    stream: StreamHandle,
    frames: u64,
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_create(
    path: *const u8, path_size: usize, frames: u64, sample_rate: u32, format: u8,
) -> *mut std::ffi::c_void {
    // Format codes: 0 float32 RIFF/RF64, 1 PCM16, 2 PCM24, 3 WAVE64 float.
    if path.is_null() || path_size == 0 || path_size > 1024 * 1024 || frames == 0
        || !(8_000..=384_000).contains(&sample_rate) || format > 3
    { return std::ptr::null_mut(); }
    // Do not create missing parent directories here. The writer's contract is
    // to fail when the destination cannot be opened, and a render must report
    // that failure instead of silently materialising a path the caller never
    // created.
    let cpath = path.cast::<std::ffi::c_char>();
    let stream = match format {
        0 => unsafe { hirari_wav_float32_stream_create(cpath, path_size, sample_rate, 2) },
        1 | 2 => unsafe { hirari_wav_pcm_stream_create(
            cpath, path_size, frames, sample_rate, 2, if format == 1 { 16 } else { 24 }, 0,
        ) },
        _ => unsafe { hirari_wav_wave64_stream_create(cpath, path_size, frames, sample_rate, 2) },
    };
    if stream.is_null() { return std::ptr::null_mut(); }
    Box::into_raw(Box::new(BounceStream {
        stream: match format {
            0 => StreamHandle::Float32(stream),
            1 | 2 => StreamHandle::Pcm(stream),
            _ => StreamHandle::Wave64(stream),
        },
        frames,
    })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_write(
    stream: *mut std::ffi::c_void, channels: *const *const f32, frames: u32,
) -> u8 {
    let Some(stream) = (unsafe { stream.cast::<BounceStream>().as_mut() }) else { return 0 };
    if frames == 0 || u64::from(frames) > stream.frames { return 0; }
    let result = unsafe { stream.stream.write(channels, frames) };
    if result != 0 { stream.frames -= u64::from(frames); }
    result
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_finish(stream: *mut std::ffi::c_void) -> u8 {
    let Some(stream) = (unsafe { stream.cast::<BounceStream>().as_mut() }) else { return 0 };
    if stream.frames != 0 { return 0; }
    unsafe { stream.stream.finish() }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_bounce_stream_free(stream: *mut std::ffi::c_void) {
    if !stream.is_null() { drop(unsafe { Box::from_raw(stream.cast::<BounceStream>()) }); }
}

impl StreamHandle {
    unsafe fn write(&mut self, channels: *const *const f32, frames: u32) -> u8 {
        match self {
            Self::Float32(stream) => unsafe { hirari_wav_float32_stream_write(*stream, channels, frames) },
            Self::Pcm(stream) => unsafe { hirari_wav_pcm_stream_write(*stream, channels, frames) },
            Self::Wave64(stream) => unsafe { hirari_wav_wave64_stream_write(*stream, channels, frames) },
        }
    }

    unsafe fn finish(&mut self) -> u8 {
        match self {
            Self::Float32(stream) => unsafe { hirari_wav_float32_stream_finish(*stream) },
            Self::Pcm(stream) => unsafe { hirari_wav_pcm_stream_finish(*stream) },
            Self::Wave64(stream) => unsafe { hirari_wav_wave64_stream_finish(*stream) },
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        unsafe {
            match self {
                Self::Float32(stream) => hirari_wav_float32_stream_free(*stream),
                Self::Pcm(stream) => hirari_wav_pcm_stream_free(*stream),
                Self::Wave64(stream) => hirari_wav_wave64_stream_free(*stream),
            }
        }
    }
}

#[cfg(test)]
mod bounce_stream_tests {
    use super::*;

    fn temp_path(extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "hirari-bounce-stream-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    #[test]
    fn every_bounce_stream_format_requires_all_frames_before_publishing() {
        let left = [0.25_f32; 16];
        let right = [-0.5_f32; 16];
        let channels = [left.as_ptr(), right.as_ptr()];
        // format, extension, WAVE64 container.
        let cases = [
            (0_u8, "wav", false),
            (1, "wav", false),
            (2, "wav", false),
            (3, "w64", true),
        ];
        for (format, extension, wave64) in cases {
            let path = temp_path(extension);
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_bounce_stream_create(bytes.as_ptr(), bytes.len(), 16, 48_000, format)
            };
            assert!(!stream.is_null(), "format {format} could not be opened");
            assert_eq!(
                unsafe { hirari_bounce_stream_write(stream, channels.as_ptr(), 17) },
                0,
                "format {format} accepted more frames than declared"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_finish(stream) },
                0,
                "format {format} published an incomplete stream"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_write(stream, channels.as_ptr(), 16) },
                1,
                "format {format} rejected a valid block"
            );
            assert_eq!(
                unsafe { hirari_bounce_stream_finish(stream) },
                1,
                "format {format} could not publish"
            );
            unsafe { hirari_bounce_stream_free(stream) };
            let data = std::fs::read(&path).unwrap();
            assert!(data.starts_with(b"RIFF"), "format {format} container mismatch");
            if wave64 {
                assert_eq!(&data[4..8], &[0x2e, 0x91, 0xcf, 0x11]);
            } else {
                assert_eq!(&data[8..12], b"WAVE");
            }
            assert_eq!(data.len() % 2, 0);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn unsupported_bounce_stream_format_is_rejected() {
        let path = temp_path("wav");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe {
            hirari_bounce_stream_create(bytes.as_ptr(), bytes.len(), 16, 48_000, 4)
        };
        assert!(stream.is_null());
    }

    fn write_planar(path: &Path, planes: &[Vec<f32>], frames: u64, format: u8) -> u8 {
        let pointers: Vec<*const f32> = planes.iter().map(|plane| plane.as_ptr()).collect();
        let bytes = path.to_str().unwrap().as_bytes();
        unsafe {
            hirari_wav_write_planar(
                bytes.as_ptr(),
                bytes.len(),
                pointers.as_ptr(),
                pointers.len(),
                frames,
                48_000,
                format,
            )
        }
    }

    fn decode(path: &Path) -> (u32, f64, Vec<f32>) {
        let bytes = path.to_str().unwrap().as_bytes();
        let decoded = crate::wav_decoder_compat::decode_wav_file(bytes).expect("decode");
        let samples = unsafe {
            std::slice::from_raw_parts(decoded.samples, decoded.sample_count).to_vec()
        };
        unsafe { crate::preview_audio_runtime::hirari_free_decoded_audio(decoded.samples, decoded.sample_count) };
        (decoded.channels, decoded.sample_rate, samples)
    }

    #[test]
    fn planar_writers_emit_expected_headers_and_replace_destinations() {
        let left = vec![0.0_f32, 0.25];
        let right = vec![-0.25_f32, 0.5];
        let stereo = [left, right];

        let pcm16 = temp_path("wav");
        assert_eq!(write_planar(&pcm16, &stereo, 2, 1), 0);
        let data = std::fs::read(&pcm16).unwrap();
        assert!(data.starts_with(b"RIFF"));
        assert_eq!(&data[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([data[34], data[35]]), 16);
        let (channels, rate, samples) = decode(&pcm16);
        assert_eq!((channels, rate), (2, 48_000.0));
        assert!(samples.iter().all(|value| value.is_finite()));

        let pcm24 = temp_path("wav");
        assert_eq!(write_planar(&pcm24, &stereo, 2, 2), 0);
        let data = std::fs::read(&pcm24).unwrap();
        assert_eq!(u16::from_le_bytes([data[34], data[35]]), 24);

        let multichannel = [
            vec![0.0_f32, 0.25],
            vec![-0.25_f32, 0.5],
            vec![0.75_f32, -0.5],
        ];
        let multi_path = temp_path("wav");
        assert_eq!(write_planar(&multi_path, &multichannel, 2, 1), 0);
        let data = std::fs::read(&multi_path).unwrap();
        assert_eq!(u16::from_le_bytes([data[22], data[23]]), 3);
        assert_eq!(decode(&multi_path).0, 3);

        // Re-exporting over an existing destination replaces it wholesale.
        let replaced = [
            vec![0.25_f32, -0.25, 0.75, -0.5, 0.125, -0.125],
            vec![-0.25_f32, 0.25, 0.5, -0.75, 0.25, -0.25],
        ];
        assert_eq!(write_planar(&multi_path, &replaced, 6, 0), 0);
        let (channels, _, samples) = decode(&multi_path);
        assert_eq!(channels, 2);
        assert_eq!(samples.len(), 12);
        assert_eq!(&samples[..], &[0.25, -0.25, -0.25, 0.25, 0.75, 0.5, -0.5, -0.75, 0.125, 0.25, -0.125, -0.25]);

        for path in [pcm16, pcm24, multi_path] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn pcm_stream_writers_publish_complete_container_sizes() {
        let left = [0.0_f32, 0.25, 0.5, 0.75];
        let right = [-0.25_f32, 0.5, -0.75, 1.0];
        let channels = [left.as_ptr(), right.as_ptr()];
        for (bit_depth, expected_bytes) in [(16_u16, 60_u64), (24, 68)] {
            let path = temp_path("wav");
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_wav_pcm_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 2, bit_depth, 0)
            };
            assert!(!stream.is_null());
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, channels.as_ptr(), 2) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, channels.as_ptr(), 2) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_finish(stream) }, 1);
            unsafe { hirari_wav_pcm_stream_free(stream) };
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected_bytes);
            let data = std::fs::read(&path).unwrap();
            assert_eq!(u16::from_le_bytes([data[34], data[35]]), bit_depth);
            let _ = std::fs::remove_file(path);
        }
        for (bit_depth, expected_bytes) in [(16_u16, 52_u64), (24, 56)] {
            let path = temp_path("wav");
            let bytes = path.to_str().unwrap().as_bytes();
            let stream = unsafe {
                hirari_wav_pcm_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 1, bit_depth, 0)
            };
            assert!(!stream.is_null());
            assert_eq!(unsafe { hirari_wav_pcm_stream_write(stream, [left.as_ptr()].as_ptr(), 4) }, 1);
            assert_eq!(unsafe { hirari_wav_pcm_stream_finish(stream) }, 1);
            unsafe { hirari_wav_pcm_stream_free(stream) };
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected_bytes);
            let data = std::fs::read(&path).unwrap();
            assert_eq!(u16::from_le_bytes([data[22], data[23]]), 1);
            assert_eq!(u16::from_le_bytes([data[34], data[35]]), bit_depth);
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn float_stream_sanitises_non_finite_samples() {
        let path = temp_path("wav");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe { hirari_wav_float32_stream_create(bytes.as_ptr().cast(), bytes.len(), 48_000, 2) };
        assert!(!stream.is_null());
        let left = [0.25_f32, f32::NAN];
        let right = [-0.5_f32, 0.75];
        let channels = [left.as_ptr(), right.as_ptr()];
        assert_eq!(unsafe { hirari_wav_float32_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_float32_stream_frames(stream) }, 2);
        assert_eq!(unsafe { hirari_wav_float32_stream_finish(stream) }, 1);
        unsafe { hirari_wav_float32_stream_free(stream) };
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 96);
        let (channels, _, samples) = decode(&path);
        assert_eq!(channels, 2);
        assert!(samples.iter().all(|value| value.is_finite()));
        assert_eq!(&samples[..], &[0.25, -0.5, 0.0, 0.75]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wave64_stream_reports_frames_and_round_trips() {
        let path = temp_path("w64");
        let bytes = path.to_str().unwrap().as_bytes();
        let stream = unsafe {
            hirari_wav_wave64_stream_create(bytes.as_ptr().cast(), bytes.len(), 4, 48_000, 2)
        };
        assert!(!stream.is_null());
        let left = [0.0_f32, 0.25, 0.5, 0.75];
        let right = [-0.25_f32, 0.5, -0.75, 1.0];
        let channels = [left.as_ptr(), right.as_ptr()];
        assert_eq!(unsafe { hirari_wav_wave64_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_wave64_stream_write(stream, channels.as_ptr(), 2) }, 1);
        assert_eq!(unsafe { hirari_wav_wave64_stream_frames(stream) }, 4);
        assert_eq!(unsafe { hirari_wav_wave64_stream_finish(stream) }, 1);
        unsafe { hirari_wav_wave64_stream_free(stream) };
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 136);
        let (rate, channels, samples) =
            crate::export::read_wave64_float32(&path).expect("wave64 round trip");
        assert_eq!((rate, channels), (48_000, 2));
        assert_eq!(samples.len(), 8);
        assert_eq!(samples[3], 0.5);
        let _ = std::fs::remove_file(path);
    }
}
