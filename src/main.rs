#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
mod serial;
mod gdt;
mod heap;
mod idt;
mod limine;
mod paging;
mod pic;
mod pmm;
mod port;
mod timer;

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
    heap_demo();

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

    timer_demo();

    halt();
}

fn timer_demo() {
    println!();
    println!("--- Temporizador (PIT -> IRQ0 -> vector 32) ---");
    unsafe {
        pic::init();
        timer::init();
        asm!("sti"); // a partir de aquí la CPU acepta interrupciones de hardware
    }
    println!("interrupciones activadas, {} Hz (1 tick = {} ms)", timer::HZ, 1000 / timer::HZ);
    let t0 = timer::ticks();
    for s in 1..=5 {
        timer::sleep_ms(1000);
        println!("uptime: {} s  (ticks={}, {} ms)", s, timer::ticks(), timer::uptime_ms());
    }
    println!("5 s esperados -> {} ticks (esperado ~500)", timer::ticks() - t0);
}

fn heap_demo() {
    use alloc::{boxed::Box, string::String, vec::Vec};

    println!();
    println!("--- Paginacion ---");
    let hhdm = limine::hhdm_offset().unwrap();
    println!("CR3 (tabla raiz) = {:#x}   NX activo: {}", paging::read_cr3(), paging::nx_enabled());
    println!("translate(HHDM + 0x1000)   = {:?}", paging::translate(hhdm + 0x1000));
    println!("translate(0xdeadbeef000)   = {:?}", paging::translate(0xdead_beef_000));
    println!("translate(heap, antes)     = {:?}", paging::translate(heap::HEAP_START));

    let frames_before = pmm::free_frames();
    unsafe { heap::init() };
    println!("heap mapeado: {} KiB en {:#x}  (frames usados: {} = paginas + tablas nuevas)",
        heap::HEAP_SIZE / 1024, heap::HEAP_START, frames_before - pmm::free_frames());
    println!("translate(heap, despues)   = {:?}", paging::translate(heap::HEAP_START));
    println!("translate(heap + 1 MiB)    = {:?}  (justo fuera: sin mapear)",
        paging::translate(heap::HEAP_START + heap::HEAP_SIZE));

    println!();
    println!("--- Heap: Box, Vec, String ---");
    let libre0 = heap::free_bytes();
    println!("heap libre al inicio: {} bytes", libre0);

    let caja = Box::new(0x1234_5678_u64);
    println!("Box<u64> en {:p} = {:#x}", &*caja, *caja);

    let mut v: Vec<u64> = Vec::new();
    for i in 0..1000 { v.push(i * i); }
    let suma: u64 = v.iter().sum();
    println!("Vec con {} elementos, suma de cuadrados = {} (esperado 332833500)", v.len(), suma);
    assert_eq!(suma, 332_833_500);

    let texto = alloc::format!("{} + {} = {}", 2, 3, 2 + 3);
    let mut s = String::from("RustOS");
    s.push_str(" con heap: ");
    s.push_str(&texto);
    println!("String: {}", s);

    #[repr(align(64))]
    struct Alineado(#[allow(dead_code)] u8);
    let al = Box::new(Alineado(1));
    let dir = &*al as *const Alineado as usize;
    println!("alineacion 64: direccion {:#x} -> {}", dir, if dir % 64 == 0 { "OK" } else { "MAL" });

    println!("heap libre con todo vivo: {} bytes", heap::free_bytes());
    drop(caja); drop(v); drop(s); drop(al); drop(texto);
    println!("heap libre tras liberar:  {} bytes -> {}", heap::free_bytes(),
        if heap::free_bytes() == libre0 { "OK, sin fugas y bloques fusionados" } else { "FUGA o fragmentacion" });

    // Pedir mas de lo que cabe debe FALLAR de forma controlada, no colgar el kernel.
    let mut grande: Vec<u8> = Vec::new();
    match grande.try_reserve(2 * 1024 * 1024) {
        Ok(_) => println!("ERROR: reservo 2 MiB en un heap de 1 MiB"),
        Err(_) => println!("reservar 2 MiB falla correctamente (heap de 1 MiB)"),
    }
    let mut mediano: Vec<u8> = Vec::new();
    mediano.try_reserve(512 * 1024).expect("512 KiB deberian caber");
    println!("reservar 512 KiB funciona (capacidad {} KiB)", mediano.capacity() / 1024);

    #[cfg(feature = "crash-heap")]
    unsafe {
        println!("Leyendo justo despues del heap (sin mapear)...");
        let _ = core::ptr::read_volatile((heap::HEAP_START + heap::HEAP_SIZE) as *const u8);
    }
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
