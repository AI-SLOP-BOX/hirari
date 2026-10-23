//! Per-thread real-time markers and floating-point denormal controls.

use std::cell::Cell;

thread_local! {
    static IS_REALTIME_THREAD: Cell<bool> = const { Cell::new(false) };
}

#[no_mangle]
pub extern "C" fn hirari_realtime_thread_set(active: bool) {
    IS_REALTIME_THREAD.with(|state| state.set(active));
}

#[no_mangle]
pub extern "C" fn hirari_realtime_thread_is_active() -> bool {
    IS_REALTIME_THREAD.with(Cell::get)
}

#[no_mangle]
pub extern "C" fn hirari_denormal_control_enter() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        let previous = _mm_getcsr();
        _mm_setcsr(previous | 0x8040);
        return u64::from(previous);
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        use std::arch::x86::{_mm_getcsr, _mm_setcsr};
        let previous = _mm_getcsr();
        _mm_setcsr(previous | 0x8040);
        return u64::from(previous);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let previous: u64;
        std::arch::asm!("mrs {value}, fpcr", value = out(reg) previous, options(nomem, nostack));
        std::arch::asm!(
            "msr fpcr, {value}", value = in(reg) (previous | (1 << 24)),
            options(nomem, nostack),
        );
        return previous;
    }
    #[allow(unreachable_code)]
    0
}

#[no_mangle]
pub extern "C" fn hirari_denormal_control_leave(previous: u64) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_mm_setcsr(previous as u32);
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        std::arch::x86::_mm_setcsr(previous as u32);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("msr fpcr, {value}", value = in(reg) previous, options(nomem, nostack));
    }
}

