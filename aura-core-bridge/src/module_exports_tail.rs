pub mod tempo_analyzer;
pub mod temporal;
pub mod timeline;
pub mod tonal_sync;
pub mod tonal;
pub mod track_alternatives;
pub mod track_freeze_manager;
pub mod transient;
pub mod transient_shaper;
pub mod transport_state;
pub mod transport_orchestrator;
pub mod true_peak_limiter;
pub mod tube_saturation;
pub mod undo_history;
pub mod undo_snapshot_history;
pub mod unified_config;
pub mod unified_engine;
pub mod va_oscillator;
pub mod vca_group_model;
pub mod vca_console;
/// Stable compatibility namespace for clients that imported the original
/// `vca` module before the implementation was split into focused modules.
pub mod vca {
    pub use crate::vca_group_model::VcaConsole;
}
pub mod vca_fader_state;
pub mod vca_gain_orchestrator;
pub mod video;
pub mod video_system;
pub mod piano_visualizer;
pub mod vintage_eq;
pub mod virtuoso_pitch;
pub mod virtuoso_pultec;
pub mod virtuoso_space;
pub mod virtuoso_tape;
pub mod virtuoso_vocal;
pub mod vocal_doubler;
pub mod vocal_tuner;
pub mod vocal_align;
pub mod voice_manager;
pub mod waveform_cache;
pub mod wavetable_oscillator;
pub mod wavetable_synth;
pub mod zero_crossing_engine;
pub mod zerocrossing;
