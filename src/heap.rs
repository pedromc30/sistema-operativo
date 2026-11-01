//! Heap del kernel: una región virtual mapeada con frames del PMM, más un
//! allocator global (lista libre ordenada por dirección, con fusión de bloques)
//! que habilita `Box`, `Vec`, `String`, etc.

use crate::{paging, pmm};
use alloc::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::mem::size_of;
use core::ptr::null_mut;

pub const HEAP_START: u64 = 0xffff_c000_0000_0000;
pub const HEAP_SIZE: u64 = 1024 * 1024; // 1 MiB (fijo por ahora)

/// Cabecera de un bloque LIBRE; vive dentro del propio bloque.
#[repr(C)]
struct Node {
    size: usize,
    next: *mut Node,
}
const NODE: usize = size_of::<Node>(); // 16 bytes: todos los bloques son múltiplos de 16

fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

struct FreeList {
    head: *mut Node,
}

impl FreeList {
    /// Inserta un bloque libre manteniendo el orden por dirección y fusionando vecinos.
    unsafe fn insert(&mut self, addr: usize, size: usize) {
        let mut prev: *mut Node = null_mut();
        let mut cur = self.head;
        while !cur.is_null() && (cur as usize) < addr {
            prev = cur;
            cur = (*cur).next;
        }
        let node = addr as *mut Node;
        node.write(Node { size, next: cur });
        if prev.is_null() { self.head = node } else { (*prev).next = node }

        // Fusionar con el siguiente si son contiguos.
        if !cur.is_null() && addr + size == cur as usize {
            (*node).size += (*cur).size;
            (*node).next = (*cur).next;
        }
        // Fusionar con el anterior si son contiguos.
        if !prev.is_null() && prev as usize + (*prev).size == addr {
            (*prev).size += (*node).size;
            (*prev).next = (*node).next;
        }
    }

    /// First-fit: primer bloque donde quepa (con la alineación pedida).
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = align_up(layout.size().max(NODE), NODE);
        let align = layout.align().max(NODE);
        let mut prev: *mut Node = null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            let start = cur as usize;
            let rsize = (*cur).size;
            let next = (*cur).next;
            let a = align_up(start, align);
            let end = a + size;
            if end <= start + rsize {
                // Sacamos el bloque de la lista y devolvemos lo que sobra (delante y detrás).
                if prev.is_null() { self.head = next } else { (*prev).next = next }
                let front = a - start;
                let back = start + rsize - end;
                if front > 0 { self.insert(start, front) }
                if back > 0 { self.insert(end, back) }
                return a as *mut u8;
            }
            prev = cur;
            cur = next;
        }
        null_mut()
    }

    unsafe fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut cur = self.head;
        while !cur.is_null() {
            total += (*cur).size;
            cur = (*cur).next;
        }
        total
    }
}

/// Envoltorio para poder tener un `static`. OJO: no hay cerrojo; es correcto
/// solo mientras haya un único hilo y los manejadores de interrupción no reserven memoria.
pub struct Heap(UnsafeCell<FreeList>);
unsafe impl Sync for Heap {}

#[global_allocator]
static ALLOCATOR: Heap = Heap(UnsafeCell::new(FreeList { head: null_mut() }));

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        (*self.0.get()).alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = align_up(layout.size().max(NODE), NODE);
        (*self.0.get()).insert(ptr as usize, size);
    }
}

pub fn free_bytes() -> usize {
    unsafe { (*ALLOCATOR.0.get()).free_bytes() }
}

/// Mapea la región del heap con frames del PMM y arranca el allocator.
///
/// # Safety
/// Una sola vez, después de `pmm::init`.
pub unsafe fn init() {
    let mut flags = paging::WRITABLE;
    if paging::nx_enabled() {
        flags |= paging::NX; // el heap son datos: que no se pueda ejecutar
    }
    for i in 0..HEAP_SIZE / 4096 {
        let frame = pmm::alloc_frame().expect("sin frames para el heap");
        paging::map_page(HEAP_START + i * 4096, frame, flags).expect("fallo al mapear el heap");
    }
    (*ALLOCATOR.0.get()).insert(HEAP_START as usize, HEAP_SIZE as usize);
}
