//! Acceso a puertos de entrada/salida (instrucciones `in` / `out`).

use core::arch::asm;

pub unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}

/// Pequeña pausa escribiendo en un puerto sin uso (el 0x80). Algunos chips
/// antiguos, como el PIC, necesitan unos instantes entre comandos.
pub unsafe fn io_wait() {
    outb(0x80, 0);
}
