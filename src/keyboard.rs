//! Teclado PS/2 (controlador i8042), distribución US.
//!
//! IRQ1 -> vector 33. Al pulsar o soltar una tecla, el controlador deja un
//! "scancode" en el puerto 0x60. Usamos el conjunto de scancodes 1, el que da
//! el controlador por defecto (con traducción). Los scancodes al SOLTAR son el
//! mismo valor con el bit 7 activado.

use crate::input;
use crate::port::inb;
use core::sync::atomic::{AtomicBool, Ordering};

const DATA: u16 = 0x60;

// Índice = scancode (0x00..=0x39). 0 = tecla sin carácter (Ctrl, Alt, ...).
static UNSHIFTED: &[u8; 58] =
    b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
static SHIFTED: &[u8; 58] =
    b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

static SHIFT: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

/// Llamada desde la IRQ1.
pub fn on_irq() {
    let sc = unsafe { inb(DATA) };

    // 0xE0 anuncia una tecla "extendida" (flechas, etc.). Aún no las usamos.
    if sc == 0xE0 {
        EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    if EXTENDED.swap(false, Ordering::Relaxed) {
        return;
    }

    let released = sc & 0x80 != 0;
    let code = sc & 0x7F;

    match code {
        0x2A | 0x36 => SHIFT.store(!released, Ordering::Relaxed), // Shift izq./der.
        _ if released => {}
        c if (c as usize) < UNSHIFTED.len() => {
            let table = if SHIFT.load(Ordering::Relaxed) { SHIFTED } else { UNSHIFTED };
            let ch = table[c as usize];
            if ch != 0 {
                input::push(ch);
            }
        }
        _ => {}
    }
}
