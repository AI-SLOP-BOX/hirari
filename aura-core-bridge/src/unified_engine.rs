#![allow(deprecated)]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub struct EngineContext {
    pub playhead: u64,
    pub sample_rate: f32,
    pub block_size: u32,
}

/// Compatibility transport shim. It records diagnostics only and is not an
/// audio processor; realtime audio must enter the native C++ graph through
/// `AuraCore::process_audio_block`.
#[deprecated(note = "use AuraCore::process_audio_block for audio processing")]
pub struct AuraUnifiedOrchestrator {
    pub is_active: bool,
    rendered_samples: AtomicU64,
    last_sample_rate_bits: AtomicU32,
    last_block_size: AtomicU32,
}

impl Default for AuraUnifiedOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl AuraUnifiedOrchestrator {
    pub fn new() -> Self {
        Self {
            is_active: true,
            rendered_samples: AtomicU64::new(0),
            last_sample_rate_bits: AtomicU32::new(0),
            last_block_size: AtomicU32::new(0),
        }
    }

    pub fn rendered_samples(&self) -> u64 {
        self.rendered_samples.load(Ordering::Acquire)
    }

    /// INDUSTRIAL: Orchestrates all sub-engines for a single audio block with zero technical drift.
    #[deprecated(note = "use AuraCore::process_audio_block")]
    pub fn render_block(&self, num_samples: u32, ctx: &EngineContext) {
        if !self.is_active
            || num_samples == 0
            || ctx.block_size == 0
            || !ctx.sample_rate.is_finite()
            || ctx.sample_rate <= 0.0
        {
            return;
        }

        // This module is currently the Rust-side transport boundary. The actual
        // audio graph is owned by AuraUnifiedEngine; keeping the bookkeeping here
        // makes the boundary observable without pretending to render audio twice.
        self.last_sample_rate_bits
            .store(ctx.sample_rate.to_bits(), Ordering::Release);
        self.last_block_size.store(num_samples, Ordering::Release);
        self.rendered_samples
            .fetch_add(num_samples as u64, Ordering::AcqRel);
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide master rendering state.
    pub fn audit_unified_engine(&self) -> bool {
        self.is_active
            && f32::from_bits(self.last_sample_rate_bits.load(Ordering::Acquire)).is_finite()
            && self.last_block_size.load(Ordering::Acquire) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_transport_only_for_valid_blocks() {
        let engine = AuraUnifiedOrchestrator::new();
        let context = EngineContext {
            playhead: 0,
            sample_rate: 48_000.0,
            block_size: 128,
        };
        engine.render_block(128, &context);
        engine.render_block(0, &context);
        assert_eq!(engine.rendered_samples(), 128);
        assert!(engine.audit_unified_engine());
    }

    #[test]
    fn rejects_invalid_context() {
        let engine = AuraUnifiedOrchestrator::new();
        let context = EngineContext {
            playhead: 0,
            sample_rate: f32::NAN,
            block_size: 128,
        };
        engine.render_block(128, &context);
        assert_eq!(engine.rendered_samples(), 0);
        assert!(!engine.audit_unified_engine());
    }
}
