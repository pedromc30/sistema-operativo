//! Red: tarjeta virtio-net + pila TCP/IP de smoltcp + una tarea del kernel que la mueve.
//!
//! La tarea `net` llama a `poll()` cada 10 ms: la pila procesa los paquetes que llegan,
//! envía los pendientes y avanza DHCP, DNS y TCP. Todo el estado vive en `NET`; para
//! tocarlo se desactivan las interrupciones un momento (un solo núcleo).

use crate::virtio_net::{self, VirtioNet};
use crate::{cpu, pci, scheduler, timer};
use alloc::{format, string::String, vec::Vec};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::{dhcpv4, dns, icmp, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
};

pub struct Net {
    dev: VirtioNet,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: smoltcp::iface::SocketHandle,
    dns: smoltcp::iface::SocketHandle,
    pub ip: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Address>,
    pub dns_servers: Vec<Ipv4Address>,
    pub dhcp_events: u32,
}

static mut NET: Option<Net> = None;

fn net() -> Option<&'static mut Net> {
    unsafe { (*(&raw mut NET)).as_mut() }
}

fn now() -> Instant {
    Instant::from_millis(timer::uptime_ms() as i64)
}

/// Busca la tarjeta de red y la pone en marcha. Devuelve un mensaje de error si no hay.
pub fn init() -> Result<(), &'static str> {
    pci::scan();
    let d = pci::find(virtio_net::VENDOR, virtio_net::DEVICE_LEGACY).ok_or("no hay tarjeta virtio-net")?;
    let mut dev = unsafe { VirtioNet::init(&d) }.ok_or("no se pudo iniciar la tarjeta virtio-net")?;
    let mac = dev.mac;

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = timer::uptime_ms() ^ u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x5a, 0xa5]);
    let iface = Interface::new(config, &mut dev, now());

    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let dns = sockets.add(dns::Socket::new(&[], Vec::new()));

    unsafe {
        *(&raw mut NET) = Some(Net {
            dev,
            iface,
            sockets,
            dhcp,
            dns,
            ip: None,
            gateway: None,
            dns_servers: Vec::new(),
            dhcp_events: 0,
        });
    }
    scheduler::spawn("net", net_task).ok_or("sin memoria para la tarea de red")?;
    Ok(())
}

fn net_task() {
    loop {
        poll();
        timer::sleep_ms(10);
    }
}

/// Un turno de la pila de red.
pub fn poll() {
    cpu::without_interrupts(|| {
        let Some(n) = net() else { return };
        n.iface.poll(now(), &mut n.dev, &mut n.sockets);
        sweep_closing(n);

        // DHCP: cuando el servidor nos da una configuración, la aplicamos.
        let change = match n.sockets.get_mut::<dhcpv4::Socket>(n.dhcp).poll() {
            Some(dhcpv4::Event::Configured(c)) => Some(Some((c.address, c.router, c.dns_servers.clone()))),
            Some(dhcpv4::Event::Deconfigured) => Some(None),
            None => None,
        };
        match change {
            Some(Some((addr, router, servers))) => {
                n.dhcp_events += 1;
                n.iface.update_ip_addrs(|a| {
                    a.clear();
                    let _ = a.push(IpCidr::Ipv4(addr));
                });
                match router {
                    Some(r) => {
                        let _ = n.iface.routes_mut().add_default_ipv4_route(r);
                    }
                    None => {
                        n.iface.routes_mut().remove_default_ipv4_route();
                    }
                }
                let ips: Vec<IpAddress> = servers.iter().map(|s| IpAddress::Ipv4(*s)).collect();
                n.sockets.get_mut::<dns::Socket>(n.dns).update_servers(&ips);
                n.ip = Some(addr);
                n.gateway = router;
                n.dns_servers = servers.iter().copied().collect();
            }
            Some(None) => {
                n.ip = None;
                n.gateway = None;
                n.iface.update_ip_addrs(|a| a.clear());
            }
            None => {}
        }
    });
}

