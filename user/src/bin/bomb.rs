//! Prueba el límite de procesos: lanza muchos programas SIN esperar y luego los recoge.

#![no_std]
#![no_main]

use user::{entry, println, spawn, strerror, wait};

entry!(main);

fn main(args: &str) {
    let intentos: usize = args.trim().parse().unwrap_or(50).min(64);
    let mut pids = [0i64; 64];
    let mut ok = 0;
    let mut rechazados = 0;
    let mut primer_error = 0;
    for _ in 0..intentos {
        let pid = spawn("hello", "bomba");
        if pid >= 0 {
            pids[ok] = pid;
            ok += 1;
        } else {
            rechazados += 1;
            primer_error = pid;
        }
    }
    println!("[bomb] {} intentos: {} procesos creados, {} rechazados ({})",
        intentos, ok, rechazados, if rechazados > 0 { strerror(primer_error) } else { "-" });
    let mut sumas = 0;
    for &pid in &pids[..ok] {
        sumas += wait(pid as u64);
    }
    println!("[bomb] recogidos {} procesos; suma de codigos de salida = {}", ok, sumas);
}
