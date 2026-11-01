//! Cargador de ejecutables ELF64 (estáticos, x86_64, tipo ET_EXEC).
//!
//! Un ELF dice, mediante "program headers" (PT_LOAD), qué trozos del archivo van a qué
//! direcciones virtuales, con qué permisos y cuánta memoria ocupan. El cargador crea esas
//! páginas en un espacio de direcciones nuevo y copia los bytes.
//!
//! El archivo es una ENTRADA NO FIABLE: cada campo se valida antes de usarlo.

use crate::{limine, paging, pmm};

pub const IMAGE_BASE: u64 = 0x40_0000; // el programa debe caer en [IMAGE_BASE, IMAGE_LIMIT)
pub const IMAGE_LIMIT: u64 = 0x70_0000;
const PAGE: u64 = 4096;
const MAX_PHDRS: usize = 16;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

#[derive(Debug)]
pub enum ElfError {
    TooShort,
    BadMagic,
    Not64BitLittleEndian,
    NotExecutable,
    WrongArch,
    BadProgramHeaders,
    SegmentOutOfFile,
    SegmentOutOfRange,
    SegmentMisaligned,
    SegmentWritableAndExecutable,
    SegmentsOverlap,
    NoLoadableSegments,
    EntryNotInCode,
    OutOfMemory,
}

impl ElfError {
    pub fn describe(&self) -> &'static str {
        match self {
            ElfError::TooShort => "archivo demasiado corto",
            ElfError::BadMagic => "no es un ELF (firma incorrecta)",
            ElfError::Not64BitLittleEndian => "no es ELF de 64 bits little-endian",
            ElfError::NotExecutable => "no es un ejecutable estatico (ET_EXEC)",
            ElfError::WrongArch => "no es x86_64",
            ElfError::BadProgramHeaders => "tabla de program headers invalida",
            ElfError::SegmentOutOfFile => "un segmento apunta fuera del archivo",
            ElfError::SegmentOutOfRange => "un segmento cae fuera de la zona de usuario permitida",
            ElfError::SegmentMisaligned => "segmento mal alineado (vaddr y offset no coinciden)",
            ElfError::SegmentWritableAndExecutable => "segmento escribible Y ejecutable (W^X)",
            ElfError::SegmentsOverlap => "segmentos solapados",
            ElfError::NoLoadableSegments => "no hay segmentos cargables",
            ElfError::EntryNotInCode => "el punto de entrada no esta en codigo ejecutable",
            ElfError::OutOfMemory => "sin memoria",
        }
    }
}

fn rd16(d: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(off..off.checked_add(2)?)?.try_into().ok()?))
}
fn rd32(d: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(off..off.checked_add(4)?)?.try_into().ok()?))
}
fn rd64(d: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(off..off.checked_add(8)?)?.try_into().ok()?))
}

fn align_up(x: u64) -> u64 {
    (x + PAGE - 1) & !(PAGE - 1)
}

