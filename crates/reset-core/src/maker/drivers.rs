//! Drivers seriais no Linux: os de CH340, CP210x, FTDI, PL2303 e CDC-ACM vêm no kernel. Aqui fica o que pode
//! dar errado em volta deles: módulo ausente ou não carregado, o brltty capturando conversores (o problema
//! clássico do Arch), o ModemManager mexendo em placas, falta de regra udev e a Arduino IDE.
//!
//! Só leitura. As correções (com root) são montadas aqui como comandos fixos e rodadas pelo app via `pkexec`.

use super::ports::{self, UsbSerial};
use super::CHIPS;
use crate::packages;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Regra udev instalada pelo RESET MAKER. Antes do 73-seat-late (que aplica o `uaccess`) e das regras do
/// ModemManager (77-mm-*), senão as marcas chegam tarde.
pub const RULE_PATH: &str = "/etc/udev/rules.d/70-reset-maker.rules";

/// Fabricantes de placas e conversores seriais que a regra cobre (VID, nome).
const RULE_VENDORS: &[(&str, &str)] = &[
    ("2341", "Arduino"),
    ("2a03", "Arduino (arduino.org)"),
    ("1a86", "WCH CH340/CH341/CH9102"),
    ("10c4", "Silicon Labs CP210x"),
    ("0403", "FTDI"),
    ("067b", "Prolific PL2303"),
    ("303a", "Espressif ESP32-S2/S3/C3/C6"),
    ("2e8a", "Raspberry Pi Pico"),
    ("239a", "Adafruit"),
    ("1b4f", "SparkFun"),
];

/// Conteúdo da regra. `uaccess` dá acesso ao usuário logado na sessão (vale na hora, sem entrar em grupo nem
/// sair da sessão); ID_MM_DEVICE_IGNORE afasta o ModemManager, que "testa" placas CDC-ACM como se fossem
/// modem e atrapalha o upload. Vale para a porta (tty) e para o próprio USB (JTAG do ESP32, picotool, DFU).
pub fn rule_content() -> String {
    let mut s = String::from(
        "# RESET EXPLODE · RESET MAKER: placas Arduino/ESP32/Pico e conversores USB-serial.\n\
         # uaccess: o usuário da sessão acessa a porta sem precisar do grupo uucp/dialout.\n\
         # ID_MM_DEVICE_IGNORE: o ModemManager não mexe nestas placas.\n\
         # Para desfazer, apague este arquivo (ou use o botão na tela Drivers seriais).\n\
         ACTION==\"remove\", GOTO=\"reset_maker_end\"\n\
         SUBSYSTEM!=\"tty|usb\", GOTO=\"reset_maker_end\"\n",
    );
    for (vid, name) in RULE_VENDORS {
        s.push_str(&format!("# {name}\nATTRS{{idVendor}}==\"{vid}\", TAG+=\"uaccess\", ENV{{ID_MM_DEVICE_IGNORE}}=\"1\"\n"));
    }
    s.push_str("# STM32 com USB CDC\nATTRS{idVendor}==\"0483\", ATTRS{idProduct}==\"5740\", TAG+=\"uaccess\", ENV{ID_MM_DEVICE_IGNORE}=\"1\"\n");
    s.push_str("LABEL=\"reset_maker_end\"\n");
    s
}

// ---------------------------------------------------------------- comandos com root (fixos)

pub const SH: &str = "/usr/bin/sh";
pub const MODPROBE: &str = "/usr/bin/modprobe";
pub const USERMOD: &str = "/usr/bin/usermod";

const INSTALL_RULE: &str = "/usr/bin/install -m 0644 -- \"$1\" /etc/udev/rules.d/70-reset-maker.rules \
    && echo 'Regra gravada em /etc/udev/rules.d/70-reset-maker.rules' \
    && /usr/bin/udevadm control --reload-rules \
    && /usr/bin/udevadm trigger --action=change --subsystem-match=tty \
    && echo 'Regras recarregadas. Desconecte e conecte a placa.'";

