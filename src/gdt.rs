//! GDT (Global Descriptor Table) y TSS (Task State Segment).
//!
//! En 64 bits la segmentación casi no se usa, pero la CPU sigue exigiendo una
//! GDT con un segmento de código válido, y el TSS guarda las pilas especiales
//! (IST) que usaremos para el "double fault".

use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CS: u16 = 0x08; // entrada 1
pub const KERNEL_DS: u16 = 0x10; // entrada 2
const TSS_SEL: u16 = 0x18; // entrada 3 (ocupa 2 huecos de 8 bytes)

/// Índice IST (1..=7) que usará el double fault.
pub const DOUBLE_FAULT_IST: u8 = 1;

#[repr(C, packed)]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

#[repr(C, align(16))]
struct Stack([u8; 4096 * 5]);

static mut DOUBLE_FAULT_STACK: Stack = Stack([0; 4096 * 5]);

static mut TSS: Tss = Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: 0,
};

static mut GDT: [u64; 5] = [0; 5];

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

/// # Safety
/// Debe llamarse una sola vez, al principio, con las interrupciones desactivadas.
pub unsafe fn init() {
    // La pila crece hacia abajo: apuntamos al final del bloque.
    let stack_top = (&raw const DOUBLE_FAULT_STACK as u64) + size_of::<Stack>() as u64;
    TSS.ist[(DOUBLE_FAULT_IST - 1) as usize] = stack_top;
    TSS.iomap_base = size_of::<Tss>() as u16; // sin bitmap de E/S

    // Descriptor del TSS (16 bytes en modo largo, dos entradas de la GDT).
    let base = &raw const TSS as u64;
    let limit = (size_of::<Tss>() - 1) as u64;
    let low = (limit & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x89u64 << 40) // presente, tipo "TSS 64 bits disponible"
        | (((limit >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;

    GDT = [
        0,                  // 0x00: descriptor nulo obligatorio
        0x00AF_9A00_0000_FFFF, // 0x08: código 64 bits, ring 0
        0x00CF_9200_0000_FFFF, // 0x10: datos, ring 0
        low,                // 0x18: TSS (parte baja)
        high,               //       TSS (parte alta)
    ];

    let ptr = DescriptorPointer {
        limit: (size_of::<[u64; 5]>() - 1) as u16,
        base: &raw const GDT as u64,
    };
    asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));

    // Recargar CS no se puede con un `mov`: se hace con un "far return".
    asm!(
        "push {cs}",
        "lea {tmp}, [rip + 2f]",
        "push {tmp}",
        "retfq",
        "2:",
        cs = in(reg) KERNEL_CS as u64,
        tmp = lateout(reg) _,
        options(preserves_flags),
    );
    // Segmentos de datos.
    asm!(
        "mov ds, {0:x}",
        "mov es, {0:x}",
        "mov ss, {0:x}",
        in(reg) KERNEL_DS,
        options(nostack, preserves_flags),
    );
    asm!(
        "mov fs, {0:x}",
        "mov gs, {0:x}",
        in(reg) 0u16,
        options(nostack, preserves_flags),
    );
    // Cargar el registro de tarea con el selector del TSS.
    asm!("ltr {0:x}", in(reg) TSS_SEL, options(nostack, preserves_flags));
}
