//! Llamadas al sistema (instrucción SYSCALL/SYSRET).
//!
//! ABI propia:
//!   rax = nº de llamada; argumentos en rdi, rsi, rdx; resultado en rax
//!   (negativo = error). El kernel destruye rcx y r11 (los usa la propia CPU) y
//!   los registros de argumento; conserva rbx, rbp, rsp, r12-r15.
//!
//!   0 exit(código)          termina el proceso
//!   1 write(ptr, len)       escribe bytes en la consola
//!   2 sleep_ms(ms)          duerme sin consumir CPU
//!   3 getpid()              id del proceso

use crate::{cpu, paging, scheduler, serial, timer};
use core::arch::global_asm;

const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_SFMASK: u32 = 0xC000_0084;

/// Pila de kernel de la tarea actual; la fija el planificador en cada cambio.
#[no_mangle]
static mut SYSCALL_KERNEL_RSP: u64 = 0;
/// Aparcamiento de 2 instrucciones para el rsp de usuario (con IF=0, sin carreras).
#[no_mangle]
static mut SYSCALL_USER_RSP: u64 = 0;

pub fn set_kernel_stack(top: u64) {
    unsafe { SYSCALL_KERNEL_RSP = top };
}

extern "C" {
    fn syscall_entry();
}

// SYSCALL no cambia de pila ni guarda nada salvo: rcx <- rip de usuario,
// r11 <- rflags de usuario. El resto es cosa nuestra: cambiamos a la pila de
// kernel de la tarea, guardamos lo que hay que restaurar y llamamos a Rust.
global_asm!(
    r#"
    .text
    .global syscall_entry
    syscall_entry:
        mov [rip + SYSCALL_USER_RSP], rsp
        mov rsp, [rip + SYSCALL_KERNEL_RSP]
        push qword ptr [rip + SYSCALL_USER_RSP]   # rsp de usuario
        push r11                                  # rflags de usuario
        push rcx                                  # rip de usuario
        sub rsp, 8                                # alinear a 16 antes del call
        # (rax, rdi, rsi, rdx) -> (rdi, rsi, rdx, rcx) de la ABI de C
        mov rcx, rdx
        mov rdx, rsi
        mov rsi, rdi
        mov rdi, rax
        sti                                       # SFMASK las desactivó: permitir expropiación
        call syscall_dispatch
        cli
        add rsp, 8
        pop rcx
        pop r11
        pop rsp                                   # vuelve a la pila de usuario
        sysretq
    "#
);

/// # Safety
/// Después de cargar la GDT con los segmentos de usuario.
pub unsafe fn init() {
    cpu::wrmsr(MSR_EFER, cpu::rdmsr(MSR_EFER) | 1); // EFER.SCE: habilita SYSCALL
    // STAR[47:32] = CS kernel (0x08; SS = +8);  STAR[63:48] = base SYSRET (0x10).
    cpu::wrmsr(MSR_STAR, (0x10u64 << 48) | (0x08u64 << 32));
    cpu::wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);
    cpu::wrmsr(MSR_SFMASK, 0x700); // limpia TF, IF y DF al entrar
}

const EFAULT: i64 = -14;
const EINVAL: i64 = -22;
const ENOSYS: i64 = -38;

#[no_mangle]
extern "C" fn syscall_dispatch(nr: u64, a0: u64, a1: u64, _a2: u64) -> i64 {
    match nr {
        0 => scheduler::exit_current(),
        1 => sys_write(a0, a1),
        2 => {
            timer::sleep_ms(a0);
            0
        }
        3 => scheduler::current_id() as i64,
        _ => ENOSYS,
    }
}

fn sys_write(ptr: u64, len: u64) -> i64 {
    if len > 4096 {
        return EINVAL;
    }
    // NUNCA fiarse de un puntero del usuario: comprobar que todo el rango es suyo.
    if !paging::user_range_ok(ptr, len) {
        return EFAULT;
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    serial::write_bytes(bytes);
    len as i64
}
