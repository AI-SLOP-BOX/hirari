use std::cell::UnsafeCell;
use std::ffi::c_char;
use std::fmt::{self, Write};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU32, Ordering};

const QUEUE_CAPACITY: u32 = 256;
const MESSAGE_CAPACITY: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HirariAdvicePacket {
    pub level: u32,
    pub message: [c_char; MESSAGE_CAPACITY],
    pub timestamp: u64,
}

#[repr(align(64))]
struct CacheLineAtomic(AtomicU32);

struct AdviceQueue {
    buffer: [UnsafeCell<MaybeUninit<HirariAdvicePacket>>; QUEUE_CAPACITY as usize],
    write: CacheLineAtomic,
    read: CacheLineAtomic,
}

unsafe impl Sync for AdviceQueue {}

impl AdviceQueue {
    const fn new() -> Self {
        Self {
            buffer: [const { UnsafeCell::new(MaybeUninit::uninit()) }; QUEUE_CAPACITY as usize],
            write: CacheLineAtomic(AtomicU32::new(0)),
            read: CacheLineAtomic(AtomicU32::new(0)),
        }
    }

    fn push(&self, packet: HirariAdvicePacket) -> bool {
        let write = self.write.0.load(Ordering::Relaxed);
        let read = self.read.0.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= QUEUE_CAPACITY {
            return false;
        }
        unsafe {
            (*self.buffer[(write & (QUEUE_CAPACITY - 1)) as usize].get()).write(packet);
        }
        self.write.0.store(write.wrapping_add(1), Ordering::Release);
        true
    }

    fn pop(&self) -> Option<HirariAdvicePacket> {
        let read = self.read.0.load(Ordering::Relaxed);
        let write = self.write.0.load(Ordering::Acquire);
        if read == write {
            return None;
        }
        let packet = unsafe {
            (*self.buffer[(read & (QUEUE_CAPACITY - 1)) as usize].get()).assume_init_read()
        };
        self.read.0.store(read.wrapping_add(1), Ordering::Release);
        Some(packet)
    }
}

static ADVICE_QUEUE: AdviceQueue = AdviceQueue::new();

struct FixedMessage {
    bytes: [u8; MESSAGE_CAPACITY - 1],
    length: usize,
}

impl FixedMessage {
    fn new() -> Self {
        Self {
            bytes: [0; MESSAGE_CAPACITY - 1],
            length: 0,
        }
    }

    fn into_c_chars(self) -> [c_char; MESSAGE_CAPACITY] {
        let mut result = [0; MESSAGE_CAPACITY];
        for (destination, source) in result.iter_mut().zip(self.bytes) {
            *destination = source as c_char;
        }
        result
    }
}

impl Write for FixedMessage {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.bytes.len().saturating_sub(self.length);
        let count = value.len().min(available);
        self.bytes[self.length..self.length + count].copy_from_slice(&value.as_bytes()[..count]);
        self.length += count;
        Ok(())
    }
}

fn make_advice(lufs_integrated: f32, true_peak: f32, timestamp: u64) -> HirariAdvicePacket {
    let (level, message) = if true_peak > -0.1 {
        let mut message = FixedMessage::new();
        let _ = write!(
            message,
            "TRUE PEAK VIOLATION: Digital clipping imminent at {true_peak:.1} dBTP."
        );
        (2, message)
    } else if lufs_integrated > -14.0 {
        let mut message = FixedMessage::new();
        let _ = write!(
            message,
            "LOUDNESS OVERAGE: Integrated LUFS ({lufs_integrated:.1}) exceeds streaming targets."
        );
        (1, message)
    } else {
        let mut message = FixedMessage::new();
        let _ = message.write_str("SIGNAL COMPLIANT: EBU R128 tolerances maintained.");
        (0, message)
    };
    HirariAdvicePacket {
        level,
        message: message.into_c_chars(),
        timestamp,
    }
}

/// Creates and enqueues a fixed-size analysis packet without heap allocation.
#[no_mangle]
pub extern "C" fn hirari_neural_advice_evaluate(
    lufs_integrated: f32,
    true_peak: f32,
    timestamp: u64,
) -> bool {
    ADVICE_QUEUE.push(make_advice(lufs_integrated, true_peak, timestamp))
}

/// Pops one packet from the single-producer/single-consumer advice queue.
#[no_mangle]
pub unsafe extern "C" fn hirari_neural_advice_pop(output: *mut HirariAdvicePacket) -> bool {
    if output.is_null() {
        return false;
    }
    let Some(packet) = ADVICE_QUEUE.pop() else {
        return false;
    };
    output.write(packet);
    true
}

#[cfg(test)]
mod tests {
    use super::{make_advice, AdviceQueue, HirariAdvicePacket};
    use std::ffi::CStr;

    fn text(packet: &HirariAdvicePacket) -> &str {
        unsafe { CStr::from_ptr(packet.message.as_ptr()) }
            .to_str()
            .unwrap()
    }

    #[test]
    fn advice_thresholds_and_messages_match_legacy_policy() {
        let critical = make_advice(-20.0, -0.05, 17);
        assert_eq!(critical.level, 2);
        assert_eq!(critical.timestamp, 17);
        assert_eq!(
            text(&critical),
            "TRUE PEAK VIOLATION: Digital clipping imminent at -0.1 dBTP."
        );

        let warning = make_advice(-13.95, -1.0, 18);
        assert_eq!(warning.level, 1);
        assert_eq!(
            text(&warning),
            "LOUDNESS OVERAGE: Integrated LUFS (-13.9) exceeds streaming targets."
        );

        let info = make_advice(-14.0, -0.1, 19);
        assert_eq!(info.level, 0);
        assert_eq!(
            text(&info),
            "SIGNAL COMPLIANT: EBU R128 tolerances maintained."
        );
    }

    #[test]
    fn fixed_spsc_queue_preserves_order_and_rejects_full_pushes() {
        let queue = AdviceQueue::new();
        for index in 0..256 {
            assert!(queue.push(make_advice(-20.0, -1.0, index)));
        }
        assert!(!queue.push(make_advice(-20.0, -1.0, 256)));
        for index in 0..256 {
            assert_eq!(queue.pop().unwrap().timestamp, index);
        }
        assert!(queue.pop().is_none());
    }
}