const REMOVE_RULE: &str = "/usr/bin/rm -f -- /etc/udev/rules.d/70-reset-maker.rules \
    && /usr/bin/udevadm control --reload-rules \
    && /usr/bin/udevadm trigger --action=change --subsystem-match=tty \
    && echo 'Regra removida.'";

/// Desativa regras do brltty sem desinstalar nada: um link para /dev/null em /etc com o mesmo nome
/// tem prioridade sobre o arquivo de /usr/lib (é o jeito padrão de "mascarar" uma regra udev).
const MASK_RULES: &str = "for f in \"$@\"; do /usr/bin/ln -sfn /dev/null \"/etc/udev/rules.d/$f\" || exit 1; \
    echo \"Regra $f desativada\"; done; /usr/bin/udevadm control --reload-rules \
    && echo 'Pronto. Desconecte e conecte o conversor.'";

const UNMASK_RULES: &str = "for f in \"$@\"; do if [ \"$(/usr/bin/readlink \"/etc/udev/rules.d/$f\")\" = /dev/null ]; then \
    /usr/bin/rm -f \"/etc/udev/rules.d/$f\" && echo \"Regra $f reativada\"; fi; done; /usr/bin/udevadm control --reload-rules";

/// Argumentos do `sh` para instalar a regra a partir de um arquivo temporário já escrito pelo app.
pub fn install_rule_args(tmp: &Path) -> Vec<String> {
    vec!["-c".into(), INSTALL_RULE.into(), "sh".into(), tmp.to_string_lossy().into_owned()]
}

pub fn remove_rule_args() -> Vec<String> {
    vec!["-c".into(), REMOVE_RULE.into()]
}

/// Nome de arquivo de regra seguro ("90-brltty-device.rules").
pub fn valid_rule_name(n: &str) -> bool {
    n.ends_with(".rules") && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub fn mask_args(names: &[String], unmask: bool) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Err("nenhuma regra para mudar".into());
    }
    if let Some(bad) = names.iter().find(|n| !valid_rule_name(n)) {
        return Err(format!("nome de regra inválido: {bad}"));
    }
    let mut a = vec!["-c".to_string(), if unmask { UNMASK_RULES } else { MASK_RULES }.to_string(), "sh".to_string()];
    a.extend(names.iter().cloned());
    Ok(a)
}

/// Só os módulos da tabela de chips podem ser carregados pela interface.
pub fn valid_module(m: &str) -> bool {
    CHIPS.iter().any(|c| c.module == m)
}

// ---------------------------------------------------------------- módulos do kernel

#[derive(Debug, Clone, Serialize)]
pub struct ModStatus {
    /// Arquivo do módulo em /usr/lib/modules/<kernel>/ (None se embutido ou ausente).
    pub file: Option<String>,
    pub builtin: bool,
    pub loaded: bool,
}

impl ModStatus {
    pub fn available(&self) -> bool {
        self.file.is_some() || self.builtin
    }
}

/// Índice dos módulos do kernel em uso (modules.dep e modules.builtin), lido uma vez.
pub struct ModIndex {
    pub kernel: String,
    files: HashMap<String, String>,
    builtin: HashSet<String>,
}

/// "kernel/drivers/usb/class/cdc-acm.ko.zst" -> "cdc_acm"
pub fn module_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let base = file.split(".ko").next().unwrap_or(file);
    base.replace('-', "_")
}

impl ModIndex {
    pub fn read(root: &Path) -> ModIndex {
        let kernel = crate::util::read_trim(root.join("proc/sys/kernel/osrelease")).unwrap_or_default();
        let dir = ["usr/lib/modules", "lib/modules"].iter().map(|d| root.join(d).join(&kernel)).find(|d| d.is_dir());
        let (dep, builtin) = match &dir {
            Some(d) => (fs::read_to_string(d.join("modules.dep")).unwrap_or_default(), fs::read_to_string(d.join("modules.builtin")).unwrap_or_default()),
            None => Default::default(),
        };
        ModIndex::parse(kernel, &dep, &builtin)
    }

