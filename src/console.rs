//! Consola de texto sobre el framebuffer.
//!
//! Una cuadrícula de caracteres (columnas x filas) dibujada con una fuente de 8x16
//! píxeles (ampliada x2 en pantallas muy grandes). Guarda una copia del texto de cada celda ("shadow buffer") para
//! poder pintar el cursor y borrarlo sin perder la letra que había debajo.
//! Entiende UTF-8 y un subconjunto de secuencias ANSI: colores (SGR), mover el cursor,
//! borrar pantalla/línea y mostrar/ocultar el cursor.

use crate::font;
use crate::limine::{self, Framebuffer};
use core::ptr;

const MAX_COLS: usize = 256;
const MAX_ROWS: usize = 128;

/// Paleta de 16 colores (la clásica de VGA / ANSI).
const PALETTE: [(u8, u8, u8); 16] = [
    (0, 0, 0), (170, 0, 0), (0, 170, 0), (170, 85, 0),
    (0, 0, 170), (170, 0, 170), (0, 170, 170), (170, 170, 170),
    (85, 85, 85), (255, 85, 85), (85, 255, 85), (255, 255, 85),
    (85, 85, 255), (255, 85, 255), (85, 255, 255), (255, 255, 255),
];

const DEFAULT_FG: u8 = 7;
const DEFAULT_BG: u8 = 0;

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: u8,
    bg: u8,
}

/// Todo a cero: así la tabla va a .bss (no ocupa espacio en el ejecutable) y no hace falta
/// construirla en la pila. El carácter '\0' se dibuja como un espacio.
const ZERO_CELL: Cell = Cell { ch: '\0', fg: 0, bg: 0 };

/// La copia del texto de la pantalla (256 KB). NO puede vivir en la pila del kernel.
static mut CELLS: [Cell; MAX_COLS * MAX_ROWS] = [ZERO_CELL; MAX_COLS * MAX_ROWS];

#[derive(Clone, Copy, PartialEq)]
enum Esc {
    None,
    Escape, // acabamos de ver ESC
    Csi,    // ESC [ ... (parámetros)
}

struct Console {
    fb: Framebuffer,
    palette_px: [u32; 16],
    scale: usize,
    cols: usize,
    rows: usize,
    cx: usize,
    cy: usize,
    fg: u8,
    bg: u8,
    bold: bool,
    reverse: bool,
    cursor_visible: bool,
    cursor_drawn: Option<(usize, usize)>,
    esc: Esc,
    params: [u16; 4],
    nparams: usize,
    param_active: bool,
    private: bool,
    utf8_need: u8,
    utf8_acc: u32,
    cells: &'static mut [Cell],
}

static mut CONSOLE: Option<Console> = None;

fn con() -> Option<&'static mut Console> {
    unsafe { (*(&raw mut CONSOLE)).as_mut() }
}

/// Prepara la consola con el framebuffer de Limine. Devuelve false si no hay pantalla
/// utilizable (entonces todo sigue saliendo solo por el puerto serie).
pub fn init() -> bool {
    let Some(fb) = limine::framebuffer() else { return false };
    if fb.bpp != 32 || fb.red_size != 8 || fb.green_size != 8 || fb.blue_size != 8 {
        return false; // solo soportamos 32 bits por píxel, 8 por canal
    }
    let scale = if fb.width >= 2400 { 2 } else { 1 };
    let cols = (fb.width / (font::WIDTH * scale)).min(MAX_COLS);
    let rows = (fb.height / (font::HEIGHT * scale)).min(MAX_ROWS);
    if cols == 0 || rows == 0 {
        return false;
    }
    let mut palette_px = [0u32; 16];
    for (i, &(r, g, b)) in PALETTE.iter().enumerate() {
        palette_px[i] = ((r as u32) << fb.red_shift) | ((g as u32) << fb.green_shift) | ((b as u32) << fb.blue_shift);
    }
    let mut c = Console {
        fb,
        palette_px,
        scale,
        cols,
        rows,
        cx: 0,
        cy: 0,
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
        bold: false,
        reverse: false,
        cursor_visible: true,
        cursor_drawn: None,
        esc: Esc::None,
        params: [0; 4],
        nparams: 0,
        param_active: false,
        private: false,
        utf8_need: 0,
        utf8_acc: 0,
        cells: unsafe { &mut *(&raw mut CELLS) },
    };
    c.clear_screen();
    c.show_cursor();
    unsafe { *(&raw mut CONSOLE) = Some(c) };
    true
}

/// (columnas, filas, ancho en píxeles, alto en píxeles)
pub fn info() -> Option<(usize, usize, usize, usize)> {
    con().map(|c| (c.cols, c.rows, c.fb.width, c.fb.height))
}

pub fn write_str(s: &str) {
    write_bytes(s.as_bytes());
}

pub fn write_bytes(bytes: &[u8]) {
    if let Some(c) = con() {
        c.hide_cursor();
        for &b in bytes {
            c.write_byte(b);
        }
        c.show_cursor();
    }
}