/// Texto con el estado de la red, para el comando `net`.
pub fn info() -> String {
    cpu::without_interrupts(|| {
        let Some(n) = net() else {
            return String::from("red: sin tarjeta de red");
        };
        let m = n.dev.mac;
        let mut s = format!(
            "tarjeta: virtio-net  MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
            m[0], m[1], m[2], m[3], m[4], m[5]
        );
        match n.ip {
            Some(ip) => {
                s.push_str(&format!("IP: {}\n", ip));
                match n.gateway {
                    Some(g) => s.push_str(&format!("puerta de enlace: {}\n", g)),
                    None => s.push_str("puerta de enlace: (ninguna)\n"),
                }
                let dns: Vec<String> = n.dns_servers.iter().map(|d| format!("{}", d)).collect();
                s.push_str(&format!("DNS: {}\n", dns.join(", ")));
            }
            None => s.push_str("IP: (sin configurar: DHCP en curso o sin servidor)\n"),
        }
        s.push_str(&format!("paquetes: {} recibidos, {} enviados\n", n.dev.rx_packets, n.dev.tx_packets));
        s.push_str(&format!("sockets TCP abiertos: {}", socks().iter().filter(|x| x.is_some()).count()));
        s
    })
}


// ---------------------------------------------------------------------------------------
// Códigos de error (negativos, como en las syscalls)
// ---------------------------------------------------------------------------------------
pub const EBADF: i64 = -9;
pub const ENFILE: i64 = -23;
pub const ENETDOWN: i64 = -100;
pub const ECONNRESET: i64 = -104;
pub const ETIMEDOUT: i64 = -110;
pub const ECONNREFUSED: i64 = -111;
pub const ENOTFOUND: i64 = -112; // el nombre no existe o el DNS no responde
pub const EINVAL: i64 = -22;

// ---------------------------------------------------------------------------------------
// Sockets TCP
// ---------------------------------------------------------------------------------------
const MAX_SOCKETS: usize = 8;
const TCP_BUF: usize = 16 * 1024;

struct Sock {
    handle: smoltcp::iface::SocketHandle,
    owner: u64,
    closing_since: Option<u64>, // ms; el socket ya no es del proceso y espera a cerrarse
}

static mut SOCKS: [Option<Sock>; MAX_SOCKETS] = [const { None }; MAX_SOCKETS];
static mut NEXT_PORT: u16 = 49152;

fn socks() -> &'static mut [Option<Sock>; MAX_SOCKETS] {
    unsafe { &mut *(&raw mut SOCKS) }
}

/// Libera los sockets cerrados (o que llevan demasiado tiempo cerrándose).
fn sweep_closing(n: &mut Net) {
    let t = timer::uptime_ms();
    for slot in socks().iter_mut() {
        let Some(sk) = slot else { continue };
        let Some(since) = sk.closing_since else { continue };
        let st = n.sockets.get::<tcp::Socket>(sk.handle).state();
        if st == tcp::State::Closed || st == tcp::State::TimeWait || t.saturating_sub(since) > 8000 {
            n.sockets.get_mut::<tcp::Socket>(sk.handle).abort();
            n.sockets.remove(sk.handle);
            *slot = None;
        }
    }
}

fn owned(id: usize, owner: u64) -> Result<&'static mut Sock, i64> {
    match socks().get_mut(id) {
        Some(Some(sk)) if sk.owner == owner && sk.closing_since.is_none() => Ok(sk),
        _ => Err(EBADF),
    }
}

#[derive(PartialEq, Debug)]
pub enum TcpStatus {
    Connecting,
    Connected,
    Closed,
}

/// Empieza a conectar con `ip:port`. La conexión se completa después (ver `tcp_status`).
pub fn tcp_open(owner: u64, ip: [u8; 4], port: u16) -> Result<usize, i64> {
    cpu::without_interrupts(|| {
        let n = net().ok_or(ENETDOWN)?;
        if n.ip.is_none() {
            return Err(ENETDOWN);
        }
        let id = socks().iter().position(|s| s.is_none()).ok_or(ENFILE)?;
        let mut sock = tcp::Socket::new(
            tcp::SocketBuffer::new(alloc::vec![0u8; TCP_BUF]),
            tcp::SocketBuffer::new(alloc::vec![0u8; TCP_BUF]),
        );
        let local = unsafe {
            let p = NEXT_PORT;
            NEXT_PORT = if NEXT_PORT >= 65000 { 49152 } else { NEXT_PORT + 1 };
            p
        };
        sock.connect(n.iface.context(), (Ipv4Address::from_octets(ip), port), local).map_err(|_| ENETDOWN)?;
        let handle = n.sockets.add(sock);
        socks()[id] = Some(Sock { handle, owner, closing_since: None });
        Ok(id)
    })
}

