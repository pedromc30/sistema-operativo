//! Lector de líneas con edición e historial para programas de usuario.
//!
//! Flechas, Inicio/Fin, Supr, Retroceso, Ctrl+A/E/U/K/L/C y historial con flecha arriba/abajo.
//! Las teclas especiales llegan como secuencias ANSI (ESC [ A ...), tanto del teclado PS/2
//! como del terminal por el puerto serie.

use crate::{input_ready, print, println, read, sleep_ms};
use alloc::{string::String, vec::Vec};

enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Delete,
    Ignored,
}

fn next_byte() -> Option<u8> {
    let mut b = [0u8; 1];
    if read(&mut b) < 1 { None } else { Some(b[0]) }
}

/// Espera un instante a que llegue el resto de una secuencia y la lee si está.
fn next_byte_soon() -> Option<u8> {
    if !input_ready() {
        sleep_ms(10); // por el puerto serie los bytes pueden llegar con un pequeño retraso
    }
    if input_ready() { next_byte() } else { None }
}

fn read_key() -> Option<Key> {
    let c = next_byte()?;
    if c < 0x80 && c != 0x1b {
        return Some(Key::Char(c as char));
    }
    if c >= 0x80 {
        // Carácter UTF-8 de varios bytes (ñ, á, €...)
        let (mut acc, need) = match c {
            0xC2..=0xDF => ((c & 0x1F) as u32, 1),
            0xE0..=0xEF => ((c & 0x0F) as u32, 2),
            0xF0..=0xF4 => ((c & 0x07) as u32, 3),
            _ => return Some(Key::Ignored),
        };
        for _ in 0..need {
            match next_byte_soon() {
                Some(b) if b & 0xC0 == 0x80 => acc = (acc << 6) | (b & 0x3F) as u32,
                _ => return Some(Key::Ignored),
            }
        }
        return Some(char::from_u32(acc).map(Key::Char).unwrap_or(Key::Ignored));
    }
    match next_byte_soon() {
        Some(b'[') | Some(b'O') => {}
        _ => return Some(Key::Ignored), // ESC suelto
    }
    let mut param: u32 = 0;
    loop {
        let Some(b) = next_byte_soon() else { return Some(Key::Ignored) };
        match b {
            b'0'..=b'9' => param = param * 10 + (b - b'0') as u32,
            b';' => param = 0,
            b'A' => return Some(Key::Up),
            b'B' => return Some(Key::Down),
            b'C' => return Some(Key::Right),
            b'D' => return Some(Key::Left),
            b'H' => return Some(Key::Home),
            b'F' => return Some(Key::End),
            b'~' => {
                return Some(match param {
                    1 | 7 => Key::Home,
                    3 => Key::Delete,
                    4 | 8 => Key::End,
                    _ => Key::Ignored,
                })
            }
            0x40..=0x7e => return Some(Key::Ignored),
            _ => {}
        }
    }
}

fn redraw(prompt: &str, line: &[char], pos: usize) {
    let text: String = line.iter().collect();
    print!("\r{}{}\x1b[K", prompt, text);
    if pos < line.len() {
        print!("\x1b[{}D", line.len() - pos);
    }
}

/// Lee una línea. Devuelve `None` con Ctrl+D en línea vacía (fin de entrada).
pub fn readline(prompt: &str, history: &mut Vec<String>) -> Option<String> {
    let mut line: Vec<char> = Vec::new();
    let mut pos = 0usize;
    let mut hist = history.len();
    let mut draft: Vec<char> = Vec::new();
    print!("{}", prompt);
    loop {
        match read_key()? {
            Key::Char('\n') => {
                println!();
                let text: String = line.iter().collect();
                if !text.trim().is_empty() && history.last() != Some(&text) {
                    if history.len() >= 50 {
                        history.remove(0);
                    }
                    history.push(text.clone());
                }
                return Some(text);
            }
            Key::Char('\x04') if line.is_empty() => {
                println!();
                return None;
            }
            Key::Char('\x08') | Key::Char('\x7f') => {
                if pos > 0 {
                    line.remove(pos - 1);
                    pos -= 1;
                    redraw(prompt, &line, pos);
                }
            }
            Key::Delete => {
                if pos < line.len() {
                    line.remove(pos);
                    redraw(prompt, &line, pos);
                }
            }
            Key::Left => {
                if pos > 0 {
                    pos -= 1;
                    print!("\x1b[D");
                }
            }
            Key::Right => {
                if pos < line.len() {
                    pos += 1;
                    print!("\x1b[C");
                }
            }
            Key::Home | Key::Char('\x01') => {
                pos = 0;
                redraw(prompt, &line, pos);
            }
            Key::End | Key::Char('\x05') => {
                pos = line.len();
                redraw(prompt, &line, pos);
            }
            Key::Char('\x15') => {
                line.drain(..pos);
                pos = 0;
                redraw(prompt, &line, pos);
            }
            Key::Char('\x0b') => {
                line.truncate(pos);
                redraw(prompt, &line, pos);
            }
            Key::Char('\x0c') => {
                print!("\x1b[2J\x1b[H");
                redraw(prompt, &line, pos);
            }
            Key::Char('\x03') => {
                println!("^C");
                return Some(String::new());
            }
            Key::Up => {
                if hist > 0 {
                    if hist == history.len() {
                        draft = line.clone();
                    }
                    hist -= 1;
                    line = history[hist].chars().collect();
                    pos = line.len();
                    redraw(prompt, &line, pos);
                }
            }
            Key::Down => {
                if hist < history.len() {
                    hist += 1;
                    line = if hist == history.len() { draft.clone() } else { history[hist].chars().collect() };
                    pos = line.len();
                    redraw(prompt, &line, pos);
                }
            }
            Key::Char(c) if !c.is_control() => {
                line.insert(pos, c);
                pos += 1;
                if pos == line.len() {
                    print!("{}", c);
                } else {
                    redraw(prompt, &line, pos);
                }
            }
            _ => {}
        }
    }
}
