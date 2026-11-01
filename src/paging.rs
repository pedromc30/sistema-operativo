//! Paginación x86_64 de 4 niveles.
//!
//! Seguimos usando las tablas que dejó Limine (CR3), pero ahora podemos
//! consultarlas (`translate`) y añadir mapeos nuevos (`map_page`).
//! Las tablas son frames físicos; para tocarlas las leemos por el HHDM.

use crate::{limine, pmm};
use core::arch::asm;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2; // accesible desde modo usuario (debe estar en TODOS los niveles)
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

/// ¿Está activo el bit NX en la CPU? (EFER.NXE, bit 11). Si no, poner NX en una
/// entrada provocaría un fallo por "bit reservado".
pub fn nx_enabled() -> bool {
    crate::cpu::rdmsr(0xC000_0080) & (1 << 11) != 0
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

/// Mapea una página en el espacio de direcciones ACTUAL.
///
/// # Safety
/// Ver `map_page_in`.
pub unsafe fn map_page(virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    map_page_in(read_cr3() & ADDR_MASK, virt, phys, flags)
}

/// Mapea la página virtual `virt` al frame físico `phys` en el espacio de
/// direcciones cuya tabla raíz (PML4) está en el frame `root`.
///
/// Si `flags` incluye USER, las tablas intermedias también se marcan USER (la CPU
/// exige el permiso en todos los niveles).
///
/// # Safety
/// `virt` y `phys` alineados a 4 KiB; `phys` debe ser un frame que podamos usar.
pub unsafe fn map_page_in(root: u64, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    debug_assert!(virt % PAGE == 0 && phys % PAGE == 0);
    let user = flags & USER;
    let mut tbl = root;

    for level in (2..=4).rev() {
        let entry = table(tbl).add(index(virt, level));
        let e = *entry;
        if e & PRESENT == 0 {
            let frame = pmm::alloc_frame().ok_or(MapError::OutOfMemory)?;
            core::ptr::write_bytes(table(frame), 0, 512);
            *entry = frame | PRESENT | WRITABLE | user;
            tbl = frame;
        } else if e & HUGE != 0 {
            return Err(MapError::InsideHugePage);
        } else {
            if user != 0 && e & USER == 0 {
                *entry = e | USER;
            }
            tbl = e & ADDR_MASK;
        }
    }

    let leaf = table(tbl).add(index(virt, 1));
    if *leaf & PRESENT != 0 {
        return Err(MapError::AlreadyMapped);
    }
    *leaf = phys | flags | PRESENT;
    asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    Ok(())
}

pub unsafe fn write_cr3(phys: u64) {
    asm!("mov cr3, {}", in(reg) phys, options(nostack, preserves_flags));
}

/// Crea un espacio de direcciones nuevo: una PML4 vacía en la mitad baja (usuario)
/// que COMPARTE la mitad alta (kernel, HHDM, heap) con el espacio actual.
/// Devuelve la dirección física de la PML4.
pub fn new_address_space() -> Option<u64> {
    let frame = pmm::alloc_frame()?;
    unsafe {
        let new = table(frame);
        let cur = table(read_cr3() & ADDR_MASK);
        core::ptr::write_bytes(new, 0, 256); // mitad baja: vacía
        for i in 256..512 {
            *new.add(i) = *cur.add(i); // mitad alta: la misma que el kernel
        }
    }
    Some(frame)
}

unsafe fn free_tables(tbl: u64, level: u32) {
    let t = table(tbl);
    for i in 0..512 {
        let e = *t.add(i);
        if e & PRESENT == 0 || e & HUGE != 0 {
            continue; // nunca creamos páginas grandes en el espacio de usuario
        }
        if level == 1 {
            pmm::free_frame(e & ADDR_MASK); // página de datos del proceso
        } else {
            free_tables(e & ADDR_MASK, level - 1);
        }
    }
    pmm::free_frame(tbl);
}

/// Libera todo lo que el proceso tenía en la mitad baja (páginas y tablas) y la PML4.
/// La mitad alta NO se toca: es del kernel y está compartida.
///
/// # Safety
/// `root` no puede ser el CR3 activo.
pub unsafe fn destroy_address_space(root: u64) {
    let pml4 = table(root);
    for i in 0..256 {
        let e = *pml4.add(i);
        if e & PRESENT != 0 {
            free_tables(e & ADDR_MASK, 3);
        }
    }
    pmm::free_frame(root);
}

/// ¿Puede el modo usuario acceder a `virt` (presente y USER en todos los niveles)?
fn user_accessible(virt: u64) -> bool {
    if virt >= 0x0000_8000_0000_0000 {
        return false; // mitad alta: del kernel
    }
    let mut tbl = read_cr3() & ADDR_MASK;
    for level in (1..=4).rev() {
        let e = unsafe { *table(tbl).add(index(virt, level)) };
        if e & PRESENT == 0 || e & USER == 0 {
            return false;
        }
        if level == 1 || (level <= 3 && e & HUGE != 0) {
            return true;
        }
        tbl = e & ADDR_MASK;
    }
    false
}

/// Valida un puntero que viene del usuario: TODO el rango [ptr, ptr+len) debe ser
/// memoria de usuario mapeada. Si no se hiciera, el kernel podría leer memoria suya
/// (o fallar él mismo) por orden de un programa malicioso.
pub fn user_range_ok(ptr: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let Some(end) = ptr.checked_add(len) else { return false };
    let mut page = ptr & !(PAGE - 1);
    while page < end {
        if !user_accessible(page) {
            return false;
        }
        page += PAGE;
    }
    true
}
