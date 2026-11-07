pub mod audio_quantizer;
pub mod temporal;
pub mod tpdf_dither;
pub mod timeline;
pub mod tonal_sync;
pub mod tonal;
pub mod transient;
pub mod true_peak_limiter;
pub mod pro_limiter;
pub mod tube_saturation;
pub mod undo_transaction;
pub mod vca_group_model;
/// Stable compatibility namespace for clients that imported the original
/// `vca` module before the implementation was split into focused modules.
pub mod vca {
    pub use crate::vca_group_model::VcaConsole;
}
pub mod video;
pub(crate) mod video_native;
pub mod video_system;
pub mod piano_visualizer;
pub mod virtuoso_pultec;
pub mod virtuoso_space;
pub mod virtuoso_tape;
pub mod virtuoso_vocal;
pub mod waveform_cache;
pub mod wav_diagnostic;
pub mod wavetable_oscillator;
pub mod stem_separator_validation;
pub mod mpe_manager;
pub mod forensic_journaler;
pub mod forensic_scratchpad;
pub mod fft_utils;
pub mod dsp_utils;
pub mod wav_decoder_compat;
pub mod hot_swap_state;
pub mod realtime_thread;
include!("cli_render_tests.rs");

#[cfg(feature = "dsp-differential-reference")]
pub mod dsp_reference;
pub mod region_processing_ops;
pub mod engine_native;
pub mod signalsmith_stretch;
pub(crate) mod dsp_fixtures;
