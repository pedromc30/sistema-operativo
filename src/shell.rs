//! Mini shell: lee una línea, la parte en palabras (usa el heap) y ejecuta.

use crate::{fs, heap, input, pmm, process, scheduler, timer};
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

/// Ejecuta un programa: primero busca un ELF en el initrd; si no, un programa interno.
fn run_program(prog: &str, args: &str) {
    let result = if let Some(data) = fs::find(prog) {
        process::spawn_elf(prog, data, args).map(|id| (id, data.len()))
    } else if let Some(code) = process::program(prog) {
        process::spawn_raw(prog, code)
            .map(|id| (id, code.len()))
            .map_err(alloc::string::String::from)
    } else {
        println!("programa desconocido '{}'. Usa 'ls'.", prog);
        return;
    };
    match result {
        Ok((id, size)) => println!("proceso '{}' creado (id {}, {} bytes)", prog, id, size),
        Err(e) => println!("no se pudo ejecutar '{}': {}", prog, e),
    }
}

/// Prueba el cargador con ELF corruptos: todos deben rechazarse SIN dejar fugas de memoria.
fn elf_test() {
    let Some(good) = fs::find("hello") else {
        println!("elftest necesita el ELF 'hello' en el initrd");
        return;
    };
    let rd64 = |d: &[u8], o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
    let phoff = rd64(good, 32) as usize;
    // (nombre, modificación)
    type Mut = (&'static str, fn(&mut alloc::vec::Vec<u8>, usize));
    let cases: [Mut; 12] = [
        ("firma incorrecta", |d, _| d[0] = 0),
        ("32 bits", |d, _| d[4] = 1),
        ("big-endian", |d, _| d[5] = 2),
        ("no es ET_EXEC (ET_DYN)", |d, _| d[16] = 3),
        ("arquitectura ARM", |d, _| d[18] = 0xB7),
        ("truncado a 40 bytes", |d, _| d.truncate(40)),
        ("phoff fuera del archivo", |d, _| d[32..40].copy_from_slice(&u64::MAX.to_le_bytes())),
        ("filesz > memsz", |d, p| d[p + 32..p + 40].copy_from_slice(&(1u64 << 40).to_le_bytes())),
        ("segmento en memoria del kernel", |d, p| d[p + 16..p + 24].copy_from_slice(&0xffff_ffff_8000_0000u64.to_le_bytes())),
        ("segmento escribible y ejecutable", |d, p| d[p + 4] = 7),
        ("offset fuera del archivo", |d, p| d[p + 8..p + 16].copy_from_slice(&(1u64 << 30).to_le_bytes())),
        ("entrada fuera del codigo", |d, _| d[24..32].copy_from_slice(&0x7f_0000u64.to_le_bytes())),
    ];
    let frames0 = pmm::free_frames();
    let mut rejected = 0;
    for (name, f) in cases.iter() {
        let mut bad = good.to_vec();
        f(&mut bad, phoff);
        match process::spawn_elf("elftest", &bad, "") {
            Ok(id) => println!("  [FALLO] '{}': el kernel ACEPTO el ELF (id {})", name, id),
            Err(e) => {
                rejected += 1;
                println!("  [ok] {:<34} -> {}", name, e);
            }
        }
    }
    // Y el ELF bueno sin tocar debe aceptarse (si no, el test no demostraria nada).
    match process::spawn_elf("elftest-ok", good, "prueba") {
        Ok(_) => println!("  [ok] ELF sin modificar            -> aceptado"),
        Err(e) => println!("  [FALLO] el ELF bueno fue rechazado: {}", e),
    }
    println!("rechazados {} de {} ELF corruptos", rejected, cases.len());
    let _ = frames0; // el balance de frames se comprueba con `mem` antes y despues de que termine el proceso bueno
}

/// Fuzzing del cargador: corrompe al azar 1 a 4 bytes de las cabeceras de un ELF válido y
/// lo carga (sin ejecutarlo). El cargador nunca debe colgarse, entrar en pánico ni perder memoria.
fn elf_fuzz(iterations: u64) {
    let Some(good) = fs::find("hello") else {
        println!("elffuzz necesita el ELF 'hello' en el initrd");
        return;
    };
    let frames0 = pmm::free_frames();
    let mut state: u64 = 0x1234_5678_9abc_def1 ^ timer::ticks(); // xorshift64
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut accepted, mut rejected) = (0u64, 0u64);
    let mut reasons: alloc::collections::BTreeMap<&'static str, u64> = alloc::collections::BTreeMap::new();
    for _ in 0..iterations {
        let mut bad = good.to_vec();
        let header_len = 64 + 56 * 4; // cabecera ELF + 4 program headers
        for _ in 0..(1 + next() % 4) {
            let pos = (next() % header_len as u64) as usize;
            bad[pos] = next() as u8;
        }
        match process::check_elf(&bad) {
            Ok(()) => accepted += 1,
            Err(e) => {
                rejected += 1;
                *reasons.entry(e).or_insert(0) += 1;
            }
        }
    }
    println!("{} ELF corrompidos al azar: {} aceptados (pasan la validacion), {} rechazados", iterations, accepted, rejected);
    for (r, n) in &reasons {
        println!("  {:>5} x {}", n, r);
    }
    println!("frames libres: antes={} despues={} -> {}", frames0, pmm::free_frames(),
        if frames0 == pmm::free_frames() { "OK, sin fugas" } else { "FUGA!" });
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
                println!("  ls            lista los archivos del initrd");
                println!("  cat <archivo> muestra un archivo de texto");
                println!("  run <prog> [args] ejecuta un programa en modo usuario (ELF del initrd o interno)");
                println!("  elftest       prueba el cargador ELF con archivos corruptos");
                println!("  elffuzz [n]   fuzzing del cargador ELF (n iteraciones, por defecto 1000)");
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
            ["run", prog, rest @ ..] => run_program(prog, &rest.join(" ")),
            ["run"] => println!("uso: run <programa> [argumentos]   (ver 'ls')"),
            ["ls"] => {
                println!("{:<14} {:>8}", "ARCHIVO", "BYTES");
                for (name, size) in fs::list() {
                    println!("{:<14} {:>8}", name, size);
                }
                println!("(ademas, programas internos en ensamblador: {:?})", process::PROGRAMS);
            }
            ["cat", name] => match fs::find(name) {
                Some(data) => match core::str::from_utf8(data) {
                    Ok(text) => println!("{}", text),
                    Err(_) => println!("'{}' es binario ({} bytes); no se muestra", name, data.len()),
                },
                None => println!("no existe '{}'", name),
            },
            ["cat"] => println!("uso: cat <archivo>"),
            ["elftest"] => elf_test(),
            ["elffuzz", n] => elf_fuzz(n.parse().unwrap_or(1000)),
            ["elffuzz"] => elf_fuzz(1000),
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
