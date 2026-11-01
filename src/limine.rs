//! Interfaz mínima con el protocolo Limine (escrita a mano, sin crates).
//!
//! Un "request" es una estructura con un identificador mágico que Limine
//! busca dentro de nuestro ELF. Si la encuentra, rellena el campo `response`
//! con un puntero a la respuesta antes de saltar a `_start`.

use core::ptr::{addr_of, read_volatile};

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// Revisión base del protocolo. Limine pone el 3er valor a 0 si la soporta.
#[used]
#[link_section = ".requests"]
static mut BASE_REVISION: [u64; 3] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, 3];

pub fn base_revision_supported() -> bool {
    unsafe { read_volatile(addr_of!(BASE_REVISION[2])) == 0 }
}

#[repr(C)]
struct Request<R> {
    id: [u64; 4],
    revision: u64,
    response: *const R,
}

// ---- HHDM: dónde está mapeada toda la RAM física en la mitad alta ----

#[repr(C)]
struct HhdmResponse {
    _revision: u64,
    offset: u64,
}

#[used]
#[link_section = ".requests"]
static mut HHDM_REQUEST: Request<HhdmResponse> = Request {
    id: [COMMON_MAGIC[0], COMMON_MAGIC[1], 0x48dcf1cb8ad2b852, 0x63984e959a98244b],
    revision: 0,
    response: core::ptr::null(),
};

/// Offset tal que `dirección_virtual = dirección_física + offset`.
pub fn hhdm_offset() -> Option<u64> {
    unsafe {
        let r = read_volatile(addr_of!(HHDM_REQUEST.response));
        if r.is_null() { None } else { Some((*r).offset) }
    }
}

// ---- Mapa de memoria física ----

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

#[repr(C)]
struct MemmapResponse {
    _revision: u64,
    entry_count: u64,
    entries: *const *const MemmapEntry,
}

#[used]
#[link_section = ".requests"]
static mut MEMMAP_REQUEST: Request<MemmapResponse> = Request {
    id: [COMMON_MAGIC[0], COMMON_MAGIC[1], 0x67cf3d9d378a806f, 0xe304acdfc50c3c62],
    revision: 0,
    response: core::ptr::null(),
};

pub const MEMMAP_USABLE: u64 = 0;

pub fn kind_name(kind: u64) -> &'static str {
    match kind {
        0 => "usable",
        1 => "reservada",
        2 => "ACPI reclamable",
        3 => "ACPI NVS",
        4 => "defectuosa",
        5 => "bootloader reclamable",
        6 => "kernel y modulos",
        7 => "framebuffer",
        _ => "desconocida",
    }
}

/// Devuelve las entradas del mapa de memoria.
pub fn memmap() -> Option<impl Iterator<Item = &'static MemmapEntry>> {
    unsafe {
        let r = read_volatile(addr_of!(MEMMAP_REQUEST.response));
        if r.is_null() {
            return None;
        }
        let count = (*r).entry_count as usize;
        let entries = (*r).entries;
        Some((0..count).map(move |i| &**entries.add(i)))
    }
}
