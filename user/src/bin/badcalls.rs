//! Lanza cada syscall con argumentos hostiles. El kernel debe devolver un error y seguir vivo.

#![no_std]
#![no_main]

use user::{entry, println, raw_syscall as sys, strerror};

entry!(main);

const KERNEL: u64 = 0xffff_ffff_8000_0000; // dirección del kernel
const CODE_RO: u64 = 0x40_0000; // nuestro propio código: mapeado pero SOLO LECTURA
const STACK_END: u64 = 0x80_0000; // justo encima del final de la pila: sin mapear
const UNMAPPED: u64 = 0x1234_0000; // zona de usuario sin mapear

static mut ERRORS: u32 = 0;

fn check(nombre: &str, got: i64, esperado: i64) {
    let ok = got == esperado;
    if !ok {
        unsafe { ERRORS += 1 };
    }
    let marca = if ok { "ok" } else { "FALLO" };
    if got < 0 {
        println!("  [{}] {:<52} -> {:>4} ({})", marca, nombre, got, strerror(got));
    } else {
        println!("  [{}] {:<52} -> {:#x}", marca, nombre, got);
    }
}

fn main(_args: &str) {
    let hello = "hello";
    let hp = hello.as_ptr() as u64;
    let mut buf = [0u8; 16];
    let bp = buf.as_mut_ptr() as u64;

    println!("[badcalls] punteros hostiles contra cada syscall (esperado: error, nunca caida):");
    // write (1)
    check("write(puntero del kernel)", sys(1, KERNEL, 16, 0, 0, 0), -14);
    check("write(rango que cruza el final de la pila)", sys(1, STACK_END - 8, 32, 0, 0, 0), -14);
    check("write(ptr + len desborda 64 bits)", sys(1, u64::MAX - 8, 100, 0, 0, 0), -14);
    check("write(memoria de usuario sin mapear)", sys(1, UNMAPPED, 8, 0, 0, 0), -14);
    check("write(len enorme)", sys(1, bp, 1 << 40, 0, 0, 0), -22);
    // read (6)
    check("read(buffer en el kernel)", sys(6, KERNEL, 16, 0, 0, 0), -14);
    check("read(buffer en NUESTRO codigo: solo lectura)", sys(6, CODE_RO, 16, 0, 0, 0), -14);
    check("read(buffer sin mapear)", sys(6, UNMAPPED, 16, 0, 0, 0), -14);
    // fs_list (9)
    check("fs_list(buffer en el kernel)", sys(9, KERNEL, 100, 0, 0, 0), -14);
    check("fs_list(buffer en codigo de solo lectura)", sys(9, CODE_RO, 100, 0, 0, 0), -14);
    // fs_read (10)
    check("fs_read(nombre en el kernel)", sys(10, KERNEL, 5, bp, 16, 0), -14);
    check("fs_read(buffer en el kernel)", sys(10, hp, 5, KERNEL, 16, 0), -14);
    check("fs_read(archivo que no existe)", sys(10, hp, 3, bp, 16, 0), -2);
    check("fs_read(offset gigante: no falla, lee 0 bytes)", sys(10, hp, 5, bp, 16, u64::MAX), 0);
    // spawn (7)
    check("spawn(nombre en el kernel)", sys(7, KERNEL, 5, hp, 0, 0), -14);
    check("spawn(argumentos en el kernel)", sys(7, hp, 5, KERNEL, 8, 0), -14);
    check("spawn(nombre de 1000 bytes)", sys(7, hp, 1000, hp, 0, 0), -22);
    check("spawn(argumentos de 100000 bytes)", sys(7, hp, 5, hp, 100_000, 0), -22);
    // wait (8)
    check("wait(proceso que no existe)", sys(8, 1 << 60, 0, 0, 0, 0), -10);
    check("wait(yo mismo)", sys(8, sys(3, 0, 0, 0, 0, 0) as u64, 0, 0, 0, 0), -10);
    // brk (5)
    let cur = sys(5, 0, 0, 0, 0, 0);
    check("brk(por debajo de la base): no cambia", sys(5, 1, 0, 0, 0, 0), cur);
    check("brk(base + 33 MiB, pasa el limite): no cambia", sys(5, cur as u64 + 33 * 1024 * 1024, 0, 0, 0, 0), cur);
    check("brk(direccion absurda): no cambia", sys(5, u64::MAX, 0, 0, 0, 0), cur);
    // llamadas inexistentes
    check("syscall 999 (no existe)", sys(999, 0, 0, 0, 0, 0), -38);
    check("syscall u64::MAX (no existe)", sys(u64::MAX, 0, 0, 0, 0, 0), -38);

    let errores = unsafe { ERRORS };
    println!("[badcalls] {} fallos. El kernel sigue vivo y no ha filtrado memoria.", errores);
    user::exit(if errores == 0 { 0 } else { 1 });
}
