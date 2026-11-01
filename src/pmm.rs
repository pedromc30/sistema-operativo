//! Gestor de memoria física (PMM): reparte "frames" (marcos) de 4 KiB.
//!
//! Usa un bitmap: 1 bit por frame (1 = ocupado, 0 = libre). El propio bitmap
//! se guarda en un trozo de RAM usable y se accede a través del HHDM.

use crate::limine::{self, MEMMAP_USABLE};

pub const FRAME_SIZE: u64 = 4096;

struct Pmm {
    bitmap: *mut u64, // dirección VIRTUAL (vía HHDM) del bitmap
    frames: u64,      // nº total de frames cubiertos (hasta la dirección más alta)
    free: u64,        // frames libres ahora mismo
    hint: u64,        // desde dónde empezar a buscar
}

static mut PMM: Pmm = Pmm { bitmap: core::ptr::null_mut(), frames: 0, free: 0, hint: 0 };

fn align_up(x: u64, a: u64) -> u64 { (x + a - 1) & !(a - 1) }
fn align_down(x: u64, a: u64) -> u64 { x & !(a - 1) }

impl Pmm {
    unsafe fn set(&mut self, frame: u64, used: bool) {
        let w = self.bitmap.add((frame / 64) as usize);
        let mask = 1u64 << (frame % 64);
        if used { *w |= mask } else { *w &= !mask }
    }
    unsafe fn is_used(&self, frame: u64) -> bool {
        *self.bitmap.add((frame / 64) as usize) & (1u64 << (frame % 64)) != 0
    }
}

/// # Safety
/// Una sola vez, tras cargar GDT/IDT. Requiere respuesta de Limine para HHDM y memmap.
pub unsafe fn init() {
    let hhdm = limine::hhdm_offset().expect("Limine no dio el HHDM");
    let map = || limine::memmap().expect("Limine no dio el mapa de memoria");

    // 1) Dirección física más alta de RAM usable -> cuántos frames necesitamos.
    let top = map()
        .filter(|e| e.kind == MEMMAP_USABLE)
        .map(|e| e.base + e.length)
        .max()
        .expect("no hay memoria usable");
    let frames = top / FRAME_SIZE;
    let bitmap_bytes = align_up((frames + 63) / 64 * 8, FRAME_SIZE);

    // 2) Buscar una región usable donde quepa el bitmap.
    let bitmap_phys = map()
        .filter(|e| e.kind == MEMMAP_USABLE && e.length >= bitmap_bytes && e.base >= 0x10_0000)
        .map(|e| e.base)
        .next()
        .expect("ninguna region usable es lo bastante grande para el bitmap");

    let p = &mut *(&raw mut PMM);
    p.bitmap = (bitmap_phys + hhdm) as *mut u64;
    p.frames = frames;

    // 3) Todo ocupado por defecto...
    core::ptr::write_bytes(p.bitmap as *mut u8, 0xFF, bitmap_bytes as usize);

    // 4) ...y liberamos solo la memoria USABLE (frames completos).
    for e in map().filter(|e| e.kind == MEMMAP_USABLE) {
        let start = align_up(e.base, FRAME_SIZE) / FRAME_SIZE;
        let end = align_down(e.base + e.length, FRAME_SIZE) / FRAME_SIZE;
        for f in start..end {
            p.set(f, false);
            p.free += 1;
        }
    }

    // 5) Reservar: el frame 0 (para que "dirección 0" nunca sea válida)
    //    y los frames que ocupa el propio bitmap.
    if !p.is_used(0) { p.set(0, true); p.free -= 1; }
    let b0 = bitmap_phys / FRAME_SIZE;
    for f in b0..b0 + bitmap_bytes / FRAME_SIZE {
        if !p.is_used(f) { p.set(f, true); p.free -= 1; }
    }
    p.hint = 1;
}

/// Reserva un frame y devuelve su dirección FÍSICA.
pub fn alloc_frame() -> Option<u64> {
    unsafe {
        let p = &mut *(&raw mut PMM);
        for i in 0..p.frames {
            let f = (p.hint + i) % p.frames;
            if !p.is_used(f) {
                p.set(f, true);
                p.free -= 1;
                p.hint = f + 1;
                return Some(f * FRAME_SIZE);
            }
        }
        None
    }
}

/// Devuelve un frame al gestor.
pub fn free_frame(phys: u64) {
    unsafe {
        let p = &mut *(&raw mut PMM);
        let f = phys / FRAME_SIZE;
        assert!(f < p.frames && p.is_used(f), "free_frame: frame invalido o ya libre");
        p.set(f, false);
        p.free += 1;
        if f < p.hint { p.hint = f; }
    }
}

pub fn free_frames() -> u64 { unsafe { (*(&raw const PMM)).free } }
pub fn total_frames() -> u64 { unsafe { (*(&raw const PMM)).frames } }
