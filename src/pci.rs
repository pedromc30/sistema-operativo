//! Bus PCI: enumeración de dispositivos por el espacio de configuración (puertos 0xCF8/0xCFC).
//!
//! Cada dispositivo PCI se identifica por (bus, dispositivo, función) y expone 256 bytes de
//! "configuración": fabricante, modelo, clase, y hasta 6 BAR (regiones de E/S o de memoria
//! donde están sus registros).

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub bars: [u32; 6],
    #[allow(dead_code)]
    pub irq_line: u8,
}

fn address(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (func as u32) << 8 | (reg as u32 & 0xFC)
}

pub fn read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, dev, func, reg));
        inl(CONFIG_DATA)
    }
}

pub fn write32(bus: u8, dev: u8, func: u8, reg: u8, val: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, dev, func, reg));
        outl(CONFIG_DATA, val);
    }
}

const MAX_DEVICES: usize = 32;
static mut DEVICES: [Option<PciDevice>; MAX_DEVICES] = [None; MAX_DEVICES];
static mut COUNT: usize = 0;

/// Recorre los buses 0..=7 y guarda lo que encuentra.
pub fn scan() {
    unsafe {
        COUNT = 0;
        for bus in 0..=7u8 {
            for dev in 0..32u8 {
                for func in 0..8u8 {
                    let id = read32(bus, dev, func, 0);
                    if id & 0xFFFF == 0xFFFF {
                        if func == 0 {
                            break; // no hay dispositivo: se saltan sus otras funciones
                        }
                        continue;
                    }
                    let class = read32(bus, dev, func, 0x08);
                    let mut bars = [0u32; 6];
                    for (i, b) in bars.iter_mut().enumerate() {
                        *b = read32(bus, dev, func, 0x10 + 4 * i as u8);
                    }
                    let d = PciDevice {
                        bus,
                        dev,
                        func,
                        vendor: id as u16,
                        device: (id >> 16) as u16,
                        class: (class >> 24) as u8,
                        subclass: (class >> 16) as u8,
                        bars,
                        irq_line: read32(bus, dev, func, 0x3C) as u8,
                    };
                    if COUNT < MAX_DEVICES {
                        DEVICES[COUNT] = Some(d);
                        COUNT += 1;
                    }
                    // Solo se miran las funciones 1..7 si la 0 dice que es multifunción.
                    if func == 0 && read32(bus, dev, 0, 0x0C) >> 16 & 0x80 == 0 {
                        break;
                    }
                }
            }
        }
    }
}

pub fn devices() -> impl Iterator<Item = PciDevice> {
    unsafe { (*(&raw const DEVICES)).iter().take(COUNT).filter_map(|d| *d) }
}

pub fn find(vendor: u16, device: u16) -> Option<PciDevice> {
    devices().find(|d| d.vendor == vendor && d.device == device)
}

/// Activa E/S (bit 0), "bus master" (bit 2: el dispositivo puede leer/escribir la memoria
/// por su cuenta, DMA) y desactiva sus interrupciones (bit 10): sondearemos por polling.
pub fn enable_io_and_dma(d: &PciDevice) {
    let cmd = read32(d.bus, d.dev, d.func, 0x04);
    write32(d.bus, d.dev, d.func, 0x04, (cmd & 0xFFFF) | 0x1 | 0x4 | 0x400);
}

pub fn class_name(class: u8, subclass: u8) -> &'static str {
    match (class, subclass) {
        (0x00, _) => "sin clase",
        (0x01, 0x01) => "controlador IDE",
        (0x01, 0x06) => "controlador SATA",
        (0x01, 0x08) => "controlador NVMe",
        (0x01, _) => "almacenamiento",
        (0x02, 0x00) => "tarjeta de red Ethernet",
        (0x02, _) => "red",
        (0x03, _) => "controlador de vídeo",
        (0x06, 0x00) => "puente host",
        (0x06, 0x01) => "puente ISA/LPC",
        (0x06, _) => "puente",
        (0x0C, 0x03) => "controlador USB",
        (0x0C, _) => "bus serie",
        _ => "otro",
    }
}
