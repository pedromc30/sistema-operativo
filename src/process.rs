//! Procesos de usuario: espacio de direcciones propio + una tarea que salta a ring 3.

use crate::{elf, limine, paging, pmm, scheduler};
use core::arch::{asm, global_asm};

pub const USER_CODE: u64 = 0x40_0000; // 4 MiB
pub const USER_STACK_TOP: u64 = 0x80_0000; // 8 MiB (la pila crece hacia abajo)
const USER_STACK_PAGES: u64 = 8;
pub const MAX_ARGS: usize = 256;
const PAGE: u64 = 4096;

/// Crea un proceso a partir de código máquina plano (los programas en ensamblador
/// incrustados): se copia tal cual en USER_CODE.
pub fn spawn_raw(name: &str, code: &[u8]) -> Result<u64, &'static str> {
    let root = paging::new_address_space().ok_or("sin memoria para el espacio de direcciones")?;
    let r = unsafe { build_flat_image(root, code).and_then(|_| build_stack(root, "")) };
    match r {
        Ok((rsp, a0, a1)) => Ok(scheduler::spawn_user(name, user_task_main, root, USER_CODE, rsp, a0, a1)),
        Err(e) => {
            unsafe { paging::destroy_address_space(root) }; // deshace lo ya mapeado
            Err(e)
        }
    }
}

/// Crea un proceso a partir de un ejecutable ELF. `args` se copia en su pila y se
/// le pasa a `_start(args_ptr, args_len)`.
pub fn spawn_elf(name: &str, elf_bytes: &[u8], args: &str) -> Result<u64, alloc::string::String> {
    use alloc::string::String;
    if args.len() > MAX_ARGS {
        return Err(String::from("argumentos demasiado largos"));
    }
    let root = paging::new_address_space().ok_or_else(|| String::from("sin memoria"))?;
    let r = unsafe { elf::load(root, elf_bytes) };
    let entry = match r {
        Ok(e) => e,
        Err(e) => {
            unsafe { paging::destroy_address_space(root) };
            return Err(String::from(e.describe()));
        }
    };
    match unsafe { build_stack(root, args) } {
        Ok((rsp, a0, a1)) => Ok(scheduler::spawn_user(name, user_task_main, root, entry, rsp, a0, a1)),
        Err(e) => {
            unsafe { paging::destroy_address_space(root) };
            Err(String::from(e))
        }
    }
}

/// Solo comprueba si un ELF se puede cargar (crea el espacio de direcciones, carga y lo
/// destruye). No ejecuta nada: sirve para probar el cargador a fondo.
pub fn check_elf(elf_bytes: &[u8]) -> Result<(), &'static str> {
    let root = paging::new_address_space().ok_or("sin memoria")?;
    let r = unsafe { elf::load(root, elf_bytes) };
    unsafe { paging::destroy_address_space(root) };
    r.map(|_| ()).map_err(|e| e.describe())
}

unsafe fn build_flat_image(root: u64, code: &[u8]) -> Result<(), &'static str> {
    let hhdm = limine::hhdm_offset().ok_or("sin HHDM")?;
    // Código: solo lectura y ejecutable (USER sin WRITABLE).
    let pages = (code.len() as u64 + PAGE - 1) / PAGE;
    for i in 0..pages {
        let frame = pmm::alloc_frame().ok_or("sin memoria")?;
        let dst = (frame + hhdm) as *mut u8;
        core::ptr::write_bytes(dst, 0, PAGE as usize); // los frames no vienen a cero
        let off = (i * PAGE) as usize;
        let n = (code.len() - off).min(PAGE as usize);
        core::ptr::copy_nonoverlapping(code.as_ptr().add(off), dst, n);
        if paging::map_page_in(root, USER_CODE + i * PAGE, frame, paging::USER).is_err() {
            pmm::free_frame(frame);
            return Err("no se pudo mapear el codigo");
        }
    }
    Ok(())
}

