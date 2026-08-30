//! Project-operation gate shared by save, load, and recovery callbacks.
//!
//! The native API is currently synchronous, but callbacks can still be
//! re-entered by a dialog or a queued UI event. Keeping the guard here makes
//! that contract explicit and gives future async work a request generation.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum OperationKind {
    Load = 1,
    Save = 2,
    Recover = 3,
    Render = 4,
}

pub(crate) struct OperationLease {
    state: Arc<AtomicU8>,
    active_generation: Arc<AtomicU64>,
    pub(crate) generation: u64,
    kind: OperationKind,
}

#[derive(Clone, Default)]
pub(crate) struct OperationGate {
    state: Arc<AtomicU8>,
    generation: Arc<AtomicU64>,
    active_generation: Arc<AtomicU64>,
}

impl OperationGate {
    pub(crate) fn try_enter(&self, kind: OperationKind) -> Option<OperationLease> {
        self.state
            .compare_exchange(0, kind as u8, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.active_generation.store(generation, Ordering::Release);
        Some(OperationLease {
            state: self.state.clone(),
            active_generation: self.active_generation.clone(),
            generation,
            kind,
        })
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.state.load(Ordering::Acquire) != 0
            && self.active_generation.load(Ordering::Acquire) == generation
    }

    #[cfg(test)]
    pub(crate) fn is_busy(&self) -> bool {
        self.state.load(Ordering::Acquire) != 0
    }
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let _ =
            self.state
                .compare_exchange(self.kind as u8, 0, Ordering::AcqRel, Ordering::Acquire);
        if self.active_generation.load(Ordering::Acquire) == self.generation {
            self.active_generation.store(0, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OperationGate, OperationKind};

    #[test]
    fn rejects_reentrant_operations_and_advances_generation() {
        let gate = OperationGate::default();
        let first = gate.try_enter(OperationKind::Load).expect("first lease");
        assert!(gate.is_busy());
        assert!(gate.try_enter(OperationKind::Save).is_none());
        let generation = first.generation;
        drop(first);
        let second = gate
            .try_enter(OperationKind::Recover)
            .expect("second lease");
        assert!(second.generation > generation);
        drop(second);
        let render = gate.try_enter(OperationKind::Render).expect("render lease");
        assert!(gate.try_enter(OperationKind::Save).is_none());
        drop(render);
        assert!(!gate.is_busy());
    }
}
