//! Puerto serie COM1 (UART 16550). Es nuestra "pantalla" por ahora.

use crate::port::{inb, outb};
use core::fmt::{self, Write};

const COM1: u16 = 0x3F8;

/// Inicializa el UART a 115200 baudios, 8 bits, sin paridad, 1 bit de stop.
pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // sin interrupciones
        outb(COM1 + 3, 0x80); // DLAB on: permite fijar el divisor de velocidad
        outb(COM1, 0x01); // divisor = 1 -> 115200 baudios
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1
        outb(COM1 + 2, 0xC7); // activa la FIFO
        outb(COM1 + 4, 0x0B);
    }
}

pub struct Serial;

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            unsafe {
                // Espera a que el transmisor esté libre (bit 5 del registro de estado).
                while inb(COM1 + 5) & 0x20 == 0 {}
                outb(COM1, b);
            }
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    let _ = Serial.write_fmt(args);
}

macro_rules! print {
    ($($arg:tt)*) => { $crate::serial::_print(format_args!($($arg)*)) };
}

macro_rules! println {
    () => { print!("\n") };
    ($($arg:tt)*) => { print!("{}\n", format_args!($($arg)*)) };
}
