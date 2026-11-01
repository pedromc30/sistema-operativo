//! Mini shell: lee una línea, la parte en palabras (usa el heap) y ejecuta.

use crate::{heap, input, pmm, timer};
use alloc::{string::String, vec::Vec};

fn read_line() -> String {
    let mut s = String::new();
    loop {
        let c = input::read_char();
        match c {
            b'\n' => {
                println!();
                return s;
            }
            0x08 | 0x7f => {
                if s.pop().is_some() {
                    print!("\x08 \x08"); // retrocede, borra y retrocede
                }
            }
            0x20..=0x7e => {
                s.push(c as char);
                print!("{}", c as char); // eco
            }
            _ => {}
        }
    }
}

pub fn run() -> ! {
    println!();
    println!("=== Shell de RustOS ===  (escribe 'help')");
    loop {
        print!("rustos> ");
        let line = read_line();
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            [] => {}
            ["help"] => {
                println!("Comandos:");
                println!("  help          esta ayuda");
                println!("  uptime        tiempo desde el arranque");
                println!("  mem           memoria libre (frames y heap)");
                println!("  echo <texto>  repite el texto");
                println!("  clear         limpia la pantalla");
                println!("  panic         provoca un kernel panic");
                println!("  halt          detiene el sistema");
            }
            ["uptime"] => {
                let ms = timer::uptime_ms();
                println!("{}.{:03} s  ({} ticks)", ms / 1000, ms % 1000, timer::ticks());
            }
            ["mem"] => {
                println!("frames libres: {} de {}  (= {} MiB libres)",
                    pmm::free_frames(), pmm::total_frames(),
                    pmm::free_frames() * pmm::FRAME_SIZE / (1024 * 1024));
                println!("heap libre:    {} de {} bytes", heap::free_bytes(), heap::HEAP_SIZE);
            }
            ["echo", rest @ ..] => println!("{}", rest.join(" ")),
            ["clear"] => print!("\x1b[2J\x1b[H"),
            ["panic"] => panic!("panico pedido desde el shell"),
            ["halt"] => {
                println!("Sistema detenido.");
                crate::halt();
            }
            [cmd, ..] => println!("comando desconocido: '{}'  (prueba 'help')", cmd),
        }
    }
}