/// Carga el ELF en el espacio de direcciones `root` y devuelve el punto de entrada.
///
/// Si falla puede dejar páginas ya mapeadas en `root`; quien llama debe destruir ese
/// espacio de direcciones (lo libera todo).
///
/// # Safety
/// `root` debe ser una PML4 válida creada con `paging::new_address_space`.
pub unsafe fn load(root: u64, d: &[u8]) -> Result<u64, ElfError> {
    use ElfError::*;
    if d.len() < 64 {
        return Err(TooShort);
    }
    if &d[0..4] != b"\x7fELF" {
        return Err(BadMagic);
    }
    if d[4] != 2 || d[5] != 1 {
        return Err(Not64BitLittleEndian);
    }
    if rd16(d, 16) != Some(2) {
        return Err(NotExecutable);
    }
    if rd16(d, 18) != Some(0x3E) {
        return Err(WrongArch);
    }
    let entry = rd64(d, 24).ok_or(TooShort)?;
    let phoff = rd64(d, 32).ok_or(TooShort)? as usize;
    let phentsize = rd16(d, 54).ok_or(TooShort)? as usize;
    let phnum = rd16(d, 56).ok_or(TooShort)? as usize;
    if phentsize != 56 || phnum == 0 || phnum > MAX_PHDRS {
        return Err(BadProgramHeaders);
    }

    let hhdm = limine::hhdm_offset().ok_or(OutOfMemory)?;
    let mut loaded = 0;
    let mut entry_ok = false;
    let mut next_free: u64 = IMAGE_BASE; // los segmentos deben ir en orden y sin solaparse

    for i in 0..phnum {
        let ph = phoff.checked_add(i * 56).ok_or(BadProgramHeaders)?;
        let p_type = rd32(d, ph).ok_or(BadProgramHeaders)?;
        if p_type != PT_LOAD {
            continue; // PT_GNU_STACK, PT_NOTE... no nos hacen falta
        }
        let p_flags = rd32(d, ph + 4).ok_or(BadProgramHeaders)?;
        let p_offset = rd64(d, ph + 8).ok_or(BadProgramHeaders)?;
        let p_vaddr = rd64(d, ph + 16).ok_or(BadProgramHeaders)?;
        let p_filesz = rd64(d, ph + 32).ok_or(BadProgramHeaders)?;
        let p_memsz = rd64(d, ph + 40).ok_or(BadProgramHeaders)?;

        if p_memsz == 0 || p_filesz > p_memsz {
            return Err(BadProgramHeaders);
        }
        // El contenido del archivo debe caber DENTRO del archivo.
        let file_end = p_offset.checked_add(p_filesz).ok_or(SegmentOutOfFile)?;
        if file_end > d.len() as u64 {
            return Err(SegmentOutOfFile);
        }
        // Y la memoria debe caer en la zona de usuario permitida.
        let mem_end = p_vaddr.checked_add(p_memsz).ok_or(SegmentOutOfRange)?;
        if p_vaddr < IMAGE_BASE || mem_end > IMAGE_LIMIT {
            return Err(SegmentOutOfRange);
        }
        if p_vaddr % PAGE != p_offset % PAGE {
            return Err(SegmentMisaligned);
        }
        if p_flags & PF_W != 0 && p_flags & PF_X != 0 {
            return Err(SegmentWritableAndExecutable);
        }
        let first_page = p_vaddr & !(PAGE - 1);
        if first_page < next_free {
            return Err(SegmentsOverlap);
        }
        let last_page_end = align_up(mem_end);
        next_free = last_page_end;

        // Permisos de las páginas de este segmento.
        let mut flags = paging::USER;
        if p_flags & PF_W != 0 {
            flags |= paging::WRITABLE;
        }
        if p_flags & PF_X == 0 && paging::nx_enabled() {
            flags |= paging::NX;
        }

        let mut page = first_page;
        while page < last_page_end {
            let frame = pmm::alloc_frame().ok_or(OutOfMemory)?;
            let dst = (frame + hhdm) as *mut u8;
            core::ptr::write_bytes(dst, 0, PAGE as usize); // ceros: aquí queda el .bss

            // Bytes del archivo que caen en esta página: la intersección de
            // [p_vaddr, p_vaddr+filesz) con [page, page+4096).
            let seg_file_start = p_vaddr;
            let seg_file_end = p_vaddr + p_filesz;
            let lo = seg_file_start.max(page);
            let hi = seg_file_end.min(page + PAGE);
            if lo < hi {
                let src_off = (p_offset + (lo - p_vaddr)) as usize;
                let n = (hi - lo) as usize;
                core::ptr::copy_nonoverlapping(
                    d.as_ptr().add(src_off),
                    dst.add((lo - page) as usize),
                    n,
                );
            }
            if let Err(e) = paging::map_page_in(root, page, frame, flags) {
                pmm::free_frame(frame); // este frame aún no cuelga de ninguna tabla
                return Err(match e {
                    paging::MapError::OutOfMemory => OutOfMemory,
                    _ => SegmentsOverlap,
                });
            }
            page += PAGE;
        }

        loaded += 1;
        if p_flags & PF_X != 0 && entry >= p_vaddr && entry < p_vaddr + p_memsz {
            entry_ok = true;
        }
    }

    if loaded == 0 {
        return Err(NoLoadableSegments);
    }
    if !entry_ok {
        return Err(EntryNotInCode);
    }
    Ok(entry)
}
