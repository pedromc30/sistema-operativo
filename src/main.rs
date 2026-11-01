#![no_std]
#![no_main]

#[macro_use]
mod serial;
mod gdt;
mod idt;

use core::arch::asm;
use core::panic::PanicInfo;

/// Tag de revisión base del protocolo Limine. Si el bootloader soporta la
/// revisión pedida (3), pone el tercer valor a 0.
#[used]
#[link_section = ".requests"]
static mut BASE_REVISION: [u64; 3] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, 3];

fn base_revision_supported() -> bool {
    unsafe { core::ptr::read_volatile(&raw const BASE_REVISION[2]) == 0 }
}

pub fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt") };
    }
}

#[no_mangle]
extern "C" fn _start() -> ! {
    serial::init();
    if !base_revision_supported() {
        println!("Limine base revision no soportada");
        halt();
    }
    println!("Hola desde RustOS (x86_64)!");

    unsafe {
        gdt::init();
        println!("[ok] GDT y TSS cargadas");
        idt::init();
        println!("[ok] IDT cargada (excepciones 0-31)");

        // Prueba de una excepcion recuperable: int3 -> handler -> vuelve aqui.
        asm!("int3");
    }
    println!("[ok] volvimos del breakpoint: las excepciones funcionan");

    // Pruebas de fallos reales (se activan con --features, ver run.sh).
    #[cfg(feature = "crash-pf")]
    unsafe {
        println!("Provocando page fault...");
        let _ = core::ptr::read_volatile(0xdead_beef_000u64 as *const u8);
    }
    #[cfg(feature = "crash-div")]
    unsafe {
        println!("Provocando division por cero...");
        asm!("xor edx, edx", "mov eax, 1", "xor ecx, ecx", "div ecx",
             out("eax") _, out("edx") _, out("ecx") _);
    }
    #[cfg(feature = "crash-ud")]
    unsafe {
        println!("Provocando opcode invalido...");
        asm!("ud2");
    }

    halt();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("KERNEL PANIC: {}", info);
    halt();
}