    pub fn parse(kernel: String, dep: &str, builtin: &str) -> ModIndex {
        let files = dep
            .lines()
            .filter_map(|l| l.split(':').next())
            .filter(|p| p.contains("/usb/") || p.contains("/tty/"))
            .map(|p| (module_name(p), p.to_string()))
            .collect();
        let builtin = builtin.lines().map(module_name).collect();
        ModIndex { kernel, files, builtin }
    }

    pub fn status(&self, root: &Path, module: &str) -> ModStatus {
        let sys = root.join("sys/module").join(module);
        let builtin = self.builtin.contains(module);
        ModStatus {
            file: self.files.get(module).cloned(),
            // módulos carregados têm "initstate"; os embutidos só têm a pasta (e estão sempre ativos)
            loaded: builtin || sys.join("initstate").exists(),
            builtin,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleRow {
    pub module: String,
    /// Chips atendidos por este módulo.
    pub chips: Vec<String>,
    pub status: ModStatus,
    /// Portas usando o módulo agora.
    pub ports: Vec<String>,
}

// ---------------------------------------------------------------- brltty

#[derive(Debug, Clone, Serialize)]
pub struct BrlRule {
    pub name: String,
    pub path: String,
    /// Pacote dono do arquivo (pacman -Qo).
    pub owner: Option<String>,
    /// Desativada por um link para /dev/null em /etc/udev/rules.d.
    pub masked: bool,
    /// Pega o CH340/CH341 (1a86:7523).
    pub catches_ch340: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Brltty {
    pub installed: Option<String>,
    /// No Arch as regras para conversores genéricos ficam no pacote à parte brltty-udev-generic.
    pub generic: Option<String>,
    pub rules: Vec<BrlRule>,
    /// Alguma regra ativa pega conversores CH340: é isso que faz a porta sumir.
    pub active: bool,
    pub running: bool,
    /// Pacote a remover para resolver (o dono da regra que pega o CH340).
    pub remove: Option<String>,
    /// Pacotes que dependem dele (se houver, remover falha: use "desativar a regra").
    pub required_by: Vec<String>,
}

/// A regra do brltty lista o conversor? (as regras usam `ATTRS{idVendor}=="1a86", ATTRS{idProduct}=="7523"`)
pub fn rule_catches_ch340(text: &str) -> bool {
    text.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("1a86") && l.contains("7523")
    })
}

fn brltty_rules(root: &Path) -> Vec<BrlRule> {
    let mut out: Vec<BrlRule> = Vec::new();
    for dir in ["usr/lib/udev/rules.d", "etc/udev/rules.d"] {
        let Ok(it) = fs::read_dir(root.join(dir)) else { continue };
        for e in it.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.contains("brltty") || !name.ends_with(".rules") || out.iter().any(|r| r.name == name) {
                continue;
            }
            let etc = root.join("etc/udev/rules.d").join(&name);
            let masked = fs::read_link(&etc).is_ok_and(|t| t == Path::new("/dev/null"));
            if dir.starts_with("etc") && masked {
                continue; // é a própria máscara: aparece pela regra original
            }
            let text = fs::read_to_string(e.path()).unwrap_or_default();
            out.push(BrlRule { path: format!("/{dir}/{name}"), name, owner: None, masked, catches_ch340: rule_catches_ch340(&text) });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// "/usr/lib/udev/rules.d/90-brltty-device.rules is owned by brltty 6.9.1-3.1" -> (caminho, pacote)
pub fn parse_owners(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (path, rest) = l.split_once(" is owned by ")?;
            Some((path.trim().to_string(), rest.split_whitespace().next()?.to_string()))
        })
        .collect()
}

/// Campo "Required By" do `pacman -Qi` (vazio se "None").
pub fn parse_required_by(text: &str) -> Vec<String> {
    let Some(l) = text.lines().find(|l| l.starts_with("Required By")) else { return Vec::new() };
    let v = l.split_once(':').map(|x| x.1.trim()).unwrap_or("");
    if v == "None" { Vec::new() } else { v.split_whitespace().map(String::from).collect() }
}

fn pacman(args: &[&str]) -> String {
    Command::new("pacman")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn brltty(installed: &HashMap<String, String>, running: bool) -> Brltty {
    let mut rules = brltty_rules(Path::new("/"));
    if !rules.is_empty() {
        let paths: Vec<&str> = rules.iter().map(|r| r.path.as_str()).collect();
        let mut args = vec!["-Qo"];
        args.extend(paths);
        let owners = parse_owners(&pacman(&args));
        for r in &mut rules {
            r.owner = owners.get(&r.path).cloned();
        }
    }
    let catching: Vec<&BrlRule> = rules.iter().filter(|r| r.catches_ch340 && !r.masked).collect();
    let remove = catching.iter().find_map(|r| r.owner.clone());
    let required_by = remove.as_deref().map(|p| parse_required_by(&pacman(&["-Qi", p]))).unwrap_or_default();
    Brltty {
        installed: installed.get("brltty").cloned(),
        generic: installed.get("brltty-udev-generic").cloned(),
        active: !catching.is_empty(),
        rules,
        running,
        remove,
        required_by,
    }
}

// ---------------------------------------------------------------- udev e ModemManager

#[derive(Debug, Clone, Serialize)]
pub struct Udev {
    pub path: String,
    pub content: String,
    pub installed: bool,
    /// Instalada, mas de uma versão anterior do app (conteúdo diferente).
    pub outdated: bool,
    /// Pacote oficial com as regras do PlatformIO (cobre centenas de placas).
    pub platformio: Option<String>,
    pub platformio_repo: Option<String>,
    pub modemmanager: bool,
}

// ---------------------------------------------------------------- Arduino IDE

#[derive(Debug, Clone, Serialize)]
pub struct IdeInstall {
    /// "Pacote", "Flatpak", "AppImage"
    pub kind: String,
    pub name: String,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ide {
    pub installs: Vec<IdeInstall>,
    pub running: bool,
    /// arduino-cli instalado (versão).
    pub cli: Option<String>,
    /// Versões nos repositórios configurados (pacman -Si): nome -> (repositório, versão).
    pub repo: HashMap<String, (String, String)>,
    /// paru/yay, para instalar do AUR pelo terminal.
    pub aur_helper: Option<String>,
    /// Flatpak com o Flathub configurado: "user" ou "system".
    pub flathub: Option<String>,
}

pub const FLATPAK_IDE: &str = "cc.arduino.IDE2";
pub const FLATPAK: &str = "/usr/bin/flatpak";

/// Saída do `pacman -Si a b c`: nome -> (repositório, versão), o primeiro repositório de cada um.
pub fn parse_si(text: &str) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let (mut repo, mut name) = (String::new(), String::new());
    for l in text.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Repository" => repo = v,
            "Name" => name = v,
            "Version" => {
                out.entry(name.clone()).or_insert((repo.clone(), v));
            }
            _ => {}
        }
    }
    out
}

/// `flatpak remotes --columns=name,options`: onde o Flathub está configurado.
pub fn parse_flathub(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.split_whitespace().next() == Some("flathub"))?;
    Some(if line.contains("user") { "user" } else { "system" }.into())
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(Into::into)
}

