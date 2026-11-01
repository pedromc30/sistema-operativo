//! Temporizador PIT 8253/8254 (canal 0) -> IRQ0 -> vector 32.
//!
//! El PIT cuenta a 1.193.182 Hz y dispara una interrupción cada vez que su
//! contador llega a 0. Elegimos el divisor para obtener la frecuencia deseada.

use crate::port::outb;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

const PIT_FREQ: u64 = 1_193_182;
const PIT_CMD: u16 = 0x43;
const PIT_CH0: u16 = 0x40;

pub const HZ: u64 = 100; // 1 tick = 10 ms

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Programa el canal 0 en modo 3 (onda cuadrada) a `HZ` interrupciones por segundo.
///
/// # Safety
/// Después de `pic::init`.
pub unsafe fn init() {
    let divisor = (PIT_FREQ / HZ) as u16; // 11931 -> ~100,0015 Hz
    outb(PIT_CMD, 0b0011_0110); // canal 0, acceso lobyte/hibyte, modo 3, binario
    outb(PIT_CH0, (divisor & 0xFF) as u8);
    outb(PIT_CH0, (divisor >> 8) as u8);
    crate::pic::unmask(0); // dejar pasar la IRQ0
}

/// Llamada desde el manejador de la interrupción (IRQ0).
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * 1000 / HZ
}

/// Espera sin gastar CPU: `hlt` detiene el procesador hasta la siguiente interrupción.
pub fn sleep_ms(ms: u64) {
    let target = ticks() + (ms * HZ + 999) / 1000;
    while ticks() < target {
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}
