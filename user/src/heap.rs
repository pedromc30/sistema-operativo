//! Allocator global de los programas de usuario, construido sobre la syscall `brk`.
//!
//! Lista libre ordenada por dirección con fusión de bloques (el mismo diseño que el heap
//! del kernel). Cuando no hay hueco, pide más memoria al kernel con `brk`, de 64 KiB en
//! 64 KiB. Un proceso tiene un solo hilo, así que no necesita cerrojo.
//! La memoria liberada se reutiliza pero NO se devuelve al sistema hasta que el proceso termina.

use crate::brk;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::mem::size_of;
use core::ptr::null_mut;

#[repr(C)]
struct Node {
    size: usize,
    next: *mut Node,
}
const NODE: usize = size_of::<Node>(); // 16: todos los bloques son múltiplos de 16
const GROW: usize = 64 * 1024;

fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

struct FreeList {
    head: *mut Node,
}

impl FreeList {
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
        if !cur.is_null() && addr + size == cur as usize {
            (*node).size += (*cur).size;
            (*node).next = (*cur).next;
        }
        if !prev.is_null() && prev as usize + (*prev).size == addr {
            (*prev).size += (*node).size;
            (*prev).next = (*node).next;
        }
    }

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
}

struct Heap(UnsafeCell<FreeList>);
unsafe impl Sync for Heap {}

#[global_allocator]
static HEAP: Heap = Heap(UnsafeCell::new(FreeList { head: null_mut() }));

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let fl = &mut *self.0.get();
        let p = fl.alloc(layout);
        if !p.is_null() {
            return p;
        }
        // Sin hueco: pedir más memoria al kernel (con margen por si hay que alinear).
        let need = align_up(layout.size().max(NODE) + layout.align(), NODE);
        let grow = align_up(need.max(GROW), 4096);
        let cur = brk(0) as usize;
        let base = align_up(cur, NODE);
        let want = base + grow;
        if brk(want as u64) as usize != want {
            return null_mut(); // límite del proceso alcanzado o sin memoria
        }
        fl.insert(base, grow);
        fl.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = align_up(layout.size().max(NODE), NODE);
        (*self.0.get()).insert(ptr as usize, size);
    }
}
