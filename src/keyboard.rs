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
static CTRL: AtomicBool = AtomicBool::new(false);
static CAPS: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

/// Secuencia ANSI que un terminal enviaría para una tecla extendida (prefijo 0xE0).
fn extended_sequence(code: u8) -> &'static [u8] {
    match code {
        0x48 => b"\x1b[A",  // flecha arriba
        0x50 => b"\x1b[B",  // flecha abajo
        0x4D => b"\x1b[C",  // flecha derecha
        0x4B => b"\x1b[D",  // flecha izquierda
        0x47 => b"\x1b[H",  // Inicio
        0x4F => b"\x1b[F",  // Fin
        0x53 => b"\x1b[3~", // Supr
        0x52 => b"\x1b[2~", // Insert
        0x49 => b"\x1b[5~", // Re Pág
        0x51 => b"\x1b[6~", // Av Pág
        0x35 => b"/",        // / del teclado numérico
        0x1C => b"\n",      // Enter del teclado numérico
        _ => b"",
    }
}

/// Llamada desde la IRQ1.
pub fn on_irq() {
    let sc = unsafe { inb(DATA) };

    // 0xE0 anuncia una tecla "extendida": el siguiente byte dice cuál es.
    if sc == 0xE0 {
        EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    let extended = EXTENDED.swap(false, Ordering::Relaxed);
    let released = sc & 0x80 != 0;
    let code = sc & 0x7F;

    if extended {
        match code {
            0x1D => CTRL.store(!released, Ordering::Relaxed), // Ctrl derecho
            _ if !released => {
                let seq = extended_sequence(code);
                if !seq.is_empty() {
                    input::push_seq(seq);
                }
            }
            _ => {}
        }
        return;
    }

    match code {
        0x2A | 0x36 => SHIFT.store(!released, Ordering::Relaxed), // Shift izq./der.
        0x1D => CTRL.store(!released, Ordering::Relaxed),         // Ctrl izquierdo
        0x3A if !released => {
            CAPS.fetch_xor(true, Ordering::Relaxed); // Bloq Mayús: alterna
        }
        _ if released => {}
        c if (c as usize) < UNSHIFTED.len() => {
            let shift = SHIFT.load(Ordering::Relaxed);
            let table = if shift { SHIFTED } else { UNSHIFTED };
            let mut ch = table[c as usize];
            if ch.is_ascii_alphabetic() && CAPS.load(Ordering::Relaxed) {
                // Bloq Mayús invierte el efecto de Shift solo en las letras.
                ch = if shift { ch.to_ascii_lowercase() } else { ch.to_ascii_uppercase() };
            }
            if CTRL.load(Ordering::Relaxed) && ch.is_ascii_alphabetic() {
                ch = ch.to_ascii_lowercase() - b'a' + 1; // Ctrl+A = 1 ... Ctrl+Z = 26
            }
            if ch != 0 {
                input::push(ch);
            }
        }
        _ => {}
    }
}
