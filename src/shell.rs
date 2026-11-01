//! Mini shell: lee una línea, la parte en palabras (usa el heap) y ejecuta.

use crate::{heap, input, pmm, process, scheduler, timer};
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

/// Tarea de demostración que DUERME entre mensajes: casi no consume CPU.
fn sleeper() {
    let name = scheduler::current_name();
    for i in 1..=5 {
        timer::sleep_ms(1000);
        println!("[{}] desperte ({}/5) a los {} ms", name, i, timer::uptime_ms());
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
                println!("  spawn <nombre> crea una tarea que cuenta durante 5 s (usa CPU)");
                println!("  sleeper <nombre> crea una tarea que duerme entre mensajes (no usa CPU)");
                println!("  run <programa> ejecuta un programa en modo usuario (hello, crash, cli, ptr)");
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
                println!("{:>3}  {:<12} {:<8} {:<11} {:>10}", "ID", "NOMBRE", "TIPO", "ESTADO", "CPU(ticks)");
                for t in scheduler::list() {
                    println!("{:>3}  {:<12} {:<8} {:<11} {:>10}", t.id, t.name,
                        if t.user { "usuario" } else { "kernel" }, t.state, t.ticks);
                }
            }
            ["spawn", name] => {
                let id = scheduler::spawn(name, worker);
                println!("tarea '{}' creada (id {})", name, id);
            }
            ["spawn"] => println!("uso: spawn <nombre>"),
            ["sleeper", name] => {
                let id = scheduler::spawn(name, sleeper);
                println!("tarea '{}' creada (id {}): duerme 1 s entre mensajes", name, id);
            }
            ["run", prog] => match process::program(prog) {
                Some(code) => match process::spawn(prog, code) {
                    Ok(id) => println!("proceso '{}' creado (id {}, {} bytes) en modo usuario", prog, id, code.len()),
                    Err(e) => println!("no se pudo crear el proceso: {}", e),
                },
                None => println!("programa desconocido '{}'. Disponibles: {:?}", prog, process::PROGRAMS),
            },
            ["run"] => println!("uso: run <programa>   (disponibles: {:?})", process::PROGRAMS),
            ["sleeper"] => println!("uso: sleeper <nombre>"),
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
