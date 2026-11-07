use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

const QUEUE_SIZE: u32 = 4096;
const ENTRY_SIZE: usize = 24;

#[repr(align(64))]
struct CacheAligned<T>(T);

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Entry { kind: u32, track_id: u32, value: f32, timestamp: u64 }
const _: () = assert!(std::mem::size_of::<Entry>() == ENTRY_SIZE);

struct Queue {
    slots: Box<[UnsafeCell<MaybeUninit<Entry>>]>,
    write: CacheAligned<AtomicU32>,
    read: CacheAligned<AtomicU32>,
}
unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Queue {
    fn new() -> Self {
        let slots = (0..QUEUE_SIZE).map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>().into_boxed_slice();
        Self { slots, write: CacheAligned(AtomicU32::new(0)), read: CacheAligned(AtomicU32::new(0)) }
    }
    fn push(&self, entry: Entry) -> bool {
        let write = self.write.0.load(Ordering::Relaxed);
        let read = self.read.0.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= QUEUE_SIZE { return false; }
        unsafe { (*self.slots[(write & (QUEUE_SIZE - 1)) as usize].get()).write(entry); }
        self.write.0.store(write.wrapping_add(1), Ordering::Release);
        true
    }
    fn pop(&self) -> Option<Entry> {
        let read = self.read.0.load(Ordering::Relaxed);
        let write = self.write.0.load(Ordering::Acquire);
        if read == write { return None; }
        let entry = unsafe { (*self.slots[(read & (QUEUE_SIZE - 1)) as usize].get()).assume_init_read() };
        self.read.0.store(read.wrapping_add(1), Ordering::Release);
        Some(entry)
    }
    fn is_empty(&self) -> bool {
        self.read.0.load(Ordering::Relaxed) == self.write.0.load(Ordering::Acquire)
    }
}

struct Inner {
    queue: Queue,
    running: AtomicBool,
    // Idle-writer wake pair. The writer checks `running`/queue emptiness while
    // holding `wake`, so a wake published after the flag is visible can never
    // be lost: the waker must acquire the same mutex before notifying.
    wake: Mutex<u64>,
    wake_signal: Condvar,
}
pub struct Journaler { inner: Arc<Inner>, writer: Mutex<Option<JoinHandle<()>>> }

fn wake_writer(inner: &Inner) {
    let mut sequence = inner.wake.lock().unwrap_or_else(|error| error.into_inner());
    *sequence = sequence.wrapping_add(1);
    inner.wake_signal.notify_all();
}

fn stop(journaler: &Journaler) {
    journaler.inner.running.store(false, Ordering::Release);
    wake_writer(&journaler.inner);
    if let Ok(mut writer) = journaler.writer.lock() {
        if let Some(handle) = writer.take() { let _ = handle.join(); }
    }
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Option<std::path::PathBuf> {
    std::str::from_utf8(bytes).ok().map(std::path::PathBuf::from)
}

fn write_entry(file: &mut File, entry: Entry) -> std::io::Result<()> {
    let mut bytes = [0u8; ENTRY_SIZE];
    bytes[0..4].copy_from_slice(&entry.kind.to_ne_bytes());
    bytes[4..8].copy_from_slice(&entry.track_id.to_ne_bytes());
    bytes[8..12].copy_from_slice(&entry.value.to_ne_bytes());
    bytes[16..24].copy_from_slice(&entry.timestamp.to_ne_bytes());
    file.write_all(&bytes)?;
    file.flush()
}

fn writer_loop(inner: Arc<Inner>, path: std::path::PathBuf) {
    let mut file = OpenOptions::new().create(true).append(true).open(path).ok();
    while inner.running.load(Ordering::Acquire) || !inner.queue.is_empty() {
        if let Some(entry) = inner.queue.pop() {
            if let Some(file) = file.as_mut() { let _ = write_entry(file, entry); }
        } else {
            let sequence = inner.wake.lock().unwrap_or_else(|error| error.into_inner());
            if inner.running.load(Ordering::Acquire) && inner.queue.is_empty() {
                let _resumed = inner.wake_signal.wait(sequence);
            }
        }
    }
}

impl Journaler {
    fn start(&self, path: Vec<u8>) -> bool {
        stop(self);
        let Some(path) = ({
            #[cfg(unix)] { Some(path_from_bytes(&path)) }
            #[cfg(not(unix))] { path_from_bytes(&path) }
        }) else { return false };
        self.inner.running.store(true, Ordering::Release);
        let inner = Arc::clone(&self.inner);
        let handle = match thread::Builder::new().spawn(move || writer_loop(inner, path)) {
            Ok(handle) => handle,
            Err(_) => { self.inner.running.store(false, Ordering::Release); return false; }
        };
        if let Ok(mut writer) = self.writer.lock() { *writer = Some(handle); }
        true
    }
}

impl Drop for Journaler { fn drop(&mut self) { stop(self); } }

#[no_mangle]
pub extern "C" fn hirari_forensic_journaler_create() -> *mut c_void {
    Box::into_raw(Box::new(Journaler {
        inner: Arc::new(Inner {
            queue: Queue::new(), running: AtomicBool::new(false),
            wake: Mutex::new(0), wake_signal: Condvar::new(),
        }),
        writer: Mutex::new(None),
    })).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<Journaler>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_start(
    state: *mut c_void, path: *const u8, len: usize,
) -> bool {
    let Some(journaler) = state.cast::<Journaler>().as_ref() else { return false };
    if len > 0 && path.is_null() { return false; }
    journaler.start(if len == 0 { Vec::new() } else { std::slice::from_raw_parts(path, len).to_vec() })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_log(
    state: *const c_void, kind: u32, track_id: u32, value: f32, timestamp: u64,
) -> bool {
    let Some(journaler) = state.cast::<Journaler>().as_ref() else { return false };
    let pushed = journaler.inner.queue.push(Entry { kind, track_id, value, timestamp });
    if pushed {
        wake_writer(&journaler.inner);
    }
    pushed
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_stop(state: *mut c_void) {
    if let Some(journaler) = state.cast::<Journaler>().as_ref() { stop(journaler); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn stop_wakes_an_idle_writer() {
        let path = std::env::temp_dir().join(format!(
            "hirari-journaler-stop-{}-{:?}.journal",
            std::process::id(),
            std::thread::current().id()
        ));
        let path_bytes = path.to_string_lossy().into_owned();
        let state = hirari_forensic_journaler_create();
        let started = unsafe {
            hirari_forensic_journaler_start(
                state,
                path_bytes.as_bytes().as_ptr(),
                path_bytes.len(),
            )
        };
        assert!(started);

        // Give the writer time to drain the empty queue and park in wait().
        // Shutdown must wake it explicitly, since no log call follows.
        std::thread::sleep(Duration::from_millis(100));
        let (done, finished) = mpsc::channel();
        let address = state as usize;
        std::thread::spawn(move || {
            unsafe { hirari_forensic_journaler_stop(address as *mut c_void) };
            let _ = done.send(());
        });
        if finished.recv_timeout(Duration::from_secs(5)).is_err() {
            // Leak the state on failure: destroying it while the writer is
            // still parked would hand a dangling pointer to the still-running
            // stop thread.
            panic!("stop() did not wake the idle writer");
        }

        unsafe { hirari_forensic_journaler_destroy(state) };
        let _ = std::fs::remove_file(&path);
    }
}
