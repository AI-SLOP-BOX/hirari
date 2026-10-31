//! Silent fallback device contract.
//!
//! Ported from the deleted C++ `Hirari::Platform` device abstraction, whose
//! only consumers were contract tests: the live engine opens CoreAudio through
//! `mac_audio_driver`. This keeps the transition contract (invalid generations
//! fail closed, a later valid generation clears the previous error, repeated
//! reconfiguration stays deterministic) exercised on the Rust side.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioDeviceConfig {
    pub sample_rate: f64,
    pub buffer_size: u32,
    pub inputs: u32,
    pub outputs: u32,
}

impl Default for AudioDeviceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            buffer_size: 256,
            inputs: 2,
            outputs: 2,
        }
    }
}

#[derive(Debug)]
pub struct SilentAudioDevice {
    config: AudioDeviceConfig,
    initialized: bool,
    running: bool,
    error: String,
}

impl Default for SilentAudioDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl SilentAudioDevice {
    pub fn new() -> Self {
        Self {
            config: AudioDeviceConfig::default(),
            initialized: false,
            running: false,
            error: "audio backend unavailable; silent fallback selected".to_owned(),
        }
    }

    /// A rejected generation is recorded as an error and never reports a
    /// usable device; the config still reflects what was requested so callers
    /// can report the rejected format.
    pub fn initialize(&mut self, config: AudioDeviceConfig) -> bool {
        self.error.clear();
        self.config = config;
        self.running = false;
        self.initialized = config.sample_rate.is_finite()
            && config.sample_rate > 0.0
            && config.buffer_size > 0
            && config.outputs > 0;
        if !self.initialized {
            self.error = "invalid audio device configuration".to_owned();
        }
        self.initialized
    }

    /// The fallback has no backend: starting it always fails closed and names
    /// the reason instead of reporting a phantom running device.
    pub fn start(&mut self) -> bool {
        self.running = false;
        self.error = "audio backend unavailable; silent fallback selected".to_owned();
        false
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn name(&self) -> &'static str {
        "Hirari Silent Audio Device"
    }

    pub fn is_hardware_available(&self) -> bool {
        false
    }

    pub fn is_silent_fallback(&self) -> bool {
        true
    }

    pub fn last_error(&self) -> &str {
        &self.error
    }

    pub fn has_error(&self) -> bool {
        !self.error.is_empty()
    }

    pub fn config(&self) -> AudioDeviceConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(sample_rate: f64, buffer_size: u32) -> AudioDeviceConfig {
        AudioDeviceConfig {
            sample_rate,
            buffer_size,
            inputs: 2,
            outputs: 2,
        }
    }

    #[test]
    fn fallback_start_fails_closed_and_names_fallback() {
        let mut device = SilentAudioDevice::new();
        assert!(device.initialize(config(48_000.0, 128)));
        assert_eq!(device.config(), config(48_000.0, 128));
        assert!(!device.start());
        assert!(device.has_error());
        assert!(device.is_silent_fallback());
        assert!(!device.is_hardware_available());
        assert!(!device.is_running());
        assert_eq!(device.name(), "Hirari Silent Audio Device");
    }

    #[test]
    fn a_failed_start_does_not_block_the_next_generation() {
        let mut device = SilentAudioDevice::new();
        assert!(device.initialize(config(48_000.0, 128)));
        assert!(!device.start());
        device.stop();
        assert!(device.initialize(config(96_000.0, 512)));
        assert_eq!(device.config().sample_rate, 96_000.0);
        assert_eq!(device.config().buffer_size, 512);
        assert!(!device.is_running());
    }

    #[test]
    fn invalid_generations_fail_closed_and_name_the_error() {
        let mut device = SilentAudioDevice::new();
        let invalid = AudioDeviceConfig {
            sample_rate: f64::NAN,
            buffer_size: 0,
            inputs: 2,
            outputs: 2,
        };
        assert!(!device.initialize(invalid));
        assert!(!device.is_running());
        assert!(device.has_error());
        assert!(device.last_error().contains("invalid"));

        let mut no_outputs = config(44_100.0, 64);
        no_outputs.outputs = 0;
        assert!(!device.initialize(no_outputs));
        assert!(device.has_error());
    }

    #[test]
    fn a_valid_reinitialization_clears_the_previous_failure() {
        let mut device = SilentAudioDevice::new();
        let invalid = AudioDeviceConfig {
            sample_rate: -1.0,
            buffer_size: 0,
            inputs: 0,
            outputs: 0,
        };
        assert!(!device.initialize(invalid));
        assert!(device.has_error());

        let recovered = config(44_100.0, 64);
        assert!(device.initialize(recovered));
        assert!(!device.has_error());
        assert_eq!(device.config(), recovered);
    }

    #[test]
    fn repeated_reconfiguration_is_deterministic() {
        let mut device = SilentAudioDevice::new();
        for round in 0..20 {
            let rate = if round % 2 == 0 { 44_100.0 } else { 96_000.0 };
            let frames = if round % 4 == 0 { 128 } else { 512 };
            assert!(device.initialize(config(rate, frames)));
            assert!(!device.has_error());
            assert_eq!(device.config().sample_rate, rate);
            assert_eq!(device.config().buffer_size, frames);
        }
    }
}
