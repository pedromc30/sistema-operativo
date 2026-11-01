#![no_std]
#![no_main]

#[macro_use]
mod serial;
mod gdt;
mod idt;
mod limine;
mod pmm;

use core::arch::asm;
use core::panic::PanicInfo;

pub fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt") };
    }
}

#[no_mangle]
extern "C" fn _start() -> ! {
    serial::init();
    if !limine::base_revision_supported() {
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

    memory_demo();

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

fn memory_demo() {
    println!();
    println!("--- Mapa de memoria fisica (Limine) ---");
    let mut usable: u64 = 0;
    for e in limine::memmap().expect("sin memmap") {
        println!("{:#012x} - {:#012x}  {:>9} KiB  {}",
            e.base, e.base + e.length, e.length / 1024, limine::kind_name(e.kind));
        if e.kind == limine::MEMMAP_USABLE { usable += e.length; }
    }
    println!("HHDM offset = {:#x}", limine::hhdm_offset().expect("sin HHDM"));
    println!("Memoria usable segun el mapa: {} MiB", usable / (1024 * 1024));

    unsafe { pmm::init() };
    println!();
    println!("--- Gestor de memoria fisica (bitmap) ---");
    println!("frames totales cubiertos: {}  libres: {}  (= {} MiB libres)",
        pmm::total_frames(), pmm::free_frames(), pmm::free_frames() * pmm::FRAME_SIZE / (1024 * 1024));

    // Prueba: reservar 3 frames, escribir en uno vía HHDM, liberar y reutilizar.
    let before = pmm::free_frames();
    let a = pmm::alloc_frame().expect("sin memoria");
    let b = pmm::alloc_frame().expect("sin memoria");
    let c = pmm::alloc_frame().expect("sin memoria");
    println!("alloc -> {:#x}, {:#x}, {:#x}  (libres: {})", a, b, c, pmm::free_frames());
    let virt = (a + limine::hhdm_offset().unwrap()) as *mut u64;
    unsafe {
        virt.write_volatile(0xCAFE_F00D_1234_5678);
        let leido = virt.read_volatile();
        println!("escribi y lei en {:#x} (virt {:#x}): {:#x}", a, virt as u64, leido);
        assert_eq!(leido, 0xCAFE_F00D_1234_5678);
    }
    pmm::free_frame(b);
    let b2 = pmm::alloc_frame().unwrap();
    println!("free({:#x}) y alloc -> {:#x} (se reutiliza: {})", b, b2, b == b2);
    pmm::free_frame(a);
    pmm::free_frame(b2);
    pmm::free_frame(c);
    println!("libres antes={} despues={} -> {}", before, pmm::free_frames(),
        if before == pmm::free_frames() { "OK, sin fugas" } else { "FUGA!" });
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("KERNEL PANIC: {}", info);
    halt();
}
