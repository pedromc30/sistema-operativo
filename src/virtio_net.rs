//! Driver de la tarjeta de red virtio-net (interfaz "legacy" por puertos de E/S).
//!
//! Virtio comunica kernel y dispositivo con "virtqueues": tablas en memoria compartida.
//! Hay dos colas: la 0 recibe paquetes (el kernel deja buffers vacíos y la tarjeta los
//! rellena) y la 1 envía (el kernel deja paquetes y la tarjeta los lee). Cada cola tiene:
//!   - descriptores: dónde está cada buffer (dirección FÍSICA, longitud, flags)
//!   - anillo "avail": buffers que el kernel entrega a la tarjeta
//!   - anillo "used": buffers que la tarjeta ya ha terminado de usar
//!
//! Se usa sondeo (polling), sin interrupciones.

use crate::limine;
use crate::pci::{self, PciDevice};
use crate::pmm;
use crate::port::{inb, inl, inw, outb, outl, outw};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

pub const VENDOR: u16 = 0x1AF4;
pub const DEVICE_LEGACY: u16 = 0x1000;

// Registros (offsets desde el puerto base de la BAR0).
const REG_HOST_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SELECT: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
const REG_CONFIG: u16 = 0x14; // MAC (6 bytes)

const STATUS_ACK: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const FEATURE_MAC: u32 = 1 << 5;

const DESC_WRITE: u16 = 2; // el dispositivo ESCRIBE en este buffer (recepción)

const HDR_LEN: usize = 10; // cabecera virtio-net (sin MRG_RXBUF)
const BUF_SIZE: usize = 2048;
const RX_BUFS: usize = 64;
const TX_BUFS: usize = 32;
const PAGE: usize = 4096;

fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

#[repr(C)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

struct Queue {
    size: usize,
    base: *mut u8,
    avail: usize, // offset del anillo "avail"
    used: usize,  // offset del anillo "used"
    last_used: u16,
}

impl Queue {
    unsafe fn new(io: u16, index: u16, hhdm: u64) -> Option<Queue> {
        outw(io + REG_QUEUE_SELECT, index);
        let size = inw(io + REG_QUEUE_SIZE) as usize;
        if size == 0 {
            return None;
        }
        // Disposición "legacy": descriptores, luego avail, y used alineado a página.
        let avail = 16 * size;
        let used = align_up(avail + 6 + 2 * size, PAGE);
        let total = used + align_up(6 + 8 * size, PAGE);
        let phys = pmm::alloc_contiguous((total / PAGE) as u64)?;
        let base = (phys + hhdm) as *mut u8;
        core::ptr::write_bytes(base, 0, total);
        outl(io + REG_QUEUE_PFN, (phys >> 12) as u32);
        Some(Queue { size, base, avail, used, last_used: 0 })
    }

    unsafe fn desc(&self, i: usize) -> *mut Desc {
        self.base.add(16 * i) as *mut Desc
    }
    unsafe fn avail_idx(&self) -> *mut u16 {
        self.base.add(self.avail + 2) as *mut u16
    }
    unsafe fn avail_ring(&self, i: usize) -> *mut u16 {
        self.base.add(self.avail + 4 + 2 * (i % self.size)) as *mut u16
    }
    unsafe fn used_idx(&self) -> *const u16 {
        self.base.add(self.used + 2) as *const u16
    }
    unsafe fn used_entry(&self, i: usize) -> (u32, u32) {
        let e = self.base.add(self.used + 4 + 8 * (i % self.size)) as *const u32;
        (read_volatile(e), read_volatile(e.add(1)))
    }

    /// Entrega el descriptor `id` a la tarjeta.
    unsafe fn push_avail(&mut self, id: u16) {
        let idx = read_volatile(self.avail_idx());
        write_volatile(self.avail_ring(idx as usize), id);
        fence(Ordering::Release); // la tarjeta debe ver el anillo antes que el índice nuevo
        write_volatile(self.avail_idx(), idx.wrapping_add(1));
    }

    /// Siguiente buffer que la tarjeta haya terminado, si hay.
    unsafe fn pop_used(&mut self) -> Option<(u32, u32)> {
        fence(Ordering::Acquire);
        if read_volatile(self.used_idx()) == self.last_used {
            return None;
        }
        let e = self.used_entry(self.last_used as usize);
        self.last_used = self.last_used.wrapping_add(1);
        Some(e)
    }
}

pub struct VirtioNet {
    io: u16,
    pub mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    rx_buf: *mut u8,
    tx_buf: *mut u8,
    tx_free: u32, // bit i = 1 si el buffer de envío i está libre
    pub rx_packets: u64,
    pub tx_packets: u64,
}

