#!/usr/bin/env python3
"""Genera src/font.rs: tabla de glifos de 8x16 píxeles para la consola.

- Latin-1 (español completo) y el símbolo €: extraídos de la fuente Terminus
  (SIL Open Font License 1.1, ver FONT-LICENSE.txt). Requiere el paquete xfonts-terminus.
- Líneas de caja y bloques: dibujados por este script (no proceden de ninguna fuente).

Uso: python3 tools/gen_font.py > src/font.rs
"""
import gzip
from PIL import PcfFontFile

DIR = "/usr/share/fonts/X11/misc/"
W, H = 8, 16

def load(name):
    return PcfFontFile.PcfFontFile(gzip.open(DIR + name))

def rows_of(img):
    """Imagen 1-bit 8x16 -> 16 bytes (bit 7 = píxel de la izquierda)."""
    out = []
    for y in range(H):
        b = 0
        for x in range(W):
            if img.getpixel((x, y)):
                b |= 0x80 >> x
        out.append(b)
    return out

glyphs = {}

l1 = load("ter-u16n_iso-8859-1.pcf.gz")
for cp in list(range(0x20, 0x7F)) + list(range(0xA0, 0x100)):
    g = l1.glyph[cp]
    assert g is not None and g[3].size == (W, H), hex(cp)
    glyphs[cp] = rows_of(g[3])
l15 = load("ter-u16n_iso-8859-15.pcf.gz")
glyphs[0x20AC] = rows_of(l15.glyph[0xA4][3])  # € está en la posición 0xA4 de ISO-8859-15

# ---- Líneas de caja (finas): se dibujan con 4 brazos desde el centro ----
CX, CY = 4, 8
def canvas():
    return [[0] * W for _ in range(H)]
def pack(c):
    return [sum((0x80 >> x) for x in range(W) if c[y][x]) for y in range(H)]
def arms(up=False, down=False, left=False, right=False):
    c = canvas()
    if up:    [c[y].__setitem__(CX, 1) for y in range(0, CY + 1)]
    if down:  [c[y].__setitem__(CX, 1) for y in range(CY, H)]
    if left:  [c[CY].__setitem__(x, 1) for x in range(0, CX + 1)]
    if right: [c[CY].__setitem__(x, 1) for x in range(CX, W)]
    return pack(c)
BOX = {
    0x2500: dict(left=1, right=1), 0x2502: dict(up=1, down=1),
    0x250C: dict(down=1, right=1), 0x2510: dict(down=1, left=1),
    0x2514: dict(up=1, right=1),   0x2518: dict(up=1, left=1),
    0x251C: dict(up=1, down=1, right=1), 0x2524: dict(up=1, down=1, left=1),
    0x252C: dict(down=1, left=1, right=1), 0x2534: dict(up=1, left=1, right=1),
    0x253C: dict(up=1, down=1, left=1, right=1),
}
for cp, a in BOX.items():
    glyphs[cp] = arms(**a)

# ---- Bloques ----
def fill(pred):
    c = canvas()
    for y in range(H):
        for x in range(W):
            c[y][x] = 1 if pred(x, y) else 0
    return pack(c)
glyphs[0x2588] = fill(lambda x, y: True)           # █
glyphs[0x2580] = fill(lambda x, y: y < H // 2)     # ▀
glyphs[0x2584] = fill(lambda x, y: y >= H // 2)    # ▄
glyphs[0x258C] = fill(lambda x, y: x < W // 2)     # ▌
glyphs[0x2590] = fill(lambda x, y: x >= W // 2)    # ▐
glyphs[0x2591] = fill(lambda x, y: (x + y) % 4 == 0)            # ░ 25 %
glyphs[0x2592] = fill(lambda x, y: (x + y) % 2 == 0)            # ▒ 50 %
glyphs[0x2593] = fill(lambda x, y: (x + y) % 4 != 0)            # ▓ 75 %
glyphs[0x25A0] = fill(lambda x, y: 1 <= x <= 6 and 4 <= y <= 11)  # ■
glyphs[0x2022] = fill(lambda x, y: 2 <= x <= 5 and 6 <= y <= 9)   # •

print("// GENERADO por tools/gen_font.py: no editar a mano.")
print("//")
print("// Glifos de 8x16 píxeles. Latin-1 y € proceden de la fuente Terminus")
print("// (SIL Open Font License 1.1; ver FONT-LICENSE.txt). Las líneas de caja y los")
print("// bloques están dibujados por el propio script.")
print("// Cada glifo son 16 bytes (uno por fila); el bit 7 es el píxel de la izquierda.")
print()
print("pub const WIDTH: usize = 8;")
print("pub const HEIGHT: usize = 16;")
print()
print(f"static GLYPHS: [(u32, [u8; 16]); {len(glyphs)}] = [")
for cp in sorted(glyphs):
    print("    (0x%04X, [%s])," % (cp, ", ".join("0x%02X" % b for b in glyphs[cp])))
print("];")
print('''
/// Glifo para caracteres que la tabla no tiene: un rectángulo hueco.
static MISSING: [u8; 16] = [
    0x00, 0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00, 0x00,
];

/// Busca el glifo de un carácter (búsqueda binaria: la tabla está ordenada).
pub fn glyph(ch: char) -> &'static [u8; 16] {
    let cp = ch as u32;
    match GLYPHS.binary_search_by_key(&cp, |&(c, _)| c) {
        Ok(i) => &GLYPHS[i].1,
        Err(_) => &MISSING,
    }
}''')
