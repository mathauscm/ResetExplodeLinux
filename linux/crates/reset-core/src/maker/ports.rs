//! Portas seriais existentes agora: /dev/ttyUSB* (conversores), /dev/ttyACM* (USB nativo: Arduino Uno/Leonardo,
//! ESP32-S3, Pico), as ttyS* que têm UART de verdade na placa-mãe, ttyAMA* (Raspberry Pi) e rfcomm* (Bluetooth).
//! Para cada uma: chip, placa provável, driver, nome fixo em /dev/serial/by-id, quem está com ela aberta e se
//! o usuário tem permissão. Também acha placas conhecidas conectadas que ficaram sem porta (brltty, módulo...).
//!
//! Tudo é lido de /sys, /dev, /proc e /etc; as funções recebem a raiz para os testes montarem um sistema falso.

use super::{board_guess, chip_for, drivers, is_bootsel};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Port {
    /// "ttyUSB0"
    pub name: String,
    /// "/dev/ttyUSB0"
    pub path: String,
    /// "Conversor USB-serial", "USB nativo (CDC-ACM)", "Serial da placa-mãe", "Bluetooth"...
    pub kind: String,
    /// Nome fixo: link em /dev/serial/by-id (ex.: "usb-1a86_USB_Serial-if00-port0").
    pub by_id: Option<String>,
    /// Link em /dev/serial/by-path (fixo enquanto a placa ficar na mesma porta USB).
    pub by_path: Option<String>,
    /// "COM1" para ttyS0 (o nome que o Windows daria).
    pub com: Option<String>,
    pub vid: Option<String>,
    pub pid: Option<String>,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub chip: Option<String>,
    pub chip_id: Option<String>,
    pub board: Option<String>,
    /// Driver do kernel em uso ("ch341-uart", "cdc_acm", "serial").
    pub driver: Option<String>,
    /// Módulo que traz esse driver ("ch341"); None se for embutido no kernel.
    pub module: Option<String>,
    /// Posição física no barramento USB ("1-2.3") e velocidade ("12 Mbps").
    pub usb: Option<String>,
    pub speed: Option<String>,
    /// Para ttyS: "16550A · E/S 0x3F8 · IRQ 4".
    pub uart: Option<String>,
    pub owner: String,
    pub group: String,
    /// "rw-rw----"
    pub mode: String,
    /// O app (e portanto o usuário) consegue abrir para leitura e escrita.
    pub access: bool,
    pub holders: Vec<Holder>,
}

/// Processo com a porta aberta.
#[derive(Debug, Clone, Serialize)]
pub struct Holder {
    pub pid: u32,
    /// Nome curto (/proc/<pid>/comm): "arduino-ide", "screen", "python3"...
    pub name: String,
    /// Linha de comando resumida.
    pub cmd: String,
    /// É o próprio RESET EXPLODE (monitor serial aberto).
    pub mine: bool,
}

