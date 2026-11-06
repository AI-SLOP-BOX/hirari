//! Frozen reference fixtures captured from the last C++ build before the
//! engine was fully migrated to Rust (`tests/fixtures/cpp_dsp_reference.bin`).
//!
//! The vendored Signalsmith C++ and the first-party reference shims were
//! deleted after these values were recorded; the differential tests now
//! compare the pure-Rust DSP against these frozen outputs.

pub(crate) struct FixtureReader<'a> {
    data: &'a [u8],
}

impl<'a> FixtureReader<'a> {
    pub(crate) fn open() -> Self {
        let bytes = include_bytes!("../../tests/fixtures/cpp_dsp_reference.bin");
        assert_eq!(&bytes[..8], b"HIRAREF1", "fixture magic");
        Self { data: &bytes[8..] }
    }

    fn u32(&mut self) -> u32 {
        let value = u32::from_le_bytes(self.data[..4].try_into().unwrap());
        self.data = &self.data[4..];
        value
    }

    fn f32s(&mut self, count: usize) -> Vec<f32> {
        let bytes = count * 4;
        let slice = &self.data[..bytes];
        self.data = &self.data[bytes..];
        slice
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    /// Kaiser PR windows captured from the C++ `Kaiser::withBandwidth` path.
    pub(crate) fn kaiser_windows(&mut self) -> Vec<(usize, usize, Vec<f32>)> {
        let mut windows = Vec::new();
        for _ in 0..3 {
            let size = self.u32() as usize;
            let interval = self.u32() as usize;
            let samples = self.f32s(size);
            windows.push((size, interval, samples));
        }
        windows
    }

    /// Skip the leading kaiser-window section (tests that start later).
    pub(crate) fn skip_kaiser_windows(&mut self) {
        for _ in 0..3 {
            let size = self.u32() as usize;
            let _interval = self.u32();
            self.f32s(size);
        }
    }

    /// Skip the STFT spectrum section.
    pub(crate) fn skip_stft_spectrum(&mut self) {
        let fft_size = self.u32() as usize;
        self.f32s(fft_size / 2);
        self.f32s(fft_size / 2);
    }

    /// STFT analysis spectrum (real, imag) and the FFT size used.
    pub(crate) fn stft_spectrum(&mut self) -> (usize, Vec<f32>, Vec<f32>) {
        let fft_size = self.u32() as usize;
        let real = self.f32s(fft_size / 2);
        let imag = self.f32s(fft_size / 2);
        (fft_size, real, imag)
    }

    /// Region-stretch reference renders: (left, right) per case, 512 frames.
    pub(crate) fn stretch_renders(&mut self) -> Vec<(Vec<f32>, Vec<f32>)> {
        let count = self.u32() as usize;
        let mut renders = Vec::with_capacity(count);
        for _ in 0..count {
            let left = self.f32s(512);
            let right = self.f32s(512);
            renders.push((left, right));
        }
        renders
    }

    fn real_fft_section(
        &mut self,
    ) -> (usize, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        let size = self.u32() as usize;
        let input = self.f32s(size);
        let half = size / 2;
        let real = self.f32s(half);
        let imag = self.f32s(half);
        let time = self.f32s(size);
        (size, input, real, imag, time)
    }

    fn has_data(&self) -> bool {
        !self.data.is_empty()
    }

    /// Analog saturator frozen reference outputs (left, right) per model.
    pub(crate) fn saturator_outputs(&mut self) -> Vec<(Vec<f32>, Vec<f32>)> {
        let count = self.u32() as usize;
        let mut outputs = Vec::with_capacity(count);
        for _ in 0..count {
            let left = self.f32s(8);
            let right = self.f32s(8);
            outputs.push((left, right));
        }
        outputs
    }
}

/// Real-FFT reference fixtures appended after the base sections:
/// (input, forward real, forward imag, inverse time) per size.
pub(crate) struct RealFftFixture {
    pub size: usize,
    pub input: Vec<f32>,
    pub forward_real: Vec<f32>,
    pub forward_imag: Vec<f32>,
    pub inverse_time: Vec<f32>,
}

pub(crate) fn read_real_fft_fixtures(
    reader: &mut FixtureReader<'_>,
) -> Vec<RealFftFixture> {
    // The appended section is a run of (size, input, real, imag, time)
    // records; it ends when the data is exhausted.
    let mut out = Vec::new();
    while reader.has_data() {
        let (size, input, forward_real, forward_imag, inverse_time) = reader.real_fft_section();
        out.push(RealFftFixture {
            size,
            input,
            forward_real,
            forward_imag,
            inverse_time,
        });
    }
    out
}
