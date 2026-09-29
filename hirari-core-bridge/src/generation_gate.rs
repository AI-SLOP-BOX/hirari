use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Small lock-free gate for asynchronous render/waveform completions.
/// A worker may publish only while its captured generation is still current.
#[derive(Debug, Default)]
pub struct GenerationGate {
    current: AtomicU64,
    exhausted: AtomicBool,
}

impl GenerationGate {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn invalidate(&self) -> u64 {
        let mut observed = self.current.load(Ordering::Acquire);
        loop {
            if observed == u64::MAX {
                // Never wrap back to a generation that an old worker may
                // still hold.  Fail closed: callers receive the sentinel,
                // while accepts() rejects every completion permanently.
                self.exhausted.store(true, Ordering::Release);
                return observed;
            }
            let next = observed + 1;
            match self.current.compare_exchange_weak(
                observed,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if next == u64::MAX {
                        self.exhausted.store(true, Ordering::Release);
                    }
                    return next;
                }
                Err(actual) => observed = actual,
            }
        }
    }
    /// Begin a new asynchronous publication generation.
    pub fn begin(&self) -> u64 {
        self.invalidate()
    }
    /// Invalidate all previously submitted work.
    pub fn cancel(&self) -> u64 {
        self.invalidate()
    }
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::Acquire)
    }
    pub fn accepts(&self, generation: u64) -> bool {
        !self.exhausted.load(Ordering::Acquire) && self.current() == generation
    }
}

#[cfg(test)]
mod tests {
    use super::GenerationGate;
    use std::sync::atomic::Ordering;
    #[test]
    fn stale_completion_is_rejected() {
        let gate = GenerationGate::new();
        let first = gate.invalidate();
        assert!(gate.accepts(first));
        let second = gate.invalidate();
        assert!(!gate.accepts(first));
        assert!(gate.accepts(second));
    }

    #[test]
    fn cancel_invalidates_a_generation_without_reusing_it() {
        let gate = GenerationGate::new();
        let submitted = gate.begin();
        let cancelled = gate.cancel();

        assert!(!gate.accepts(submitted));
        assert!(cancelled > submitted);
        assert_eq!(gate.current(), cancelled);
    }

    #[test]
    fn generation_overflow_fails_closed_instead_of_wrapping() {
        let gate = GenerationGate::new();
        gate.current.store(u64::MAX - 1, Ordering::Release);
        let terminal = gate.begin();
        assert_eq!(terminal, u64::MAX);
        assert!(!gate.accepts(terminal));
        assert_eq!(gate.begin(), u64::MAX);
        assert!(!gate.accepts(0));
    }
}