/// Placa ou conversor conhecido conectado por USB, com ou sem porta, e o diagnóstico.
#[derive(Debug, Clone, Serialize)]
pub struct UsbSerial {
    pub usb: String,
    pub vid: String,
    pub pid: String,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub chip: Option<String>,
    pub chip_id: Option<String>,
    pub module: Option<String>,
    pub board: Option<String>,
    pub ports: Vec<String>,
    /// Drivers presos às interfaces USB ("ch341", "cdc_acm", "usbfs" = um programa pegou direto).
    pub iface_drivers: Vec<String>,
    /// "ok", "warn" ou "bad"
    pub state: String,
    pub verdict: String,
    pub advice: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Permission {
    pub user: String,
    /// Grupo dono das portas seriais (uucp no Arch, dialout no Debian/Ubuntu/Fedora).
    pub group: String,
    /// O app já roda com o grupo (vale agora).
    pub active: bool,
    /// O usuário já está no grupo em /etc/group (se `active` é falso, falta sair e entrar de novo).
    pub configured: bool,
    /// Grupos ativos agora (o que `id -Gn` mostraria).
    pub groups: Vec<String>,
    /// A regra udev do RESET MAKER está instalada (dá acesso ao usuário da sessão sem precisar do grupo).
    pub uaccess_rule: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortsReport {
    pub ports: Vec<Port>,
    /// Placas/conversores conhecidos conectados (com o diagnóstico de cada um).
    pub devices: Vec<UsbSerial>,
    pub permission: Permission,
    pub modemmanager: bool,
    pub brltty: bool,
}

/// Quem é o processo atual: o que decide a permissão.
pub struct Ids {
    pub uid: u32,
    pub pid: u32,
    /// Grupos efetivos (o primário incluso).
    pub gids: Vec<u32>,
}

impl Ids {
    pub fn current() -> Ids {
        let uid = unsafe { libc::getuid() };
        let mut gids = vec![unsafe { libc::getegid() }];
        let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        if n > 0 {
            let mut buf = vec![0 as libc::gid_t; n as usize];
            let got = unsafe { libc::getgroups(n, buf.as_mut_ptr()) };
            if got > 0 {
                buf.truncate(got as usize);
                gids.extend(buf);
            }
        }
        gids.sort_unstable();
        gids.dedup();
        Ids { uid, pid: std::process::id(), gids }
    }
}

pub fn scan() -> PortsReport {
    scan_in(Path::new("/"), &Ids::current())
}

pub fn scan_in(root: &Path, ids: &Ids) -> PortsReport {
    let root = &fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let accounts = Accounts::read(root);
    let procs = processes(root, ids.pid);
    let mut ports = list_ports(root, &accounts, &procs);
    ports.sort_by(|a, b| port_rank(a).cmp(&port_rank(b)).then(natural(&a.name).cmp(&natural(&b.name))));
    let brltty = procs.comms.iter().any(|c| c == "brltty");
    let devices = usb_serial_devices(root, &ports, brltty);
    PortsReport {
        permission: permission(root, ids, &accounts, &ports),
        modemmanager: procs.comms.iter().any(|c| c == "ModemManager"),
        brltty,
        ports,
        devices,
    }
}

/// USB primeiro, depois Bluetooth e as da placa-mãe.
fn port_rank(p: &Port) -> u8 {
    if p.vid.is_some() {
        0
    } else if p.name.starts_with("rfcomm") {
        1
    } else {
        2
    }
}

/// "ttyUSB10" depois de "ttyUSB2".
fn natural(name: &str) -> (String, u32) {
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    (name[..name.len() - digits.len()].to_string(), digits.parse().unwrap_or(0))
}

fn is_candidate(name: &str) -> bool {
    ["ttyUSB", "ttyACM", "ttyS", "ttyAMA", "ttyTHS", "ttyXRUSB", "rfcomm"]
        .iter()
        .any(|p| name.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())))
}

fn link_name(p: impl AsRef<Path>) -> Option<String> {
    let t = fs::read_link(p).ok()?;
    Some(t.file_name()?.to_string_lossy().into_owned())
}

fn read(p: impl AsRef<Path>) -> Option<String> {
    crate::util::read_trim(p)
}

