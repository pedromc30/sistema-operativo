#![no_std]
#![no_main]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, format, string::String, vec::Vec};
use core::ptr::{read_volatile, write_volatile};
use user::{brk, entry, println};

entry!(main);

const PAGE: u64 = 4096;

fn main(args: &str) {
    match args.trim() {
        "shrink" => shrink(),
        "leak" => leak(),
        _ => basic(),
    }
}

fn basic() {
    let b0 = brk(0);
    println!("[memtest] heap inicial: brk={:#x}", b0);

    let mut v: Vec<u64> = Vec::new();
    for i in 0..100_000u64 {
        v.push(i * i % 1000);
    }
    let suma: u64 = v.iter().sum();
    let esperado: u64 = (0..100_000u64).map(|i| i * i % 1000).sum();
    println!("[memtest] Vec de {} elementos: suma={} {}", v.len(), suma, if suma == esperado { "OK" } else { "MAL" });

    let mut m: BTreeMap<u32, String> = BTreeMap::new();
    for i in 0..2000u32 {
        m.insert(i, format!("valor-{}", i));
    }
    println!("[memtest] BTreeMap de {} entradas: m[1234]={:?}", m.len(), m.get(&1234));

    let caja = Box::new([7u8; 4096]);
    println!("[memtest] Box de 4 KiB, primer byte={}", caja[0]);

    let b1 = brk(0);
    println!("[memtest] brk tras las reservas: {:#x} (+{} KiB)", b1, (b1 - b0) / 1024);

    drop(v);
    drop(m);
    drop(caja);
    // Liberar NO devuelve memoria al sistema, pero el hueco se reutiliza:
    let otra: Vec<u8> = Vec::with_capacity(400_000);
    let b2 = brk(0);
    println!("[memtest] tras liberar y pedir 400 KB: brk crecio {} KiB ({})", (b2 - b1) / 1024,
        if b2 == b1 { "se reutilizo el hueco" } else { "hubo que pedir mas" });
    drop(otra);

    let mut grande: Vec<u8> = Vec::new();
    match grande.try_reserve(64 * 1024 * 1024) {
        Ok(_) => println!("[memtest] ERROR: se concedieron 64 MiB"),
        Err(_) => println!("[memtest] pedir 64 MiB falla correctamente (limite: 32 MiB por proceso)"),
    }
    let r = brk(0x2000_0000_0000);
    println!("[memtest] brk a una direccion absurda no cambia nada: {}", if r == b2 { "OK" } else { "MAL" });
}

/// Crece el heap, escribe, lo encoge y comprueba que lo liberado YA NO es accesible.
fn shrink() {
    let base = brk(0);
    let top = brk(base + 1024 * 1024);
    println!("[memtest] brk: {:#x} -> {:#x} (+1 MiB)", base, top);
    let mut n = 0u64;
    let mut a = base;
    while a < top {
        unsafe { write_volatile(a as *mut u8, 0xAB) };
        a += PAGE;
        n += 1;
    }
    let mut ok = true;
    a = base;
    while a < top {
        ok &= unsafe { read_volatile(a as *const u8) } == 0xAB;
        a += PAGE;
    }
    println!("[memtest] escritas y leidas {} paginas: {}", n, if ok { "OK" } else { "MAL" });
    let back = brk(base);
    println!("[memtest] brk de vuelta a {:#x}; ahora leo la primera pagina liberada (debe fallar)...", back);
    let v = unsafe { read_volatile(base as *const u8) };
    println!("[memtest] ERROR: pude leer {} de memoria liberada", v);
}

/// Pide 8 MiB y termina sin liberar nada: el kernel debe recuperarlos al terminar.
fn leak() {
    let mut v: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);
    v.resize(8 * 1024 * 1024, 1); // tocar todas las páginas
    println!("[memtest] 8 MiB reservados y usados (brk={:#x}); termino SIN liberar", brk(0));
    core::mem::forget(v);
}
