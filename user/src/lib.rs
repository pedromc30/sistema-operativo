//! Mini librería de ejecución para programas de usuario de RustOS.
//!
//! ABI de syscalls (ver kernel/src/syscall.rs): rax = número, args en rdi, rsi, rdx;
//! resultado en rax. El kernel destruye rcx, r11 y los registros de argumento/temporales.

#![no_std]

use core::arch::asm;
use core::fmt::{self, Write};

#[inline(always)]
fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            inlateout("rax") nr as i64 => ret,
            inlateout("rdi") a0 => _,
            inlateout("rsi") a1 => _,
            inlateout("rdx") a2 => _,
            lateout("rcx") _, lateout("r11") _,
            lateout("r8") _, lateout("r9") _, lateout("r10") _,
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
