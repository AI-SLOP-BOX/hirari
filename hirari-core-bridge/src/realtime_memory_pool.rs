use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const ARENA_COUNT: usize = 8;
const ARENA_SIZE: usize = 4 * 1024 * 1024;
const ALIGNMENT: usize = 64;

struct Arena {
    data: NonNull<u8>,
    head: AtomicUsize,
}

impl Arena {
    fn new() -> Option<Self> {
        let layout = Layout::from_size_align(ARENA_SIZE, ALIGNMENT).ok()?;
        // SAFETY: Layout is non-zero and valid; ownership is retained and freed in Drop.
        let data = NonNull::new(unsafe { alloc_zeroed(layout) })?;
        Some(Self {
            data,
            head: AtomicUsize::new(0),
        })
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(ARENA_SIZE, ALIGNMENT).expect("fixed arena layout");
        // SAFETY: This pointer was allocated with the same layout in Arena::new.
        unsafe { dealloc(self.data.as_ptr(), layout) };
    }
}

// The arena bytes are disjoint between successful bump allocations. The head
// counters synchronize concurrent allocation; the RT caller owns each result.
unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

struct RealtimeMemoryPool {
    arenas: [Arena; ARENA_COUNT],
    frame_index: AtomicU32,
}

// SAFETY: all shared mutation is limited to atomic counters; arena storage is
// handed out only through unique bump-allocated ranges until the caller resets.
unsafe impl Send for RealtimeMemoryPool {}
unsafe impl Sync for RealtimeMemoryPool {}

impl RealtimeMemoryPool {
    fn new() -> Option<Self> {
        let mut arenas = Vec::new();
        arenas.try_reserve_exact(ARENA_COUNT).ok()?;
        for _ in 0..ARENA_COUNT {
            arenas.push(Arena::new()?);
        }
        let arenas = arenas.try_into().ok().expect("exact arena count");
        Some(Self {
            arenas,
            frame_index: AtomicU32::new(0),
        })
    }

    fn reset(&self, frame_index: u32) {
        let frame = frame_index as usize % ARENA_COUNT;
        self.arenas[frame].head.store(0, Ordering::Release);
        self.frame_index.store(frame as u32, Ordering::Release);
    }

    fn allocate(&self, size_bytes: usize) -> *mut u8 {
        if size_bytes == 0 || size_bytes > ARENA_SIZE || size_bytes > usize::MAX - (ALIGNMENT - 1) {
            return std::ptr::null_mut();
        }
        let frame = self.frame_index.load(Ordering::Acquire) as usize;
        let aligned_size = (size_bytes + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let head = &self.arenas[frame].head;
        let mut current = head.load(Ordering::Relaxed);
        loop {
            if current > ARENA_SIZE - aligned_size {
                return std::ptr::null_mut();
            }
            match head.compare_exchange_weak(
                current,
                current + aligned_size,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // SAFETY: successful atomic reservation keeps this aligned range
                    // within the arena and distinct from concurrent allocations.
                    return unsafe { self.arenas[frame].data.as_ptr().add(current) };
                }
                Err(observed) => current = observed,
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_realtime_memory_pool_create() -> *mut c_void {
    let Some(pool) = RealtimeMemoryPool::new() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(pool)).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_realtime_memory_pool_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: Handle comes from the matching constructor and is destroyed once.
        drop(Box::from_raw(state.cast::<RealtimeMemoryPool>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_realtime_memory_pool_reset(state: *const c_void, frame: u32) {
    if let Some(state) = state.cast::<RealtimeMemoryPool>().as_ref() {
        state.reset(frame);
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_realtime_memory_pool_allocate(
    state: *const c_void,
    size_bytes: usize,
) -> *mut c_void {
    state
        .cast::<RealtimeMemoryPool>()
        .as_ref()
        .map_or(std::ptr::null_mut(), |state| {
            state.allocate(size_bytes).cast()
        })
}

#[cfg(test)]
mod tests {
    use super::{RealtimeMemoryPool, ALIGNMENT, ARENA_SIZE};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn arena_allocations_are_aligned_bounded_and_resettable() {
        let pool = RealtimeMemoryPool::new().unwrap();
        let first = pool.allocate(1);
        let second = pool.allocate(65);
        assert!(!first.is_null() && !second.is_null());
        assert_eq!(first as usize % ALIGNMENT, 0);
        assert_eq!(second as usize % ALIGNMENT, 0);
        assert_eq!(second as usize - first as usize, ALIGNMENT);
        assert!(pool.allocate(ARENA_SIZE).is_null());
        assert!(pool.allocate(0).is_null());
        pool.reset(0);
        assert_eq!(pool.allocate(1), first);
    }

    #[test]
    fn concurrent_allocations_reserve_distinct_cache_lines() {
        let pool = Arc::new(RealtimeMemoryPool::new().unwrap());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let pool = Arc::clone(&pool);
                thread::spawn(move || {
                    (0..256)
                        .map(|_| pool.allocate(32) as usize)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let pointers: Vec<_> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(pointers.len(), 2048);
        let unique: HashSet<_> = pointers.iter().copied().collect();
        assert_eq!(unique.len(), pointers.len());
        assert!(pointers.iter().all(|pointer| pointer % ALIGNMENT == 0));
    }
}