impl VirtioNet {
    /// # Safety
    /// Una sola vez, con el PCI ya escaneado y la memoria física iniciada.
    pub unsafe fn init(d: &PciDevice) -> Option<VirtioNet> {
        if d.bars[0] & 1 == 0 {
            return None; // la BAR0 debe ser de E/S
        }
        let io = (d.bars[0] & 0xFFFC) as u16;
        let hhdm = limine::hhdm_offset()?;
        pci::enable_io_and_dma(d);

        outb(io + REG_STATUS, 0); // reinicio
        outb(io + REG_STATUS, STATUS_ACK);
        outb(io + REG_STATUS, STATUS_ACK | STATUS_DRIVER);
        let host = inl(io + REG_HOST_FEATURES);
        outl(io + REG_GUEST_FEATURES, host & FEATURE_MAC); // solo pedimos la MAC

        let mut rx = Queue::new(io, 0, hhdm)?;
        let tx = Queue::new(io, 1, hhdm)?;

        let rx_phys = pmm::alloc_contiguous((RX_BUFS * BUF_SIZE / PAGE) as u64)?;
        let tx_phys = pmm::alloc_contiguous((TX_BUFS * BUF_SIZE / PAGE) as u64)?;
        let (rx_buf, tx_buf) = ((rx_phys + hhdm) as *mut u8, (tx_phys + hhdm) as *mut u8);

        // Recepción: dejar todos los buffers vacíos a disposición de la tarjeta.
        for i in 0..RX_BUFS {
            let desc = rx.desc(i);
            (*desc).addr = rx_phys + (i * BUF_SIZE) as u64;
            (*desc).len = BUF_SIZE as u32;
            (*desc).flags = DESC_WRITE;
            (*desc).next = 0;
            rx.push_avail(i as u16);
        }
        // Envío: los descriptores se rellenan al enviar.
        for i in 0..TX_BUFS {
            let desc = tx.desc(i);
            (*desc).addr = tx_phys + (i * BUF_SIZE) as u64;
            (*desc).len = 0;
            (*desc).flags = 0;
            (*desc).next = 0;
        }

        outb(io + REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK);
        outw(io + REG_QUEUE_NOTIFY, 0); // avisar: hay buffers de recepción

        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = inb(io + REG_CONFIG + i as u16);
        }
        Some(VirtioNet {
            io,
            mac,
            rx,
            tx,
            rx_buf,
            tx_buf,
            tx_free: if TX_BUFS == 32 { u32::MAX } else { (1u32 << TX_BUFS) - 1 },
            rx_packets: 0,
            tx_packets: 0,
        })
    }

    /// Recupera los buffers de envío que la tarjeta ya ha terminado.
    fn reclaim_tx(&mut self) {
        while let Some((id, _)) = unsafe { self.tx.pop_used() } {
            self.tx_free |= 1 << id;
        }
    }

    fn alloc_tx(&mut self) -> Option<usize> {
        self.reclaim_tx();
        if self.tx_free == 0 {
            return None;
        }
        let i = self.tx_free.trailing_zeros() as usize;
        self.tx_free &= !(1 << i);
        Some(i)
    }

    fn repost_rx(&mut self, id: usize) {
        unsafe {
            self.rx.push_avail(id as u16);
            outw(self.io + REG_QUEUE_NOTIFY, 0);
        }
    }
}

pub struct VirtioRx {
    dev: *mut VirtioNet,
    id: usize,
    len: usize,
}

pub struct VirtioTx {
    dev: *mut VirtioNet,
}

impl RxToken for VirtioRx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        let d = unsafe { &mut *self.dev };
        let frame = unsafe {
            core::slice::from_raw_parts(d.rx_buf.add(self.id * BUF_SIZE + HDR_LEN), self.len.saturating_sub(HDR_LEN))
        };
        let r = f(frame);
        d.repost_rx(self.id);
        r
    }
}

static mut SCRATCH: [u8; BUF_SIZE] = [0; BUF_SIZE];

impl TxToken for VirtioTx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let d = unsafe { &mut *self.dev };
        match d.alloc_tx() {
            Some(i) if len + HDR_LEN <= BUF_SIZE => unsafe {
                let buf = core::slice::from_raw_parts_mut(d.tx_buf.add(i * BUF_SIZE), HDR_LEN + len);
                buf[..HDR_LEN].fill(0); // cabecera virtio vacía: sin offloads
                let r = f(&mut buf[HDR_LEN..]);
                (*d.tx.desc(i)).len = (HDR_LEN + len) as u32;
                d.tx.push_avail(i as u16);
                outw(d.io + REG_QUEUE_NOTIFY, 1);
                d.tx_packets += 1;
                r
            },
            // Sin buffer libre: el paquete se pierde (TCP lo retransmitirá).
            _ => unsafe { f(core::slice::from_raw_parts_mut((&raw mut SCRATCH) as *mut u8, len.min(BUF_SIZE))) },
        }
    }
}

impl Device for VirtioNet {
    type RxToken<'a> = VirtioRx;
    type TxToken<'a> = VirtioTx;

    fn receive(&mut self, _ts: Instant) -> Option<(VirtioRx, VirtioTx)> {
        let (id, len) = unsafe { self.rx.pop_used()? };
        self.rx_packets += 1;
        let me = self as *mut VirtioNet;
        Some((VirtioRx { dev: me, id: id as usize, len: len as usize }, VirtioTx { dev: me }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<VirtioTx> {
        self.reclaim_tx();
        if self.tx_free == 0 {
            return None;
        }
        Some(VirtioTx { dev: self as *mut VirtioNet })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = 1514;
        c
    }
}