/// Crea la pila de usuario y copia en ella los argumentos.
/// Devuelve (rsp inicial, puntero a args, longitud de args).
unsafe fn build_stack(root: u64, args: &str) -> Result<(u64, u64, u64), &'static str> {
    let hhdm = limine::hhdm_offset().ok_or("sin HHDM")?;
    // Lectura/escritura y, si la CPU lo soporta, no ejecutable.
    let mut flags = paging::USER | paging::WRITABLE;
    if paging::nx_enabled() {
        flags |= paging::NX;
    }
    let mut top_frame = 0;
    for i in 0..USER_STACK_PAGES {
        let frame = pmm::alloc_frame().ok_or("sin memoria")?;
        core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE as usize);
        let virt = USER_STACK_TOP - (i + 1) * PAGE;
        if paging::map_page_in(root, virt, frame, flags).is_err() {
            pmm::free_frame(frame);
            return Err("no se pudo mapear la pila");
        }
        if i == 0 {
            top_frame = frame;
        }
    }
    // Los argumentos van en la parte más alta de la pila (en la primera página).
    let len = args.len() as u64;
    let args_addr = (USER_STACK_TOP - len) & !15;
    let off = args_addr - (USER_STACK_TOP - PAGE);
    core::ptr::copy_nonoverlapping(args.as_ptr(), (top_frame + hhdm + off) as *mut u8, args.len());
    // rsp ≡ 8 (mod 16) en la entrada, como si hubiésemos llegado con un `call`.
    Ok((args_addr - 8, args_addr, len))
}

/// Lo que ejecuta (en modo kernel) la tarea de un proceso: saltar a modo usuario.
fn user_task_main() {
    let (rip, rsp, arg0, arg1) = scheduler::current_user_entry();
    unsafe { enter_user(rip, rsp, arg0, arg1) }
}

/// Pasa a ring 3 con `iretq`: la CPU saca de la pila SS, RSP, RFLAGS, CS y RIP, y los
/// carga. Al cargar un CS de DPL 3 se pasa al anillo 3.
unsafe fn enter_user(rip: u64, rsp: u64, arg0: u64, arg1: u64) -> ! {
    asm!(
        "mov ds, {ds:x}",
        "mov es, {ds:x}",
        "push {ss}",     // SS  (datos usuario, RPL 3)
        "push {rsp}",    // RSP de usuario
        "push {rflags}", // RFLAGS: IF=1 (las interrupciones siguen activas en usuario)
        "push {cs}",     // CS  (código usuario, RPL 3)
        "push {rip}",    // RIP
        "iretq",
        ds = in(reg) 0x1bu64,
        ss = in(reg) 0x1bu64,
        rsp = in(reg) rsp,
        rflags = in(reg) 0x202u64,
        cs = in(reg) 0x23u64,
        rip = in(reg) rip,
        in("rdi") arg0, // 1er argumento de _start: puntero a los argumentos
        in("rsi") arg1, // 2º argumento: su longitud
        options(noreturn),
    );
}

// ---- Programas de usuario: ensamblador independiente de la posición, ejecutado en ring 3 ----

