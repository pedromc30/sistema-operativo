//! Cliente HTTP/1.1 mínimo: `http [-i] [-s] <url>`
//!
//!   -i   muestra también las cabeceras de la respuesta
//!   -s   solo estadísticas (bytes y suma de comprobación), sin imprimir el cuerpo
//!
//! Entiende respuestas con Content-Length, fragmentadas (chunked) o delimitadas por el cierre
//! de la conexión, y sigue hasta 5 redirecciones. Solo http:// (https necesita TLS).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use user::{dns_resolve, entry, print, println, strerror, tcp_close, tcp_connect, tcp_recv, tcp_send, uptime_ms};

entry!(main);

const MAX_BODY: usize = 4 * 1024 * 1024;

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(u: &str) -> Result<Url, String> {
    let rest = match u.strip_prefix("http://") {
        Some(r) => r,
        None if u.starts_with("https://") => {
            return Err(String::from("https:// todavia no esta soportado (hace falta implementar TLS)"))
        }
        None => return Err(String::from("la URL debe empezar por http://")),
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| String::from("puerto invalido"))?),
        None => (hostport, 80),
    };
    if host.is_empty() {
        return Err(String::from("falta el nombre del servidor"));
    }
    Ok(Url { host: String::from(host), port, path: String::from(path) })
}

struct Response {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decodifica "Transfer-Encoding: chunked": bloques "<tamaño en hex>\r\n<datos>\r\n", y 0 al final.
fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = find(data, b"\r\n").ok_or_else(|| String::from("respuesta fragmentada incompleta"))?;
        let line = core::str::from_utf8(&data[..eol]).map_err(|_| String::from("tamano de bloque invalido"))?;
        let size_str = line.split(';').next().unwrap_or("").trim(); // ignora extensiones ";..."
        let size = usize::from_str_radix(size_str, 16).map_err(|_| String::from("tamano de bloque invalido"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err(String::from("respuesta fragmentada cortada"));
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
        if out.len() > MAX_BODY {
            return Err(String::from("respuesta demasiado grande"));
        }
    }
}

fn fetch(url: &Url) -> Result<Response, String> {
    let ip = dns_resolve(&url.host).map_err(|e| format!("no se pudo resolver '{}': {}", url.host, strerror(e)))?;
    let sock = tcp_connect(ip, url.port);
    if sock < 0 {
        return Err(format!("no se pudo conectar con {}.{}.{}.{}:{}: {}", ip[0], ip[1], ip[2], ip[3], url.port, strerror(sock)));
    }
    // El puerto va en "Host" solo si no es el estándar (80).
    let host_header = if url.port == 80 { url.host.clone() } else { format!("{}:{}", url.host, url.port) };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: RustOS/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.path, host_header
    );
    let mut sent = 0;
    while sent < request.len() {
        let n = tcp_send(sock, &request.as_bytes()[sent..]);
        if n < 0 {
            tcp_close(sock);
            return Err(format!("error al enviar: {}", strerror(n)));
        }
        sent += n as usize;
    }

    // Se lee todo hasta que el servidor cierra la conexión.
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = tcp_recv(sock, &mut buf);
        if n == 0 {
            break;
        }
        if n < 0 {
            tcp_close(sock);
            return Err(format!("error al recibir: {}", strerror(n)));
        }
        raw.extend_from_slice(&buf[..n as usize]);
        if raw.len() > MAX_BODY + 65536 {
            tcp_close(sock);
            return Err(String::from("respuesta demasiado grande"));
        }
    }
    tcp_close(sock);

    let head_end = find(&raw, b"\r\n\r\n").ok_or_else(|| String::from("respuesta HTTP incompleta (sin cabeceras)"))?;
    let head = core::str::from_utf8(&raw[..head_end]).map_err(|_| String::from("cabeceras no validas"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(format!("no parece HTTP: '{}'", status_line));
    }
    let status: u16 = parts.next().and_then(|s| s.parse().ok()).ok_or_else(|| String::from("codigo de estado invalido"))?;
    let reason = String::from(parts.next().unwrap_or(""));
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (String::from(k.trim()), String::from(v.trim()))))
        .collect();

    let raw_body = &raw[head_end + 4..];
    let mut resp = Response { status, reason, headers, body: Vec::new() };
    let chunked = resp.header("transfer-encoding").map_or(false, |v| v.to_ascii_lowercase().contains("chunked"));
    resp.body = if chunked {
        dechunk(raw_body)?
    } else if let Some(len) = resp.header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        if raw_body.len() < len {
            return Err(format!("cuerpo incompleto: {} de {} bytes", raw_body.len(), len));
        }
        raw_body[..len].to_vec()
    } else {
        raw_body.to_vec() // sin longitud: el cuerpo termina cuando el servidor cierra
    };
    Ok(resp)
}

/// Resuelve el valor de "Location" (absoluto o relativo) respecto a la URL actual.
fn resolve_location(base: &Url, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        String::from(loc)
    } else if loc.starts_with('/') {
        format!("http://{}:{}{}", base.host, base.port, loc)
    } else {
        let dir = base.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        format!("http://{}:{}{}/{}", base.host, base.port, dir, loc)
    }
}

fn main(args: &str) {
    let mut show_headers = false;
    let mut stats_only = false;
    let mut target: Option<&str> = None;
    for a in args.split_whitespace() {
        match a {
            "-i" => show_headers = true,
            "-s" => stats_only = true,
            _ => target = Some(a),
        }
    }
    let Some(target) = target else {
        println!("uso: http [-i] [-s] <url>      (ejemplo: http http://example.com/)");
        println!("  -i  muestra tambien las cabeceras   -s  solo estadisticas");
        user::exit(2);
    };

    let t0 = uptime_ms();
    let mut current = String::from(target);
    let mut resp = None;
    for hop in 0..=5 {
        let url = match parse_url(&current) {
            Ok(u) => u,
            Err(e) => {
                println!("http: {}", e);
                user::exit(1);
            }
        };
        match fetch(&url) {
            Err(e) => {
                println!("http: {}", e);
                user::exit(1);
            }
            Ok(r) => {
                if matches!(r.status, 301 | 302 | 303 | 307 | 308) {
                    if let Some(loc) = r.header("location") {
                        if hop == 5 {
                            println!("http: demasiadas redirecciones");
                            user::exit(1);
                        }
                        let next = resolve_location(&url, loc);
                        println!("[http] {} {} -> {}", r.status, r.reason, next);
                        current = next;
                        continue;
                    }
                }
                resp = Some(r);
                break;
            }
        }
    }
    let r = resp.unwrap();
    let ctype = r.header("content-type").unwrap_or("?");
    println!("[http] {} {}  ({} bytes, {}, {} ms)", r.status, r.reason, r.body.len(), ctype, uptime_ms() - t0);
    if show_headers {
        for (k, v) in &r.headers {
            println!("{}: {}", k, v);
        }
        println!();
    }
    if stats_only {
        let sum = r.body.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32));
        println!("bytes={} suma={}", r.body.len(), sum);
    } else if ctype.starts_with("text/") || ctype.contains("json") || ctype.contains("xml") {
        let text = String::from_utf8_lossy(&r.body);
        print!("{}", text);
        if !text.ends_with('\n') {
            println!();
        }
    } else {
        println!("(contenido binario de {} bytes: no se muestra)", r.body.len());
    }
    if r.status >= 400 {
        user::exit(1);
    }
}
