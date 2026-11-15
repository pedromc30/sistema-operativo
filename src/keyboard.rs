//! Teclado PS/2 (controlador i8042): distribución española (por defecto) y americana.
//!
//! IRQ1 -> vector 33. Al pulsar o soltar una tecla, el controlador deja un "scancode" en
//! el puerto 0x60 (conjunto 1). El scancode identifica la POSICIÓN física de la tecla, no
//! la letra: la distribución decide qué carácter produce cada posición. Los scancodes al
//! SOLTAR son el mismo valor con el bit 7 activado.
//!
//! La salida son bytes UTF-8 (para poder escribir ñ, á, €...) o secuencias ANSI (flechas).

use crate::input;
use crate::port::inb;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

const DATA: u16 = 0x60;

// ---- Distribución americana (también sirve de base para las letras del español) ----
// Índice = scancode (0x00..=0x39). 0 = tecla sin carácter (Ctrl, Alt, ...).
static US_UNSHIFTED: &[u8; 58] =
    b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
static US_SHIFTED: &[u8; 58] =
    b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    Us = 0,
    Es = 1,
}

static LAYOUT: AtomicU8 = AtomicU8::new(Layout::Es as u8);

pub fn set_layout(l: Layout) {
    LAYOUT.store(l as u8, Ordering::Relaxed);
}

pub fn layout() -> Layout {
    if LAYOUT.load(Ordering::Relaxed) == Layout::Us as u8 { Layout::Us } else { Layout::Es }
}

pub fn layout_name() -> &'static str {
    if layout() == Layout::Es { "espanol (ES)" } else { "americano (US)" }
}

// ---- Teclas muertas: no escriben nada por sí solas, esperan a la tecla siguiente ----
// Se representan con la marca Unicode del acento "combinante".
const DEAD_ACUTE: char = '\u{301}'; // ´
const DEAD_GRAVE: char = '\u{300}'; // `
const DEAD_CIRC: char = '\u{302}'; // ^
const DEAD_DIAER: char = '\u{308}'; // ¨

fn is_dead(c: char) -> bool {
    matches!(c, DEAD_ACUTE | DEAD_GRAVE | DEAD_CIRC | DEAD_DIAER)
}

/// El acento tal cual, cuando no se combina con nada.
fn dead_literal(d: char) -> char {
    match d {
        DEAD_ACUTE => '´',
        DEAD_GRAVE => '`',
        DEAD_CIRC => '^',
        _ => '¨',
    }
}

fn compose(dead: char, c: char) -> Option<char> {
    let (from, to) = match dead {
        DEAD_ACUTE => ("aeiouyAEIOUY", "áéíóúýÁÉÍÓÚÝ"),
        DEAD_GRAVE => ("aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
        DEAD_CIRC => ("aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
        _ => ("aeiouyAEIOU", "äëïöüÿÄËÏÖÜ"),
    };
    let i = from.chars().position(|x| x == c)?;
    to.chars().nth(i)
}

// ---- Distribución española (ISO): teclas que difieren de la americana ----
// (sin pulsar, con Shift, con AltGr); '\0' = nada.
fn es_key(code: u8) -> Option<(char, char, char)> {
    Some(match code {
        0x29 => ('º', 'ª', '\\'),
        0x02 => ('1', '!', '|'),
        0x03 => ('2', '"', '@'),
        0x04 => ('3', '·', '#'),
        0x05 => ('4', '$', '~'),
        0x06 => ('5', '%', '€'),
        0x07 => ('6', '&', '¬'),
        0x08 => ('7', '/', '\0'),
        0x09 => ('8', '(', '\0'),
        0x0A => ('9', ')', '\0'),
        0x0B => ('0', '=', '\0'),
        0x0C => ('\'', '?', '\0'),
        0x0D => ('¡', '¿', '\0'),
        0x12 => ('e', 'E', '€'),
        0x1A => (DEAD_GRAVE, DEAD_CIRC, '['),
        0x1B => ('+', '*', ']'),
        0x27 => ('ñ', 'Ñ', '\0'),
        0x28 => (DEAD_ACUTE, DEAD_DIAER, '{'),
        0x2B => ('ç', 'Ç', '}'),
        0x33 => (',', ';', '\0'),
        0x34 => ('.', ':', '\0'),
        0x35 => ('-', '_', '\0'),
        0x56 => ('<', '>', '\0'),
        _ => return None,
    })
}

/// (sin Shift, con Shift, con AltGr) de la tecla con ese scancode en la distribución activa.
fn keymap(code: u8) -> (char, char, char) {
    if layout() == Layout::Es {
        if let Some(t) = es_key(code) {
            return t;
        }
    }
    let i = code as usize;
    if i < US_UNSHIFTED.len() {
        (US_UNSHIFTED[i] as char, US_SHIFTED[i] as char, '\0')
    } else {
        ('\0', '\0', '\0')
    }
}

static SHIFT: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static ALTGR: AtomicBool = AtomicBool::new(false);
static CAPS: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);
static DEAD: AtomicU32 = AtomicU32::new(0); // tecla muerta pendiente (0 = ninguna)

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
        0x35 => b"/",       // / del teclado numérico
        0x1C => b"\n",      // Enter del teclado numérico
        _ => b"",
    }
}