fn list_ports(root: &Path, acc: &Accounts, procs: &Procs) -> Vec<Port> {
    let sys = root.join("sys/class/tty");
    let Ok(dir) = fs::read_dir(&sys) else { return Vec::new() };
    let links = serial_links(root);
    dir.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !is_candidate(&name) {
                return None;
            }
            let tty = sys.join(&name);
            let devdir = fs::canonicalize(tty.join("device")).ok()?; // sem "device" = terminal virtual
            let uart_type = read(tty.join("type")).and_then(|t| t.parse::<u32>().ok());
            if name.starts_with("ttyS") && uart_type.unwrap_or(0) == 0 {
                return None; // ttyS reservada pelo kernel, sem chip por trás
            }
            Some(build_port(root, &name, &tty, &devdir, uart_type, acc, procs, &links))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_port(root: &Path, name: &str, tty: &Path, devdir: &Path, uart_type: Option<u32>, acc: &Accounts, procs: &Procs, links: &Links) -> Port {
    // driver de verdade: nas ttyS dos kernels novos o primeiro nível é "port"/"ctrl" (serial-base)
    let mut driver_dir = None;
    let mut usbdir = None;
    let sys_devices = root.join("sys/devices");
    let mut cur = Some(devdir.to_path_buf());
    while let Some(d) = cur {
        if !d.starts_with(&sys_devices) {
            break;
        }
        if driver_dir.is_none() {
            if let Some(drv) = link_name(d.join("driver")).filter(|n| n != "port" && n != "ctrl") {
                driver_dir = Some((d.clone(), drv));
            }
        }
        if usbdir.is_none() && d.join("idVendor").exists() {
            usbdir = Some(d.clone());
        }
        cur = d.parent().map(Path::to_path_buf);
    }
    let driver = driver_dir.as_ref().map(|(_, n)| n.clone());
    let module = driver_dir.as_ref().and_then(|(d, _)| link_name(d.join("driver/module")));

    let (mut vid, mut pid, mut vendor, mut product, mut serial, mut chip, mut chip_id, mut board, mut usb, mut speed) =
        (None, None, None, None, None, None, None, None, None, None);
    if let Some(u) = &usbdir {
        let v = crate::util::read_hex(u.join("idVendor")).unwrap_or(0) as u16;
        let p = crate::util::read_hex(u.join("idProduct")).unwrap_or(0) as u16;
        vid = Some(format!("{v:04x}"));
        pid = Some(format!("{p:04x}"));
        vendor = read(u.join("manufacturer"));
        product = read(u.join("product"));
        serial = read(u.join("serial"));
        if let Some(c) = chip_for(v, p) {
            chip = Some(c.name.to_string());
            chip_id = Some(c.id.to_string());
        }
        board = board_guess(v, p).map(String::from);
        usb = u.file_name().map(|n| n.to_string_lossy().into_owned());
        speed = read(u.join("speed")).map(|s| format!("{s} Mbps"));
    }

    let kind = if name.starts_with("rfcomm") {
        "Bluetooth (RFCOMM)"
    } else if name.starts_with("ttyACM") {
        "USB nativo (CDC-ACM)"
    } else if usbdir.is_some() {
        "Conversor USB-serial"
    } else if name.starts_with("ttyAMA") || name.starts_with("ttyTHS") {
        "UART da placa (GPIO)"
    } else {
        "Serial da placa-mãe"
    }
    .to_string();

    let (com, uart) = if let Some(n) = name.strip_prefix("ttyS").and_then(|n| n.parse::<u32>().ok()) {
        let io = read(tty.join("port")).filter(|p| p != "0x0").map(|p| format!("E/S {}", p.to_uppercase().replace("0X", "0x")));
        let irq = read(tty.join("irq")).filter(|i| i != "0").map(|i| format!("IRQ {i}"));
        let parts: Vec<String> = [Some(uart_name(uart_type.unwrap_or(0)).to_string()), io, irq].into_iter().flatten().collect();
        (Some(format!("COM{}", n + 1)), Some(parts.join(" · ")))
    } else {
        (None, None)
    };

    let path = format!("/dev/{name}");
    let node = root.join("dev").join(name);
    let (owner, group, mode) = match fs::metadata(&node) {
        Ok(m) => (acc.user_name(m.uid()), acc.group_name(m.gid()), mode_string(m.mode())),
        Err(_) => ("?".into(), "?".into(), "—".into()),
    };
    Port {
        access: can_rw(&node),
        holders: procs.holders.get(&path).cloned().unwrap_or_default(),
        by_id: links.by_id.get(name).cloned(),
        by_path: links.by_path.get(name).cloned(),
        name: name.to_string(),
        path,
        kind,
        com,
        vid,
        pid,
        vendor,
        product,
        serial,
        chip,
        chip_id,
        board,
        driver,
        module,
        usb,
        speed,
        uart,
        owner,
        group,
        mode,
    }
}

fn uart_name(t: u32) -> &'static str {
    match t {
        1 => "8250",
        2 => "16450",
        3 => "16550",
        4 => "16550A",
        5 => "Cirrus",
        6 => "16650",
        7 => "16650V2",
        8 => "16750",
        9 => "Startech",
        10 => "16C950/954",
        11 => "16654",
        12 => "16850",
        _ => "UART",
    }
}

fn can_rw(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

/// 0o20660 -> "rw-rw----"
pub fn mode_string(mode: u32) -> String {
    let bits = ['r', 'w', 'x'];
    (0..9).map(|i| if mode & (1 << (8 - i)) != 0 { bits[i % 3] } else { '-' }).collect()
}

// ---------------------------------------------------------------- /dev/serial

#[derive(Default)]
struct Links {
    by_id: HashMap<String, String>,
    by_path: HashMap<String, String>,
}

/// /dev/serial/by-id/<nome> -> ../../ttyUSB0: devolve ttyUSB0 -> nome.
fn serial_links(root: &Path) -> Links {
    let read_dir = |sub: &str| -> HashMap<String, String> {
        let Ok(dir) = fs::read_dir(root.join("dev/serial").join(sub)) else { return HashMap::new() };
        dir.flatten()
            .filter_map(|e| Some((link_name(e.path())?, e.file_name().to_string_lossy().into_owned())))
            .collect()
    };
    Links { by_id: read_dir("by-id"), by_path: read_dir("by-path") }
}

// ---------------------------------------------------------------- processos

#[derive(Default)]
struct Procs {
    /// "/dev/ttyUSB0" -> processos com ela aberta
    holders: HashMap<String, Vec<Holder>>,
    /// nomes de todos os processos (para achar ModemManager e brltty, que rodam como root)
    comms: Vec<String>,
}

/// Uma passada por /proc. Sem root, só dá para ver os descritores dos processos do próprio usuário;
/// os nomes (comm) de todos são públicos.
fn processes(root: &Path, me: u32) -> Procs {
    let mut out = Procs::default();
    let Ok(dir) = fs::read_dir(root.join("proc")) else { return out };
    for e in dir.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let comm = read(e.path().join("comm")).unwrap_or_default();
        let Ok(fds) = fs::read_dir(e.path().join("fd")) else {
            out.comms.push(comm);
            continue;
        };
        let mut seen: Vec<String> = Vec::new();
        for fd in fds.flatten() {
            let Ok(t) = fs::read_link(fd.path()) else { continue };
            let t = t.to_string_lossy();
            if let Some(dev) = t.strip_prefix("/dev/").filter(|d| is_candidate(d)) {
                let path = format!("/dev/{dev}");
                if !seen.contains(&path) {
                    seen.push(path);
                }
            }
        }
        if !seen.is_empty() {
            let raw = fs::read(e.path().join("cmdline")).unwrap_or_default();
            let cmd = short_cmdline(&raw);
            for path in seen {
                out.holders.entry(path).or_default().push(Holder { pid, name: comm.clone(), cmd: cmd.clone(), mine: pid == me });
            }
        }
        out.comms.push(comm);
    }
    out
}