pub fn tcp_status(owner: u64, id: usize) -> Result<TcpStatus, i64> {
    cpu::without_interrupts(|| {
        let sk = owned(id, owner)?;
        let n = net().ok_or(ENETDOWN)?;
        Ok(match n.sockets.get::<tcp::Socket>(sk.handle).state() {
            tcp::State::Closed => TcpStatus::Closed,
            tcp::State::SynSent | tcp::State::SynReceived => TcpStatus::Connecting,
            _ => TcpStatus::Connected,
        })
    })
}

/// Envía lo que quepa en el búfer de salida (0 = búfer lleno, reintentar luego).
pub fn tcp_send(owner: u64, id: usize, data: &[u8]) -> Result<usize, i64> {
    cpu::without_interrupts(|| {
        let sk = owned(id, owner)?;
        let n = net().ok_or(ENETDOWN)?;
        let s = n.sockets.get_mut::<tcp::Socket>(sk.handle);
        if !s.may_send() {
            return Err(ECONNRESET);
        }
        if !s.can_send() {
            return Ok(0);
        }
        s.send_slice(data).map_err(|_| ECONNRESET)
    })
}

/// `Ok(Some(n))` = n bytes leídos (0 = aún no hay datos); `Ok(None)` = el otro lado cerró.
pub fn tcp_recv(owner: u64, id: usize, buf: &mut [u8]) -> Result<Option<usize>, i64> {
    cpu::without_interrupts(|| {
        let sk = owned(id, owner)?;
        let n = net().ok_or(ENETDOWN)?;
        match n.sockets.get_mut::<tcp::Socket>(sk.handle).recv_slice(buf) {
            Ok(k) => Ok(Some(k)),
            Err(tcp::RecvError::Finished) => Ok(None),
            Err(tcp::RecvError::InvalidState) => Err(ECONNRESET),
        }
    })
}

/// Cierra la conexión de forma ordenada; el socket se libera solo cuando termine.
pub fn tcp_close(owner: u64, id: usize) -> Result<(), i64> {
    cpu::without_interrupts(|| {
        let sk = owned(id, owner)?;
        let n = net().ok_or(ENETDOWN)?;
        n.sockets.get_mut::<tcp::Socket>(sk.handle).close();
        sk.closing_since = Some(timer::uptime_ms());
        Ok(())
    })
}

