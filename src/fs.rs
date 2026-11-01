//! Sistema de archivos en RAM (solo lectura): el "initrd".
//!
//! Los archivos de la carpeta `initrd/` se incrustan en el kernel al compilar
//! (ver build.rs). Es plano: sin directorios, sin escritura.

include!(concat!(env!("OUT_DIR"), "/initrd.rs"));

pub fn find(name: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
}

pub fn list() -> impl Iterator<Item = (&'static str, usize)> {
    FILES.iter().map(|(n, d)| (*n, d.len()))
}
