#![no_std]
#![no_main]

use user::{entry, getpid, println, sleep_ms};

entry!(main);

fn main(args: &str) {
    println!("[hello] Hola desde un programa en Rust, cargado desde un ELF (pid {})", getpid());
    println!("[hello] argumentos: {:?}", args);
    for i in 1..=3 {
        println!("[hello] vuelta {}", i);
        sleep_ms(300);
    }
    println!("[hello] fin");
}
