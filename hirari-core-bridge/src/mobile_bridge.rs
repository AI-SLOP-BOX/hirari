use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const QUEUE_SIZE: u32 = 1024;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TouchEvent { track_id: u32, param_id: u32, value: f32, timestamp: u64 }

#[repr(align(64))]
struct Aligned<T>(T);

struct SpscQueue {
    slots: Box<[UnsafeCell<MaybeUninit<TouchEvent>>]>,
    write: Aligned<AtomicU32>,
    read: Aligned<AtomicU32>,
}
unsafe impl Send for SpscQueue {}
unsafe impl Sync for SpscQueue {}

impl SpscQueue {
    fn new() -> Self {
        let slots = (0..QUEUE_SIZE).map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect::<Vec<_>>().into_boxed_slice();
        Self { slots, write: Aligned(AtomicU32::new(0)), read: Aligned(AtomicU32::new(0)) }
    }
    fn push(&self, event: TouchEvent) -> bool {
        let write = self.write.0.load(Ordering::Relaxed);
        let read = self.read.0.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= QUEUE_SIZE { return false; }
        unsafe { (*self.slots[(write & (QUEUE_SIZE - 1)) as usize].get()).write(event); }
        self.write.0.store(write.wrapping_add(1), Ordering::Release);
        true
    }
    fn pop(&self) -> Option<TouchEvent> {
        let read = self.read.0.load(Ordering::Relaxed);
        let write = self.write.0.load(Ordering::Acquire);
        if read == write { return None; }
        let event = unsafe { (*self.slots[(read & (QUEUE_SIZE - 1)) as usize].get()).assume_init_read() };
        self.read.0.store(read.wrapping_add(1), Ordering::Release);
        Some(event)
    }
}

struct MobileBridge {
    socket: libc::c_int,
    address: libc::sockaddr_in,
    incoming: SpscQueue,
    bytes_sent: AtomicU64,
}

impl MobileBridge {
    fn new() -> Self {
        let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        address.sin_family = libc::AF_INET as libc::sa_family_t;
        address.sin_port = 9000u16.to_be();
        address.sin_addr.s_addr = libc::INADDR_BROADCAST;
        if socket >= 0 {
            unsafe {
                let flags = libc::fcntl(socket, libc::F_GETFL, 0);
                libc::fcntl(socket, libc::F_SETFL, flags | libc::O_NONBLOCK);
                let enabled: libc::c_int = 1;
                libc::setsockopt(socket, libc::SOL_SOCKET, libc::SO_BROADCAST,
                    (&enabled as *const libc::c_int).cast(), std::mem::size_of_val(&enabled) as libc::socklen_t);
            }
        }
        Self { socket, address, incoming: SpscQueue::new(), bytes_sent: AtomicU64::new(0) }
    }

    fn broadcast(&self, track_id: u32, param_id: u32, value: f32) {
        let mut packet = [0u8; 12];
        packet[0..4].copy_from_slice(&track_id.to_ne_bytes());
        packet[4..8].copy_from_slice(&param_id.to_ne_bytes());
        packet[8..12].copy_from_slice(&value.to_ne_bytes());
        if self.socket >= 0 {
            unsafe {
                libc::sendto(self.socket, packet.as_ptr().cast(), packet.len(), 0,
                    (&self.address as *const libc::sockaddr_in).cast(),
                    std::mem::size_of_val(&self.address) as libc::socklen_t);
            }
        }
        self.bytes_sent.fetch_add(packet.len() as u64, Ordering::SeqCst);
    }
}

impl Drop for MobileBridge {
    fn drop(&mut self) { if self.socket >= 0 { unsafe { libc::close(self.socket); } } }
}

#[no_mangle]
pub extern "C" fn hirari_mobile_bridge_create() -> *mut c_void {
    Box::into_raw(Box::new(MobileBridge::new())).cast()
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mobile_bridge_destroy(state: *mut c_void) {
    if !state.is_null() { drop(Box::from_raw(state.cast::<MobileBridge>())); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mobile_bridge_broadcast(
    state: *const c_void, track_id: u32, param_id: u32, value: f32,
) {
    if let Some(state) = state.cast::<MobileBridge>().as_ref() { state.broadcast(track_id, param_id, value); }
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mobile_bridge_push_event(
    state: *const c_void, track_id: u32, param_id: u32, value: f32, timestamp: u64,
) -> bool {
    let Some(state) = state.cast::<MobileBridge>().as_ref() else { return false };
    state.incoming.push(TouchEvent { track_id, param_id, value, timestamp })
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mobile_bridge_pop_event(
    state: *const c_void, track_id: *mut u32, param_id: *mut u32,
    value: *mut f32, timestamp: *mut u64,
) -> bool {
    let Some(state) = state.cast::<MobileBridge>().as_ref() else { return false };
    if track_id.is_null() || param_id.is_null() || value.is_null() || timestamp.is_null() { return false; }
    let Some(event) = state.incoming.pop() else { return false };
    *track_id = event.track_id; *param_id = event.param_id;
    *value = event.value; *timestamp = event.timestamp;
    true
}

#[no_mangle]
pub unsafe extern "C" fn hirari_mobile_bridge_bytes_sent(state: *const c_void) -> u64 {
    state.cast::<MobileBridge>().as_ref().map_or(0, |state| state.bytes_sent.load(Ordering::SeqCst))
}
