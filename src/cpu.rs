//! Utilidades de CPU: control de interrupciones.

use core::arch::asm;

const IF_FLAG: u64 = 1 << 9;

pub fn interrupts_enabled() -> bool {
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    flags & IF_FLAG != 0
}

/// Ejecuta `f` con las interrupciones desactivadas y las restaura DESPUÉS a como estaban.
/// En un solo núcleo es la forma de proteger datos compartidos con manejadores de IRQ.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let was_enabled = interrupts_enabled();
    unsafe { asm!("cli", options(nostack)) };
    let r = f();
    if was_enabled {
        unsafe { asm!("sti", options(nostack)) };
    }
    r
}

pub fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
             options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

/// # Safety
/// Escribir un MSR puede cambiar el comportamiento de toda la CPU.
pub unsafe fn wrmsr(msr: u32, val: u64) {
    asm!("wrmsr", in("ecx") msr, in("eax") val as u32, in("edx") (val >> 32) as u32,
         options(nomem, nostack, preserves_flags));
}