fn ide(installed: &HashMap<String, String>, comms: &[String]) -> Ide {
    let mut installs = Vec::new();
    for (pkg, label) in [("arduino-ide", "Arduino IDE 2"), ("arduino-ide-bin", "Arduino IDE 2 (AUR)"), ("arduino-ide-appimage", "Arduino IDE 2 (AUR)"), ("arduino", "Arduino IDE 1.8 (antiga)")] {
        if let Some(v) = installed.get(pkg) {
            installs.push(IdeInstall { kind: "Pacote".into(), name: label.into(), version: Some(v.clone()), path: Some(pkg.into()) });
        }
    }
    let mut flat_dirs = vec![std::path::PathBuf::from("/var/lib/flatpak/app")];
    if let Some(h) = home() {
        flat_dirs.push(h.join(".local/share/flatpak/app"));
    }
    for d in &flat_dirs {
        let p = d.join(FLATPAK_IDE);
        if p.is_dir() {
            installs.push(IdeInstall { kind: "Flatpak".into(), name: "Arduino IDE 2".into(), version: None, path: Some(p.to_string_lossy().into_owned()) });
        }
    }
    if let Some(h) = home() {
        for sub in ["Applications", "Aplicativos", "Downloads", "AppImages", ".local/bin", ""] {
            let Ok(it) = fs::read_dir(h.join(sub)) else { continue };
            for e in it.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                let l = n.to_lowercase();
                if l.starts_with("arduino-ide") && l.ends_with(".appimage") {
                    let version = n.split('_').nth(1).map(String::from);
                    installs.push(IdeInstall { kind: "AppImage".into(), name: "Arduino IDE 2".into(), version, path: Some(e.path().to_string_lossy().into_owned()) });
                }
            }
        }
    }
    let flathub = Path::new(FLATPAK).exists().then(|| {
        let out = Command::new(FLATPAK).args(["remotes", "--columns=name,options"]).output();
        parse_flathub(&out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default())
    });
    Ide {
        installs,
        running: comms.iter().any(|c| c == "arduino-ide" || c.starts_with("Arduino IDE")),
        cli: installed.get("arduino-cli").cloned(),
        repo: parse_si(&pacman(&["-Si", "arduino-ide", "arduino-ide-bin", "arduino-cli", "platformio-core-udev"])),
        aur_helper: ["paru", "yay"].iter().find(|h| Path::new("/usr/bin").join(h).exists()).map(|h| h.to_string()),
        flathub: flathub.flatten(),
    }
}

