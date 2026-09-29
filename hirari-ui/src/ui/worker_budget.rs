//! Process-wide bound for expensive project/background filesystem work.
use std::sync::{Condvar, Mutex, OnceLock};

const MAX_CONCURRENT: usize = 2;
static STATE: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

pub(crate) struct WorkerPermit;

impl WorkerPermit {
    pub(crate) fn try_acquire() -> Option<Self> {
        let (active, _) = STATE.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut active = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *active >= MAX_CONCURRENT {
            return None;
        }
        *active += 1;
        Some(Self)
    }

    pub(crate) fn acquire() -> Self {
        let (active, changed) = STATE.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut active = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *active >= MAX_CONCURRENT {
            active = changed
                .wait(active)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *active += 1;
        Self
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let (active, changed) = STATE.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut active = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        changed.notify_one();
    }
}