/// "/usr/bin/python3\0-m\0serial.tools.miniterm\0/dev/ttyUSB0\0" -> "python3 -m serial.tools.miniterm /dev/ttyUSB0"
pub fn short_cmdline(raw: &[u8]) -> String {
    let mut parts = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned());
    let Some(first) = parts.next() else { return String::new() };
    let mut s = first.rsplit('/').next().unwrap_or(&first).to_string();
    for p in parts {
        s.push(' ');
        s.push_str(&p);
    }
    if s.chars().count() > 90 {
        s = s.chars().take(89).collect::<String>() + "…";
    }
    s
}

/// O processo `pid` está com `path` aberto agora (confere antes de encerrar alguém).
pub fn holds(pid: u32, path: &str) -> bool {
    let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { return false };
    fds.flatten().any(|fd| fs::read_link(fd.path()).is_ok_and(|t| t.as_os_str() == path))
}

// ---------------------------------------------------------------- usuários e grupos

#[derive(Default)]
pub struct Accounts {
    users: HashMap<u32, (String, u32)>,
    groups: Vec<(String, u32, Vec<String>)>,
}

impl Accounts {
    pub fn read(root: &Path) -> Accounts {
        let passwd = fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
        let group = fs::read_to_string(root.join("etc/group")).unwrap_or_default();
        Accounts::parse(&passwd, &group)
    }

    pub fn parse(passwd: &str, group: &str) -> Accounts {
        let users = passwd
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                Some((f.get(2)?.parse().ok()?, (f[0].to_string(), f.get(3)?.parse().ok()?)))
            })
            .collect();
        let groups = group
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                let members = f.get(3).map(|m| m.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
                Some((f[0].to_string(), f.get(2)?.parse().ok()?, members))
            })
            .collect();
        Accounts { users, groups }
    }

    pub fn user_name(&self, uid: u32) -> String {
        self.users.get(&uid).map(|u| u.0.clone()).unwrap_or_else(|| uid.to_string())
    }

    pub fn group_name(&self, gid: u32) -> String {
        self.groups.iter().find(|g| g.1 == gid).map(|g| g.0.clone()).unwrap_or_else(|| gid.to_string())
    }

    pub fn group_exists(&self, name: &str) -> bool {
        self.groups.iter().any(|g| g.0 == name)
    }

    /// Membro configurado: está na lista do grupo ou é o grupo primário dele.
    pub fn is_member(&self, uid: u32, group: &str) -> bool {
        let Some((user, primary)) = self.users.get(&uid) else { return false };
        self.groups.iter().any(|(n, gid, members)| n == group && (gid == primary || members.contains(user)))
    }

    fn gid_of(&self, group: &str) -> Option<u32> {
        self.groups.iter().find(|g| g.0 == group).map(|g| g.1)
    }
}

/// Grupo das portas seriais nesta distribuição: o dono das portas que existem; senão o que a regra padrão
/// do udev manda (KERNEL=="tty[A-Z]*..." GROUP="uucp" no Arch); senão uucp ou dialout, o que existir.
fn serial_group(root: &Path, acc: &Accounts, ports: &[Port]) -> String {
    if let Some(p) = ports.iter().find(|p| p.group != "root" && p.group != "?") {
        return p.group.clone();
    }
    for dir in ["etc/udev/rules.d", "usr/lib/udev/rules.d", "lib/udev/rules.d"] {
        if let Ok(text) = fs::read_to_string(root.join(dir).join("50-udev-default.rules")) {
            if let Some(g) = udev_tty_group(&text) {
                return g;
            }
        }
    }
    if acc.group_exists("uucp") { "uucp" } else { "dialout" }.to_string()
}

