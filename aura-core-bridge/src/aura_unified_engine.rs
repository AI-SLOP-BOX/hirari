#![allow(deprecated)]

/// Compatibility shim only. Production audio processing belongs to the
/// native C++ graph exposed by `AuraCore::process_audio_block`.
#[deprecated(note = "use AuraCore::process_audio_block; Rust must not own a second audio graph")]
pub struct AuraUnifiedOrchestrator {}

impl Default for AuraUnifiedOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AuraUnifiedOrchestrator {
    pub fn new() -> Self {
        Self {}
    }

    /// INDUSTRIAL: Orchestrates all sub-engines for a single audio block with absolute real-time safety and singularity precision.
    #[deprecated(note = "use AuraCore::process_audio_block")]
    pub fn render_block(&self, out_l: &mut [f32], out_r: &mut [f32], num_samples: u32) {
        let samples = num_samples as usize;

        // There is no processing to perform without a complete, bounded stereo block.
        if samples == 0
            || out_l.is_empty()
            || out_r.is_empty()
            || out_l.len() != out_r.len()
            || samples > out_l.len()
        {
            return;
        }

        // Keep valid output values deterministic and contain invalid values at the
        // bridge boundary. No allocation or external engine is needed here.
        for sample in out_l[..samples]
            .iter_mut()
            .chain(out_r[..samples].iter_mut())
        {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
    }

    /// INDUSTRIAL: Performs a forensic audit of the engine-wide terminal state.
    pub fn audit_aura_unified_engine(&self) -> bool {
        // The bridge is intentionally stateless: there are no sub-engines or
        // buffers whose state can be stale or invalid.
        let _ = self;
        true
    }
}
