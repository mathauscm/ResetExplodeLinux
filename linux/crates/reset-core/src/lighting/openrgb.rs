//! Cliente do servidor SDK do OpenRGB (localhost:6742), portado do `OpenRgbBackend` do app do Windows.
//! No Linux o OpenRGB cobre a placa-mãe MSI (Mystic Light), periféricos Razer, memórias RGB e muitos outros.
//! Protocolo binário little-endian; negociamos a versão 3.

use super::{LightDevice, Zone};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const REQ_CONTROLLER_COUNT: u32 = 0;
const REQ_CONTROLLER_DATA: u32 = 1;
const REQ_PROTOCOL_VERSION: u32 = 40;
const SET_CLIENT_NAME: u32 = 50;
const RESIZE_ZONE: u32 = 1000;
const UPDATE_LEDS: u32 = 1050;
const SET_CUSTOM_MODE: u32 = 1100;
const UPDATE_MODE: u32 = 1101;

pub const DEFAULT_PORT: u16 = 6742;

const FLAG_HAS_BRIGHTNESS: u32 = 1 << 4;
const FLAG_PER_LED_COLOR: u32 = 1 << 5;

struct Controller {
    index: u32,
    active_mode: i32,
    /// Bytes do modo ativo no momento da conexão, para devolver o dispositivo como estava.
    mode_bytes: Option<Vec<u8>>,
    /// Cores dos LEDs no momento da conexão, quando o modo de antes era de cor por LED (o "Static" da MSI):
    /// devolver só o modo deixaria as nossas cores nele.
    original_colors: Option<Vec<[u8; 3]>>,
    leds: usize,
}

pub struct OpenRgb {
    stream: TcpStream,
    proto: u32,
    controllers: HashMap<String, Controller>,
}