fn emit(ch: char) {
    let mut b = [0u8; 4];
    input::push_seq(ch.encode_utf8(&mut b).as_bytes());
}

/// Una tecla con carácter ha sido pulsada.
fn key_pressed(code: u8) {
    let shift = SHIFT.load(Ordering::Relaxed);
    let altgr = ALTGR.load(Ordering::Relaxed);
    let caps = CAPS.load(Ordering::Relaxed);
    let (normal, shifted, alt) = keymap(code);

    // Es una "letra" solo si tiene una mayúscula DISTINTA (ñ, ç, a-z...). OJO: º y ª cuentan
    // como minúsculas en Unicode pero no tienen mayúscula; con Shift deben dar su par (ª).
    let upper = normal.to_uppercase().next().unwrap_or(normal);
    let is_letter = normal.is_lowercase() && upper != normal;
    let mut ch = if altgr && alt != '\0' {
        alt
    } else if is_letter {
        // Bloq Mayús invierte el efecto de Shift en las letras.
        if shift ^ caps { upper } else { normal }
    } else if shift {
        shifted
    } else {
        normal
    };
    if ch == '\0' {
        return;
    }

    if CTRL.load(Ordering::Relaxed) && ch.is_ascii_alphabetic() {
        DEAD.store(0, Ordering::Relaxed);
        ch = (ch.to_ascii_lowercase() as u8 - b'a' + 1) as char; // Ctrl+A = 1 ... Ctrl+Z = 26
        if ch == '\x03' && crate::scheduler::interrupt_foreground() {
            return; // Ctrl+C con un programa en primer plano: se mata, no se escribe
        }
        emit(ch);
        return;
    }

    if is_dead(ch) {
        let pending = DEAD.swap(0, Ordering::Relaxed);
        if pending == ch as u32 {
            emit(dead_literal(ch)); // la misma tecla muerta dos veces: el acento suelto
        } else {
            if pending != 0 {
                emit(dead_literal(char::from_u32(pending).unwrap_or('´')));
            }
            DEAD.store(ch as u32, Ordering::Relaxed);
        }
        return;
    }

    let pending = DEAD.swap(0, Ordering::Relaxed);
    if pending != 0 {
        let dead = char::from_u32(pending).unwrap_or('´');
        if ch == ' ' {
            emit(dead_literal(dead)); // acento + espacio = el acento solo
        } else if let Some(c) = compose(dead, ch) {
            emit(c); // acento + vocal = vocal acentuada
        } else {
            emit(dead_literal(dead));
            emit(ch);
        }
        return;
    }
    emit(ch);
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
            0x38 => ALTGR.store(!released, Ordering::Relaxed), // AltGr (Alt derecho)
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
        c => key_pressed(c),
    }
}