/// Un proceso ha terminado: se cortan y liberan todos sus sockets.
pub fn close_owned(owner: u64) {
    if let Some(n) = net() {
        for slot in socks().iter_mut() {
            if let Some(sk) = slot {
                if sk.owner == owner {
                    n.sockets.get_mut::<tcp::Socket>(sk.handle).abort();
                    n.sockets.remove(sk.handle);
                    *slot = None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// DNS
// ---------------------------------------------------------------------------------------

/// Convierte "a.b.c.d" en 4 bytes.
pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut it = s.split('.');
    for b in out.iter_mut() {
        *b = it.next()?.parse().ok()?;
    }
    if it.next().is_some() { None } else { Some(out) }
}

/// Resuelve un nombre a una dirección IPv4 (bloquea hasta 5 s). Una IP escrita con
/// puntos se devuelve tal cual, sin consultar al DNS.
pub fn resolve(name: &str) -> Result<[u8; 4], i64> {
    if let Some(ip) = parse_ip(name) {
        return Ok(ip);
    }
    let handle = cpu::without_interrupts(|| {
        let n = net().ok_or(ENETDOWN)?;
        if n.ip.is_none() {
            return Err(ENETDOWN);
        }
        n.sockets
            .get_mut::<dns::Socket>(n.dns)
            .start_query(n.iface.context(), name, DnsQueryType::A)
            .map_err(|_| EINVAL)
    })?;
    let deadline = timer::uptime_ms() + 5000;
    loop {
        let r = cpu::without_interrupts(|| {
            let n = net()?;
            match n.sockets.get_mut::<dns::Socket>(n.dns).get_query_result(handle) {
                Ok(addrs) => Some(
                    addrs
                        .iter()
                        .find_map(|a| match a {
                            IpAddress::Ipv4(v) => Some(v.octets()),
                        })
                        .ok_or(ENOTFOUND),
                ),
                Err(dns::GetQueryResultError::Pending) => None,
                Err(dns::GetQueryResultError::Failed) => Some(Err(ENOTFOUND)),
            }
        });
        if let Some(res) = r {
            return res;
        }
        if timer::uptime_ms() > deadline {
            cpu::without_interrupts(|| {
                if let Some(n) = net() {
                    n.sockets.get_mut::<dns::Socket>(n.dns).cancel_query(handle);
                }
            });
            return Err(ETIMEDOUT);
        }
        timer::sleep_ms(10);
    }
}

// ---------------------------------------------------------------------------------------
// Ping (ICMP)
// ---------------------------------------------------------------------------------------

/// Envía un "echo request" y espera la respuesta. Devuelve los milisegundos que tardó.
pub fn ping(ip: [u8; 4], seq: u16) -> Result<u64, i64> {
    const IDENT: u16 = 0x5255;
    let handle = cpu::without_interrupts(|| {
        let n = net().ok_or(ENETDOWN)?;
        if n.ip.is_none() {
            return Err(ENETDOWN);
        }
        let mut sock = icmp::Socket::new(
            icmp::PacketBuffer::new(alloc::vec![icmp::PacketMetadata::EMPTY; 2], alloc::vec![0u8; 512]),
            icmp::PacketBuffer::new(alloc::vec![icmp::PacketMetadata::EMPTY; 2], alloc::vec![0u8; 512]),
        );
        sock.bind(icmp::Endpoint::Ident(IDENT)).map_err(|_| EINVAL)?;
        let h = n.sockets.add(sock);

        let data = [0x61u8; 32];
        let repr = Icmpv4Repr::EchoRequest { ident: IDENT, seq_no: seq, data: &data };
        let mut bytes = alloc::vec![0u8; repr.buffer_len()];
        repr.emit(&mut Icmpv4Packet::new_unchecked(&mut bytes), &ChecksumCapabilities::default());
        let s = n.sockets.get_mut::<icmp::Socket>(h);
        let _ = s.send_slice(&bytes, IpAddress::Ipv4(Ipv4Address::from_octets(ip)));
        Ok(h)
    })?;
    let start = timer::uptime_ms();
    let result = loop {
        let got = cpu::without_interrupts(|| {
            let n = net()?;
            let s = n.sockets.get_mut::<icmp::Socket>(handle);
            if !s.can_recv() {
                return None;
            }
            let mut buf = [0u8; 256];
            let (len, _) = s.recv_slice(&mut buf).ok()?;
            let pkt = Icmpv4Packet::new_checked(&buf[..len]).ok()?;
            match Icmpv4Repr::parse(&pkt, &ChecksumCapabilities::default()) {
                Ok(Icmpv4Repr::EchoReply { ident: IDENT, seq_no, .. }) if seq_no == seq => Some(true),
                _ => None,
            }
        });
        if got == Some(true) {
            break Ok(timer::uptime_ms() - start);
        }
        if timer::uptime_ms() - start > 3000 {
            break Err(ETIMEDOUT);
        }
        timer::sleep_ms(10);
    };
    cpu::without_interrupts(|| {
        if let Some(n) = net() {
            n.sockets.remove(handle);
        }
    });
    result
}

/// Cambia el servidor DNS (para pruebas o si el de DHCP no sirve).
pub fn set_dns(ip: [u8; 4]) -> bool {
    cpu::without_interrupts(|| {
        let Some(n) = net() else { return false };
        let a = Ipv4Address::from_octets(ip);
        n.sockets.get_mut::<dns::Socket>(n.dns).update_servers(&[IpAddress::Ipv4(a)]);
        n.dns_servers = alloc::vec![a];
        true
    })
}
