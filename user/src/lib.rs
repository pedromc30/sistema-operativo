//! Mini librería de ejecución para programas de usuario de RustOS.
//!
//! ABI de syscalls (ver kernel/src/syscall.rs): rax = número, args en rdi, rsi, rdx;
//! resultado en rax. El kernel destruye rcx, r11 y los registros de argumento/temporales.

#![no_std]

extern crate alloc;

pub mod heap;

use core::arch::asm;
use core::fmt::{self, Write};

#[inline(always)]
fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    syscall5(nr, a0, a1, a2, 0, 0)
}

/// Llamada al sistema cruda (para probar el kernel con argumentos arbitrarios).
pub fn raw_syscall(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    syscall5(nr, a0, a1, a2, a3, a4)
}

/// rax = nº; argumentos en rdi, rsi, rdx, r10, r8 (r10 en lugar de rcx: SYSCALL usa rcx).
#[inline(always)]
fn syscall5(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            inlateout("rax") nr as i64 => ret,
            inlateout("rdi") a0 => _,
            inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _,
            inlateout("r10") a3 => _,
            inlateout("r8") a4 => _,
            lateout("rcx") _, lateout("r11") _, lateout("r9") _,
            options(nostack));
    }
    ret
}

pub fn exit(code: u64) -> ! {
    syscall3(0, code, 0, 0);
    loop {} // el kernel no vuelve de exit
}

pub fn write(bytes: &[u8]) -> i64 {
    syscall3(1, bytes.as_ptr() as u64, bytes.len() as u64, 0)
}

pub fn sleep_ms(ms: u64) {
    syscall3(2, ms, 0, 0);
}

pub fn getpid() -> u64 {
    syscall3(3, 0, 0, 0) as u64
}

pub fn uptime_ms() -> u64 {
    syscall3(4, 0, 0, 0) as u64
}

/// Mueve el fin del heap. `brk(0)` lo consulta. Devuelve el fin actual (igual al pedido
/// si se pudo; si no, el que ya había).
pub fn brk(addr: u64) -> u64 {
    syscall3(5, addr, 0, 0) as u64
}

/// Lee de la consola: bloquea hasta que haya algo y devuelve cuántos bytes ha puesto en `buf`.
pub fn read(buf: &mut [u8]) -> i64 {
    syscall3(6, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

/// Ejecuta un programa del initrd. Devuelve su id (>= 0) o un error (< 0).
pub fn spawn(name: &str, args: &str) -> i64 {
    syscall5(7, name.as_ptr() as u64, name.len() as u64, args.as_ptr() as u64, args.len() as u64, 0)
}

/// Espera a que termine el proceso `pid` y devuelve su código de salida.
pub fn wait(pid: u64) -> i64 {
    syscall3(8, pid, 0, 0)
}

/// Escribe en `buf` una línea "nombre tamaño" por archivo del initrd.
pub fn fs_list(buf: &mut [u8]) -> i64 {
    syscall3(9, buf.as_mut_ptr() as u64, buf.len() as u64, 0)
}

/// Lee de un archivo del initrd a partir de `offset`. Devuelve los bytes leídos (0 = fin).
pub fn fs_read(name: &str, buf: &mut [u8], offset: u64) -> i64 {
    syscall5(10, name.as_ptr() as u64, name.len() as u64, buf.as_mut_ptr() as u64, buf.len() as u64, offset)
}

pub fn strerror(code: i64) -> &'static str {
    match code {
        -2 => "no existe el archivo",
        -8 => "no es un ejecutable valido",
        -10 => "no existe ese proceso (o ya se recogio su salida)",
        -11 => "demasiados procesos",
        -12 => "sin memoria",
        -14 => "puntero invalido",
        -22 => "argumento invalido",
        -38 => "llamada al sistema desconocida",
        _ => "error desconocido",
    }
}

// ---- print!/println!: se formatea en un búfer y se envía en UNA sola syscall ----

struct Buf {
    data: [u8; 256],
    len: usize,
}

impl Buf {
    fn flush(&mut self) {
        if self.len > 0 {
            write(&self.data[..self.len]);
            self.len = 0;
        }
    }
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.len == self.data.len() {
                self.flush();
            }
            self.data[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let mut b = Buf { data: [0; 256], len: 0 };
    let _ = b.write_fmt(args);
    b.flush();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::print!("{}\n", format_args!($($arg)*)) };
}

/// Define el punto de entrada `_start`. El kernel pasa los argumentos como texto.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn _start(args: *const u8, len: usize) -> ! {
            let args = unsafe { core::slice::from_raw_parts(args, len) };
            let args = core::str::from_utf8(args).unwrap_or("");
            $main(args);
            $crate::exit(0)
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[panic] {}", info);
    exit(101)
}
