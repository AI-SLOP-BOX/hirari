use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type TaskDispatch = unsafe extern "C" fn(*mut c_void);

#[derive(Clone, Copy)]
struct QueuedTask {
    data: *mut c_void,
    dispatch: TaskDispatch,
}

// The pointer identifies a C++ job that remains alive until dequeue. Rust only
// moves the opaque token through the queue and never dereferences it.
unsafe impl Send for QueuedTask {}

struct ParallelWork {
    end: u32,
    next: std::sync::atomic::AtomicU64,
    callback: unsafe extern "C" fn(u32, *mut c_void),
    context: *mut c_void,
    workers_active: std::sync::atomic::AtomicUsize,
}

unsafe impl Send for ParallelWork {}

unsafe extern "C" fn dispatch_parallel_work(data: *mut c_void) {
    let Some(work) = (unsafe { data.cast::<ParallelWork>().as_ref() }) else {
        return;
    };
    loop {
        let index = work.next.fetch_add(1, Ordering::Relaxed);
        if index >= u64::from(work.end) {
            break;
        }
        unsafe { (work.callback)(index as u32, work.context) };
    }
    work.workers_active.fetch_sub(1, Ordering::Release);
}

struct WorkQueue {
    capacity: usize,
    tasks: Mutex<VecDeque<QueuedTask>>,
}

struct SchedulerRun {
    queues: Vec<Arc<WorkQueue>>,
    running: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
    dispatch: TaskDispatch,
}

impl SchedulerRun {
    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for SchedulerRun {
    fn drop(&mut self) {
        self.stop();
    }
}

struct AudioTaskScheduler {
    run: Mutex<Option<SchedulerRun>>,
}

impl AudioTaskScheduler {
    fn new() -> Self {
        Self {
            run: Mutex::new(None),
        }
    }

    fn start(
        &self,
        worker_count: usize,
        queue_capacity: usize,
        dispatch: unsafe extern "C" fn(*mut c_void),
        enter: unsafe extern "C" fn(u32) -> u64,
        leave: unsafe extern "C" fn(u64),
    ) -> bool {
        self.stop();
        let worker_count = worker_count.max(1);
        let mut queues = Vec::new();
        if queues.try_reserve_exact(worker_count).is_err() {
            return false;
        }
        for _ in 0..worker_count {
            let Some(queue) = WorkQueue::new(queue_capacity) else {
                return false;
            };
            queues.push(Arc::new(queue));
        }
        let running = Arc::new(AtomicBool::new(true));
        let mut workers = Vec::new();
        if workers.try_reserve_exact(worker_count).is_err() {
            return false;
        }
        for index in 0..worker_count {
            let queues = queues.clone();
            let worker_running = Arc::clone(&running);
            match thread::Builder::new()
                .name(format!("hirari-audio-{index}"))
                .spawn(move || {
                    // SAFETY: These C++ hooks configure only the current worker thread.
                    let fp_state = unsafe { enter(index as u32) };
                    worker_loop(index, &queues, &worker_running);
                    unsafe { leave(fp_state) };
                }) {
                Ok(worker) => workers.push(worker),
                Err(_) => {
                    running.store(false, Ordering::Release);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return false;
                }
            }
        }
        let Ok(mut run) = self.run.lock() else {
            running.store(false, Ordering::Release);
            for worker in workers {
                let _ = worker.join();
            }
            return false;
        };
        *run = Some(SchedulerRun {
            queues,
            running,
            workers,
            dispatch,
        });
        true
    }

    fn stop(&self) {
        let Ok(mut run) = self.run.lock() else {
            return;
        };
        if let Some(mut active) = run.take() {
            active.running.store(false, Ordering::Release);
            drop(run);
            for worker in active.workers.drain(..) {
                let _ = worker.join();
            }
        }
    }

    fn thread_count(&self) -> usize {
        self.run
            .lock()
            .ok()
            .and_then(|run| run.as_ref().map(|active| active.queues.len()))
            .unwrap_or(0)
    }

    fn is_running(&self) -> bool {
        self.run
            .lock()
            .ok()
            .and_then(|run| {
                run.as_ref()
                    .map(|active| active.running.load(Ordering::Acquire))
            })
            .unwrap_or(false)
    }

    fn push(&self, worker: usize, data: *mut c_void) -> bool {
        let dispatch = self
            .run
            .lock()
            .ok()
            .and_then(|run| run.as_ref().map(|active| active.dispatch));
        dispatch.is_some_and(|dispatch| self.push_with_dispatch(worker, data, dispatch))
    }

