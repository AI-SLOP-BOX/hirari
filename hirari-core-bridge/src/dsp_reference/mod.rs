// Frozen scalar reference oracles for the differential DSP tests.
//
// Each submodule is a faithful port of the former tests/dsp_reference/*.cpp
// file. They were C++ copies of implementations that had already moved to
// Rust, kept alive only to prove numeric equivalence under the
// `dsp-differential-reference` feature. The oracle now lives in Rust and the
// differential tests keep comparing the production path against it.

pub mod analysis;
pub mod analysis_structure;
pub mod audio_note_curve;
pub mod automation;
pub mod builtin_midi;
pub mod chromaglow;
pub mod deesser;
pub mod master_limiter;
pub mod effect_chain;
pub mod midi_buffer;
pub mod region_gain;
pub mod stem_splitter;
pub mod track_pdc_delay;
pub mod transient_shaper;
pub mod bus_audio;
pub mod builtin_gain;
pub mod control_room_audio;
pub mod master_output;
pub mod stereo_dsp;
pub mod track_channel_eq;
pub mod masking_analysis;
pub mod poly_sampler;
pub mod preview_synth;
pub mod region_pitch_correction;
pub mod region_resampler;
pub mod region_warp;
pub mod sampler;
pub mod spectral;
pub mod track_holographic_panner;
pub mod wsola;
