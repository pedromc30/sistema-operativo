//! Shell de RustOS en ESPACIO DE USUARIO: lee con `read`, ejecuta con `spawn` y espera con `wait`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use user::readline::readline;
use user::{entry, fs_list, fs_read, getpid, println, spawn, strerror, uptime_ms, wait};

entry!(main);

fn cmd_ls() {
    let mut buf = [0u8; 1024];
    let n = fs_list(&mut buf);
    if n < 0 {
        println!("ls: {}", strerror(n));
        return;
    }
    println!("{:<14} {:>8}", "ARCHIVO", "BYTES");
    for l in core::str::from_utf8(&buf[..n as usize]).unwrap_or("").lines() {
        let mut it = l.split(' ');
        println!("{:<14} {:>8}", it.next().unwrap_or(""), it.next().unwrap_or(""));
    }
}

fn cmd_cat(name: &str) {
    // Se lee el archivo entero en un Vec: usa el heap (brk) del propio proceso.
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = fs_read(name, &mut chunk, data.len() as u64);
        if n < 0 {
            println!("cat: {}: {}", name, strerror(n));
            return;
        }
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n as usize]);
        if data.len() > 1 << 20 {
            println!("cat: archivo demasiado grande");
            return;
        }
    }
    match core::str::from_utf8(&data) {
        Ok(t) => println!("{}", t),
        Err(_) => println!("cat: '{}' es binario ({} bytes)", name, data.len()),
    }
}

fn main(_args: &str) {
    println!("[sh] shell en espacio de usuario (pid {}). Escribe 'help'.", getpid());
    let mut history: Vec<String> = Vec::new();
    loop {
        let Some(line) = readline("usuario$ ", &mut history) else { break };
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            [] => {}
            ["exit"] => break,
            ["help"] => {
                println!("Comandos internos: help, exit, ls, cat <archivo>, pid, uptime, echo <texto>");
                println!("Cualquier otro nombre se ejecuta como programa del initrd (ver 'ls'):");
                println!("  hello, primes [n [rep]], crash [stack|ud|kernel], memtest [shrink|leak], launcher");
            }
            ["pid"] => println!("{}", getpid()),
            ["uptime"] => println!("{} ms", uptime_ms()),
            ["ls"] => cmd_ls(),
            ["cat", f] => cmd_cat(f),
            ["cat"] => println!("uso: cat <archivo>"),
            ["echo", rest @ ..] => println!("{}", rest.join(" ")),
            [prog, rest @ ..] => {
                let args = rest.join(" ");
                let pid = spawn(prog, &args);
                if pid < 0 {
                    println!("sh: no se pudo ejecutar '{}': {}", prog, strerror(pid));
                } else {
                    let code = wait(pid as u64);
                    println!("[{} (pid {}) termino con codigo {}]", prog, pid, code);
                }
            }
        }
    }
    println!("[sh] adios");
}
