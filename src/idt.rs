//! IDT (Interrupt Descriptor Table) y manejo de excepciones de la CPU.
//!
//! Flujo: la CPU detecta un error -> consulta la IDT -> salta al "stub" de
//! ensamblador de ese vector -> el stub guarda los registros y llama a
//! `exception_handler` (Rust) -> imprimimos información y paramos (o seguimos).

use crate::gdt;
use core::arch::{asm, global_asm};
use core::mem::size_of;

/// Estado de la CPU en el momento de la excepción. El orden de los campos
/// coincide EXACTAMENTE con el orden en que los stubs empujan a la pila.
#[repr(C)]
pub struct Frame {
    r15: u64, r14: u64, r13: u64, r12: u64,
    r11: u64, r10: u64, r9: u64, r8: u64,
    rbp: u64, rdi: u64, rsi: u64, rdx: u64,
    rcx: u64, rbx: u64, rax: u64,
    vector: u64,
    error_code: u64,
    // Lo siguiente lo empuja la propia CPU:
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

// Cada excepción (0..31) tiene su stub. Unas la CPU empuja un código de error
// y otras no; para que la pila sea uniforme, en las que no, empujamos un 0.
global_asm!(
    r#"
    .macro isr_noerr n
    isr_stub_\n:
        push 0
        push \n
        jmp isr_common
    .endm

    .macro isr_err n
    isr_stub_\n:
        push \n
        jmp isr_common
    .endm

    .irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31
        isr_noerr \n
    .endr
    .irp n, 8,10,11,12,13,14,17,21,29,30
        isr_err \n
    .endr

    isr_common:
        push rax
        push rbx
        push rcx
        push rdx
        push rsi
        push rdi
        push rbp
        push r8
        push r9
        push r10
        push r11
        push r12
        push r13
        push r14
        push r15
        mov rdi, rsp          # 1er argumento: puntero al Frame
        cld
        call exception_handler
        pop r15
        pop r14
        pop r13
        pop r12
        pop r11
        pop r10
        pop r9
        pop r8
        pop rbp
        pop rdi
        pop rsi
        pop rdx
        pop rcx
        pop rbx
        pop rax
        add rsp, 16           # descarta vector + código de error
        iretq

    .section .rodata
    .balign 8
    .global isr_stub_table
    isr_stub_table:
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
        .quad isr_stub_\n
    .endr
    .text
    "#
);

extern "C" {
    static isr_stub_table: [u64; 32];
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl Entry {
    const MISSING: Entry = Entry {
        offset_low: 0, selector: 0, ist: 0, type_attr: 0,
        offset_mid: 0, offset_high: 0, zero: 0,
    };

    fn new(handler: u64, ist: u8) -> Entry {
        Entry {
            offset_low: handler as u16,
            selector: gdt::KERNEL_CS,
            ist,
            type_attr: 0x8E, // presente, ring 0, "interrupt gate"
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            zero: 0,
        }
    }
}

static mut IDT: [Entry; 256] = [Entry::MISSING; 256];

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

/// # Safety
/// Requiere que la GDT ya esté cargada.
pub unsafe fn init() {
    for v in 0..32 {
        // El double fault (8) usa su propia pila (IST) para poder reportarse
        // incluso si la pila normal está corrupta o desbordada.
        let ist = if v == 8 { gdt::DOUBLE_FAULT_IST } else { 0 };
        IDT[v] = Entry::new(isr_stub_table[v], ist);
    }
    let ptr = DescriptorPointer {
        limit: (size_of::<[Entry; 256]>() - 1) as u16,
        base: &raw const IDT as u64,
    };
    asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
}

const NAMES: [&str; 32] = [
    "Division por cero (#DE)", "Debug (#DB)", "NMI", "Breakpoint (#BP)",
    "Overflow (#OF)", "Rango excedido (#BR)", "Opcode invalido (#UD)",
    "Dispositivo no disponible (#NM)", "DOUBLE FAULT (#DF)", "Coprocessor overrun",
    "TSS invalido (#TS)", "Segmento no presente (#NP)", "Fallo de pila (#SS)",
    "Proteccion general (#GP)", "PAGE FAULT (#PF)", "Reservada",
    "x87 (#MF)", "Alineacion (#AC)", "Machine check (#MC)", "SIMD (#XM)",
    "Virtualizacion (#VE)", "Control protection (#CP)", "Reservada", "Reservada",
    "Reservada", "Reservada", "Reservada", "Reservada",
    "Hypervisor injection (#HV)", "VMM communication (#VC)", "Seguridad (#SX)", "Reservada",
];

#[no_mangle]
extern "C" fn exception_handler(f: &mut Frame) {
    let name = NAMES.get(f.vector as usize).copied().unwrap_or("Desconocida");

    // El breakpoint es una "trampa": se puede continuar tras ella.
    if f.vector == 3 {
        println!("[#BP] breakpoint en rip={:#x}; continuamos", f.rip);
        return;
    }

    println!();
    println!("=== EXCEPCION DE LA CPU: {} ===", name);
    println!("vector={}  error_code={:#x}", f.vector, f.error_code);
    println!("rip={:#018x}  cs={:#x}  rflags={:#x}", f.rip, f.cs, f.rflags);
    println!("rsp={:#018x}  ss={:#x}", f.rsp, f.ss);
    println!("rax={:#018x} rbx={:#018x} rcx={:#018x}", f.rax, f.rbx, f.rcx);
    println!("rdx={:#018x} rsi={:#018x} rdi={:#018x}", f.rdx, f.rsi, f.rdi);
    println!("rbp={:#018x}  r8={:#018x}  r9={:#018x}", f.rbp, f.r8, f.r9);
    println!("r10={:#018x} r11={:#018x} r12={:#018x}", f.r10, f.r11, f.r12);
    println!("r13={:#018x} r14={:#018x} r15={:#018x}", f.r13, f.r14, f.r15);

    if f.vector == 14 {
        let cr2: u64;
        unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags)) };
        println!("Direccion que provoco el fallo (CR2) = {:#x}", cr2);
        println!(
            "Causa: {} | {} | {}{}",
            if f.error_code & 1 != 0 { "violacion de proteccion" } else { "pagina NO presente" },
            if f.error_code & 2 != 0 { "escritura" } else { "lectura" },
            if f.error_code & 4 != 0 { "desde modo usuario" } else { "desde modo kernel" },
            if f.error_code & 16 != 0 { " | fetch de instruccion" } else { "" },
        );
    }

    println!("Sistema detenido.");
    crate::halt();
}
