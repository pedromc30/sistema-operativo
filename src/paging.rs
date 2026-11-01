//! Paginación x86_64 de 4 niveles.
//!
//! Seguimos usando las tablas que dejó Limine (CR3), pero ahora podemos
//! consultarlas (`translate`) y añadir mapeos nuevos (`map_page`).
//! Las tablas son frames físicos; para tocarlas las leemos por el HHDM.

use crate::{limine, pmm};
use core::arch::asm;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const HUGE: u64 = 1 << 7;
pub const NX: u64 = 1 << 63; // "no ejecutable"; solo válido si EFER.NXE=1

const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const PAGE: u64 = 4096;

#[derive(Debug)]
pub enum MapError {
    AlreadyMapped,
    InsideHugePage,
    OutOfMemory,
}

fn hhdm() -> u64 {
    limine::hhdm_offset().expect("sin HHDM")
}

pub fn read_cr3() -> u64 {
    let v: u64;
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
             options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

/// ¿Está activo el bit NX en la CPU? (EFER.NXE, bit 11). Si no, poner NX en una
/// entrada provocaría un fallo por "bit reservado".
pub fn nx_enabled() -> bool {
    rdmsr(0xC000_0080) & (1 << 11) != 0
}

/// Puntero a la tabla que vive en el frame físico `phys`, vía HHDM.
unsafe fn table(phys: u64) -> *mut u64 {
    (phys + hhdm()) as *mut u64
}

/// Índice (0..511) de `virt` en la tabla del nivel dado (4 = raíz ... 1 = hoja).
fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1FF) as usize
}

/// Traduce una dirección virtual a física recorriendo las tablas. `None` si no está mapeada.
pub fn translate(virt: u64) -> Option<u64> {
    let mut tbl = read_cr3() & ADDR_MASK;
    for level in (1..=4).rev() {
        let e = unsafe { *table(tbl).add(index(virt, level)) };
        if e & PRESENT == 0 {
            return None;
        }
        if level == 3 && e & HUGE != 0 {
            return Some((e & 0x000f_ffff_c000_0000) | (virt & 0x3fff_ffff)); // página de 1 GiB
        }
        if level == 2 && e & HUGE != 0 {
            return Some((e & 0x000f_ffff_ffe0_0000) | (virt & 0x1f_ffff)); // página de 2 MiB
        }
        if level == 1 {
            return Some((e & ADDR_MASK) | (virt & 0xfff));
        }
        tbl = e & ADDR_MASK;
    }
    None
}

/// Mapea la página virtual `virt` al frame físico `phys` con las banderas dadas.
///
/// # Safety
/// `virt` y `phys` deben estar alineados a 4 KiB y `phys` debe ser un frame
/// que podamos usar (p. ej. recién obtenido de `pmm::alloc_frame`).
pub unsafe fn map_page(virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    debug_assert!(virt % PAGE == 0 && phys % PAGE == 0);
    let mut tbl = read_cr3() & ADDR_MASK;

    // Bajamos por los niveles 4, 3 y 2; si falta una tabla intermedia, la creamos.
    for level in (2..=4).rev() {
        let entry = table(tbl).add(index(virt, level));
        let e = *entry;
        if e & PRESENT == 0 {
            let frame = pmm::alloc_frame().ok_or(MapError::OutOfMemory)?;
            core::ptr::write_bytes(table(frame), 0, 512); // tabla vacía
            // En los niveles intermedios damos permisos amplios; el recorte
            // real de permisos se hace en la entrada hoja.
            *entry = frame | PRESENT | WRITABLE;
            tbl = frame;
        } else if e & HUGE != 0 {
            return Err(MapError::InsideHugePage);
        } else {
            tbl = e & ADDR_MASK;
        }
    }

    let leaf = table(tbl).add(index(virt, 1));
    if *leaf & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *leaf = phys | flags | PRESENT;
    // Invalida la caché de traducciones (TLB) para esa página.
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    Ok(())
}
