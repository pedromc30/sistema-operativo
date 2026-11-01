//! Cola de entrada de caracteres (anillo de 4096 bytes).
//!
//! La rellenan los manejadores de interrupción (teclado PS/2 y puerto serie) y
//! la vacía el código normal. Hay un único productor (la IRQ) y un único
//! consumidor, así que bastan dos índices atómicos, sin cerrojos ni heap
//! (dentro de una interrupción NO queremos reservar memoria).

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const SIZE: usize = 4096;

static BUF: [AtomicU8; SIZE] = [const { AtomicU8::new(0) }; SIZE];
static HEAD: AtomicUsize = AtomicUsize::new(0); // siguiente posición a escribir
static TAIL: AtomicUsize = AtomicUsize::new(0); // siguiente posición a leer

/// Llamada desde una IRQ. Si la cola está llena, descarta el carácter.
pub fn push(c: u8) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % SIZE;
    if next == TAIL.load(Ordering::Acquire) {
        return; // llena
    }
    BUF[head].store(c, Ordering::Relaxed);
    HEAD.store(next, Ordering::Release);
    crate::scheduler::wake_input(); // despierta a quien espere teclas
}

pub fn has_data() -> bool {
    TAIL.load(Ordering::Acquire) != HEAD.load(Ordering::Acquire)
}

pub fn pop() -> Option<u8> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    let c = BUF[tail].load(Ordering::Relaxed);
    TAIL.store((tail + 1) % SIZE, Ordering::Release);
    Some(c)
}

/// Bloquea la tarea hasta que haya un carácter. Mientras espera NO consume CPU:
/// el planificador no la ejecuta hasta que una IRQ de teclado la despierte.
pub fn read_char() -> u8 {
    loop {
        if let Some(c) = pop() {
            return c;
        }
        crate::scheduler::wait_for_input();
    }
}
