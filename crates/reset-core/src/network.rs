//! Rede: adaptadores (cabo e Wi-Fi), gateway, DNS e o diagnóstico de lag do `NetworkTest.cs`: latência, jitter e
//! perda até o roteador e a internet, tempo de resposta do DNS, velocidade de download/upload e latência sob carga
//! (bufferbloat). Sem root: o ping do sistema já pode mandar ICMP, o DNS é uma consulta UDP feita aqui e a
//! velocidade usa o `curl` com o mesmo servidor público do Windows (speed.cloudflare.com). Não grava nada no disco.

use crate::util::{read_hex, read_i64, read_trim, read_u64};
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::CStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const NET: &str = "/sys/class/net";
const DOWN_URL: &str = "https://speed.cloudflare.com/__down?bytes=25000000";
const UP_URL: &str = "https://speed.cloudflare.com/__up";

// ---------------------------------------------------------------- adaptadores

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct WifiLink {
    pub connected: bool,
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub freq_mhz: Option<f64>,
    /// "2,4 GHz", "5 GHz", "6 GHz"
    pub band: Option<String>,
    pub signal_dbm: Option<i32>,
    /// 0..100, aproximado a partir do dBm (-90 = 0, -30 = 100).
    pub signal_pct: Option<u32>,
    pub rx_mbps: Option<f64>,
    pub tx_mbps: Option<f64>,
    /// "Wi-Fi 6", "Wi-Fi 5"... pelo tipo de modulação da taxa (HE/VHT/HT).
    pub standard: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct EthLink {
    /// Maior velocidade que a placa do PC aceita (Mbps).
    pub nic_max_mbps: Option<u32>,
    /// Maior velocidade que o outro lado (roteador/switch) anuncia.
    pub partner_max_mbps: Option<u32>,
    pub duplex: Option<String>,
    pub autoneg: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Iface {
    pub name: String,
    /// "ethernet" | "wifi" | "vpn" | "bridge" | "virtual"
    pub kind: &'static str,
    pub kind_label: &'static str,
    /// operstate: "up", "down", "dormant"...
    pub state: String,
    pub up: bool,
    pub mac: Option<String>,
    pub mtu: Option<u64>,
    pub speed_mbps: Option<u32>,
    pub driver: Option<String>,
    /// Nome do chip pelo banco pci.ids: "Realtek RTL8111/8168/8211/8411 PCI Express Gigabit Ethernet Controller".
    pub model: Option<String>,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub wifi: Option<WifiLink>,
    pub eth: Option<EthLink>,
    /// É por ela que sai a internet (rota padrão).
    pub default: bool,
    /// Aviso sobre o link: Wi-Fi fraco, cabo negociando abaixo do que a placa aceita...
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gateway {
    pub iface: String,
    pub address: String,
    pub metric: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetOverview {
    pub hostname: Option<String>,
    pub interfaces: Vec<Iface>,
    pub gateway: Option<Gateway>,
    pub dns: Vec<String>,
    /// O sistema usa o systemd-resolved (127.0.0.53) na frente dos servidores acima.
    pub resolved: bool,
}

/// Rotas padrão de /proc/net/route (IPv4), da de menor métrica para a maior.
pub fn parse_routes(text: &str) -> Vec<Gateway> {
    let mut v: Vec<Gateway> = text
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            let (iface, dest, gw, flags, metric, mask) = (c.first()?, c.get(1)?, c.get(2)?, c.get(3)?, c.get(6)?, c.get(7)?);
            let flags = u32::from_str_radix(flags, 16).ok()?;
            // destino 0.0.0.0/0 com o bit RTF_GATEWAY (2) e a rota ativa (RTF_UP, 1)
            if *dest != "00000000" || *mask != "00000000" || flags & 3 != 3 {
                return None;
            }
            let addr = Ipv4Addr::from(u32::from_str_radix(gw, 16).ok()?.to_ne_bytes());
            Some(Gateway { iface: iface.to_string(), address: addr.to_string(), metric: metric.parse().unwrap_or(0) })
        })
        .collect();
    v.sort_by_key(|g| g.metric);
    v
}

/// Servidores `nameserver` de um resolv.conf.
pub fn parse_resolv(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        if w.next() == Some("nameserver") {
            if let Some(s) = w.next().filter(|s| s.parse::<IpAddr>().is_ok()) {
                if !out.iter().any(|x| x == s) {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

/// DNS de verdade: com o systemd-resolved, o /etc/resolv.conf só aponta para 127.0.0.53; os servidores ficam
/// em /run/systemd/resolve/resolv.conf.
pub fn dns_servers() -> (Vec<String>, bool) {
    let etc = parse_resolv(&fs::read_to_string("/etc/resolv.conf").unwrap_or_default());
    let resolved = etc.iter().any(|s| s == "127.0.0.53" || s == "127.0.0.54");
    if resolved {
        let up = parse_resolv(&fs::read_to_string("/run/systemd/resolve/resolv.conf").unwrap_or_default());
        if !up.is_empty() {
            return (up, true);
        }
    }
    (etc, resolved)
}

/// Lê a saída do `iw dev <if> link`:
/// ```text
/// Connected to aa:bb:cc:dd:ee:ff (on wlan0)
///     SSID: MinhaRede
///     freq: 5180.0
///     signal: -52 dBm
///     rx bitrate: 866.7 MBit/s VHT-MCS 9 80MHz short GI VHT-NSS 2
/// ```
pub fn parse_iw_link(text: &str) -> WifiLink {
    let mut w = WifiLink::default();
    let mut rates = String::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("Connected to ") {
            w.connected = true;
            w.bssid = rest.split_whitespace().next().map(String::from);
        } else if let Some((k, v)) = t.split_once(':') {
            let v = v.trim();
            let first = v.split_whitespace().next().unwrap_or("");
            match k {
                "SSID" => w.ssid = Some(v.to_string()).filter(|s| !s.is_empty()),
                "freq" => w.freq_mhz = first.parse().ok(),
                "signal" => w.signal_dbm = first.parse().ok(),
                "rx bitrate" => {
                    w.rx_mbps = first.parse().ok();
                    rates.push_str(v);
                }
                "tx bitrate" => {
                    w.tx_mbps = first.parse().ok();
                    rates.push(' ');
                    rates.push_str(v);
                }
                _ => {}
            }
        }
    }
    w.band = w.freq_mhz.map(|f| if f < 3000.0 { "2,4 GHz" } else if f < 5925.0 { "5 GHz" } else { "6 GHz" }.to_string());
    w.signal_pct = w.signal_dbm.map(signal_pct);
    let six_ghz = w.freq_mhz.is_some_and(|f| f >= 5925.0);
    w.standard = if rates.contains("EHT-") {
        Some("Wi-Fi 7")
    } else if rates.contains("HE-") {
        Some(if six_ghz { "Wi-Fi 6E" } else { "Wi-Fi 6" })
    } else if rates.contains("VHT-") {
        Some("Wi-Fi 5")
    } else if rates.contains("MCS") {
        Some("Wi-Fi 4")
    } else {
        None
    }
    .map(String::from);
    w
}

pub fn signal_pct(dbm: i32) -> u32 {
    ((dbm + 90) as f64 * 100.0 / 60.0).clamp(0.0, 100.0) as u32
}

/// Sinal pela /proc/net/wireless (quando o `iw` não existe): coluna "level", em dBm.
pub fn parse_proc_wireless(text: &str, iface: &str) -> Option<i32> {
    let line = text.lines().find(|l| l.trim_start().starts_with(&format!("{iface}:")))?;
    let level = line.split_whitespace().nth(3)?;
    level.trim_end_matches('.').parse::<f64>().ok().map(|v| v as i32)
}

/// Velocidade de um modo do ethtool ("1000baseT/Full" → 1000).
fn mode_mbps(m: &str) -> Option<u32> {
    m.split("base").next()?.parse().ok()
}

/// Lê a saída do `ethtool <if>` (funciona sem root): o que a placa aceita e o que o outro lado anuncia.
pub fn parse_ethtool(text: &str) -> EthLink {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut key = String::new();
    for l in text.lines().skip_while(|l| !l.starts_with("Settings for")).skip(1) {
        match l.split_once(':') {
            Some((k, v)) => {
                key = k.trim().to_string();
                fields.insert(key.clone(), v.trim().to_string());
            }
            None => {
                if let Some(v) = fields.get_mut(&key) {
                    v.push(' ');
                    v.push_str(l.trim());
                }
            }
        }
    }
    let max = |k: &str| fields.get(k).and_then(|v| v.split_whitespace().filter_map(mode_mbps).max());
    EthLink {
        nic_max_mbps: max("Supported link modes"),
        partner_max_mbps: max("Link partner advertised link modes"),
        duplex: fields.get("Duplex").filter(|d| !d.starts_with("Unknown")).map(|d| if d == "Full" { "completo" } else { "meio (half)" }.to_string()),
        autoneg: fields.get("Auto-negotiation").map(|v| v == "on"),
    }
}

/// Explica um cabo que negociou abaixo do que a placa aceita.
pub fn eth_note(speed: Option<u32>, e: &EthLink) -> Option<String> {
    let speed = speed?;
    let nic = e.nic_max_mbps?;
    if speed >= nic {
        return None;
    }
    Some(match e.partner_max_mbps {
        Some(p) if p <= speed => format!(
            "O link está em {speed} Mbps porque o outro lado (roteador ou switch) só anuncia até {p} Mbps. A placa do PC aceita {nic} Mbps: ligue numa porta gigabit para ganhar velocidade na rede local."
        ),
        Some(p) => format!(
            "A placa ({nic} Mbps) e o roteador ({p} Mbps) aceitam mais, mas o link negociou só {speed} Mbps. Quase sempre é cabo ruim ou mal crimpado: troque por um Cat5e ou Cat6."
        ),
        None => format!("O link negociou {speed} Mbps, mas a placa aceita {nic} Mbps. Verifique o cabo e a porta do roteador."),
    })
}

/// (fabricante, chip) no formato do /usr/share/hwdata/pci.ids.
pub fn pci_lookup(text: &str, vendor: u32, device: u32) -> (Option<String>, Option<String>) {
    let v = format!("{vendor:04x}");
    let d = format!("{device:04x}");
    let mut vendor_name = None;
    for l in text.lines() {
        if l.starts_with('#') || l.is_empty() {
            continue;
        }
        if !l.starts_with('\t') {
            if vendor_name.is_some() {
                break; // passou do fabricante sem achar o chip
            }
            if let Some(rest) = l.strip_prefix(&v) {
                vendor_name = Some(rest.trim().to_string());
            }
        } else if vendor_name.is_some() && !l.starts_with("\t\t") {
            if let Some(rest) = l[1..].strip_prefix(&d) {
                return (vendor_name, Some(rest.trim().to_string()));
            }
        }
    }
    (vendor_name, None)
}

fn short_vendor(v: &str) -> String {
    let w = v.split([' ', ',']).next().unwrap_or(v);
    let mut c = w.chars();
    // "MEDIATEK" → "MediaTek" fica estranho; só tira o caixa-alta total
    if w.len() > 3 && w.chars().all(|c| c.is_ascii_uppercase()) {
        c.next().map(|f| f.to_string() + &c.as_str().to_lowercase()).unwrap_or_default()
    } else {
        w.to_string()
    }
}

/// Endereços IPv4/IPv6 (com prefixo) de cada interface, pelo getifaddrs da libc.
fn addresses() -> HashMap<String, (Vec<String>, Vec<String>)> {
    let mut map: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    // SAFETY: getifaddrs devolve uma lista ligada que só é lida aqui e liberada no fim com freeifaddrs.
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return map;
        }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            p = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
                continue;
            }
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().to_string();
            let entry = map.entry(name).or_default();
            match (*ifa.ifa_addr).sa_family as i32 {
                libc::AF_INET => {
                    let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let ip = Ipv4Addr::from(a.sin_addr.s_addr.to_ne_bytes());
                    let prefix = if ifa.ifa_netmask.is_null() { 32 } else { (*(ifa.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr.count_ones() };
                    entry.0.push(format!("{ip}/{prefix}"));
                }
                libc::AF_INET6 => {
                    let a = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    let ip = Ipv6Addr::from(a.sin6_addr.s6_addr);
                    let prefix: u32 = if ifa.ifa_netmask.is_null() {
                        128
                    } else {
                        (*(ifa.ifa_netmask as *const libc::sockaddr_in6)).sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum()
                    };
                    entry.1.push(format!("{ip}/{prefix}"));
                }
                _ => {}
            }
        }
        libc::freeifaddrs(ifap);
    }
    map
}

fn run_text(prog: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(prog).args(args).env("LC_ALL", "C").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Programa do sistema pelo caminho absoluto (/usr/bin, /usr/sbin, /bin, /sbin).
fn tool(name: &str) -> Option<String> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"].iter().map(|d| format!("{d}/{name}")).find(|p| Path::new(p).exists())
}

fn kind_of(dir: &Path, name: &str) -> (&'static str, &'static str) {
    if dir.join("wireless").exists() || dir.join("phy80211").exists() {
        ("wifi", "Wi-Fi")
    } else if dir.join("bridge").exists() {
        ("bridge", "Ponte (bridge)")
    } else if dir.join("tun_flags").exists() || name.starts_with("wg") || name.starts_with("tailscale") || name.starts_with("zt") {
        ("vpn", "VPN / túnel")
    } else if !dir.join("device").exists() {
        ("virtual", "Virtual")
    } else {
        ("ethernet", "Ethernet (cabo)")
    }
}

pub fn overview() -> NetOverview {
    let addrs = addresses();
    let gateways = parse_routes(&fs::read_to_string("/proc/net/route").unwrap_or_default());
    let gateway = gateways.first().cloned();
    let pci_ids = fs::read_to_string("/usr/share/hwdata/pci.ids").unwrap_or_default();
    let wireless = fs::read_to_string("/proc/net/wireless").unwrap_or_default();
    let iw = tool("iw");
    let ethtool = tool("ethtool");

    let mut interfaces: Vec<Iface> = fs::read_dir(NET)
        .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|dir| {
            let name = dir.file_name()?.to_string_lossy().to_string();
            if read_u64(dir.join("type")) == Some(772) {
                return None; // loopback
            }
            let (kind, kind_label) = kind_of(&dir, &name);
            let state = read_trim(dir.join("operstate")).unwrap_or_else(|| "unknown".into());
            let carrier = read_u64(dir.join("carrier")) == Some(1);
            let up = carrier && (state == "up" || state == "unknown");
            let stat = |f: &str| read_u64(dir.join("statistics").join(f)).unwrap_or(0);
            let driver = fs::read_link(dir.join("device/driver")).ok().and_then(|p| Some(p.file_name()?.to_string_lossy().to_string()));
            let model = match (read_hex(dir.join("device/vendor")), read_hex(dir.join("device/device"))) {
                (Some(v), Some(d)) => match pci_lookup(&pci_ids, v, d) {
                    (Some(vn), Some(dn)) => Some(format!("{} {dn}", short_vendor(&vn))),
                    _ => None,
                },
                _ => None,
            };
            let speed_mbps = if up { read_i64(dir.join("speed")).filter(|s| *s > 0).map(|s| s as u32) } else { None };
            let (ipv4, ipv6) = addrs.get(&name).cloned().unwrap_or_default();
            let wifi = (kind == "wifi").then(|| {
                let mut w = iw.as_deref().and_then(|iw| run_text(iw, &["dev", &name, "link"])).map(|t| parse_iw_link(&t)).unwrap_or_default();
                if w.signal_dbm.is_none() && up {
                    w.signal_dbm = parse_proc_wireless(&wireless, &name);
                    w.signal_pct = w.signal_dbm.map(signal_pct);
                    w.connected = w.signal_dbm.is_some();
                }
                w
            });
            let eth = (kind == "ethernet" && up).then(|| ethtool.as_deref().and_then(|e| run_text(e, &[&name])).map(|t| parse_ethtool(&t)).unwrap_or_default());
            let note = match (&wifi, &eth) {
                (Some(w), _) => w.signal_dbm.filter(|d| *d < -70).map(|d| {
                    format!("Sinal fraco ({d} dBm): espere quedas e latência oscilando. Aproxime o PC do roteador, prefira 5 GHz ou use cabo.")
                }),
                (_, Some(e)) => eth_note(speed_mbps, e),
                _ => None,
            };
            Some(Iface {
                default: gateway.as_ref().is_some_and(|g| g.iface == name),
                speed_mbps: speed_mbps.or_else(|| wifi.as_ref().and_then(|w| w.tx_mbps.map(|v| v as u32))),
                mac: read_trim(dir.join("address")).filter(|m| m != "00:00:00:00:00:00"),
                mtu: read_u64(dir.join("mtu")),
                rx_bytes: stat("rx_bytes"),
                tx_bytes: stat("tx_bytes"),
                rx_errors: stat("rx_errors"),
                tx_errors: stat("tx_errors"),
                rx_dropped: stat("rx_dropped"),
                name,
                kind,
                kind_label,
                state,
                up,
                driver,
                model,
                ipv4,
                ipv6,
                wifi,
                eth,
                note,
            })
        })
        .collect();
    // a da internet primeiro, depois as físicas conectadas, depois o resto
    let rank = |i: &Iface| (!i.default, !i.up, !matches!(i.kind, "ethernet" | "wifi"), i.name.clone());
    interfaces.sort_by_key(rank);
    let (dns, resolved) = dns_servers();
    NetOverview { hostname: read_trim("/proc/sys/kernel/hostname"), interfaces, gateway, dns, resolved }
}

/// Contadores de bytes de cada interface, para a interface calcular o tráfego por segundo.
#[derive(Debug, Clone, Serialize)]
pub struct Traffic {
    pub name: String,
    pub rx: u64,
    pub tx: u64,
}

pub fn traffic() -> Vec<Traffic> {
    fs::read_dir(NET)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name() != "lo")
                .map(|e| {
                    let s = |f: &str| read_u64(e.path().join("statistics").join(f)).unwrap_or(0);
                    Traffic { name: e.file_name().to_string_lossy().to_string(), rx: s("rx_bytes"), tx: s("tx_bytes") }
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- ping

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PingLine {
    Reply { seq: u32, ms: f64 },
    Summary { sent: u32, received: u32 },
}

/// Uma linha do `ping` (iputils, com LC_ALL=C):
/// `64 bytes from 1.1.1.1: icmp_seq=1 ttl=53 time=45.5 ms` e `30 packets transmitted, 29 received, ...`.
pub fn parse_ping_line(l: &str) -> Option<PingLine> {
    if l.contains("packets transmitted") {
        let n: Vec<u32> = l.split(',').filter_map(|p| p.split_whitespace().next()?.parse().ok()).collect();
        return Some(PingLine::Summary { sent: *n.first()?, received: *n.get(1)? });
    }
    let field = |k: &str| l.split_whitespace().find_map(|w| w.strip_prefix(k));
    let seq = field("icmp_seq=")?.parse().ok()?;
    let ms = field("time=")?.parse().ok()?;
    Some(PingLine::Reply { seq, ms })
}

#[derive(Debug, Clone, Serialize)]
pub struct PingStat {
    pub name: String,
    pub address: String,
    pub sent: u32,
    pub lost: u32,
    pub loss_pct: f64,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub jitter_ms: f64,
    pub answered: bool,
}

/// Estatística igual à do Windows: jitter = média da diferença entre respostas seguidas.
pub fn ping_stat(name: &str, address: &str, rtts: &[f64], sent: u32) -> PingStat {
    let sent = sent.max(rtts.len() as u32);
    let lost = sent - rtts.len() as u32;
    let loss_pct = if sent == 0 { 0.0 } else { 100.0 * lost as f64 / sent as f64 };
    if rtts.is_empty() {
        return PingStat { name: name.into(), address: address.into(), sent, lost, loss_pct, min_ms: 0.0, avg_ms: 0.0, max_ms: 0.0, jitter_ms: 0.0, answered: false };
    }
    let jitter = if rtts.len() > 1 { rtts.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (rtts.len() - 1) as f64 } else { 0.0 };
    PingStat {
        name: name.into(),
        address: address.into(),
        sent,
        lost,
        loss_pct,
        min_ms: rtts.iter().copied().fold(f64::MAX, f64::min),
        avg_ms: rtts.iter().sum::<f64>() / rtts.len() as f64,
        max_ms: rtts.iter().copied().fold(0.0, f64::max),
        jitter_ms: jitter,
        answered: true,
    }
}

/// Espera o processo terminar, matando-o se passar do prazo ou se o teste for cancelado.
fn wait_or_kill(child: &mut Child, deadline: Instant, cancel: &AtomicBool) -> Option<std::process::ExitStatus> {
    loop {
        if let Ok(Some(s)) = child.try_wait() {
            return Some(s);
        }
        if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// `count` pings a cada 200 ms (o menor intervalo permitido sem root). Cada resposta vai para `on_reply`.
/// Devolve (tempos, enviados). Para no cancelamento ou quando `stop` liga (latência sob carga).
fn ping_loop(ping: &str, addr: &str, count: u32, cancel: &AtomicBool, stop: Option<&AtomicBool>, on_reply: &(dyn Fn(u32, f64) + Sync)) -> (Vec<f64>, u32) {
    let child = Command::new(ping)
        .args(["-n", "-i", "0.2", "-W", "1.5", "-c", &count.to_string(), addr])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return (Vec::new(), 0) };
    let mut rtts = Vec::new();
    let mut max_seq = 0;
    let mut sent = None;
    let out = child.stdout.take().unwrap();
    // o leitor fica num thread: assim o cancelamento não espera a próxima linha do ping
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    loop {
        if cancel.load(Ordering::Relaxed) || stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(l) => match parse_ping_line(&l) {
                Some(PingLine::Reply { seq, ms }) => {
                    max_seq = max_seq.max(seq);
                    rtts.push(ms);
                    on_reply(seq, ms);
                }
                Some(PingLine::Summary { sent: s, .. }) => sent = Some(s),
                None => {}
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    let _ = child.wait();
    let _ = reader.join();
    let sent = sent.unwrap_or(if stop.is_some() || cancel.load(Ordering::Relaxed) { max_seq } else { count });
    (rtts, sent)
}

// ---------------------------------------------------------------- DNS

/// Consulta DNS tipo A para `name`, com o identificador `id`.
pub fn dns_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // recursão desejada, 1 pergunta
    for label in name.split('.').filter(|l| !l.is_empty()) {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // fim do nome, tipo A, classe IN
    q
}

/// A resposta é para esta consulta e veio sem erro (NOERROR)?
pub fn dns_answer_ok(id: u16, resp: &[u8]) -> bool {
    resp.len() >= 12 && resp[0..2] == id.to_be_bytes() && resp[2] & 0x80 != 0 && resp[3] & 0x0F == 0
}

/// Tempo (ms) de uma consulta ao servidor `server`, ou None se não respondeu em 2 s.
pub fn dns_time(server: IpAddr, name: &str, id: u16) -> Option<f64> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse().unwrap() } else { "[::]:0".parse().unwrap() };
    let s = UdpSocket::bind(bind).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let q = dns_query(id, name);
    let t = Instant::now();
    s.send_to(&q, SocketAddr::new(server, 53)).ok()?;
    let mut buf = [0u8; 1500];
    let deadline = t + Duration::from_secs(2);
    while Instant::now() < deadline {
        let (n, from) = s.recv_from(&mut buf).ok()?;
        if from.ip() == server && dns_answer_ok(id, &buf[..n]) {
            return Some(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    None
}

fn dns_label(ip: &str, gateway: Option<&str>) -> String {
    match ip {
        "1.1.1.1" | "1.0.0.1" | "2606:4700:4700::1111" | "2606:4700:4700::1001" => "Cloudflare".into(),
        "8.8.8.8" | "8.8.4.4" | "2001:4860:4860::8888" | "2001:4860:4860::8844" => "Google".into(),
        "9.9.9.9" | "149.112.112.112" => "Quad9".into(),
        "208.67.222.222" | "208.67.220.220" => "OpenDNS".into(),
        _ if Some(ip) == gateway => "Roteador".into(),
        _ => "Provedor".into(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DnsResult {
    pub name: String,
    pub server: String,
    /// É um dos DNS que o seu sistema usa.
    pub yours: bool,
    /// Mediana de 3 consultas.
    pub ms: Option<f64>,
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

// ---------------------------------------------------------------- velocidade (curl + speed.cloudflare.com)

/// Download por `secs` segundos com `streams` conexões em paralelo. `tick(segundos, mbps_agora, mbps_media)`
/// é chamado 4x por segundo. Devolve a média em Mbps.
fn download(curl: &str, secs: u64, streams: usize, cancel: &AtomicBool, tick: &dyn Fn(f64, f64, f64)) -> Result<f64, String> {
    let total = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);
    std::thread::scope(|sc| {
        for _ in 0..streams {
            sc.spawn(|| {
                while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
                    let left = deadline.saturating_duration_since(Instant::now()).as_secs_f64() + 1.0;
                    let child = Command::new(curl)
                        .args(["-s", "-f", "-L", "--max-time", &format!("{left:.1}"), "-o", "-", DOWN_URL])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn();
                    let Ok(mut child) = child else {
                        failed.fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    let mut out = child.stdout.take().unwrap();
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut got = 0u64;
                    loop {
                        match out.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                got += n as u64;
                                total.fetch_add(n as u64, Ordering::Relaxed);
                            }
                        }
                        if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    if got == 0 {
                        failed.fetch_add(1, Ordering::Relaxed);
                        return; // servidor fora do ar ou bloqueado: não adianta insistir
                    }
                }
            });
        }
        meter(&total, start, deadline, cancel, tick);
    });
    finish(&total, start, &failed, streams, cancel)
}

/// Upload: cada conexão envia 1 MB por vez (como o Windows) e só conta os envios concluídos.
fn upload(curl: &str, secs: u64, streams: usize, cancel: &AtomicBool, tick: &dyn Fn(f64, f64, f64)) -> Result<f64, String> {
    let total = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);
    // dados pseudoaleatórios (xorshift): não comprimem no caminho
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let payload: Vec<u8> = (0..1_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::thread::scope(|sc| {
        for _ in 0..streams {
            sc.spawn(|| {
                while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
                    // --max-time 0 seria "sem limite": nunca menos que 0,2 s
                    let left = deadline.saturating_duration_since(Instant::now()).as_secs_f64().max(0.2);
                    let child = Command::new(curl)
                        .args(["-s", "-f", "-o", "/dev/null", "-w", "%{size_upload}", "--max-time", &format!("{left:.1}")])
                        .args(["-H", "Content-Type: application/octet-stream", "--data-binary", "@-", UP_URL])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn();
                    let Ok(mut child) = child else {
                        failed.fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    let mut stdin = child.stdin.take().unwrap();
                    let _ = stdin.write_all(&payload);
                    drop(stdin);
                    let ok = wait_or_kill(&mut child, deadline + Duration::from_millis(300), cancel).is_some_and(|s| s.success());
                    let mut out = String::new();
                    if let Some(mut o) = child.stdout.take() {
                        let _ = o.read_to_string(&mut out);
                    }
                    match out.trim().parse::<f64>() {
                        Ok(n) if ok && n > 0.0 => {
                            total.fetch_add(n as u64, Ordering::Relaxed);
                        }
                        _ if Instant::now() < deadline && !cancel.load(Ordering::Relaxed) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        _ => {}
                    }
                }
            });
        }
        meter(&total, start, deadline, cancel, tick);
    });
    finish(&total, start, &failed, streams, cancel)
}

fn meter(total: &AtomicU64, start: Instant, deadline: Instant, cancel: &AtomicBool, tick: &dyn Fn(f64, f64, f64)) {
    let mut last = (start, 0u64);
    while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        let now = Instant::now();
        let b = total.load(Ordering::Relaxed);
        let inst = mbps(b - last.1, now.duration_since(last.0).as_secs_f64());
        let avg = mbps(b, now.duration_since(start).as_secs_f64());
        tick(now.duration_since(start).as_secs_f64(), inst, avg);
        last = (now, b);
    }
}

fn finish(total: &AtomicU64, start: Instant, failed: &AtomicU64, streams: usize, cancel: &AtomicBool) -> Result<f64, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelado".into());
    }
    let b = total.load(Ordering::Relaxed);
    if b == 0 {
        return Err(if failed.load(Ordering::Relaxed) as usize >= streams {
            "speed.cloudflare.com inacessível ou bloqueado".into()
        } else {
            "nenhum dado transferido".into()
        });
    }
    Ok(mbps(b, start.elapsed().as_secs_f64()))
}

pub fn mbps(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        0.0
    } else {
        bytes as f64 * 8.0 / 1e6 / secs
    }
}

// ---------------------------------------------------------------- diagnóstico completo

/// Andamento do teste, mandado para a interface enquanto roda.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TestEvent {
    Stage { text: String, frac: f64 },
    Targets { names: Vec<String>, addresses: Vec<String> },
    Ping { target: usize, seq: u32, ms: f64 },
    Loaded { seq: u32, ms: f64 },
    Speed { dir: &'static str, t: f64, mbps: f64, avg: f64 },
    Dns { result: DnsResult },
}

#[derive(Debug, Clone, Serialize)]
pub struct NetReport {
    /// "good" | "warn" | "bad"
    pub verdict: &'static str,
    /// "ESTÁVEL" | "ATENÇÃO" | "RUIM" | "SEM REDE"
    pub label: &'static str,
    pub reasons: Vec<String>,
    pub pings: Vec<PingStat>,
    pub dns: Vec<DnsResult>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    /// Média do ping até 1.1.1.1 durante o download.
    pub loaded_ms: Option<f64>,
    pub speed_error: Option<String>,
    pub adapter: Option<String>,
    pub link: Option<String>,
    pub ip: Option<String>,
    pub gateway: Option<String>,
}

/// Veredito do `NetworkTest.cs`, com os mesmos limites. `pings[0]` é o roteador; os outros, a internet.
/// `link_note` é o aviso do adaptador (Wi-Fi fraco, cabo abaixo da velocidade) e se ele pesa no veredito: um link de
/// 100 Mbps só é problema quando a internet chega nesse teto.
pub fn analyze(
    pings: &[PingStat],
    dns_ms: Option<f64>,
    loaded_ms: Option<f64>,
    down: Option<f64>,
    speed_failed: bool,
    link_note: Option<(&str, bool)>,
) -> (&'static str, &'static str, Vec<String>) {
    let mut warn: Vec<String> = Vec::new();
    let mut bad = false;
    let mut harmless = 0; // avisos que não pioram o veredito
    let gw = &pings[0];
    let inet = &pings[1..];
    let net: Vec<&PingStat> = inet.iter().filter(|p| p.answered).collect();
    let net_avg = if net.is_empty() { 0.0 } else { net.iter().map(|p| p.avg_ms).sum::<f64>() / net.len() as f64 };
    let net_loss = if inet.is_empty() { 0.0 } else { inet.iter().map(|p| p.loss_pct).sum::<f64>() / inet.len() as f64 };
    let net_jit = net.iter().map(|p| p.jitter_ms).fold(0.0, f64::max);

    let lan_bad = gw.answered && (gw.loss_pct > 2.0 || gw.avg_ms > 30.0 || gw.jitter_ms > 15.0);
    if lan_bad {
        warn.push(format!(
            "A rede local está instável (roteador: {:.0} ms, perda {}%, jitter {:.0} ms). Se for Wi-Fi, aproxime-se do roteador ou use cabo.",
            gw.avg_ms,
            fmt1(gw.loss_pct),
            gw.jitter_ms
        ));
    }
    if !gw.answered {
        warn.push("O roteador não responde a ping (alguns modelos bloqueiam). Isso por si só não é problema.".into());
        harmless += 1;
    }
    if net.is_empty() {
        bad = true;
        warn.push("Nenhum dos servidores da internet respondeu: sem acesso à internet ou ping bloqueado pela rede.".into());
    } else {
        if net_loss > 5.0 || net_avg > 150.0 {
            bad = true;
        }
        if net_loss > 1.0 {
            let origem = if lan_bad { "Provavelmente vem da rede local." } else { "Como o roteador está bem, o problema deve estar na linha ou no provedor." };
            warn.push(format!("Há perda de pacotes até a internet ({}%). {origem}", fmt1(net_loss)));
        }
        if net_avg > 80.0 {
            let extra = if lan_bad { "" } else { " O roteador responde bem, então o atraso vem do provedor." };
            warn.push(format!("Latência alta até a internet ({net_avg:.0} ms).{extra}"));
        }
        if net_jit > 20.0 {
            warn.push(format!("O jitter está alto ({net_jit:.0} ms): a latência oscila, o que causa engasgos em jogos online."));
        }
    }
    if let (Some(l), Some(best)) = (loaded_ms, net.iter().map(|p| p.avg_ms).reduce(f64::min)) {
        if l - best > 100.0 {
            warn.push(format!(
                "A latência sobe para {l:.0} ms quando a conexão está cheia (bufferbloat). Ative SQM/QoS no roteador ou limite a velocidade a ~90%."
            ));
        }
    }
    if let Some(d) = dns_ms.filter(|d| *d > 150.0) {
        warn.push(format!("O DNS demorou {d:.0} ms. Trocar para 1.1.1.1 ou 8.8.8.8 pode acelerar o carregamento de sites."));
    }
    if let Some(d) = down.filter(|d| *d < 10.0) {
        warn.push(format!("Download baixo ({} Mbps) para jogos, streaming e atualizações.", fmt1(d)));
    }
    if speed_failed && down.is_none() {
        warn.push("Não foi possível medir a velocidade (speed.cloudflare.com inacessível ou bloqueado).".into());
    }
    if let Some((n, counts)) = link_note {
        warn.push(n.to_string());
        if !counts {
            harmless += 1;
        }
    }

    let (verdict, label) = if bad {
        ("bad", "RUIM")
    } else if warn.len() > harmless {
        ("warn", "ATENÇÃO")
    } else {
        ("good", "ESTÁVEL")
    };
    // estável: os avisos que sobraram são só informativos (roteador mudo, cabo de 100 Mbps sem efeito)
    if verdict == "good" {
        warn.insert(0, "Latência baixa, sem perda de pacotes e sem sinais de bufferbloat. Se o jogo ainda engasga, o problema provavelmente não é a conexão: veja o Gargalo e as temperaturas.".into());
    }
    (verdict, label, warn)
}

/// 1 casa decimal com vírgula, sem ",0" no fim ("0,5", "3").
fn fmt1(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").unwrap_or(&s).replace('.', ",")
}

/// Roda o diagnóstico inteiro (~30 s), mandando o andamento para `emit`. Para assim que `cancel` liga.
pub fn run_test(cancel: &AtomicBool, emit: &(dyn Fn(TestEvent) + Sync)) -> Result<NetReport, String> {
    let stage = |text: &str, frac: f64| emit(TestEvent::Stage { text: text.into(), frac });
    let cancelled = || cancel.load(Ordering::Relaxed);
    stage("Procurando a conexão ativa…", 0.02);
    let ov = overview();
    let Some(gw) = ov.gateway.clone() else {
        return Ok(NetReport {
            verdict: "bad",
            label: "SEM REDE",
            reasons: vec!["Nenhuma conexão ativa com acesso ao roteador. Verifique o cabo, o Wi-Fi ou o adaptador.".into()],
            pings: Vec::new(),
            dns: Vec::new(),
            download_mbps: None,
            upload_mbps: None,
            loaded_ms: None,
            speed_error: None,
            adapter: None,
            link: None,
            ip: None,
            gateway: None,
        });
    };
    let nic = ov.interfaces.iter().find(|i| i.name == gw.iface);
    let ping = tool("ping").ok_or("o comando ping não está instalado (pacote iputils)")?;

    // 1) latência / jitter / perda: roteador e dois DNS públicos, em paralelo (30 pacotes cada)
    stage("Medindo latência e perda de pacotes (≈ 7 s)…", 0.08);
    let targets = [("Roteador", gw.address.clone()), ("Internet · Cloudflare", "1.1.1.1".into()), ("Internet · Google", "8.8.8.8".into())];
    emit(TestEvent::Targets { names: targets.iter().map(|t| t.0.to_string()).collect(), addresses: targets.iter().map(|t| t.1.clone()).collect() });
    let pings: Vec<PingStat> = std::thread::scope(|sc| {
        let hs: Vec<_> = targets
            .iter()
            .enumerate()
            .map(|(i, (name, addr))| {
                let ping = &ping;
                sc.spawn(move || {
                    let (rtts, sent) = ping_loop(ping, addr, 30, cancel, None, &|seq, ms| emit(TestEvent::Ping { target: i, seq, ms }));
                    ping_stat(name, addr, &rtts, sent)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    if cancelled() {
        return Err("cancelado".into());
    }

    // 2) DNS: os seus servidores contra Cloudflare e Google (3 consultas cada, vale a mediana)
    stage("Testando o DNS…", 0.30);
    let mut servers: Vec<(String, bool)> = ov.dns.iter().map(|s| (s.clone(), true)).collect();
    for p in ["1.1.1.1", "8.8.8.8"] {
        if !servers.iter().any(|(s, _)| s == p) {
            servers.push((p.into(), false));
        }
    }
    let mut dns = Vec::new();
    for (k, (server, yours)) in servers.iter().take(5).enumerate() {
        let Ok(ip) = server.parse::<IpAddr>() else { continue };
        let mut times: Vec<f64> = (0..3).filter_map(|n| dns_time(ip, "www.cloudflare.com", 0x5200 + (k * 3 + n) as u16)).collect();
        let r = DnsResult { name: dns_label(server, Some(&gw.address)), server: server.clone(), yours: *yours, ms: median(&mut times) };
        emit(TestEvent::Dns { result: r.clone() });
        dns.push(r);
        if cancelled() {
            return Err("cancelado".into());
        }
    }
    let dns_ms = dns.iter().filter(|d| d.yours).find_map(|d| d.ms);

    // 3) download com ping em paralelo (latência sob carga) e 4) upload
    let curl = tool("curl");
    let (mut down, mut up, mut loaded_ms, mut speed_error) = (None, None, None, None);
    if let Some(curl) = curl.as_deref() {
        stage("Medindo a velocidade de download (≈ 8 s)…", 0.36);
        let stop = AtomicBool::new(false);
        let (res, loaded) = std::thread::scope(|sc| {
            let lp = sc.spawn(|| ping_loop(&ping, "1.1.1.1", 60, cancel, Some(&stop), &|seq, ms| emit(TestEvent::Loaded { seq, ms })));
            let r = download(curl, 8, 4, cancel, &|t, mbps, avg| {
                emit(TestEvent::Speed { dir: "down", t, mbps, avg });
                emit(TestEvent::Stage { text: "Medindo a velocidade de download (≈ 8 s)…".into(), frac: 0.36 + 0.30 * (t / 8.0).min(1.0) });
            });
            stop.store(true, Ordering::Relaxed);
            (r, lp.join().unwrap())
        });
        if cancelled() {
            return Err("cancelado".into());
        }
        match res {
            Ok(v) => down = Some(v),
            Err(e) => speed_error = Some(e),
        }
        loaded_ms = (loaded.0.len() >= 5).then(|| loaded.0.iter().sum::<f64>() / loaded.0.len() as f64);

        stage("Medindo a velocidade de upload (≈ 6 s)…", 0.66);
        match upload(curl, 6, 3, cancel, &|t, mbps, avg| {
            emit(TestEvent::Speed { dir: "up", t, mbps, avg });
            emit(TestEvent::Stage { text: "Medindo a velocidade de upload (≈ 6 s)…".into(), frac: 0.66 + 0.28 * (t / 6.0).min(1.0) });
        }) {
            Ok(v) => up = Some(v),
            Err(e) => {
                if cancelled() {
                    return Err("cancelado".into());
                }
                speed_error.get_or_insert(e);
            }
        }
    } else {
        speed_error = Some("o curl não está instalado: a velocidade não foi medida".into());
    }

    stage("Analisando…", 0.95);
    // o aviso do cabo só pesa se o download bateu no teto do link; o do Wi-Fi fraco pesa sempre
    let link_note = nic.and_then(|n| {
        let note = n.note.as_deref()?;
        let capped = matches!((n.speed_mbps, down), (Some(s), Some(d)) if d >= 0.8 * s as f64);
        Some(if n.kind == "ethernet" && capped {
            (format!("{note} O download medido ({} Mbps) está batendo nesse teto.", fmt1(down.unwrap_or(0.0))), true)
        } else {
            (note.to_string(), n.kind == "wifi")
        })
    });
    let (verdict, label, reasons) = analyze(&pings, dns_ms, loaded_ms, down, speed_error.is_some(), link_note.as_ref().map(|(n, c)| (n.as_str(), *c)));
    let link = nic.map(|n| match (&n.wifi, n.speed_mbps) {
        (Some(w), _) => {
            let parts: Vec<String> = [w.ssid.clone(), w.band.clone(), w.signal_dbm.map(|d| format!("{d} dBm")), w.tx_mbps.map(|t| format!("{t:.0} Mbps"))].into_iter().flatten().collect();
            parts.join(" · ")
        }
        (None, Some(s)) => format!("{s} Mbps"),
        _ => "—".into(),
    });
    stage("Concluído", 1.0);
    Ok(NetReport {
        verdict,
        label,
        reasons,
        pings,
        dns,
        download_mbps: down,
        upload_mbps: up,
        loaded_ms,
        speed_error,
        adapter: nic.map(|n| format!("{} · {}", n.name, n.kind_label)),
        link,
        ip: nic.and_then(|n| n.ipv4.first().cloned()),
        gateway: Some(gw.address),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotas() {
        let t = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                 wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                 enp42s0\t00000000\t0102A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                 enp42s0\t0002A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let g = parse_routes(t);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].iface, "enp42s0");
        assert_eq!(g[0].address, "192.168.2.1");
        assert_eq!(g[1].address, "192.168.1.1");
    }

    #[test]
    fn resolv() {
        let t = "# gerado\nnameserver 8.8.8.8\nnameserver 8.8.4.4\nnameserver 8.8.8.8\nnameserver lixo\nsearch .\noptions edns0\n";
        assert_eq!(parse_resolv(t), ["8.8.8.8", "8.8.4.4"]);
    }

    #[test]
    fn wifi_conectado() {
        let t = "Connected to aa:bb:cc:dd:ee:ff (on wlan0)\n\tSSID: Casa 5G\n\tfreq: 5180.0\n\tRX: 123 bytes (4 packets)\n\
                 \tsignal: -58 dBm\n\trx bitrate: 1200.9 MBit/s 80MHz HE-MCS 11 HE-NSS 2 HE-GI 0 HE-DCM 0\n\
                 \ttx bitrate: 960.7 MBit/s 80MHz HE-MCS 9 HE-NSS 2 HE-GI 0 HE-DCM 0\n\tbss flags: short-slot-time\n";
        let w = parse_iw_link(t);
        assert!(w.connected);
        assert_eq!(w.ssid.as_deref(), Some("Casa 5G"));
        assert_eq!(w.bssid.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(w.band.as_deref(), Some("5 GHz"));
        assert_eq!(w.signal_dbm, Some(-58));
        assert_eq!(w.signal_pct, Some(53));
        assert_eq!(w.rx_mbps, Some(1200.9));
        assert_eq!(w.tx_mbps, Some(960.7));
        assert_eq!(w.standard.as_deref(), Some("Wi-Fi 6"));
        let d = parse_iw_link("Not connected.\n");
        assert!(!d.connected && d.ssid.is_none());
        let n = parse_iw_link("Connected to x (on wlan0)\n\tfreq: 2437\n\ttx bitrate: 144.4 MBit/s MCS 15 short GI\n");
        assert_eq!((n.band.as_deref(), n.standard.as_deref()), (Some("2,4 GHz"), Some("Wi-Fi 4")));
    }

    #[test]
    fn proc_wireless() {
        let t = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\n wlan0: 0000   54.  -56.  -256        0      0      0      0     12        0\n";
        assert_eq!(parse_proc_wireless(t, "wlan0"), Some(-56));
        assert_eq!(parse_proc_wireless(t, "wlan1"), None);
    }

    const ETHTOOL: &str = "Settings for enp42s0:\n\tSupported ports: [ TP\t MII ]\n\tSupported link modes:   10baseT/Half 10baseT/Full\n\
        \t                        100baseT/Half 100baseT/Full\n\t                        1000baseT/Full\n\
        \tSupports auto-negotiation: Yes\n\tLink partner advertised link modes:  10baseT/Half 10baseT/Full\n\
        \t                                     100baseT/Half 100baseT/Full\n\tLink partner advertised pause frame use: No\n\
        \tSpeed: 100Mb/s\n\tDuplex: Full\n\tAuto-negotiation: on\n\tLink detected: yes\n";

    #[test]
    fn ethtool_e_aviso_do_cabo() {
        let e = parse_ethtool(ETHTOOL);
        assert_eq!(e.nic_max_mbps, Some(1000));
        assert_eq!(e.partner_max_mbps, Some(100));
        assert_eq!(e.duplex.as_deref(), Some("completo"));
        assert_eq!(e.autoneg, Some(true));
        assert!(eth_note(Some(100), &e).unwrap().contains("outro lado"));
        let cabo = EthLink { nic_max_mbps: Some(1000), partner_max_mbps: Some(1000), ..Default::default() };
        assert!(eth_note(Some(100), &cabo).unwrap().contains("cabo ruim"));
        assert_eq!(eth_note(Some(1000), &cabo), None);
        assert_eq!(parse_ethtool("Settings for x:\n\tSupported link modes:   Not reported\n").nic_max_mbps, None);
    }

    #[test]
    fn pci_ids() {
        let t = "# comentário\n10ec  Realtek Semiconductor Co., Ltd.\n\t8139  RTL-8100/8101L/8139 PCI Fast Ethernet Adapter\n\
                 \t8168  RTL8111/8168/8211/8411 PCI Express Gigabit Ethernet Controller\n\t\t1462 7c95  MS-7C95\n\
                 14c3  MEDIATEK Corp.\n\t7961  MT7921 802.11ax PCI Express Wireless Network Adapter\n";
        let (v, d) = pci_lookup(t, 0x10ec, 0x8168);
        assert_eq!(short_vendor(&v.unwrap()), "Realtek");
        assert_eq!(d.as_deref(), Some("RTL8111/8168/8211/8411 PCI Express Gigabit Ethernet Controller"));
        let (v, d) = pci_lookup(t, 0x14c3, 0x7961);
        assert_eq!(short_vendor(&v.unwrap()), "Mediatek");
        assert!(d.unwrap().starts_with("MT7921"));
        assert_eq!(pci_lookup(t, 0x10ec, 0x9999).1, None);
    }

    #[test]
    fn linhas_do_ping() {
        assert_eq!(parse_ping_line("64 bytes from 1.1.1.1: icmp_seq=12 ttl=53 time=45.5 ms"), Some(PingLine::Reply { seq: 12, ms: 45.5 }));
        assert_eq!(parse_ping_line("64 bytes from 192.168.2.1: icmp_seq=1 ttl=64 time=0.412 ms"), Some(PingLine::Reply { seq: 1, ms: 0.412 }));
        assert_eq!(parse_ping_line("30 packets transmitted, 29 received, 3.33333% packet loss, time 5803ms"), Some(PingLine::Summary { sent: 30, received: 29 }));
        assert_eq!(parse_ping_line("5 packets transmitted, 0 received, +5 errors, 100% packet loss"), Some(PingLine::Summary { sent: 5, received: 0 }));
        assert_eq!(parse_ping_line("From 192.168.2.1 icmp_seq=3 Destination Host Unreachable"), None);
        assert_eq!(parse_ping_line("PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data."), None);
    }

    #[test]
    fn estatistica_do_ping() {
        let s = ping_stat("X", "1.1.1.1", &[10.0, 14.0, 12.0], 4);
        assert_eq!((s.sent, s.lost), (4, 1));
        assert_eq!(s.loss_pct, 25.0);
        assert_eq!((s.min_ms, s.max_ms, s.avg_ms), (10.0, 14.0, 12.0));
        assert_eq!(s.jitter_ms, 3.0);
        let n = ping_stat("X", "y", &[], 30);
        assert!(!n.answered && n.loss_pct == 100.0);
    }

    #[test]
    fn consulta_dns() {
        let q = dns_query(0xBEEF, "www.cloudflare.com");
        assert_eq!(&q[..4], &[0xBE, 0xEF, 0x01, 0x00]);
        assert_eq!(&q[12..16], &[3, b'w', b'w', b'w']);
        assert_eq!(&q[q.len() - 5..], &[0, 0, 1, 0, 1]);
        assert_eq!(q.len(), 12 + 1 + 3 + 1 + 10 + 1 + 3 + 1 + 4);
        let mut r = q.clone();
        r[2] |= 0x80;
        assert!(dns_answer_ok(0xBEEF, &r));
        assert!(!dns_answer_ok(0xBEEE, &r));
        r[3] |= 3; // NXDOMAIN
        assert!(!dns_answer_ok(0xBEEF, &r));
        assert!(!dns_answer_ok(0xBEEF, &q), "consulta não é resposta");
    }

    fn st(avg: f64, loss: f64, jit: f64) -> PingStat {
        PingStat { name: "x".into(), address: "y".into(), sent: 30, lost: 0, loss_pct: loss, min_ms: avg, avg_ms: avg, max_ms: avg, jitter_ms: jit, answered: true }
    }

    #[test]
    fn veredito() {
        let ok = [st(1.0, 0.0, 0.2), st(12.0, 0.0, 1.0), st(14.0, 0.0, 1.5)];
        let (v, l, r) = analyze(&ok, Some(20.0), Some(30.0), Some(300.0), false, None);
        assert_eq!((v, l), ("good", "ESTÁVEL"));
        assert!(r[0].starts_with("Latência baixa"));

        let mut silent = ok.clone();
        silent[0] = ping_stat("Roteador", "192.168.0.1", &[], 30);
        assert_eq!(analyze(&silent, None, None, Some(300.0), false, None).0, "good", "roteador mudo não piora");

        let bloat = analyze(&ok, None, Some(180.0), Some(300.0), false, None);
        assert_eq!(bloat.0, "warn");
        assert!(bloat.2[0].contains("bufferbloat"));

        let lossy = [st(1.0, 0.0, 0.2), st(40.0, 8.0, 5.0), st(42.0, 6.0, 5.0)];
        let (v, _, r) = analyze(&lossy, None, None, None, true, None);
        assert_eq!(v, "bad");
        assert!(r.iter().any(|x| x.contains("perda de pacotes até a internet (7%)") && x.contains("provedor")));
        assert!(r.iter().any(|x| x.contains("speed.cloudflare.com")));

        let offline = [st(1.0, 0.0, 0.2), ping_stat("a", "b", &[], 30), ping_stat("a", "b", &[], 30)];
        assert_eq!(analyze(&offline, None, None, None, false, None).1, "RUIM");

        let (v, _, r) = analyze(&ok, None, None, Some(90.0), false, Some(("cabo", true)));
        assert_eq!((v, r.as_slice()), ("warn", ["cabo".to_string()].as_slice()));
        assert_eq!(analyze(&ok, None, None, Some(40.0), false, Some(("cabo", false))).0, "good", "aviso informativo");
    }

    #[test]
    fn numeros() {
        assert_eq!(fmt1(0.5), "0,5");
        assert_eq!(fmt1(3.0), "3");
        assert_eq!(mbps(12_500_000, 1.0), 100.0);
        assert_eq!(signal_pct(-30), 100);
        assert_eq!(signal_pct(-95), 0);
    }
}