/// Borra la pantalla y lleva el cursor a la esquina superior izquierda. Solo afecta a la
/// pantalla (no a la salida por serie).
pub fn clear() {
    if let Some(c) = con() {
        c.hide_cursor();
        c.clear_screen();
        c.cx = 0;
        c.cy = 0;
        c.show_cursor();
    }
}

impl Console {
    fn cell(&mut self, col: usize, row: usize) -> &mut Cell {
        &mut self.cells[row * MAX_COLS + col]
    }

    /// Dibuja una celda en pantalla a partir de la copia de texto. Con `invert`
    /// intercambia colores (así se pinta el cursor).
    fn draw_cell(&mut self, col: usize, row: usize, invert: bool) {
        let cell = self.cells[row * MAX_COLS + col];
        let (fg, bg) = if invert { (cell.bg, cell.fg) } else { (cell.fg, cell.bg) };
        let (fgpx, bgpx) = (self.palette_px[fg as usize], self.palette_px[bg as usize]);
        let g = font::glyph(if cell.ch == '\0' { ' ' } else { cell.ch });
        let s = self.scale;
        let x0 = col * font::WIDTH * s;
        let y0 = row * font::HEIGHT * s;
        for (gy, bits) in g.iter().enumerate() {
            for sy in 0..s {
                let y = y0 + gy * s + sy;
                let line = unsafe { self.fb.addr.add(y * self.fb.pitch + x0 * 4) as *mut u32 };
                for gx in 0..font::WIDTH {
                    let px = if bits & (0x80 >> gx) != 0 { fgpx } else { bgpx };
                    for sx in 0..s {
                        unsafe { line.add(gx * s + sx).write_volatile(px) };
                    }
                }
            }
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, px: u32) {
        for yy in y..(y + h).min(self.fb.height) {
            let line = unsafe { self.fb.addr.add(yy * self.fb.pitch + x * 4) as *mut u32 };
            for xx in 0..w.min(self.fb.width - x) {
                unsafe { line.add(xx).write_volatile(px) };
            }
        }
    }

    fn hide_cursor(&mut self) {
        if let Some((c, r)) = self.cursor_drawn.take() {
            self.draw_cell(c, r, false);
        }
    }

    fn show_cursor(&mut self) {
        if self.cursor_visible {
            let c = self.cx.min(self.cols - 1);
            self.draw_cell(c, self.cy, true);
            self.cursor_drawn = Some((c, self.cy));
        }
    }

    fn clear_screen(&mut self) {
        let bg = self.palette_px[self.bg as usize];
        self.fill_rect(0, 0, self.fb.width, self.fb.height, bg);
        let blank = Cell { ch: ' ', fg: self.fg, bg: self.bg };
        for r in 0..self.rows {
            for c in 0..self.cols {
                *self.cell(c, r) = blank;
            }
        }
        self.cursor_drawn = None;
    }

    /// Vacía las celdas [desde, hasta) de una fila con el color de fondo actual.
    fn clear_span(&mut self, row: usize, from: usize, to: usize) {
        let blank = Cell { ch: ' ', fg: self.fg, bg: self.bg };
        for c in from..to.min(self.cols) {
            *self.cell(c, row) = blank;
            self.draw_cell(c, row, false);
        }
    }

    fn scroll(&mut self) {
        // Copia de texto: cada fila sube una posición.
        for r in 1..self.rows {
            for c in 0..self.cols {
                let v = self.cells[r * MAX_COLS + c];
                self.cells[(r - 1) * MAX_COLS + c] = v;
            }
        }
        let blank = Cell { ch: ' ', fg: self.fg, bg: self.bg };
        for c in 0..self.cols {
            *self.cell(c, self.rows - 1) = blank;
        }
        // Píxeles: se mueve el bloque de pantalla de una sola vez.
        let cell_h = font::HEIGHT * self.scale;
        let text_h = self.rows * cell_h;
        unsafe {
            ptr::copy(
                self.fb.addr.add(cell_h * self.fb.pitch),
                self.fb.addr,
                (text_h - cell_h) * self.fb.pitch,
            );
        }
        let bg = self.palette_px[self.bg as usize];
        self.fill_rect(0, text_h - cell_h, self.fb.width, cell_h, bg);
    }

    fn newline(&mut self) {
        self.cx = 0;
        if self.cy + 1 >= self.rows {
            self.scroll();
        } else {
            self.cy += 1;
        }
    }

    fn emit(&mut self, ch: char) {
        if self.cx >= self.cols {
            self.newline();
        }
        let (mut fg, mut bg) = (self.fg, self.bg);
        if self.reverse {
            core::mem::swap(&mut fg, &mut bg);
        }
        *self.cell(self.cx, self.cy) = Cell { ch, fg, bg };
        self.draw_cell(self.cx, self.cy, false);
        self.cx += 1;
    }

    fn write_byte(&mut self, b: u8) {
        match self.esc {
            Esc::Escape => {
                if b == b'[' {
                    self.esc = Esc::Csi;
                    self.params = [0; 4];
                    self.nparams = 0;
                    self.param_active = false;
                    self.private = false;
                } else {
                    self.esc = Esc::None;
                }
                return;
            }
            Esc::Csi => {
                match b {
                    b'0'..=b'9' => {
                        if self.nparams < self.params.len() {
                            self.params[self.nparams] = self.params[self.nparams].saturating_mul(10).saturating_add((b - b'0') as u16);
                        }
                        self.param_active = true;
                    }
                    b';' => {
                        if self.nparams + 1 < self.params.len() {
                            self.nparams += 1;
                        }
                        self.param_active = true;
                    }
                    b'?' => self.private = true,
                    b'@'..=b'~' => {
                        if self.param_active {
                            self.nparams += 1;
                        }
                        self.csi(b);
                        self.esc = Esc::None;
                    }
                    _ => self.esc = Esc::None,
                }
                return;
            }
            Esc::None => {}
        }

        // Continuación de un carácter UTF-8 de varios bytes.
        if self.utf8_need > 0 {
            if b & 0xC0 == 0x80 {
                self.utf8_acc = (self.utf8_acc << 6) | (b & 0x3F) as u32;
                self.utf8_need -= 1;
                if self.utf8_need == 0 {
                    let ch = char::from_u32(self.utf8_acc).unwrap_or('?');
                    self.emit(ch);
                }
                return;
            }
            self.utf8_need = 0; // secuencia rota: se trata este byte como uno nuevo
        }

        match b {
            0x1b => self.esc = Esc::Escape,
            b'\n' => self.newline(),
            b'\r' => self.cx = 0,
            0x08 => self.cx = self.cx.saturating_sub(1),
            b'\t' => {
                let next = (self.cx / 8 + 1) * 8;
                while self.cx < next.min(self.cols) {
                    self.emit(' ');
                }
            }
            0x20..=0x7e => self.emit(b as char),
            0xC2..=0xDF => {
                self.utf8_need = 1;
                self.utf8_acc = (b & 0x1F) as u32;
            }
            0xE0..=0xEF => {
                self.utf8_need = 2;
                self.utf8_acc = (b & 0x0F) as u32;
            }
            0xF0..=0xF4 => {
                self.utf8_need = 3;
                self.utf8_acc = (b & 0x07) as u32;
            }
            _ => {}
        }
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.nparams && self.params[i] != 0 { self.params[i] } else { default }
    }

    fn csi(&mut self, fin: u8) {
        match fin {
            b'H' | b'f' => {
                let row = self.param(0, 1) as usize;
                let col = self.param(1, 1) as usize;
                self.cy = (row - 1).min(self.rows - 1);
                self.cx = (col - 1).min(self.cols - 1);
            }
            b'A' => self.cy = self.cy.saturating_sub(self.param(0, 1) as usize),
            b'B' => self.cy = (self.cy + self.param(0, 1) as usize).min(self.rows - 1),
            b'C' => self.cx = (self.cx + self.param(0, 1) as usize).min(self.cols - 1),
            b'D' => self.cx = self.cx.saturating_sub(self.param(0, 1) as usize),
            b'J' => match if self.nparams > 0 { self.params[0] } else { 0 } {
                2 | 3 => self.clear_screen(),
                1 => {
                    for r in 0..self.cy {
                        self.clear_span(r, 0, self.cols);
                    }
                    self.clear_span(self.cy, 0, self.cx + 1);
                }
                _ => {
                    self.clear_span(self.cy, self.cx, self.cols);
                    for r in self.cy + 1..self.rows {
                        self.clear_span(r, 0, self.cols);
                    }
                }
            },
            b'K' => match if self.nparams > 0 { self.params[0] } else { 0 } {
                1 => self.clear_span(self.cy, 0, self.cx + 1),
                2 => self.clear_span(self.cy, 0, self.cols),
                _ => self.clear_span(self.cy, self.cx, self.cols),
            },
            b'm' => self.sgr(),
            b'h' | b'l' if self.private && self.nparams > 0 && self.params[0] == 25 => {
                self.cursor_visible = fin == b'h';
            }
            _ => {}
        }
    }

    /// SGR: colores y estilos.
    fn sgr(&mut self) {
        if self.nparams == 0 {
            self.reset_attrs();
            return;
        }
        for i in 0..self.nparams {
            match self.params[i] {
                0 => self.reset_attrs(),
                1 => {
                    self.bold = true;
                    if self.fg < 8 {
                        self.fg += 8;
                    }
                }
                7 => self.reverse = true,
                27 => self.reverse = false,
                n @ 30..=37 => self.fg = (n - 30) as u8 + if self.bold { 8 } else { 0 },
                39 => self.fg = DEFAULT_FG,
                n @ 40..=47 => self.bg = (n - 40) as u8,
                49 => self.bg = DEFAULT_BG,
                n @ 90..=97 => self.fg = (n - 90) as u8 + 8,
                n @ 100..=107 => self.bg = (n - 100) as u8 + 8,
                _ => {}
            }
        }
    }

    fn reset_attrs(&mut self) {
        self.fg = DEFAULT_FG;
        self.bg = DEFAULT_BG;
        self.bold = false;
        self.reverse = false;
    }
}
