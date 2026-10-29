use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::stem_separator_validation::{backend_name, validate_model, validation_message};
use crate::stem_splitter::{split_stems, StemPlanes};

/// Optional neural-runtime backends registered by an independently linked
/// adapter. The engine itself never vendors a runtime ABI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StemSeparatorBackend {
    Heuristic = 0,
    OnnxRuntime = 1,
    LibTorch = 2,
    ExternalProcess = 3,
}

impl StemSeparatorBackend {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Heuristic,
            1 => Self::OnnxRuntime,
            2 => Self::LibTorch,
            3 => Self::ExternalProcess,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        backend_name(self.code())
    }
}

#[derive(Clone, Debug)]
pub struct StemSeparatorConfig {
    pub backend: StemSeparatorBackend,
    pub model_path: String,
    pub model_sample_rate: u32,
    pub max_channels: u32,
}

impl Default for StemSeparatorConfig {
    fn default() -> Self {
        Self {
            backend: StemSeparatorBackend::Heuristic,
            model_path: String::new(),
            model_sample_rate: 44_100,
            max_channels: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StemSeparatorStatus {
    pub backend: StemSeparatorBackend,
    pub backend_name: String,
    pub display_name: String,
    pub model_file_name: String,
    pub available: bool,
}

pub type SplitFunction = dyn Fn(&[Vec<f32>], f64) -> Option<StemPlanes> + Send + Sync;

#[derive(Clone)]
struct OssProvider {
    config: StemSeparatorConfig,
    split: Arc<SplitFunction>,
}

impl OssProvider {
    /// Mirrors the C++ adapter: a configured but unusable backend reports
    /// unavailable rather than silently falling back to the heuristic split.
    fn available(&self) -> bool {
        self.config.backend != StemSeparatorBackend::Heuristic
            && !self.config.model_path.is_empty()
    }

    fn model_file_name(&self) -> String {
        Path::new(&self.config.model_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
enum Provider {
    Heuristic,
    Model(OssProvider),
}

/// Control-thread registry. The active provider is replaced atomically under
/// a mutex; separation itself never runs on an audio callback.
pub struct StemSeparatorRegistry {
    provider: Mutex<Provider>,
}

impl StemSeparatorRegistry {
    pub const fn new() -> Self {
        Self {
            provider: Mutex::new(Provider::Heuristic),
        }
    }

    fn snapshot(&self) -> Provider {
        self.provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_oss_provider(&self, config: StemSeparatorConfig, split: Arc<SplitFunction>) {
        let mut provider = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *provider = Provider::Model(OssProvider { config, split });
    }

    pub fn install_oss_provider(
        &self,
        config: StemSeparatorConfig,
        split: Option<Arc<SplitFunction>>,
    ) -> Result<(), String> {
        let code = validate_model(
            config.backend.code(),
            &config.model_path,
            config.model_sample_rate,
            config.max_channels,
        );
        if code != 0 {
            return Err(validation_message(code).to_string_lossy().into_owned());
        }
        let Some(split) = split else {
            return Err("OSS provider callback is empty".to_string());
        };
        self.set_oss_provider(config, split);
        Ok(())
    }

    pub fn reset_to_heuristic(&self) {
        let mut provider = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *provider = Provider::Heuristic;
    }

    pub fn status(&self) -> StemSeparatorStatus {
        match self.snapshot() {
            Provider::Heuristic => StemSeparatorStatus {
                backend: StemSeparatorBackend::Heuristic,
                backend_name: StemSeparatorBackend::Heuristic.name().to_string(),
                display_name: StemSeparatorBackend::Heuristic.name().to_string(),
                model_file_name: String::new(),
                available: true,
            },
            Provider::Model(provider) => {
                let backend_name = provider.config.backend.name().to_string();
                let model_file_name = provider.model_file_name();
                let display_name = if model_file_name.is_empty() {
                    backend_name.clone()
                } else {
                    model_file_name.clone()
                };
                StemSeparatorStatus {
                    backend: provider.config.backend,
                    backend_name,
                    display_name,
                    model_file_name,
                    available: provider.available(),
                }
            }
        }
    }

    pub fn split(&self, input: &[Vec<f32>], sample_rate: f64) -> Option<StemPlanes> {
        match self.snapshot() {
            Provider::Heuristic => split_stems(input, sample_rate),
            Provider::Model(provider) => {
                if !provider.available() || !sample_rate.is_finite() || sample_rate <= 0.0 {
                    return None;
                }
                (provider.split)(input, sample_rate)
            }
        }
    }
}

impl Default for StemSeparatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn global() -> &'static StemSeparatorRegistry {
    static REGISTRY: StemSeparatorRegistry = StemSeparatorRegistry::new();
    &REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_model_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("hirari-{}-{name}", std::process::id()))
    }

    fn two_plane_tone(samples: usize) -> Vec<Vec<f32>> {
        (0..2)
            .map(|_| (0..samples).map(|index| index as f32 / samples as f32).collect())
            .collect()
    }

    #[test]
    fn install_rejects_an_empty_callback_and_invalid_models() {
        let registry = StemSeparatorRegistry::new();
        let mut config = StemSeparatorConfig {
            backend: StemSeparatorBackend::OnnxRuntime,
            ..StemSeparatorConfig::default()
        };
        let split: Arc<SplitFunction> = Arc::new(|input, rate| split_stems(input, rate));

        let error = registry
            .install_oss_provider(config.clone(), Some(Arc::clone(&split)))
            .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(error, "model path is empty");

        config.model_path = temp_model_path("callback.onnx").to_string_lossy().into_owned();
        let path = std::path::PathBuf::from(&config.model_path);
        std::fs::write(&path, b"test").unwrap();
        let error = registry.install_oss_provider(config.clone(), None).unwrap_err();
        assert_eq!(error, "OSS provider callback is empty");

        registry.install_oss_provider(config, Some(split)).unwrap();
        let status = registry.status();
        assert_eq!(status.backend, StemSeparatorBackend::OnnxRuntime);
        assert_eq!(status.backend_name, "ONNX Runtime");
        assert_eq!(status.display_name, path.file_name().unwrap().to_string_lossy());
        assert!(status.available);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn heuristic_registry_splits_and_reports_itself() {
        let registry = StemSeparatorRegistry::new();
        let input = two_plane_tone(64);
        let stems = registry.split(&input, 48_000.0).unwrap();
        assert_eq!(stems.vocals.len(), 2);
        assert_eq!(stems.vocals[0].len(), 64);
        assert_eq!(stems.drums[0].len(), 64);
        assert_eq!(stems.bass[0].len(), 64);
        assert_eq!(stems.other[0].len(), 64);

        let status = registry.status();
        assert_eq!(status.backend, StemSeparatorBackend::Heuristic);
        assert!(status.available);
        assert_eq!(status.display_name, "Heuristic");
    }

    #[test]
    fn multichannel_split_keeps_non_finite_inputs_out_of_the_stems() {
        let registry = StemSeparatorRegistry::new();
        let mut input: Vec<Vec<f32>> = (0..3)
            .map(|channel| {
                (0..16)
                    .map(|index| (channel + index) as f32 * 0.01)
                    .collect()
            })
            .collect();
        input[2][3] = f32::NAN;
        let stems = registry.split(&input, 48_000.0).unwrap();
        for plane in [&stems.drums, &stems.bass, &stems.vocals, &stems.other] {
            assert_eq!(plane.len(), 3);
            assert!(plane.iter().all(|channel| channel.len() == 16));
            assert!(plane.iter().flatten().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn model_provider_dispatches_to_the_registered_callback() {
        let registry = StemSeparatorRegistry::new();
        let path = temp_model_path("dispatch.onnx");
        std::fs::write(&path, b"test").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let split: Arc<SplitFunction> = Arc::new(move |input, _| {
            observed.fetch_add(1, Ordering::Relaxed);
            split_stems(input, 44_100.0)
        });
        registry
            .install_oss_provider(
                StemSeparatorConfig {
                    backend: StemSeparatorBackend::OnnxRuntime,
                    model_path: path.to_string_lossy().into_owned(),
                    ..StemSeparatorConfig::default()
                },
                Some(split),
            )
            .unwrap();
        assert!(registry.split(&two_plane_tone(8), 44_100.0).is_some());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(registry.split(&two_plane_tone(8), 0.0).is_none());
        registry.reset_to_heuristic();
        assert_eq!(registry.status().backend, StemSeparatorBackend::Heuristic);
        std::fs::remove_file(path).unwrap();
    }
}