impl OpenRgb {
    pub fn connect(host: &str, port: u16) -> Result<(Self, Vec<LightDevice>), String> {
        let addr = (host, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next())
            .ok_or_else(|| format!("endereço inválido: {host}:{port}"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|_| format!("o OpenRGB não está com o servidor SDK ligado em {host}:{port}."))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
        let mut c = OpenRgb { stream, proto: 3, controllers: HashMap::new() };
        let devices = c.handshake().map_err(|e| format!("falha na conversa com o OpenRGB: {e}"))?;
        Ok((c, devices))
    }

    pub fn protocol(&self) -> u32 {
        self.proto
    }

    fn handshake(&mut self) -> std::io::Result<Vec<LightDevice>> {
        self.write(0, SET_CLIENT_NAME, b"RESET EXPLODE\0")?;
        self.write(0, REQ_PROTOCOL_VERSION, &3u32.to_le_bytes())?;
        let server = u32_at(&self.read_type(REQ_PROTOCOL_VERSION)?, 0);
        self.proto = server.min(3);
        if self.proto < 3 {
            return Err(io_err(&format!("protocolo {} do OpenRGB é antigo. Atualize para a versão 0.9 ou mais nova.", self.proto)));
        }

        self.list()
    }

    /// Lê (de novo) a lista de dispositivos. O modo guardado para devolver no fim é sempre o da primeira
    /// leitura: numa releitura o dispositivo já está no nosso modo Direct/Static.
    pub fn list(&mut self) -> std::io::Result<Vec<LightDevice>> {
        self.write(0, REQ_CONTROLLER_COUNT, &[])?;
        let count = u32_at(&self.read_type(REQ_CONTROLLER_COUNT)?, 0);
        let mut list = Vec::new();
        for i in 0..count {
            self.write(i, REQ_CONTROLLER_DATA, &self.proto.to_le_bytes())?;
            let data = self.read_type(REQ_CONTROLLER_DATA)?;
            let p = parse_controller(&data, self.proto)?;
            // controlador ARGB com todos os conectores em 0 LEDs ainda aparece, para dar para configurar
            if p.leds == 0 && !p.zones.iter().any(|z| z.resizable) {
                continue;
            }
            let id = format!("{i}:{}", p.name);
            let c = self.controllers.entry(id.clone()).or_insert(Controller {
                index: i,
                active_mode: p.active_mode,
                mode_bytes: p.mode_bytes,
                original_colors: p.active_per_led.then(|| p.colors.clone()),
                leds: p.leds,
            });
            c.leds = p.leds;
            let vendor = if p.vendor.trim().is_empty() { "—".to_string() } else { p.vendor.clone() };
            list.push(LightDevice {
                id,
                name: p.name,
                detail: format!("OpenRGB · {vendor} · {} LEDs", p.leds),
                kind: p.kind,
                zones: p.zones,
                led_x: (0..p.leds).map(|k| if p.leds > 1 { k as f64 / (p.leds - 1) as f64 } else { 0.0 }).collect(),
            });
        }
        Ok(list)
    }

    /// Muda quantos LEDs há num conector endereçável (JRAINBOW, ARGB). Em 0, o que estiver ligado nele fica apagado.
    pub fn resize_zone(&mut self, id: &str, zone: u32, size: u32) -> Result<(), String> {
        let idx = self.index(id)?;
        let mut p = Vec::with_capacity(8);
        p.extend_from_slice(&(zone as i32).to_le_bytes());
        p.extend_from_slice(&(size as i32).to_le_bytes());
        self.write(idx, RESIZE_ZONE, &p).map_err(|e| e.to_string())
    }

    /// Coloca o dispositivo no modo de cor por LED que o próprio OpenRGB escolhe (Direct na memória,
    /// Static na placa MSI). Testado no hardware: é o que acende certo.
    pub fn acquire(&mut self, id: &str) -> Result<(), String> {
        let idx = self.index(id)?;
        self.write(idx, SET_CUSTOM_MODE, &[]).map_err(|e| e.to_string())
    }

    /// Põe o brilho do modo ativo no máximo, se o modo tiver brilho e ele estiver abaixo. O brilho que o
    /// usuário escolhe no app já vai nas cores; somado ao do modo, a peça fica mais apagada que o pedido.
    pub fn full_brightness(&mut self, id: &str) -> Result<(), String> {
        let idx = self.index(id)?;
        let p = self
            .write(idx, REQ_CONTROLLER_DATA, &self.proto.to_le_bytes())
            .and_then(|_| self.read_type(REQ_CONTROLLER_DATA))
            .and_then(|d| parse_controller(&d, self.proto))
            .map_err(|e| e.to_string())?;
        let (Some(mut mode), Some((offset, cur, max))) = (p.mode_bytes, p.active_brightness) else { return Ok(()) };
        if cur >= max {
            return Ok(());
        }
        mode[offset..offset + 4].copy_from_slice(&max.to_le_bytes());
        let mut pkt = Vec::with_capacity(8 + mode.len());
        pkt.extend_from_slice(&((8 + mode.len()) as u32).to_le_bytes());
        pkt.extend_from_slice(&p.active_mode.to_le_bytes());
        pkt.extend_from_slice(&mode);
        self.write(idx, UPDATE_MODE, &pkt).map_err(|e| e.to_string())
    }

    pub fn send(&mut self, id: &str, colors: &[[u8; 3]]) -> Result<(), String> {
        let idx = self.index(id)?;
        let n = colors.len().min(u16::MAX as usize);
        let mut p = Vec::with_capacity(6 + 4 * n);
        p.extend_from_slice(&((6 + 4 * n) as u32).to_le_bytes());
        p.extend_from_slice(&(n as u16).to_le_bytes());
        for c in &colors[..n] {
            p.extend_from_slice(&[c[0], c[1], c[2], 0]);
        }
        self.write(idx, UPDATE_LEDS, &p).map_err(|e| e.to_string())
    }

    /// Volta o dispositivo ao modo e às cores de quando conectamos.
    pub fn release(&mut self, id: &str) {
        let Some(c) = self.controllers.get(id) else { return };
        let Some(mode) = &c.mode_bytes else { return };
        let mut p = Vec::with_capacity(8 + mode.len());
        p.extend_from_slice(&((8 + mode.len()) as u32).to_le_bytes());
        p.extend_from_slice(&c.active_mode.to_le_bytes());
        p.extend_from_slice(mode);
        let idx = c.index;
        // se o tamanho de um conector ARGB mudou desde então, completa com a última cor
        let colors = c.original_colors.as_ref().filter(|o| !o.is_empty()).map(|o| {
            let mut v: Vec<[u8; 3]> = o.iter().take(c.leds).copied().collect();
            v.resize(c.leds, *o.last().unwrap());
            v
        });
        let _ = self.write(idx, UPDATE_MODE, &p);
        if let Some(colors) = colors {
            let _ = self.send(id, &colors);
        }
    }

    fn index(&self, id: &str) -> Result<u32, String> {
        self.controllers.get(id).map(|c| c.index).ok_or_else(|| format!("dispositivo desconhecido: {id}"))
    }

    fn write(&mut self, dev: u32, kind: u32, payload: &[u8]) -> std::io::Result<()> {
        let mut h = Vec::with_capacity(16 + payload.len());
        h.extend_from_slice(b"ORGB");
        h.extend_from_slice(&dev.to_le_bytes());
        h.extend_from_slice(&kind.to_le_bytes());
        h.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        h.extend_from_slice(payload);
        self.stream.write_all(&h)?;
        self.stream.flush()
    }

    /// Lê pacotes até chegar o do tipo pedido (ignora avisos assíncronos do servidor).
    fn read_type(&mut self, wanted: u32) -> std::io::Result<Vec<u8>> {
        for _ in 0..20 {
            let mut h = [0u8; 16];
            self.stream.read_exact(&mut h)?;
            if &h[..4] != b"ORGB" {
                return Err(io_err("resposta inválida (não é o protocolo do OpenRGB)"));
            }
            let kind = u32_at(&h, 8);
            let size = u32_at(&h, 12) as usize;
            if size > 4_000_000 {
                return Err(io_err("pacote grande demais"));
            }
            let mut data = vec![0u8; size];
            self.stream.read_exact(&mut data)?;
            if kind == wanted {
                return Ok(data);
            }
        }
        Err(io_err("o OpenRGB não respondeu como esperado"))
    }
}

impl Drop for OpenRgb {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

struct Parsed {
    name: String,
    vendor: String,
    kind: String,
    zones: Vec<Zone>,
    leds: usize,
    colors: Vec<[u8; 3]>,
    active_per_led: bool,
    /// Brilho do modo ativo: (posição do campo dentro de `mode_bytes`, atual, máximo).
    active_brightness: Option<(usize, u32, u32)>,
    active_mode: i32,
    mode_bytes: Option<Vec<u8>>,
}

fn parse_controller(data: &[u8], proto: u32) -> std::io::Result<Parsed> {
    let mut r = Cursor::new(data);
    r.u32()?; // tamanho
    let kind = device_type(r.i32()?);
    let name = r.str()?;
    let vendor = if proto >= 1 { r.str()? } else { String::new() };
    r.str()?; // descrição
    r.str()?; // versão
    r.str()?; // serial
    r.str()?; // local
    let num_modes = r.u16()?;
    let active_mode = r.i32()?;
    let mut mode_bytes = None;
    let mut active_per_led = false;
    let mut active_brightness = None;
    for m in 0..num_modes as i32 {
        let start = r.position() as usize;
        r.str()?; // nome
        r.skip(4)?; // valor
        let flags = r.u32()?;
        r.skip(4 * 2)?; // velocidade mín/máx
        let bright_max = if proto >= 3 {
            r.skip(4)?; // brilho mín
            Some(r.u32()?)
        } else {
            None
        };
        r.skip(4 * 2)?; // cores mín/máx
        r.skip(4)?; // velocidade
        let bright_at = r.position() as usize - start;
        let bright = if proto >= 3 { Some(r.u32()?) } else { None };
        r.skip(4 * 2)?; // direção, modo de cor
        let nc = r.u16()? as usize;
        r.skip(nc * 4)?;
        if m == active_mode {
            mode_bytes = Some(data[start..r.position() as usize].to_vec());
            active_per_led = flags & FLAG_PER_LED_COLOR != 0;
            if let (Some(cur), Some(max), true) = (bright, bright_max, flags & FLAG_HAS_BRIGHTNESS != 0) {
                active_brightness = Some((bright_at, cur, max));
            }
        }
    }
    let num_zones = r.u16()?;
    let mut zones = Vec::new();
    for _ in 0..num_zones {
        let name = r.str()?;
        r.skip(4)?; // tipo (único, linear, matriz)
        let (min, max, leds) = (r.u32()?, r.u32()?, r.u32()?);
        zones.push(Zone { name, leds, min, max, resizable: min != max, order: Default::default() });
        let matrix = r.u16()? as usize;
        r.skip(matrix)?;
    }
    let leds = r.u16()? as usize;
    for _ in 0..leds {
        r.str()?; // nome do LED
        r.skip(4)?; // valor
    }
    let nc = r.u16()? as usize;
    let mut colors = Vec::with_capacity(nc);
    for _ in 0..nc {
        let c = r.u32()?.to_le_bytes();
        colors.push([c[0], c[1], c[2]]);
    }
    Ok(Parsed { name, vendor, kind, zones, leds, colors, active_per_led, active_brightness, active_mode, mode_bytes })
}

/// device_type_t do OpenRGB.
fn device_type(t: i32) -> String {
    let n = match t {
        0 => "Placa-mãe", 1 => "Memória", 2 => "Placa de vídeo", 3 => "Cooler", 4 => "Fita de LED",
        5 => "Teclado", 6 => "Mouse", 7 => "Mousepad", 8 => "Headset", 9 => "Suporte de headset",
        10 => "Gamepad", 11 => "Luz", 12 => "Caixa de som", 13 => "Virtual", 14 => "Armazenamento",
        15 => "Gabinete", 16 => "Microfone", 17 => "Acessório", 18 => "Teclado numérico", 19 => "Notebook",
        20 => "Monitor",
        _ => "Dispositivo",
    };
    n.to_string()
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}

fn io_err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

trait ReadExt {
    fn u16(&mut self) -> std::io::Result<u16>;
    fn u32(&mut self) -> std::io::Result<u32>;
    fn i32(&mut self) -> std::io::Result<i32>;
    fn skip(&mut self, n: usize) -> std::io::Result<()>;
    fn str(&mut self) -> std::io::Result<String>;
}

impl ReadExt for Cursor<&[u8]> {
    fn u16(&mut self) -> std::io::Result<u16> {
        let mut b = [0u8; 2];
        self.read_exact(&mut b)?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn i32(&mut self) -> std::io::Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn skip(&mut self, n: usize) -> std::io::Result<()> {
        let p = self.position() as usize + n;
        if p > self.get_ref().len() {
            return Err(io_err("dados do controlador truncados"));
        }
        self.set_position(p as u64);
        Ok(())
    }
    /// Texto com tamanho u16 na frente e \0 no fim.
    fn str(&mut self) -> std::io::Result<String> {
        let n = self.u16()? as usize;
        let start = self.position() as usize;
        self.skip(n)?;
        let b = &self.get_ref()[start..start + n];
        let end = b.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(out: &mut Vec<u8>, t: &str) {
        out.extend_from_slice(&((t.len() + 1) as u16).to_le_bytes());
        out.extend_from_slice(t.as_bytes());
        out.push(0);
    }

    /// Controlador sintético com 1 modo, 1 zona e 3 LEDs, no formato do protocolo 3.
    #[test]
    fn parse_v3() {
        let mut d = vec![0, 0, 0, 0];
        d.extend_from_slice(&0i32.to_le_bytes()); // placa-mãe
        for t in ["B550M PRO-VDH WIFI", "MSI", "desc", "v1", "", "HID"] {
            s(&mut d, t);
        }
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 modo
        d.extend_from_slice(&0i32.to_le_bytes()); // ativo = 0
        let mode_start = d.len();
        s(&mut d, "Static");
        d.extend_from_slice(&0i32.to_le_bytes()); // valor
        d.extend_from_slice(&FLAG_PER_LED_COLOR.to_le_bytes());
        d.extend_from_slice(&[0u8; 4 * 2 + 4 * 2 + 4 * 2 + 4 + 4 + 4 * 2]);
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&[255, 0, 0, 0]);
        let mode_end = d.len();
        d.extend_from_slice(&1u16.to_le_bytes()); // 1 zona
        s(&mut d, "JRGB1");
        for v in [0u32, 1, 1, 1] {
            d.extend_from_slice(&v.to_le_bytes()); // tipo, mín, máx, atual
        }
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&3u16.to_le_bytes()); // 3 LEDs
        for n in ["LED 1", "LED 2", "LED 3"] {
            s(&mut d, n);
            d.extend_from_slice(&0u32.to_le_bytes());
        }
        d.extend_from_slice(&3u16.to_le_bytes()); // cores atuais: a "cor de antes" do usuário
        for _ in 0..3 {
            d.extend_from_slice(&[220, 138, 221, 0]);
        }

        let p = parse_controller(&d, 3).unwrap();
        check(&p, &d, mode_start, mode_end);

        // conversa completa com um servidor falso: aperto de mão, lista, modo Custom, cores e devolução
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ctrl = d.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            loop {
                let mut h = [0u8; 16];
                if s.read_exact(&mut h).is_err() {
                    break;
                }
                let (dev, kind, len) = (u32_at(&h, 4), u32_at(&h, 8), u32_at(&h, 12) as usize);
                let mut body = vec![0u8; len];
                s.read_exact(&mut body).unwrap();
                let reply = |s: &mut std::net::TcpStream, payload: &[u8]| {
                    let mut out = b"ORGB".to_vec();
                    out.extend_from_slice(&dev.to_le_bytes());
                    out.extend_from_slice(&kind.to_le_bytes());
                    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                    out.extend_from_slice(payload);
                    s.write_all(&out).unwrap();
                };
                match kind {
                    REQ_PROTOCOL_VERSION => reply(&mut s, &4u32.to_le_bytes()), // servidor mais novo: cliente fica no 3
                    REQ_CONTROLLER_COUNT => reply(&mut s, &1u32.to_le_bytes()),
                    REQ_CONTROLLER_DATA => reply(&mut s, &ctrl),
                    _ => {}
                }
                got.push((kind, body));
                // a devolução termina com as cores originais
                if kind == UPDATE_LEDS && got.iter().any(|g| g.0 == UPDATE_MODE) {
                    break;
                }
            }
            got
        });