    fn push_with_dispatch(&self, worker: usize, data: *mut c_void, dispatch: TaskDispatch) -> bool {
        if data.is_null() {
            return false;
        }
        let Ok(run) = self.run.lock() else {
            return false;
        };
        let Some(active) = run
            .as_ref()
            .filter(|active| active.running.load(Ordering::Acquire))
        else {
            return false;
        };
        active
            .queues
            .get(worker % active.queues.len())
            .is_some_and(|queue| queue.push(QueuedTask { data, dispatch }))
    }

    fn steal(&self, worker: usize) -> Option<*mut c_void> {
        let queue = self.run.lock().ok().and_then(|run| {
            run.as_ref()
                .filter(|active| active.running.load(Ordering::Acquire))
                .and_then(|active| active.queues.get(worker).cloned())
        })?;
        queue.pop(true).map(|task| task.data)
    }

    fn parallel_for(
        &self,
        start: u32,
        end: u32,
        context: *mut c_void,
        callback: unsafe extern "C" fn(u32, *mut c_void),
    ) {
        if start >= end {
            return;
        }
        let queues = self.run.lock().ok().and_then(|run| {
            run.as_ref()
                .filter(|active| active.running.load(Ordering::Acquire))
                .map(|active| active.queues.clone())
        });
        let Some(queues) = queues.filter(|queues| !queues.is_empty()) else {
            for index in start..end {
                unsafe { callback(index, context) };
            }
            return;
        };

        let work = ParallelWork {
            end,
            next: std::sync::atomic::AtomicU64::new(u64::from(start)),
            callback,
            context,
            workers_active: std::sync::atomic::AtomicUsize::new(queues.len()),
        };
        let work_ptr = (&work as *const ParallelWork).cast_mut().cast();
        for worker in 0..queues.len() {
            if !self.push_with_dispatch(worker, work_ptr, dispatch_parallel_work) {
                work.workers_active.fetch_sub(1, Ordering::Release);
            }
        }

        // The caller participates as a worker while the persistent pool runs
        // the same atomic work cursor. The stack context stays alive until all
        // queued callbacks have returned.
        loop {
            let index = work.next.fetch_add(1, Ordering::Relaxed);
            if index >= u64::from(end) {
                break;
            }
            unsafe { callback(index as u32, context) };
        }
        while work.workers_active.load(Ordering::Acquire) != 0 {
            thread::yield_now();
        }
    }
}

#[no_mangle]
pub extern "C" fn hirari_audio_scheduler_create() -> *mut c_void {
    Box::into_raw(Box::new(AudioTaskScheduler::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_destroy(state: *mut c_void) {
    if !state.is_null() {
        // SAFETY: State is created above and its owner destroys it exactly once.
        drop(Box::from_raw(state.cast::<AudioTaskScheduler>()));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_start(
    state: *const c_void,
    workers: usize,
    queue_capacity: usize,
    dispatch: Option<unsafe extern "C" fn(*mut c_void)>,
    enter: Option<unsafe extern "C" fn(u32) -> u64>,
    leave: Option<unsafe extern "C" fn(u64)>,
) -> bool {
    let (Some(scheduler), Some(dispatch), Some(enter), Some(leave)) = (
        state.cast::<AudioTaskScheduler>().as_ref(),
        dispatch,
        enter,
        leave,
    ) else {
        return false;
    };
    scheduler.start(workers, queue_capacity, dispatch, enter, leave)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_stop(state: *const c_void) {
    if let Some(scheduler) = state.cast::<AudioTaskScheduler>().as_ref() {
        scheduler.stop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_thread_count(state: *const c_void) -> usize {
    state
        .cast::<AudioTaskScheduler>()
        .as_ref()
        .map_or(0, AudioTaskScheduler::thread_count)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_is_running(state: *const c_void) -> bool {
    state
        .cast::<AudioTaskScheduler>()
        .as_ref()
        .is_some_and(|scheduler| scheduler.is_running())
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_push(
    state: *const c_void,
    worker: usize,
    data: *mut c_void,
) -> bool {
    state
        .cast::<AudioTaskScheduler>()
        .as_ref()
        .is_some_and(|scheduler| scheduler.push(worker, data))
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_steal(
    state: *const c_void,
    worker: usize,
    data: *mut *mut c_void,
) -> bool {
    let (Some(scheduler), Some(data)) =
        (state.cast::<AudioTaskScheduler>().as_ref(), data.as_mut())
    else {
        return false;
    };
    let Some(task) = scheduler.steal(worker) else {
        return false;
    };
    *data = task;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_audio_scheduler_parallel_for(
    state: *const c_void,
    start: u32,
    end: u32,
    context: *mut c_void,
    callback: Option<unsafe extern "C" fn(u32, *mut c_void)>,
) {
    let (Some(scheduler), Some(callback)) = (state.cast::<AudioTaskScheduler>().as_ref(), callback)
    else {
        return;
    };
    scheduler.parallel_for(start, end, context, callback);
}

fn worker_loop(index: usize, queues: &[Arc<WorkQueue>], running: &AtomicBool) {
    loop {
        if let Some(task) = queues[index].pop(false) {
            // SAFETY: C++ enqueues only live AudioTaskJob objects and this callback
            // dispatches the task before its owning arena or heap job is retired.
            unsafe { (task.dispatch)(task.data) };
            continue;
        }

        let mut stole = false;
        for offset in 1..queues.len() {
            if let Some(task) = queues[(index + offset) % queues.len()].pop(true) {
                unsafe { (task.dispatch)(task.data) };
                stole = true;
                break;
            }
        }
        if stole {
            continue;
        }

        for _ in 0..64 {
            if !running.load(Ordering::Relaxed) {
                break;
            }
            thread::yield_now();
            if let Some(task) = queues[index].pop(true) {
                unsafe { (task.dispatch)(task.data) };
                stole = true;
                break;
            }
        }
        if !stole {
            if !running.load(Ordering::Relaxed)
                && queues
                    .iter()
                    .all(|queue| queue.tasks.lock().map_or(true, |tasks| tasks.is_empty()))
            {
                break;
            }
            if running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

impl WorkQueue {
    fn new(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            return None;
        }
        let mut tasks = VecDeque::new();
        tasks.try_reserve_exact(capacity).ok()?;
        Some(Self {
            capacity,
            tasks: Mutex::new(tasks),
        })
    }

    fn push(&self, task: QueuedTask) -> bool {
        let Ok(mut tasks) = self.tasks.lock() else {
            return false;
        };
        if tasks.len() >= self.capacity {
            return false;
        }
        tasks.push_back(task);
        true
    }

    fn pop(&self, steal: bool) -> Option<QueuedTask> {
        let Ok(mut tasks) = self.tasks.lock() else {
            return None;
        };
        if steal {
            tasks.pop_front()
        } else {
            tasks.pop_back()
        }
    }
}

#[cfg(test)]
mod tests {
    unsafe extern "C" fn noop_dispatch(_: *mut c_void) {}
    use super::WorkQueue;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    unsafe extern "C" {
        fn hirari_audio_task_scheduler_cpp_smoke() -> bool;
        fn hirari_rust_thread_pool_cpp_smoke() -> bool;
        fn hirari_scae_stem_pipeline_cpp_smoke() -> bool;
    }

    #[test]
    fn owner_pops_lifo_and_stealers_take_oldest_first() {
        let queue = WorkQueue::new(4).unwrap();
        let tokens = [1usize, 2, 3];
        for token in tokens {
            assert!(queue.push(super::QueuedTask {
                data: token as *mut c_void,
                dispatch: noop_dispatch,
            }));
        }
        assert_eq!(queue.pop(true).unwrap().0 as usize, 1);
        assert_eq!(queue.pop(false).unwrap().0 as usize, 3);
        assert_eq!(queue.pop(false).unwrap().0 as usize, 2);
        assert!(queue.pop(false).is_none());
    }

    #[test]
    fn bounded_queue_rejects_full_and_supports_concurrent_producers() {
        let queue = Arc::new(WorkQueue::new(128).unwrap());
        let accepted = Arc::new(AtomicUsize::new(0));
        let producers: Vec<_> = (0..4)
            .map(|_| {
                let queue = Arc::clone(&queue);
                let accepted = Arc::clone(&accepted);
                thread::spawn(move || {
                    for _ in 0..32 {
                        if queue.push(super::QueuedTask {
                            data: Arc::as_ptr(&queue) as *mut c_void,
                            dispatch: noop_dispatch,
                        }) {
                            accepted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().unwrap();
        }
        assert_eq!(accepted.load(Ordering::Relaxed), 128);
        assert!(!queue.push(super::QueuedTask {
            data: 1usize as *mut c_void,
            dispatch: noop_dispatch,
        }));
    }

    #[test]
    fn cpp_scheduler_executes_parallel_for_through_rust_queues() {
        // SAFETY: The helper starts, drains, and joins its local singleton worker set.
        assert!(unsafe { hirari_audio_task_scheduler_cpp_smoke() });
    }

    #[test]
    fn cpp_future_thread_pool_runs_and_propagates_exceptions_via_rust_workers() {
        // SAFETY: The smoke owns all submitted work and waits for each future.
        assert!(unsafe { hirari_rust_thread_pool_cpp_smoke() });
    }

    #[test]
    fn scae_stem_pipeline_runs_on_rust_owned_background_workers() {
        // SAFETY: The fixture waits for its callback and owns the input through completion.
        assert!(unsafe { hirari_scae_stem_pipeline_cpp_smoke() });
    }
}
