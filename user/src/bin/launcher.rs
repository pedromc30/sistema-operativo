//! Prueba spawn/wait desde modo usuario: lanza varios programas y recoge sus códigos de salida.

#![no_std]
#![no_main]

use user::{entry, getpid, println, spawn, strerror, wait};

entry!(main);

fn main(_args: &str) {
    println!("[launcher pid {}] voy a lanzar varios programas y esperar a cada uno", getpid());
    let casos: [(&str, &str); 5] = [
        ("hello", "desde-launcher"),
        ("primes", "50000 3"),
        ("crash", "stack"),
        ("crash", ""),
        ("noexiste", ""),
    ];
    for (nombre, args) in casos {
        let pid = spawn(nombre, args);
        if pid < 0 {
            println!("[launcher] spawn('{}') fallo: {} ({})", nombre, strerror(pid), pid);
            continue;
        }
        let codigo = wait(pid as u64);
        let nota = if codigo >= 128 { " (muerto por una excepcion: 128 + n.º de excepcion)" } else { "" };
        println!("[launcher] '{}' (pid {}) -> codigo de salida {}{}", nombre, pid, codigo, nota);
    }
    // Esperar dos veces al mismo proceso, o a uno que no existe, debe dar error.
    println!("[launcher] wait(9999) -> {} ({})", wait(9999), strerror(wait(9999)));
    println!("[launcher] fin");
}