/// Lê o GROUP da linha das portas seriais na regra padrão do udev.
pub fn udev_tty_group(rules: &str) -> Option<String> {
    let line = rules.lines().find(|l| !l.trim_start().starts_with('#') && l.contains("KERNEL==\"tty[A-Z]"))?;
    let rest = &line[line.find("GROUP=\"")? + 7..];
    Some(rest[..rest.find('"')?].to_string())
}

fn permission(root: &Path, ids: &Ids, acc: &Accounts, ports: &[Port]) -> Permission {
    let group = serial_group(root, acc, ports);
    let gid = acc.gid_of(&group);
    let mut groups: Vec<String> = ids.gids.iter().map(|g| acc.group_name(*g)).collect();
    groups.sort();
    Permission {
        user: acc.user_name(ids.uid),
        active: gid.is_some_and(|g| ids.gids.contains(&g)),
        configured: acc.is_member(ids.uid, &group),
        groups,
        uaccess_rule: root.join(drivers::RULE_PATH.trim_start_matches('/')).exists(),
        group,
    }
}

/// Nome de usuário ou grupo seguro para ir na linha de comando do usermod.
pub fn valid_account_name(s: &str) -> bool {
    let mut c = s.chars();
    s.len() <= 32
        && c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
        && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

// ---------------------------------------------------------------- placas conectadas (com ou sem porta)

fn usb_serial_devices(root: &Path, ports: &[Port], brltty: bool) -> Vec<UsbSerial> {
    let base = root.join("sys/bus/usb/devices");
    let Ok(dir) = fs::read_dir(&base) else { return Vec::new() };
    let mut mods: Option<drivers::ModIndex> = None;
    let mut out: Vec<UsbSerial> = dir
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            if id.contains(':') {
                return None; // interface, não o aparelho
            }
            let d: PathBuf = e.path();
            let v = crate::util::read_hex(d.join("idVendor"))? as u16;
            let p = crate::util::read_hex(d.join("idProduct"))? as u16;
            let chip = chip_for(v, p);
            if chip.is_none() && !is_bootsel(v, p) {
                return None;
            }
            let mut iface_drivers: Vec<String> = fs::read_dir(&d)
                .map(|it| {
                    it.flatten()
                        .filter(|i| i.file_name().to_string_lossy().starts_with(&format!("{id}:")))
                        .filter_map(|i| link_name(i.path().join("driver")))
                        .collect()
                })
                .unwrap_or_default();
            iface_drivers.sort();
            iface_drivers.dedup();
            let my_ports: Vec<String> = ports.iter().filter(|x| x.usb.as_deref() == Some(id.as_str())).map(|x| x.path.clone()).collect();
            let module = chip.map(|c| c.module);
            let (state, verdict, advice) = diagnose(v, p, chip.map(|c| c.id), module, &my_ports, &iface_drivers, brltty, || {
                mods.get_or_insert_with(|| drivers::ModIndex::read(root)).status(root, module.unwrap_or(""))
            });
            Some(UsbSerial {
                usb: id,
                vid: format!("{v:04x}"),
                pid: format!("{p:04x}"),
                vendor: read(d.join("manufacturer")),
                product: read(d.join("product")),
                chip: chip.map(|c| c.name.to_string()),
                chip_id: chip.map(|c| c.id.to_string()),
                module: module.map(String::from),
                board: board_guess(v, p).map(String::from),
                ports: my_ports,
                iface_drivers,
                state: state.into(),
                verdict,
                advice,
            })
        })
        .collect();
    out.sort_by(|a, b| a.usb.cmp(&b.usb));
    out
}