/// Argumentos do flatpak para instalar a IDE (sem root no modo usuário; no modo sistema o próprio flatpak
/// pede a senha pelo polkit).
pub fn flatpak_ide_args(scope: &str) -> Result<Vec<String>, String> {
    let scope = match scope {
        "user" => "--user",
        "system" => "--system",
        _ => return Err("escopo inválido".into()),
    };
    Ok([scope, "install", "-y", "--noninteractive", "flathub", FLATPAK_IDE].iter().map(|s| s.to_string()).collect())
}

// ---------------------------------------------------------------- relatório da tela

#[derive(Debug, Clone, Serialize)]
pub struct DriversReport {
    pub kernel: String,
    pub modules: Vec<ModuleRow>,
    pub devices: Vec<UsbSerial>,
    pub brltty: Brltty,
    pub udev: Udev,
    pub ide: Ide,
    pub pacman_locked: bool,
}

pub fn report() -> DriversReport {
    let root = Path::new("/");
    let scan = ports::scan();
    let comms = process_names();
    let installed = packages::installed_versions();
    let idx = ModIndex::read(root);

    let mut modules: Vec<ModuleRow> = Vec::new();
    for c in CHIPS {
        match modules.iter_mut().find(|m| m.module == c.module) {
            Some(m) => {
                if !m.chips.iter().any(|x| x == c.name) {
                    m.chips.push(c.name.into());
                }
            }
            None => modules.push(ModuleRow { module: c.module.into(), chips: vec![c.name.into()], status: idx.status(root, c.module), ports: Vec::new() }),
        }
    }
    for p in &scan.ports {
        if let Some(m) = modules.iter_mut().find(|m| Some(&m.module) == p.module.as_ref()) {
            m.ports.push(p.path.clone());
        }
    }

    let installed_rule = fs::read_to_string(RULE_PATH).ok();
    let ide = ide(&installed, &comms);
    let udev = Udev {
        path: RULE_PATH.into(),
        content: rule_content(),
        installed: installed_rule.is_some(),
        outdated: installed_rule.as_deref().is_some_and(|r| r != rule_content()),
        platformio: installed.get("platformio-core-udev").cloned(),
        platformio_repo: ide.repo.get("platformio-core-udev").map(|r| r.1.clone()),
        modemmanager: scan.modemmanager,
    };
    DriversReport {
        kernel: idx.kernel.clone(),
        modules,
        devices: scan.devices,
        brltty: brltty(&installed, scan.brltty),
        udev,
        ide,
        pacman_locked: packages::pacman_locked(),
    }
}

