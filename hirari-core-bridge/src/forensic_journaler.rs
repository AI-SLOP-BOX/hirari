use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
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
    waiter: Mutex<Option<Waiter>>,
}
#[derive(Clone, Copy)]
struct Waiter {
    context: usize,
    load: unsafe extern "C" fn(*mut c_void) -> u64,
    wait: unsafe extern "C" fn(*mut c_void, u64),
}
pub struct Journaler { inner: Arc<Inner>, writer: Mutex<Option<JoinHandle<()>>> }

fn stop(journaler: &Journaler) {
    journaler.inner.running.store(false, Ordering::Release);
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
            let waiter = inner.waiter.lock().unwrap_or_else(|e| e.into_inner()).as_ref().copied();
            if let Some(waiter) = waiter {
                let observed = unsafe { (waiter.load)(waiter.context as *mut c_void) };
                if inner.running.load(Ordering::Acquire) && inner.queue.is_empty() {
                    unsafe { (waiter.wait)(waiter.context as *mut c_void, observed); }
                }
            }
        }
    }
}

impl Journaler {
    fn start(&self, path: Vec<u8>, waiter: Waiter) -> bool {
        stop(self);
        let Some(path) = ({
            #[cfg(unix)] { Some(path_from_bytes(&path)) }
            #[cfg(not(unix))] { path_from_bytes(&path) }
        }) else { return false };
        if let Ok(mut current_waiter) = self.inner.waiter.lock() { *current_waiter = Some(waiter); }
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
            waiter: Mutex::new(None),
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
    state: *mut c_void, path: *const u8, len: usize, wait_context: *mut c_void,
    load_sequence: Option<unsafe extern "C" fn(*mut c_void) -> u64>,
    wait_sequence: Option<unsafe extern "C" fn(*mut c_void, u64)>,
) -> bool {
    let Some(journaler) = state.cast::<Journaler>().as_ref() else { return false };
    if len > 0 && path.is_null() { return false; }
    let (Some(load), Some(wait)) = (load_sequence, wait_sequence) else { return false };
    journaler.start(if len == 0 { Vec::new() } else { std::slice::from_raw_parts(path, len).to_vec() },
        Waiter { context: wait_context as usize, load, wait })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_log(
    state: *const c_void, kind: u32, track_id: u32, value: f32, timestamp: u64,
) -> bool {
    let Some(journaler) = state.cast::<Journaler>().as_ref() else { return false };
    let pushed = journaler.inner.queue.push(Entry { kind, track_id, value, timestamp });
    pushed
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_log_description(
    state: *const c_void,
    kind: u32,
    description: *const u8,
    description_len: usize,
    value: f32,
) -> bool {
    if description_len > 0 && description.is_null() {
        return false;
    }
    let bytes = if description_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(description, description_len) }
    };
    let hash = bytes.iter().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
    });
    unsafe { hirari_forensic_journaler_log(state, kind, hash, value, 0) }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_stop(state: *mut c_void) {
    if let Some(journaler) = state.cast::<Journaler>().as_ref() { stop(journaler); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_forensic_journaler_recover(
    path: *const u8, path_len: usize, context: *mut c_void,
    callback: Option<unsafe extern "C" fn(*mut c_void, u32, u32, f32, u64)>,
) -> bool {
    if path_len > 0 && path.is_null() { return false; }
    let bytes = if path_len == 0 { &[] } else { std::slice::from_raw_parts(path, path_len) };
    let Some(callback) = callback else { return false };
    #[cfg(unix)]
    let path = path_from_bytes(bytes);
    #[cfg(not(unix))]
    let Some(path) = path_from_bytes(bytes) else { return false };
    let Ok(mut file) = File::open(path) else { return true };
    loop {
        let mut row = [0u8; ENTRY_SIZE];
        match file.read_exact(&mut row) {
            Ok(()) => {
                let entry = Entry {
                    kind: u32::from_ne_bytes(row[0..4].try_into().unwrap()),
                    track_id: u32::from_ne_bytes(row[4..8].try_into().unwrap()),
                    value: f32::from_ne_bytes(row[8..12].try_into().unwrap()),
                    timestamp: u64::from_ne_bytes(row[16..24].try_into().unwrap()),
                };
                callback(context, entry.kind, entry.track_id, entry.value, entry.timestamp);
            }
            Err(_) => return true,
        }
    }
}
