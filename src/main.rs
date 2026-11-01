#![no_std]
#![no_main]

use core::arch::asm;
use core::fmt::{self, Write};
use core::panic::PanicInfo;

/// Tag de revisión base del protocolo Limine. Si el bootloader soporta la
/// revisión pedida (3), pone el tercer valor a 0.
#[used]
#[link_section = ".requests"]
static mut BASE_REVISION: [u64; 3] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, 3];

fn base_revision_supported() -> bool {
    unsafe { core::ptr::read_volatile(&raw const BASE_REVISION[2]) == 0 }
}

const COM1: u16 = 0x3F8;

unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
}

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags));
    v
}

/// Inicializa el UART 16550 a 115200 baudios, 8N1.
fn serial_init() {
    unsafe {
        outb(COM1 + 1, 0x00); // sin interrupciones
        outb(COM1 + 3, 0x80); // DLAB on
        outb(COM1, 0x01);     // divisor = 1 (115200)
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8 bits, sin paridad, 1 stop
        outb(COM1 + 2, 0xC7); // FIFO
        outb(COM1 + 4, 0x0B);
    }
}

struct Serial;

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            unsafe {
                while inb(COM1 + 5) & 0x20 == 0 {}
                outb(COM1, b);
            }
        }
        Ok(())
    }
}

macro_rules! println {
    ($($arg:tt)*) => {{
        let _ = writeln!(Serial, $($arg)*);
    }};
}

fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt") };
    }
}

#[no_mangle]
extern "C" fn _start() -> ! {
    serial_init();
    if !base_revision_supported() {
        println!("Limine base revision no soportada");
        halt();
    }
    println!("Hola desde RustOS (x86_64)!");
    halt();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("KERNEL PANIC: {}", info);
    halt();
}