fn process_names() -> Vec<String> {
    let Ok(dir) = fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| crate::util::read_trim(e.path().join("comm")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_de_modulo() {
        assert_eq!(module_name("kernel/drivers/usb/class/cdc-acm.ko.zst"), "cdc_acm");
        assert_eq!(module_name("kernel/drivers/usb/serial/ftdi_sio.ko.xz"), "ftdi_sio");
        assert_eq!(module_name("kernel/drivers/usb/serial/usbserial.ko"), "usbserial");
        let idx = ModIndex::parse(
            "7.3.0-4-cachyos".into(),
            "kernel/drivers/usb/serial/ch341.ko.zst:\nkernel/drivers/usb/class/cdc-acm.ko.zst:\nkernel/sound/x.ko.zst: kernel/a.ko\n",
            "kernel/drivers/usb/serial/usbserial.ko\n",
        );
        let root = Path::new("/nao-existe");
        assert!(idx.status(root, "ch341").available());
        assert!(!idx.status(root, "ch341").loaded);
        assert!(idx.status(root, "cdc_acm").file.is_some());
        assert!(!idx.status(root, "pl2303").available());
        let b = idx.status(root, "usbserial");
        assert!(b.builtin && b.loaded);
    }

    #[test]
    fn regras_do_brltty() {
        let rule = "# Generic\nACTION==\"add\", SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"1A86\", ATTRS{idProduct}==\"7523\", ENV{BRLTTY_BRAILLE_DRIVER}=\"bm\"\n";
        assert!(rule_catches_ch340(rule));
        assert!(!rule_catches_ch340("# 1a86 7523 só em comentário\nSUBSYSTEM==\"usb\", ATTRS{idVendor}==\"0f4e\"\n"));
        let o = parse_owners("/usr/lib/udev/rules.d/90-brltty-device.rules is owned by brltty-udev-generic 6.9.1-3.1\n");
        assert_eq!(o["/usr/lib/udev/rules.d/90-brltty-device.rules"], "brltty-udev-generic");
        assert_eq!(parse_required_by("Name            : brltty\nRequired By     : brltty-udev-generic  python-brltty\n"), ["brltty-udev-generic", "python-brltty"]);
        assert!(parse_required_by("Required By     : None\n").is_empty());
    }

    #[test]
    fn repositorios() {
        let si = "Repository      : cachyos-extra-v3\nName            : arduino-cli\nVersion         : 1.5.1-1.1\n\n\
                  Repository      : extra\nName            : arduino-cli\nVersion         : 1.5.1-1\n\n\
                  Repository      : extra\nName            : platformio-core-udev\nVersion         : 6.2.0-1\n";
        let r = parse_si(si);
        assert_eq!(r["arduino-cli"], ("cachyos-extra-v3".to_string(), "1.5.1-1.1".to_string()));
        assert_eq!(r["platformio-core-udev"].1, "6.2.0-1");
        assert!(!r.contains_key("arduino-ide"));
        assert_eq!(parse_flathub("flathub\tuser\nfedora\tsystem\n").as_deref(), Some("user"));
        assert_eq!(parse_flathub("flathub\tsystem\n").as_deref(), Some("system"));
        assert_eq!(parse_flathub("fedora\tsystem\n"), None);
        assert!(flatpak_ide_args("../x").is_err());
    }

    #[test]
    fn comandos_fixos() {
        assert!(valid_rule_name("90-brltty-device.rules"));
        assert!(!valid_rule_name("../../shadow"));
        assert!(!valid_rule_name("a b.rules"));
        assert!(mask_args(&["90-brltty-device.rules".into()], false).unwrap().last().unwrap() == "90-brltty-device.rules");
        assert!(mask_args(&["x;rm -rf /.rules".into()], false).is_err());
        assert!(valid_module("ch341") && valid_module("cdc_acm"));
        assert!(!valid_module("nvidia"));
        let c = rule_content();
        assert!(c.contains("ATTRS{idVendor}==\"1a86\", TAG+=\"uaccess\""));
        assert!(c.trim_end().ends_with("LABEL=\"reset_maker_end\""));
        let a = install_rule_args(Path::new("/run/user/1000/x.rules"));
        assert_eq!(a[3], "/run/user/1000/x.rules");
    }
}
