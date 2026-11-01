#![no_std]
#![no_main]

use core::arch::asm;
use core::hint::black_box;
use user::{entry, println};

entry!(main);

#[allow(unconditional_recursion)]
fn recurse(n: u64) -> u64 {
    let buf = [black_box(n as u8); 256];
    black_box(&buf);
    recurse(n + 1) + buf[0] as u64
}

fn main(args: &str) {
    match args.trim() {
        "stack" => {
            println!("[crash] desbordando la pila con recursion infinita...");
            recurse(0);
        }
        "ud" => {
            println!("[crash] ejecutando una instruccion invalida (ud2)...");
            unsafe { asm!("ud2") };
        }
        "kernel" => {
            println!("[crash] leyendo memoria del kernel...");
            unsafe { core::ptr::read_volatile(0xffff_ffff_8000_0000 as *const u8) };
        }
        _ => {
            println!("[crash] leyendo la direccion 0 (puntero nulo)...");
            unsafe { core::ptr::read_volatile(black_box(0usize) as *const u8) };
        }
    }
    println!("[crash] ERROR: no deberia llegar aqui");
}
