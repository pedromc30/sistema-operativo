//! PIC 8259: el controlador clásico de interrupciones de hardware (IRQ 0-15).
//!
//! Por defecto las IRQ 0-7 llegan a los vectores 8-15, que COLISIONAN con las
//! excepciones de la CPU (el 8 es el double fault...). Por eso lo "remapeamos"
//! para que las IRQ lleguen a los vectores 32-47.

use crate::port::{inb, io_wait, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

pub const VECTOR_BASE: u8 = 32;

/// Remapea los dos PIC (maestro y esclavo) y enmascara todas las IRQ.
///
/// # Safety
/// Una sola vez, con las interrupciones desactivadas.
pub unsafe fn init() {
    // ICW1: iniciar la secuencia de inicialización (y esperar ICW4).
    outb(PIC1_CMD, 0x11); io_wait();
    outb(PIC2_CMD, 0x11); io_wait();
    // ICW2: desplazamiento de vectores (maestro: 32..39, esclavo: 40..47).
    outb(PIC1_DATA, VECTOR_BASE); io_wait();
    outb(PIC2_DATA, VECTOR_BASE + 8); io_wait();
    // ICW3: cómo se conectan entre sí (el esclavo cuelga de la IRQ2 del maestro).
    outb(PIC1_DATA, 0b0000_0100); io_wait();
    outb(PIC2_DATA, 2); io_wait();
    // ICW4: modo 8086.
    outb(PIC1_DATA, 0x01); io_wait();
    outb(PIC2_DATA, 0x01); io_wait();
    // Máscaras: bit a 1 = IRQ desactivada. Empezamos con todas desactivadas.
    outb(PIC1_DATA, 0xFF);
    outb(PIC2_DATA, 0xFF);
}

/// Activa (desenmascara) una IRQ concreta.
pub unsafe fn unmask(irq: u8) {
    let (port, bit) = if irq < 8 { (PIC1_DATA, irq) } else { (PIC2_DATA, irq - 8) };
    let m = inb(port);
    outb(port, m & !(1 << bit));
    // Si es del esclavo, la línea de cascada (IRQ2) del maestro debe estar activa.
    if irq >= 8 {
        let m1 = inb(PIC1_DATA);
        outb(PIC1_DATA, m1 & !(1 << 2));
    }
}

/// "End Of Interrupt": avisa al PIC de que ya atendimos la IRQ. Sin esto no
/// vuelve a enviar interrupciones de esa línea.
pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_CMD, 0x20);
        }
        outb(PIC1_CMD, 0x20);
    }
}