#[allow(clippy::too_many_arguments)]
fn diagnose(
    vid: u16,
    pid: u16,
    chip: Option<&str>,
    module: Option<&str>,
    ports: &[String],
    iface: &[String],
    brltty: bool,
    module_status: impl FnOnce() -> drivers::ModStatus,
) -> (&'static str, String, String) {
    if !ports.is_empty() {
        return ("ok", format!("Funcionando em {}", ports.join(", ")), String::new());
    }
    if is_bootsel(vid, pid) {
        return (
            "ok",
            "Em modo BOOTSEL (gravação)".into(),
            "Aparece como pendrive RPI-RP2, não como porta serial: arraste o arquivo .uf2 ou envie pela IDE. Para voltar ao normal, desconecte e conecte sem segurar o BOOTSEL.".into(),
        );
    }
    if vid == 0x0403 && pid == 0x0000 {
        return (
            "bad",
            "Chip FTDI falsificado travado (PID 0000)".into(),
            "Um driver FTDI do Windows zerou o identificador deste chip, e o Linux não o reconhece mais. Dá para regravar o PID 6001 com o ftdi_eeprom (pacote libftdi) ou trocar o conversor.".into(),
        );
    }
    if iface.iter().any(|d| d == "usbfs") {
        return if brltty || chip == Some("ch340") {
            (
                "bad",
                "Capturado pelo brltty (leitor de braille)".into(),
                "O brltty acha que conversores CH340/CP210x são linhas braille e prende a placa, então a porta /dev/ttyUSB some. Corrija em RESET MAKER › Drivers seriais.".into(),
            )
        } else {
            ("warn", "Em uso direto por um programa (sem porta serial)".into(), "Algum programa abriu o USB direto (gravador, depurador ou esptool em modo especial). Feche-o e conecte a placa de novo.".into())
        };
    }
    let module = module.unwrap_or("");
    if iface.iter().any(|d| d == module) {
        return ("warn", format!("Driver {module} carregado, mas sem porta"), "Desconecte e conecte de novo. Se continuar, veja as mensagens do kernel com `journalctl -k -b | tail`.".into());
    }
    let st = module_status();
    if !st.available() {
        return ("bad", format!("O kernel em uso não tem o módulo {module}"), "Kernels personalizados às vezes deixam drivers USB-serial de fora. Instale o kernel padrão da distribuição (linux ou linux-cachyos) ou um que inclua este módulo.".into());
    }
    if !st.loaded {
        return ("warn", format!("Módulo {module} não carregado"), format!("Normalmente o sistema carrega sozinho ao conectar. Carregue em Drivers seriais ou rode `sudo modprobe {module}` e conecte a placa de novo."));
    }
    ("warn", "Conectado, mas sem driver".into(), "Desconecte e conecte de novo, de preferência numa porta USB traseira e sem hub.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Monta um /sys, /dev, /proc e /etc falsos com um CH340, um ESP32-S3 (USB nativo), um CH340 capturado
    /// pelo brltty, uma ttyS real (16550A) e uma ttyS vazia.
    fn fake_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("reset-maker-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&root);
        let w = |p: &str, s: &str| {
            let f = root.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, s).unwrap();
        };
        let dir = |p: &str| fs::create_dir_all(root.join(p)).unwrap();
        let ln = |target: &str, at: &str| {
            let f = root.join(at);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            symlink(target, f).unwrap();
        };
        let abs = |p: &str| root.join(p).to_string_lossy().into_owned();

        // drivers
        dir("sys/bus/usb-serial/drivers/ch341-uart");
        dir("sys/bus/usb/drivers/ch341");
        dir("sys/bus/usb/drivers/cdc_acm");
        dir("sys/bus/usb/drivers/usbfs");
        dir("sys/module/ch341");
        dir("sys/module/cdc_acm");
        ln(&abs("sys/module/ch341"), "sys/bus/usb-serial/drivers/ch341-uart/module");
        ln(&abs("sys/module/cdc_acm"), "sys/bus/usb/drivers/cdc_acm/module");
        dir("sys/bus/serial-base/drivers/port");
        dir("sys/bus/pnp/drivers/serial");

        // CH340 em 1-2
        let ch = "sys/devices/pci0000:00/0000:00:14.0/usb1/1-2";
        w(&format!("{ch}/idVendor"), "1a86\n");
        w(&format!("{ch}/idProduct"), "7523\n");
        w(&format!("{ch}/product"), "USB Serial\n");
        w(&format!("{ch}/speed"), "12\n");
        dir(&format!("{ch}/1-2:1.0/ttyUSB0/tty/ttyUSB0"));
        ln(&abs("sys/bus/usb/drivers/ch341"), &format!("{ch}/1-2:1.0/driver"));
        ln(&abs("sys/bus/usb-serial/drivers/ch341-uart"), &format!("{ch}/1-2:1.0/ttyUSB0/driver"));
        ln(&abs(&format!("{ch}/1-2:1.0/ttyUSB0")), "sys/class/tty/ttyUSB0/device");
        ln(&abs(ch), "sys/bus/usb/devices/1-2");

        // ESP32-S3 em 1-3 (cdc_acm direto na interface)
        let es = "sys/devices/pci0000:00/0000:00:14.0/usb1/1-3";
        w(&format!("{es}/idVendor"), "303a\n");
        w(&format!("{es}/idProduct"), "1001\n");
        w(&format!("{es}/manufacturer"), "Espressif\n");
        w(&format!("{es}/product"), "USB JTAG/serial debug unit\n");
        w(&format!("{es}/serial"), "F4:12:FA:00:11:22\n");
        dir(&format!("{es}/1-3:1.0/tty/ttyACM0"));
        dir(&format!("{es}/1-3:1.2"));
        ln(&abs("sys/bus/usb/drivers/cdc_acm"), &format!("{es}/1-3:1.0/driver"));
        ln(&abs(&format!("{es}/1-3:1.0")), "sys/class/tty/ttyACM0/device");
        ln(&abs(es), "sys/bus/usb/devices/1-3");

        // CH340 capturado em 1-4: interface presa ao usbfs, sem tty
        let cap = "sys/devices/pci0000:00/0000:00:14.0/usb1/1-4";
        w(&format!("{cap}/idVendor"), "1a86\n");
        w(&format!("{cap}/idProduct"), "7523\n");
        dir(&format!("{cap}/1-4:1.0"));
        ln(&abs("sys/bus/usb/drivers/usbfs"), &format!("{cap}/1-4:1.0/driver"));
        ln(&abs(cap), "sys/bus/usb/devices/1-4");

        // mouse qualquer: não aparece
        w("sys/devices/pci0000:00/0000:00:14.0/usb1/1-5/idVendor", "046d\n");
        w("sys/devices/pci0000:00/0000:00:14.0/usb1/1-5/idProduct", "c52b\n");
        ln(&abs("sys/devices/pci0000:00/0000:00:14.0/usb1/1-5"), "sys/bus/usb/devices/1-5");

        // ttyS0 real e ttyS1 vazia
        let s0 = "sys/devices/pnp0/00:00/00:00:0/00:00:0.0";
        dir(s0);
        ln(&abs("sys/bus/serial-base/drivers/port"), &format!("{s0}/driver"));
        ln(&abs("sys/bus/pnp/drivers/serial"), "sys/devices/pnp0/00:00/driver");
        ln(&abs(s0), "sys/class/tty/ttyS0/device");
        w("sys/class/tty/ttyS0/type", "4\n");
        w("sys/class/tty/ttyS0/port", "0x3F8\n");
        w("sys/class/tty/ttyS0/irq", "4\n");
        let s1 = "sys/devices/platform/serial8250/serial8250:0/serial8250:0.1";
        dir(s1);
        ln(&abs(s1), "sys/class/tty/ttyS1/device");
        w("sys/class/tty/ttyS1/type", "0\n");
        dir("sys/class/tty/tty1"); // terminal virtual

        // /dev
        for n in ["ttyUSB0", "ttyACM0", "ttyS0", "ttyS1"] {
            w(&format!("dev/{n}"), "");
        }
        ln("../../ttyUSB0", "dev/serial/by-id/usb-1a86_USB_Serial-if00-port0");
        ln("../../ttyACM0", "dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_F4:12:FA:00:11:22-if00");
        ln("../../ttyUSB0", "dev/serial/by-path/pci-0000:00:14.0-usb-0:2:1.0-port0");

        // /proc: o monitor serial de alguém com a ttyUSB0 aberta, o ModemManager e o brltty
        w("proc/4242/comm", "python3\n");
        w("proc/4242/cmdline", "/usr/bin/python3\0-m\0serial.tools.miniterm\0/dev/ttyUSB0\x00115200\0");
        ln("/dev/ttyUSB0", "proc/4242/fd/3");
        ln("/dev/null", "proc/4242/fd/0");
        w("proc/900/comm", "ModemManager\n");
        w("proc/901/comm", "brltty\n");

        // os arquivos do teste são do usuário que roda o teste: o grupo deles vira o "uucp" do sistema falso
        let gid = fs::metadata(root.join("dev/ttyUSB0")).unwrap().gid();
        w("etc/passwd", "root:x:0:0::/root:/bin/bash\nmaker:x:1000:4242::/home/maker:/bin/bash\n");
        w("etc/group", &format!("root:x:0:\nuucp:x:{gid}:maker\nmaker:x:4242:\nwheel:x:998:maker\n"));
        w("usr/lib/udev/rules.d/50-udev-default.rules", "KERNEL==\"tty[A-Z]*[0-9]|rfcomm[0-9]*\", GROUP=\"uucp\"\n");
        root
    }

    #[test]
    fn varredura_completa() {
        let root = fake_root();
        let r = scan_in(&root, &Ids { uid: 1000, pid: 1, gids: vec![4242, 998] });
        let names: Vec<&str> = r.ports.iter().map(|p| p.name.as_str()).collect();
        // USB primeiro (em ordem), depois a da placa-mãe; ttyS1 sem chip e tty1 virtual ficam de fora
        assert_eq!(names, ["ttyACM0", "ttyUSB0", "ttyS0"]);

        let usb = r.ports.iter().find(|p| p.name == "ttyUSB0").unwrap();
        assert_eq!(usb.chip.as_deref(), Some("WCH CH340/CH341"));
        assert_eq!(usb.driver.as_deref(), Some("ch341-uart"));
        assert_eq!(usb.module.as_deref(), Some("ch341"));
        assert_eq!(usb.usb.as_deref(), Some("1-2"));
        assert_eq!(usb.speed.as_deref(), Some("12 Mbps"));
        assert_eq!(usb.by_id.as_deref(), Some("usb-1a86_USB_Serial-if00-port0"));
        assert!(usb.by_path.is_some());
        assert_eq!(usb.holders.len(), 1);
        assert_eq!(usb.holders[0].name, "python3");
        assert_eq!(usb.holders[0].cmd, "python3 -m serial.tools.miniterm /dev/ttyUSB0 115200");
        assert_eq!(usb.kind, "Conversor USB-serial");

        let acm = r.ports.iter().find(|p| p.name == "ttyACM0").unwrap();
        assert_eq!(acm.board.as_deref(), Some("ESP32-S3/C3/C6 (USB-Serial/JTAG nativo)"));
        assert_eq!(acm.driver.as_deref(), Some("cdc_acm"));
        assert_eq!(acm.serial.as_deref(), Some("F4:12:FA:00:11:22"));
        assert!(acm.holders.is_empty());

        let s0 = r.ports.iter().find(|p| p.name == "ttyS0").unwrap();
        assert_eq!(s0.com.as_deref(), Some("COM1"));
        assert_eq!(s0.uart.as_deref(), Some("16550A · E/S 0x3F8 · IRQ 4"));
        assert_eq!(s0.driver.as_deref(), Some("serial"));
        assert!(s0.vid.is_none());

        assert!(r.modemmanager);
        assert!(r.brltty);
        // grupo do Arch, configurado em /etc/group mas não ativo no processo: falta sair e entrar
        assert_eq!(r.permission.group, "uucp");
        assert!(r.permission.configured);
        assert!(!r.permission.active);
        assert_eq!(r.permission.user, "maker");

        let devs: Vec<(&str, &str)> = r.devices.iter().map(|d| (d.usb.as_str(), d.state.as_str())).collect();
        assert_eq!(devs, [("1-2", "ok"), ("1-3", "ok"), ("1-4", "bad")]);
        assert!(r.devices[2].verdict.contains("brltty"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn contas_e_grupos() {
        let a = Accounts::parse("ana:x:1000:1000::/home/ana:/bin/sh\n", "uucp:x:984:\nana:x:1000:\ndialout:x:20:bob,ana\n");
        assert!(!a.is_member(1000, "uucp"));
        assert!(a.is_member(1000, "dialout"));
        assert!(a.is_member(1000, "ana"));
        assert_eq!(a.group_name(984), "uucp");
        assert_eq!(a.group_name(5), "5");
        assert_eq!(udev_tty_group("# x\nKERNEL==\"tty[A-Z]*[0-9]|pppox[0-9]*\", GROUP=\"dialout\"\n").as_deref(), Some("dialout"));
        assert!(valid_account_name("uucp"));
        assert!(valid_account_name("mathauscm"));
        assert!(!valid_account_name("-G"));
        assert!(!valid_account_name("root;rm"));
        assert!(!valid_account_name(""));
    }

    #[test]
    fn formatos() {
        assert_eq!(mode_string(0o20660), "rw-rw----");
        assert_eq!(mode_string(0o100644), "rw-r--r--");
        assert!(is_candidate("ttyUSB0"));
        assert!(is_candidate("rfcomm1"));
        assert!(!is_candidate("tty1"));
        assert!(!is_candidate("ttyS"));
        assert!(!is_candidate("ttyUSB0x"));
        assert_eq!(short_cmdline(b"/usr/bin/screen\0/dev/ttyACM0\0"), "screen /dev/ttyACM0");
        assert!(natural("ttyUSB2") < natural("ttyUSB10"));
    }

    #[test]
    fn diagnosticos() {
        let mods = || drivers::ModStatus { file: Some("x".into()), builtin: false, loaded: false };
        let none = || drivers::ModStatus { file: None, builtin: false, loaded: false };
        assert_eq!(diagnose(0x0403, 0, Some("ftdi"), Some("ftdi_sio"), &[], &[], false, mods).0, "bad");
        let (s, v, _) = diagnose(0x1a86, 0x7523, Some("ch340"), Some("ch341"), &[], &[], false, mods);
        assert_eq!((s, v.as_str()), ("warn", "Módulo ch341 não carregado"));
        assert_eq!(diagnose(0x1a86, 0x7523, Some("ch340"), Some("ch341"), &[], &[], false, none).0, "bad");
        assert_eq!(diagnose(0x2e8a, 3, None, None, &[], &[], false, none).0, "ok");
    }
}
