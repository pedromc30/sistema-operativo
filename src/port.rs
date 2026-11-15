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

pub unsafe fn outw(port: u16, val: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
}

pub unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags));
    v
}

pub unsafe fn outl(port: u16, val: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags));
}

pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags));
    v
}

/// Pequeña pausa escribiendo en un puerto sin uso (el 0x80). Algunos chips
/// antiguos, como el PIC, necesitan unos instantes entre comandos.
pub unsafe fn io_wait() {
    outb(0x80, 0);
}