global_asm!(
    r#"
    .section .rodata.userprogs,"a"

    .macro UWRITE msg, endmsg   # endmsg = constante con la longitud
        lea rdi, [rip + \msg]
        mov esi, offset \endmsg
        mov eax, 1              # SYS_WRITE
        syscall
    .endm
    .macro UEXIT
        xor edi, edi
        xor eax, eax            # SYS_EXIT(0)
        syscall
    .endm

    # --- hello: escribe, duerme 3 veces con una syscall bloqueante y termina ---
    .global user_hello_start
    user_hello_start:
        UWRITE uh_m1, uh_m1_len
        mov r12d, 3
    uh_loop:
        UWRITE uh_m2, uh_m2_len
        mov edi, 500
        mov eax, 2              # SYS_SLEEP_MS(500)
        syscall
        dec r12d
        jnz uh_loop
        UWRITE uh_m3, uh_m3_len
        UEXIT
    uh_m1: .ascii "[usuario] Hola desde el anillo 3 (modo usuario)!\n"
    uh_m1_end:
    .equ uh_m1_len, uh_m1_end - uh_m1
    uh_m2: .ascii "[usuario] durmiendo 500 ms...\n"
    uh_m2_end:
    .equ uh_m2_len, uh_m2_end - uh_m2
    uh_m3: .ascii "[usuario] adios\n"
    uh_m3_end:
    .equ uh_m3_len, uh_m3_end - uh_m3
    .global user_hello_end
    user_hello_end:

    # --- crash: intenta LEER memoria del kernel -> page fault, el kernel lo mata ---
    .global user_crash_start
    user_crash_start:
        UWRITE uc_m1, uc_m1_len
        mov rax, 0xffffffff80000000
        mov rbx, [rax]
        UWRITE uc_m2, uc_m2_len
        UEXIT
    uc_m1: .ascii "[usuario] voy a leer memoria del kernel...\n"
    uc_m1_end:
    .equ uc_m1_len, uc_m1_end - uc_m1
    uc_m2: .ascii "[usuario] ERROR: esto no deberia imprimirse\n"
    uc_m2_end:
    .equ uc_m2_len, uc_m2_end - uc_m2
    .global user_crash_end
    user_crash_end:

    # --- cli: intenta una instruccion privilegiada -> #GP ---
    .global user_cli_start
    user_cli_start:
        UWRITE ul_m1, ul_m1_len
        cli
        UWRITE ul_m2, ul_m2_len
        UEXIT
    ul_m1: .ascii "[usuario] voy a ejecutar CLI (privilegiada)...\n"
    ul_m1_end:
    .equ ul_m1_len, ul_m1_end - ul_m1
    ul_m2: .ascii "[usuario] ERROR: esto no deberia imprimirse\n"
    ul_m2_end:
    .equ ul_m2_len, ul_m2_end - ul_m2
    .global user_cli_end
    user_cli_end:

    # --- ptr: pasa punteros invalidos al kernel; debe rechazarlos sin caerse ---
    .global user_ptr_start
    user_ptr_start:
        UWRITE up_m1, up_m1_len
        mov rdi, 0xffffffff80000000   # direccion del kernel
        mov esi, 16
        mov eax, 1
        syscall
        test rax, rax
        jns up_fail
        mov rdi, 0x1234000            # direccion de usuario SIN mapear
        mov esi, 16
        mov eax, 1
        syscall
        test rax, rax
        jns up_fail
        UWRITE up_ok, up_ok_len
        jmp up_exit
    up_fail:
        UWRITE up_bad, up_bad_len
    up_exit:
        UEXIT
    up_m1: .ascii "[usuario] enviando punteros invalidos al kernel...\n"
    up_m1_end:
    .equ up_m1_len, up_m1_end - up_m1
    up_ok: .ascii "[usuario] OK: el kernel rechazo ambos punteros (devolvio error)\n"
    up_ok_end:
    .equ up_ok_len, up_ok_end - up_ok
    up_bad: .ascii "[usuario] FALLO: el kernel acepto un puntero invalido\n"
    up_bad_end:
    .equ up_bad_len, up_bad_end - up_bad
    .global user_ptr_end
    user_ptr_end:

    .text
    "#
);

macro_rules! blob {
    ($start:ident, $end:ident) => {{
        extern "C" {
            static $start: u8;
            static $end: u8;
        }
        unsafe {
            let s = &raw const $start;
            let e = &raw const $end;
            core::slice::from_raw_parts(s, e as usize - s as usize)
        }
    }};
}

/// Programas de usuario en ensamblador incrustados en el kernel (pruebas de protección).
pub fn program(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "asm-hello" => blob!(user_hello_start, user_hello_end),
        "asm-crash" => blob!(user_crash_start, user_crash_end),
        "cli" => blob!(user_cli_start, user_cli_end),
        "ptr" => blob!(user_ptr_start, user_ptr_end),
        _ => return None,
    })
}

pub const PROGRAMS: &[&str] = &["asm-hello", "asm-crash", "cli", "ptr"];
