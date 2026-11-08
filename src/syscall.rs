//! Llamadas al sistema (instrucción SYSCALL/SYSRET).
//!
//! ABI propia:
//!   rax = nº de llamada; argumentos en rdi, rsi, rdx, r10, r8 (hasta 5);
//!   resultado en rax (negativo = error). El kernel destruye rcx y r11 (los usa la
//!   propia CPU) y los registros de argumento y temporales; conserva rbx, rbp, rsp, r12-r15.
//!
//!    0 exit(código)                       termina el proceso
//!    1 write(ptr, len)                    escribe bytes en la consola
//!    2 sleep_ms(ms)                       duerme sin consumir CPU
//!    3 getpid()                           id del proceso
//!    4 uptime_ms()                        milisegundos desde el arranque
//!    5 brk(dir)                           mueve el fin del heap; brk(0) lo consulta
//!    6 read(ptr, len)                     lee de la consola (bloquea hasta haber algo)
//!    7 spawn(nombre, n, args, m)          ejecuta un programa del initrd; devuelve su id
//!    8 wait(id)                           espera a que termine y devuelve su código
//!    9 fs_list(ptr, len)                  escribe "nombre tamaño\n" por cada archivo
//!   10 fs_read(nombre, n, ptr, len, off)  lee de un archivo del initrd

use crate::{cpu, fs, input, paging, pmm, process, scheduler, serial, timer};
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
        # (rax; rdi, rsi, rdx, r10, r8) -> (rdi; rsi, rdx, rcx, r8, r9) de la ABI de C.
        # El orden importa: cada mov lee un registro antes de que se sobrescriba.
        mov r9, r8
        mov r8, r10
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
const ENOENT: i64 = -2;
const ENOEXEC: i64 = -8;
const EAGAIN: i64 = -11;
const ENOMEM: i64 = -12;

#[no_mangle]
extern "C" fn syscall_dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    match nr {
        0 => scheduler::exit_with(a0 as i64),
        1 => sys_write(a0, a1),
        2 => {
            timer::sleep_ms(a0);
            0
        }
        3 => scheduler::current_id() as i64,
        4 => timer::uptime_ms() as i64,
        5 => sys_brk(a0),
        6 => sys_read(a0, a1),
        7 => sys_spawn(a0, a1, a2, a3),
        8 => scheduler::wait_for_exit(a0),
        9 => sys_fs_list(a0, a1),
        10 => sys_fs_read(a0, a1, a2, a3, a4),
        _ => ENOSYS,
    }
}

/// Texto que viene del usuario: se valida el rango y que sea UTF-8 antes de tocarlo.
fn user_str(ptr: u64, len: u64, max: u64) -> Result<&'static str, i64> {
    if len > max {
        return Err(EINVAL);
    }
    if !paging::user_range_ok(ptr, len) {
        return Err(EFAULT);
    }
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(bytes).map_err(|_| EINVAL)
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

/// brk(dir): mueve el fin del heap del proceso (entre USER_HEAP_BASE y +32 MiB).
/// Devuelve el nuevo fin, o el actual si no se pudo (como el brk de Linux).
fn sys_brk(addr: u64) -> i64 {
    let (start, cur) = scheduler::brk_info();
    if addr == 0 || addr == cur {
        return cur as i64;
    }
    if addr < start || addr > start + process::USER_HEAP_MAX {
        return cur as i64;
    }
    const PAGE: u64 = 4096;
    let up = |x: u64| (x + PAGE - 1) & !(PAGE - 1);
    let (old_top, new_top) = (up(cur), up(addr));

    if new_top > old_top {
        let hhdm = crate::limine::hhdm_offset().unwrap();
        let mut flags = paging::USER | paging::WRITABLE;
        if paging::nx_enabled() {
            flags |= paging::NX;
        }
        let mut page = old_top;
        while page < new_top {
            let ok = match pmm::alloc_frame() {
                Some(frame) => {
                    // Un frame reciclado puede tener datos de OTRO proceso: a cero siempre.
                    unsafe { core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE as usize) };
                    if unsafe { paging::map_page(page, frame, flags) }.is_ok() {
                        true
                    } else {
                        pmm::free_frame(frame);
                        false
                    }
                }
                None => false,
            };
            if !ok {
                // Sin memoria: deshacer lo que se hizo en esta llamada.
                let mut p = old_top;
                while p < page {
                    if let Some(f) = unsafe { paging::unmap_page(p) } {
                        pmm::free_frame(f);
                    }
                    p += PAGE;
                }
                return cur as i64;
            }
            page += PAGE;
        }
    } else if new_top < old_top {
        let mut page = new_top;
        while page < old_top {
            if let Some(f) = unsafe { paging::unmap_page(page) } {
                pmm::free_frame(f);
            }
            page += PAGE;
        }
    }
    scheduler::set_brk(addr);
    addr as i64
}

/// read(ptr, len): bloquea hasta que haya al menos un byte en la consola y devuelve
/// todos los disponibles (hasta `len`).
fn sys_read(ptr: u64, len: u64) -> i64 {
    if len == 0 {
        return 0;
    }
    let len = len.min(4096);
    if !paging::user_range_writable(ptr, len) {
        return EFAULT;
    }
    let buf = ptr as *mut u8;
    let first = input::read_char(); // duerme hasta que llegue algo
    unsafe { buf.write(first) };
    let mut n = 1u64;
    while n < len {
        match input::pop() {
            Some(c) => {
                unsafe { buf.add(n as usize).write(c) };
                n += 1;
            }
            None => break,
        }
    }
    n as i64
}

fn sys_spawn(name_ptr: u64, name_len: u64, args_ptr: u64, args_len: u64) -> i64 {
    let name = match user_str(name_ptr, name_len, 64) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let args = match user_str(args_ptr, args_len, process::MAX_ARGS as u64) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let Some(data) = fs::find(name) else { return ENOENT };
    match process::spawn_elf(name, data, args) {
        Ok(id) => id as i64,
        Err(e) => match e.as_str() {
            "demasiados procesos" => EAGAIN,
            m if m.starts_with("sin memoria") => ENOMEM,
            _ => ENOEXEC,
        },
    }
}

fn sys_fs_list(ptr: u64, len: u64) -> i64 {
    let len = len.min(4096);
    if !paging::user_range_writable(ptr, len) {
        return EFAULT;
    }
    let mut text = alloc::string::String::new();
    for (name, size) in fs::list() {
        text.push_str(&alloc::format!("{} {}\n", name, size));
    }
    let n = text.len().min(len as usize);
    unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), ptr as *mut u8, n) };
    n as i64
}

fn sys_fs_read(name_ptr: u64, name_len: u64, buf: u64, buf_len: u64, offset: u64) -> i64 {
    let name = match user_str(name_ptr, name_len, 64) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let buf_len = buf_len.min(4096);
    if !paging::user_range_writable(buf, buf_len) {
        return EFAULT;
    }
    let Some(data) = fs::find(name) else { return ENOENT };
    let off = (offset as usize).min(data.len());
    let n = (data.len() - off).min(buf_len as usize);
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr().add(off), buf as *mut u8, n) };
    n as i64
}
