//! Mini shell: lee una línea, la parte en palabras (usa el heap) y ejecuta.

use crate::{heap, input, pmm, scheduler, timer};
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

/// Tarea de demostración: cuenta lo más rápido que puede (sin ceder nunca la CPU)
/// e informa una vez por segundo. Solo la expropiación del temporizador la frena.
fn worker() {
    let name = scheduler::current_name();
    let mut n: u64 = 0;
    let mut last = timer::ticks();
    let mut shown = 0;
    while shown < 5 {
        n = core::hint::black_box(n + 1);
        if timer::ticks() - last >= timer::HZ {
            last += timer::HZ;
            shown += 1;
            println!("[{}] contador={}", name, n);
        }
    }
    println!("[{}] termina", name);
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
                println!("  ps            lista las tareas");
                println!("  spawn <nombre> crea una tarea que cuenta durante 5 s");
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
            ["ps"] => {
                println!("{:>3}  {:<12} {:<11} {:>10}", "ID", "NOMBRE", "ESTADO", "CPU(ticks)");
                for t in scheduler::list() {
                    println!("{:>3}  {:<12} {:<11} {:>10}", t.id, t.name, t.state, t.ticks);
                }
            }
            ["spawn", name] => {
                let id = scheduler::spawn(name, worker);
                println!("tarea '{}' creada (id {})", name, id);
            }
            ["spawn"] => println!("uso: spawn <nombre>"),
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