        let (mut c, devs) = OpenRgb::connect("127.0.0.1", port).unwrap();
        assert_eq!(c.protocol(), 3);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].led_x, vec![0.0, 0.5, 1.0]);
        c.acquire(&devs[0].id).unwrap();
        c.send(&devs[0].id, &[[1, 2, 3], [4, 5, 6], [7, 8, 9]]).unwrap();
        c.release(&devs[0].id);
        let got = server.join().unwrap();
        let kinds: Vec<u32> = got.iter().map(|g| g.0).collect();
        assert_eq!(kinds, [SET_CLIENT_NAME, REQ_PROTOCOL_VERSION, REQ_CONTROLLER_COUNT, REQ_CONTROLLER_DATA, SET_CUSTOM_MODE, UPDATE_LEDS, UPDATE_MODE, UPDATE_LEDS]);
        assert_eq!(got[5].1, [18, 0, 0, 0, 3, 0, 1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0]);
        // devolve exatamente os bytes do modo que estava ativo e depois as cores de antes
        assert_eq!(&got[6].1[8..], &d[mode_start..mode_end]);
        assert_eq!(got[7].1, [18, 0, 0, 0, 3, 0, 220, 138, 221, 0, 220, 138, 221, 0, 220, 138, 221, 0]);
    }

    /// Modo "Direct" da Kingston Fury: tem brilho (80 de 100). O campo apontado tem que ser o do brilho.
    #[test]
    fn brilho_do_modo() {
        let mut d = vec![0, 0, 0, 0];
        d.extend_from_slice(&1i32.to_le_bytes()); // memória
        for t in ["Kingston Fury DDR4 DRAM", "Kingston", "", "", "", ""] {
            s(&mut d, t);
        }
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        let mode_start = d.len();
        s(&mut d, "Direct");
        // valor, flags, vel mín/máx, brilho mín/máx, cores mín/máx, vel, brilho, direção, modo de cor
        for v in [0u32, FLAG_PER_LED_COLOR | FLAG_HAS_BRIGHTNESS, 0, 0, 0, 100, 0, 0, 0, 80, 0, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes()); // sem cores do modo
        d.extend_from_slice(&0u16.to_le_bytes()); // sem zonas
        d.extend_from_slice(&0u16.to_le_bytes()); // sem LEDs
        d.extend_from_slice(&0u16.to_le_bytes()); // sem cores
        let p = parse_controller(&d, 3).unwrap();
        let (at, cur, max) = p.active_brightness.unwrap();
        assert_eq!((cur, max), (80, 100));
        assert_eq!(u32_at(&d[mode_start..], at), 80);
    }

    /// Precisa do OpenRGB rodando e mexe no hardware: `cargo test -p reset-core -- --ignored`.
    #[test]
    #[ignore]
    fn brilho_no_maximo_de_verdade() {
        let (mut c, devs) = OpenRgb::connect("127.0.0.1", DEFAULT_PORT).unwrap();
        for d in &devs {
            c.acquire(&d.id).unwrap();
            c.full_brightness(&d.id).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
        for d in &devs {
            let idx = c.index(&d.id).unwrap();
            c.write(idx, REQ_CONTROLLER_DATA, &3u32.to_le_bytes()).unwrap();
            let p = parse_controller(&c.read_type(REQ_CONTROLLER_DATA).unwrap(), 3).unwrap();
            if let Some((_, cur, max)) = p.active_brightness {
                assert_eq!(cur, max, "{}", d.name);
            }
        }
    }

    fn check(p: &Parsed, d: &[u8], mode_start: usize, mode_end: usize) {
        assert_eq!(p.name, "B550M PRO-VDH WIFI");
        assert_eq!(p.vendor, "MSI");
        assert_eq!(p.kind, "Placa-mãe");
        assert_eq!(p.zones.len(), 1);
        assert_eq!((p.zones[0].name.as_str(), p.zones[0].leds, p.zones[0].resizable), ("JRGB1", 1, false));
        assert_eq!(p.leds, 3);
        assert!(p.active_per_led);
        assert_eq!(p.colors, vec![[220, 138, 221]; 3]);
        assert_eq!(p.mode_bytes.as_deref(), Some(&d[mode_start..mode_end]));
    }
}
